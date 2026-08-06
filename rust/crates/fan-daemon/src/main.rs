//! Headless fan-curves engine daemon.
//!
//! Runs the ported `FanEngine` on a jittered ~1 s tick, exactly like the WPF
//! app's timer (850–1150 ms so sampling never phase-locks onto periodic system
//! activity — all downstream time logic runs on the monotonic clock, so the
//! uneven spacing is harmless). Ctrl+C (and engine drop on any exit path) hands
//! every header back to the BIOS.
//!
//! Only the simulated backend exists so far; `--sim` is therefore implied.
//! Real hardware backends (PawnIO/NCT6686D on Windows, hwmon on Linux) are the
//! next phases.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use fan_core::backend::SensorKind;
use fan_core::rng::Rng;
use fan_core::{ChannelStatus, FanEngine, HardwareBackend, OutputReason, Profile, SimulatedBackend};

struct Args {
    sim: bool,
    apply: bool,
    verbose: bool,
    ticks: Option<u64>,
    profile: Option<PathBuf>,
}

fn parse_args() -> Args {
    let mut args = Args { sim: false, apply: true, verbose: false, ticks: None, profile: None };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--sim" => args.sim = true,
            "--no-apply" => args.apply = false,
            "--verbose" => args.verbose = true,
            "--ticks" => args.ticks = it.next().and_then(|v| v.parse().ok()),
            "--profile" => args.profile = it.next().map(PathBuf::from),
            other => {
                eprintln!("unknown argument: {other}");
                eprintln!(
                    "usage: fan-daemon [--sim] [--no-apply] [--verbose] [--ticks N] [--profile path]"
                );
                std::process::exit(2);
            }
        }
    }
    args
}

/// `%AppData%\FanCurves` on Windows (the WPF app's directory — same profile.json),
/// `$XDG_CONFIG_HOME/FanCurves` (or `~/.config/FanCurves`) elsewhere.
fn config_dir() -> PathBuf {
    #[cfg(windows)]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            return PathBuf::from(appdata).join("FanCurves");
        }
    }
    #[cfg(not(windows))]
    {
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            return PathBuf::from(xdg).join("FanCurves");
        }
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(".config").join("FanCurves");
        }
    }
    PathBuf::from(".")
}

/// Boards expose dead thermistor inputs that report 0 °C (or 127 °C for
/// "invalid") under names like "CPU" — never trust a sensor by name alone.
fn plausible(v: Option<f64>) -> bool {
    matches!(v, Some(t) if t > 5.0 && t < 115.0)
}

fn contains_ci(hay: &str, needle: &str) -> bool {
    hay.to_lowercase().contains(&needle.to_lowercase())
}

/// Needle order is a preference ranking: the real CPU die sensor first
/// (AMD "Tctl", Intel "CPU Package"), generic "CPU"-named ones last.
fn find_sensor<B: HardwareBackend>(hw: &B, needles: &[&str]) -> Option<String> {
    for n in needles {
        if let Some(hit) = hw.sensors().iter().find(|s| {
            s.kind == SensorKind::Temp && contains_ci(&s.name, n) && plausible(hw.read_value(&s.id))
        }) {
            return Some(hit.id.clone());
        }
    }
    None
}

