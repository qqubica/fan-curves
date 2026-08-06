//! Headless fan-curves engine daemon.
//!
//! Runs the ported `FanEngine` on a jittered ~1 s tick, exactly like the WPF
//! app's timer (850–1150 ms so sampling never phase-locks onto periodic system
//! activity — all downstream time logic runs on the monotonic clock, so the
//! uneven spacing is harmless). On every exit path the headers go back to the
//! BIOS. Console output is CHANGES ONLY (`--verbose` for the per-tick line);
//! the durable record is the telemetry CSV + behavior log (same schema as the
//! WPF app, sim-prefixed). A local socket (see `ipc.rs`) serves the on-demand
//! UI; binding it doubles as the single-instance check.
//!
//! Only the simulated backend exists so far; `--sim` is therefore implied.
//! Real hardware backends (PawnIO/NCT6686D on Windows, hwmon on Linux) are the
//! next phases.

mod ipc;
mod telemetry;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use fan_core::backend::{HwControl, HwSensor, SensorKind};
use fan_core::rng::Rng;
use fan_core::{ChannelStatus, FanEngine, HardwareBackend, OutputReason, Profile, SimulatedBackend};

use ipc::Shared;
use telemetry::TelemetryLog;

/// The daemon's one concrete backend type: enum dispatch keeps `FanEngine<B>`
/// static while letting the platform pick at runtime. `--sim` everywhere;
/// without it, Linux gets hwmon; the Windows native backend is the next phase.
pub enum Backend {
    Sim(SimulatedBackend),
    #[cfg(target_os = "linux")]
    Hwmon(fan_core::hwmon::HwmonBackend),
}

impl HardwareBackend for Backend {
    fn description(&self) -> &str {
        match self {
            Backend::Sim(b) => b.description(),
            #[cfg(target_os = "linux")]
            Backend::Hwmon(b) => b.description(),
        }
    }
    fn is_simulated(&self) -> bool {
        match self {
            Backend::Sim(b) => b.is_simulated(),
            #[cfg(target_os = "linux")]
            Backend::Hwmon(b) => b.is_simulated(),
        }
    }
    fn sensors(&self) -> &[HwSensor] {
        match self {
            Backend::Sim(b) => b.sensors(),
            #[cfg(target_os = "linux")]
            Backend::Hwmon(b) => b.sensors(),
        }
    }
    fn controls(&self) -> &[HwControl] {
        match self {
            Backend::Sim(b) => b.controls(),
            #[cfg(target_os = "linux")]
            Backend::Hwmon(b) => b.controls(),
        }
    }
    fn update(&mut self) {
        match self {
            Backend::Sim(b) => b.update(),
            #[cfg(target_os = "linux")]
            Backend::Hwmon(b) => b.update(),
        }
    }
    fn read_value(&self, sensor_id: &str) -> Option<f64> {
        match self {
            Backend::Sim(b) => b.read_value(sensor_id),
            #[cfg(target_os = "linux")]
            Backend::Hwmon(b) => b.read_value(sensor_id),
        }
    }
    fn set_control(&mut self, control_id: &str, percent: f64) {
        match self {
            Backend::Sim(b) => b.set_control(control_id, percent),
            #[cfg(target_os = "linux")]
            Backend::Hwmon(b) => b.set_control(control_id, percent),
        }
    }
    fn release_control(&mut self, control_id: &str) {
        match self {
            Backend::Sim(b) => b.release_control(control_id),
            #[cfg(target_os = "linux")]
            Backend::Hwmon(b) => b.release_control(control_id),
        }
    }
    fn read_control_rpm(&self, control_id: &str) -> Option<f64> {
        match self {
            Backend::Sim(b) => b.read_control_rpm(control_id),
            #[cfg(target_os = "linux")]
            Backend::Hwmon(b) => b.read_control_rpm(control_id),
        }
    }
    fn internal_sensor_count(&self) -> usize {
        match self {
            Backend::Sim(b) => b.internal_sensor_count(),
            #[cfg(target_os = "linux")]
            Backend::Hwmon(b) => b.internal_sensor_count(),
        }
    }
    fn set_sensor_history_window(&mut self, hours: f64) {
        match self {
            Backend::Sim(b) => b.set_sensor_history_window(hours),
            #[cfg(target_os = "linux")]
            Backend::Hwmon(b) => b.set_sensor_history_window(hours),
        }
    }
}

struct Args {
    sim: bool,
    apply: bool,
    verbose: bool,
    ticks: Option<u64>,
    profile: Option<PathBuf>,
    send: Option<String>,
}