/// Port of the WPF app's `AutoAssign` + `AssignEmptyChannels`: prune identifiers
/// this backend doesn't know (they heal on the next launch of the other app),
/// de-duplicate headers across channels (a header obeys ONE PWM value — the
/// last writer silently wins otherwise), and wire empty channels to plausible
/// defaults. Pump headers are never auto-assigned; the case channel only takes
/// explicitly chassis-named headers.
fn auto_assign<B: HardwareBackend>(profile: &mut Profile, hw: &B) -> bool {
    let known_controls: std::collections::HashSet<String> =
        hw.controls().iter().map(|c| c.id.clone()).collect();
    let mut changed = false;

    let mut claimed = std::collections::HashSet::new();
    for ch in &mut profile.channels {
        let before = ch.control_ids.len();
        ch.control_ids.retain(|id| claimed.insert(id.clone()));
        changed |= ch.control_ids.len() != before;
    }

    for ch in &mut profile.channels {
        let before = (ch.sensor_ids.len(), ch.control_ids.len());
        ch.sensor_ids.retain(|id| plausible(hw.read_value(id)));
        ch.control_ids.retain(|id| known_controls.contains(id));
        changed |= (ch.sensor_ids.len(), ch.control_ids.len()) != before;

        if ch.control_ids.is_empty() {
            let is_cpu = contains_ci(&ch.name, "CPU");
            let control = hw.controls().iter().filter(|c| !contains_ci(&c.name, "Pump")).find(|c| {
                if is_cpu {
                    contains_ci(&c.name, "CPU")
                } else {
                    ["System", "Case", "Chassis", "CHA"].iter().any(|n| contains_ci(&c.name, n))
                }
            });
            if let Some(control) = control {
                ch.control_ids.push(control.id.clone());
                changed = true;
            }
        }
    }

    let cpu_temp = find_sensor(hw, &["Tctl", "CPU Package", "CPU"]);
    let gpu_temp = find_sensor(hw, &["GPU Core", "GPU Hot Spot", "GPU"]);
    for ch in &mut profile.channels {
        if !ch.sensor_ids.is_empty() {
            continue;
        }
        let is_cpu = contains_ci(&ch.name, "CPU");
        let mut ids = Vec::new();
        if let Some(t) = &cpu_temp {
            ids.push(t.clone());
        }
        if !is_cpu {
            if let Some(t) = &gpu_temp {
                if !ids.contains(t) {
                    ids.push(t.clone());
                }
            }
        }
        if !ids.is_empty() {
            ch.sensor_ids = ids;
            changed = true;
        }
    }
    changed
}

/// Last tick's state per channel, for change-only output — a daemon must not
/// spend steady-state work narrating "nothing happened" every second (same
/// philosophy as the WPF app's behavior log: CHANGES only).
struct Prev {
    applied: bool,
    on: bool,
    target: f64,
    reason: OutputReason,
}

fn is_ramp(r: OutputReason) -> bool {
    matches!(r, OutputReason::RampUp | OutputReason::RampDown)
}

/// None↔Ramp transitions are implied by the target line that precedes every
/// ramp — logging them too is noise (mirrors the C# IsPureRampFlip rule).
fn is_pure_ramp_flip(a: OutputReason, b: OutputReason) -> bool {
    (a == OutputReason::None && is_ramp(b)) || (is_ramp(a) && b == OutputReason::None)
}

/// The moment's vitals, appended to every change line.
fn context(s: &ChannelStatus) -> String {
    let mut out = format!("out {:.0}%", s.output_percent);
    if !s.effective_temp.is_nan() {
        out.push_str(&format!(" · avg {:.1}°", s.effective_temp));
    }
    if let Some(raw) = s.raw_temp {
        out.push_str(&format!(" · now {raw:.1}°"));
    }
    out
}

fn log_transitions(now: f64, s: &ChannelStatus, prev: Option<&Prev>) {
    let Some(p) = prev else {
        println!(
            "[{now:8.1}s] {:<12} {} — {}",
            s.name,
            if s.applied { "driving" } else { "monitoring (not applying)" },
            context(s)
        );
        return;
    };
    if s.applied != p.applied {
        println!(
            "[{now:8.1}s] {:<12} {} — {}",
            s.name,
            if s.applied { "now driving" } else { "released to BIOS" },
            context(s)
        );
    }
    let on = s.output_percent > 0.01;
    if on != p.on {
        println!(
            "[{now:8.1}s] {:<12} fan {} — {}",
            s.name,
            if on { "ON" } else { "OFF" },
            context(s)
        );
    }
    if (s.target_percent - p.target).abs() > 0.5 {
        let why = describe(s);
        println!(
            "[{now:8.1}s] {:<12} target {:.0}% → {:.0}%{} — {}",
            s.name,
            p.target,
            s.target_percent,
            if why.is_empty() { String::new() } else { format!(" [{why}]") },
            context(s)
        );
    }
    if s.reason != p.reason && !is_pure_ramp_flip(p.reason, s.reason) {
        let why = describe(s);
        let why = if why.is_empty() { "steady on the curve".to_string() } else { why };
        println!("[{now:8.1}s] {:<12} → {why} — {}", s.name, context(s));
    }
}

/// Same vocabulary as the why-chip / behavior log.
fn describe(s: &ChannelStatus) -> String {
    match s.reason {
        OutputReason::RampUp => format!("ramping up to {:.0}%", s.target_percent),
        OutputReason::RampDown => format!("ramping down to {:.0}%", s.target_percent),
        OutputReason::StepDownHold => {
            format!("step down to {:.0}% in {:.0} s", s.reason_level, s.reason_seconds)
        }
        OutputReason::Hysteresis => "hysteresis hold".into(),
        OutputReason::ZeroSnap => format!("zero snap (curve asks {:.0}%)", s.reason_level),
        OutputReason::MinFloor => format!("safety floor (curve asks {:.0}%)", s.reason_level),
        OutputReason::IdleKick => "idle kick".into(),
        OutputReason::StopProbe => "stop probe (trial stop)".into(),
        OutputReason::None => String::new(),
    }
}

fn main() {
    let args = parse_args();
    if !args.sim {
        eprintln!("only --sim exists so far (real hardware backends are the next phase); running the simulation");
    }

    // Dev flows never write the real config (the C# Profile.ReadOnly contract):
    // auto-assign against the sim backend would prune the machine's real IDs.
    let read_only = args.profile.is_none();
    let profile_path = args.profile.clone().unwrap_or_else(|| config_dir().join("profile.json"));
    let mut profile = Profile::load_or_default(&profile_path);
    println!("profile: \"{}\" from {}", profile.name, profile_path.display());

    let mut hw = SimulatedBackend::new();
    hw.update(); // one reading so auto-assign sees plausible values
    if auto_assign(&mut profile, &hw) && !read_only {
        if let Err(e) = profile.save(&profile_path) {
            eprintln!("could not save profile: {e}");
        }
    }
    for ch in &profile.channels {
        println!(
            "  {}: sensors [{}] controls [{}]",
            ch.name,
            ch.sensor_ids.join(", "),
            ch.control_ids.join(", ")
        );
    }

    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = Arc::clone(&stop);
        ctrlc::set_handler(move || stop.store(true, Ordering::SeqCst))
            .expect("install Ctrl+C handler");
    }

    let mut engine = FanEngine::new(hw, profile);
    if args.apply {
        engine.apply();
    }

    let seed = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.subsec_nanos() as u64).unwrap_or(1);
    let mut jitter = Rng::new(seed | 1);
    let start = Instant::now();
    let mut ticks = 0u64;

    let mut prev: Vec<Option<Prev>> = Vec::new();
    while !stop.load(Ordering::SeqCst) {
        let now = start.elapsed().as_secs_f64();
        let statuses = engine.tick(now);

        if args.verbose {
            let line = statuses
                .iter()
                .map(|s| {
                    let why = describe(s);
                    format!(
                        "{}: now {} avg {} → {:>3.0}%{}",
                        s.name,
                        s.raw_temp.map_or("--".into(), |t| format!("{t:.1}°")),
                        if s.effective_temp.is_nan() { "--".into() } else { format!("{:.1}°", s.effective_temp) },
                        s.output_percent,
                        if why.is_empty() { String::new() } else { format!(" ({why})") },
                    )
                })
                .collect::<Vec<_>>()
                .join("  |  ");
            println!("[{now:8.1}s] {line}");
        } else {
            prev.resize_with(statuses.len(), || None);
            for (s, p) in statuses.iter().zip(prev.iter_mut()) {
                log_transitions(now, s, p.as_ref());
                *p = Some(Prev {
                    applied: s.applied,
                    on: s.output_percent > 0.01,
                    target: s.target_percent,
                    reason: s.reason,
                });
            }
        }

        ticks += 1;
        if let Some(limit) = args.ticks {
            if ticks >= limit {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(jitter.range(850, 1151)));
    }

    // Engine drop hands every header back to the BIOS; say so for the log.
    drop(engine);
    println!("released all controls to BIOS, exiting");
}