fn parse_args() -> Args {
    let mut args =
        Args { sim: false, apply: true, verbose: false, ticks: None, profile: None, send: None };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--sim" => args.sim = true,
            "--no-apply" => args.apply = false,
            "--verbose" => args.verbose = true,
            "--ticks" => args.ticks = it.next().and_then(|v| v.parse().ok()),
            "--profile" => args.profile = it.next().map(PathBuf::from),
            "--send" => args.send = it.next(),
            other => {
                eprintln!("unknown argument: {other}");
                eprintln!(
                    "usage: fan-daemon [--sim] [--no-apply] [--verbose] [--ticks N] [--profile path]\n       fan-daemon --send '{{\"cmd\":\"status\"}}'"
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

/// Last tick's state per channel, for change-only console output — a daemon
/// must not spend steady-state work narrating "nothing happened" every second
/// (same philosophy as the behavior log: CHANGES only).
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

    // Client mode: one request line to a running daemon, print the reply, done.
    if let Some(request) = &args.send {
        match ipc::send(request) {
            Ok(reply) => println!("{reply}"),
            Err(e) => {
                eprintln!("could not reach the daemon: {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    let use_real = !args.sim && cfg!(target_os = "linux");
    if !args.sim && !use_real {
        eprintln!("the native Windows backend is not wired up yet; running the simulation");
    }

    // Local-offset lookup is only reliable before other threads exist; captured
    // once, so a DST flip mid-run shifts log timestamps until restart.
    let utc_offset_secs = time::UtcOffset::current_local_offset()
        .map(|o| o.whole_seconds() as i64)
        .unwrap_or(0);

    // Dev flows never write the real config (the C# Profile.ReadOnly contract):
    // auto-assign against the sim backend would prune the machine's real IDs.
    // A REAL backend may save — pruning against real hardware is the healing
    // behaviour, same as the WPF app.
    let read_only = args.profile.is_none() && !use_real;
    let profile_path = args.profile.clone().unwrap_or_else(|| config_dir().join("profile.json"));
    let mut profile = Profile::load_or_default(&profile_path);
    println!("profile: \"{}\" from {}", profile.name, profile_path.display());

    #[cfg(target_os = "linux")]
    let mut hw = if use_real {
        Backend::Hwmon(fan_core::hwmon::HwmonBackend::new())
    } else {
        Backend::Sim(SimulatedBackend::new())
    };
    #[cfg(not(target_os = "linux"))]
    let mut hw = Backend::Sim(SimulatedBackend::new());
    println!("backend: {}", hw.description());
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

    // The socket is also the single-instance lock.
    let listener = match ipc::bind() {
        Ok(l) => l,
        Err(e) => {
            eprintln!("another fan-daemon appears to be running (socket busy: {e})");
            std::process::exit(1);
        }
    };

    let mut engine = FanEngine::new(hw, profile);
    if args.apply {
        engine.apply();
    }

    let shared = Arc::new(Shared {
        engine: Mutex::new(engine),
        latest: Mutex::new(Vec::new()),
        telemetry: Mutex::new(TelemetryLog::new(config_dir(), true, utc_offset_secs)),
        profile_path,
        read_only,
        stop: AtomicBool::new(false),
    });
    {
        let shared = Arc::clone(&shared);
        ctrlc::set_handler(move || shared.stop.store(true, Ordering::SeqCst))
            .expect("install Ctrl+C handler");
    }
    shared.telemetry.lock().unwrap().event(&format!(
        "daemon started (sim backend, v{}, {})",
        env!("CARGO_PKG_VERSION"),
        if args.apply { "applying" } else { "monitoring" }
    ));
    {
        let shared = Arc::clone(&shared);
        std::thread::spawn(move || ipc::serve(listener, shared));
    }

    let seed = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.subsec_nanos() as u64).unwrap_or(1);
    let mut jitter = Rng::new(seed | 1);
    let start = Instant::now();
    let mut ticks = 0u64;
    let mut prev: Vec<Option<Prev>> = Vec::new();

    while !shared.stop.load(Ordering::SeqCst) {
        let now = start.elapsed().as_secs_f64();
        let (statuses, profile_snapshot, telemetry_on) = {
            let mut engine = shared.engine.lock().unwrap();
            let statuses = engine.tick(now);
            (statuses, engine.profile().clone(), engine.profile().telemetry_logging_enabled)
        };
        *shared.latest.lock().unwrap() = statuses.clone();

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

        if telemetry_on {
            shared.telemetry.lock().unwrap().record(&profile_snapshot, &statuses);
        }

        ticks += 1;
        if let Some(limit) = args.ticks {
            if ticks >= limit {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(jitter.range(850, 1151)));
    }

    // Explicit handback: the IPC thread keeps its Arc alive, so the engine's
    // Drop through the Arc is not guaranteed to run — release here, always.
    {
        let mut telemetry = shared.telemetry.lock().unwrap();
        telemetry.event("daemon exiting — headers to BIOS");
        telemetry.flush();
    }
    shared.engine.lock().unwrap().stop_applying();
    println!("released all controls to BIOS, exiting");
}
