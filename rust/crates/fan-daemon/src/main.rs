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
//! Backends: `--sim` runs the simulation anywhere; otherwise Windows drives the
//! NCT6686D through PawnIO (admin required) and Linux uses hwmon/sysfs. If a
//! real backend cannot start, the daemon says why and falls back to simulation
//! rather than pretending to control fans.

#[cfg(windows)]
mod autostart;
mod ipc;
#[cfg(windows)]
mod nct6686;
#[cfg(windows)]
mod pawnio;
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
/// static while letting the platform pick at runtime.
pub enum Backend {
    Sim(SimulatedBackend),
    #[cfg(target_os = "linux")]
    Hwmon(fan_core::hwmon::HwmonBackend),
    #[cfg(windows)]
    Nct(nct6686::Nct6686Backend),
}

impl HardwareBackend for Backend {
    fn description(&self) -> &str {
        match self {
            Backend::Sim(b) => b.description(),
            #[cfg(target_os = "linux")]
            Backend::Hwmon(b) => b.description(),
            #[cfg(windows)]
            Backend::Nct(b) => b.description(),
        }
    }
    fn is_simulated(&self) -> bool {
        match self {
            Backend::Sim(b) => b.is_simulated(),
            #[cfg(target_os = "linux")]
            Backend::Hwmon(b) => b.is_simulated(),
            #[cfg(windows)]
            Backend::Nct(b) => b.is_simulated(),
        }
    }
    fn sensors(&self) -> &[HwSensor] {
        match self {
            Backend::Sim(b) => b.sensors(),
            #[cfg(target_os = "linux")]
            Backend::Hwmon(b) => b.sensors(),
            #[cfg(windows)]
            Backend::Nct(b) => b.sensors(),
        }
    }
    fn controls(&self) -> &[HwControl] {
        match self {
            Backend::Sim(b) => b.controls(),
            #[cfg(target_os = "linux")]
            Backend::Hwmon(b) => b.controls(),
            #[cfg(windows)]
            Backend::Nct(b) => b.controls(),
        }
    }
    fn update(&mut self) {
        match self {
            Backend::Sim(b) => b.update(),
            #[cfg(target_os = "linux")]
            Backend::Hwmon(b) => b.update(),
            #[cfg(windows)]
            Backend::Nct(b) => b.update(),
        }
    }
    fn read_value(&self, sensor_id: &str) -> Option<f64> {
        match self {
            Backend::Sim(b) => b.read_value(sensor_id),
            #[cfg(target_os = "linux")]
            Backend::Hwmon(b) => b.read_value(sensor_id),
            #[cfg(windows)]
            Backend::Nct(b) => b.read_value(sensor_id),
        }
    }
    fn set_control(&mut self, control_id: &str, percent: f64) {
        match self {
            Backend::Sim(b) => b.set_control(control_id, percent),
            #[cfg(target_os = "linux")]
            Backend::Hwmon(b) => b.set_control(control_id, percent),
            #[cfg(windows)]
            Backend::Nct(b) => b.set_control(control_id, percent),
        }
    }
    fn release_control(&mut self, control_id: &str) {
        match self {
            Backend::Sim(b) => b.release_control(control_id),
            #[cfg(target_os = "linux")]
            Backend::Hwmon(b) => b.release_control(control_id),
            #[cfg(windows)]
            Backend::Nct(b) => b.release_control(control_id),
        }
    }
    fn read_control_rpm(&self, control_id: &str) -> Option<f64> {
        match self {
            Backend::Sim(b) => b.read_control_rpm(control_id),
            #[cfg(target_os = "linux")]
            Backend::Hwmon(b) => b.read_control_rpm(control_id),
            #[cfg(windows)]
            Backend::Nct(b) => b.read_control_rpm(control_id),
        }
    }
    fn internal_sensor_count(&self) -> usize {
        match self {
            Backend::Sim(b) => b.internal_sensor_count(),
            #[cfg(target_os = "linux")]
            Backend::Hwmon(b) => b.internal_sensor_count(),
            #[cfg(windows)]
            Backend::Nct(b) => b.internal_sensor_count(),
        }
    }
    fn set_sensor_history_window(&mut self, hours: f64) {
        match self {
            Backend::Sim(b) => b.set_sensor_history_window(hours),
            #[cfg(target_os = "linux")]
            Backend::Hwmon(b) => b.set_sensor_history_window(hours),
            #[cfg(windows)]
            Backend::Nct(b) => b.set_sensor_history_window(hours),
        }
    }
}

struct Args {
    sim: bool,
    apply: bool,
    verbose: bool,
    probe: bool,
    save_profile: bool,
    /// (header index, percent) — write-path self-test on one header, then exit.
    selftest: Option<(usize, f64)>,
    /// Register (true) or remove (false) the start-with-Windows task, then exit.
    autostart: Option<bool>,
    ticks: Option<u64>,
    profile: Option<PathBuf>,
    send: Option<String>,
}

fn parse_args() -> Args {
    let mut args = Args {
        sim: false,
        apply: true,
        verbose: false,
        probe: false,
        save_profile: false,
        selftest: None,
        autostart: None,
        ticks: None,
        profile: None,
        send: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--sim" => args.sim = true,
            "--no-apply" => args.apply = false,
            "--verbose" => args.verbose = true,
            "--probe" => args.probe = true,
            "--save-profile" => args.save_profile = true,
            "--selftest-write" => {
                args.selftest = it.next().and_then(|v| v.parse().ok()).map(|i| {
                    (i, it.next().and_then(|v| v.parse().ok()).unwrap_or(40.0))
                });
            }
            "--ticks" => args.ticks = it.next().and_then(|v| v.parse().ok()),
            "--profile" => args.profile = it.next().map(PathBuf::from),
            "--send" => args.send = it.next(),
            "--install-autostart" => args.autostart = Some(true),
            "--uninstall-autostart" => args.autostart = Some(false),
            other => {
                eprintln!("unknown argument: {other}");
                eprintln!(
                    "usage: fan-daemon [--sim] [--no-apply] [--verbose] [--ticks N] [--profile path]\n       fan-daemon --probe            (dump sensors/controls read-only, then exit)\n       fan-daemon --install-autostart | --uninstall-autostart\n       fan-daemon --send '{{\"cmd\":\"status\"}}'"
                );
                std::process::exit(2);
            }
        }
    }
    args
}

/// Read-only hardware inventory (the daemon's `sensors.txt`): two updates a
/// second apart so lagging readings settle, print everything, exit. Never
/// writes a control — safe to run alongside whatever currently drives the fans.
fn probe(hw: &mut Backend) {
    println!("backend: {}", hw.description());
    hw.update();
    std::thread::sleep(Duration::from_millis(1000));
    hw.update();
    println!("-- sensors:");
    for s in hw.sensors() {
        let kind = match s.kind {
            SensorKind::Temp => "temp",
            SensorKind::Rpm => "rpm",
        };
        let v = hw.read_value(&s.id).map_or("null".into(), |v| format!("{v:.2}"));
        println!("{kind:<5} {:<45} {:<55} = {v}", s.id, s.name);
    }
    println!("-- controls:");
    for c in hw.controls() {
        let rpm = hw.read_control_rpm(&c.id).map_or("--".into(), |r| format!("{r:.0} rpm"));
        println!("ctrl  {:<45} {:<55} @ {rpm}", c.id, c.name);
    }
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

    // Autostart management: do it and exit, never as a side effect of running.
    #[cfg(windows)]
    if let Some(install) = args.autostart {
        let r = if install { autostart::install() } else { autostart::uninstall() };
        match r {
            Ok(()) => {
                println!(
                    "start-with-Windows {} (task \"{}\")",
                    if install { "ENABLED" } else { "disabled" },
                    autostart::TASK_NAME
                );
                if install && autostart::conflicting_wpf_task() {
                    eprintln!(
                        "WARNING: the WPF app's \"{}\" task is also registered. Two controllers \
                         writing the same headers is last-writer-wins — disable one:\n  \
                         schtasks /Change /TN {} /DISABLE",
                        autostart::WPF_TASK_NAME,
                        autostart::WPF_TASK_NAME
                    );
                }
            }
            Err(e) => {
                eprintln!("could not change autostart: {e}\n  (needs an elevated shell)");
                std::process::exit(1);
            }
        }
        return;
    }

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

    // Try the platform's real backend unless --sim was asked for; a failure is
    // reported and demoted to simulation, never silently pretended away.
    let mut hw = None;
    if !args.sim {
        #[cfg(windows)]
        match nct6686::Nct6686Backend::detect() {
            Ok(b) => hw = Some(Backend::Nct(b)),
            Err(e) => eprintln!(
                "no hardware control: {e}\n  (needs administrator + PawnIO installed; \
                 falling back to simulation)"
            ),
        }
        #[cfg(target_os = "linux")]
        {
            hw = Some(Backend::Hwmon(fan_core::hwmon::HwmonBackend::new()));
        }
    }
    let real = hw.is_some();

    // Local-offset lookup is only reliable before other threads exist; captured
    // once, so a DST flip mid-run shifts log timestamps until restart.
    let utc_offset_secs = time::UtcOffset::current_local_offset()
        .map(|o| o.whole_seconds() as i64)
        .unwrap_or(0);

    let mut hw = hw.unwrap_or_else(|| Backend::Sim(SimulatedBackend::new()));
    if args.probe {
        probe(&mut hw);
        return;
    }
    #[cfg(windows)]
    if let Some((index, percent)) = args.selftest {
        match &mut hw {
            Backend::Nct(b) => {
                b.update(); // populate rpm readings for the report
                if let Err(e) = b.selftest_write(index, percent) {
                    eprintln!("self-test failed: {e}");
                    std::process::exit(1);
                }
            }
            _ => eprintln!("--selftest-write needs the real backend (run elevated)"),
        }
        return;
    }

    // The shared `profile.json` is the WPF app's live config, and that app is
    // still the shipping fan controller — so the daemon treats it as READ-ONLY
    // unless told otherwise (`--save-profile`) or pointed at its own file
    // (`--profile`). This is the C# `Profile.ReadOnly` contract widened: a sim
    // run must not prune real IDs, and a Rust run must not race the WPF app
    // over the file that holds Kuba's manual header assignments.
    let read_only = args.profile.is_none() && !args.save_profile;
    let profile_path = args.profile.clone().unwrap_or_else(|| config_dir().join("profile.json"));
    let mut profile = Profile::load_or_default(&profile_path);
    println!("profile: \"{}\" from {}", profile.name, profile_path.display());
    println!("backend: {}", hw.description());
    #[cfg(windows)]
    {
        // Say the residency situation out loud: a port that cannot survive a
        // reboot silently hands the machine back to the WPF app.
        let mine = autostart::installed();
        let theirs = autostart::conflicting_wpf_task();
        println!(
            "autostart: daemon {}{}",
            if mine { "ON" } else { "off (--install-autostart to enable)" },
            if theirs { ", WPF app's task ALSO registered" } else { "" }
        );
        if mine && theirs {
            eprintln!("WARNING: both would start at logon and fight over the headers.");
        }
    }
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
        history: Mutex::new(Vec::new()),
        // Sim runs get the -sim log filenames, exactly like the WPF dev flows.
        telemetry: Mutex::new(TelemetryLog::new(config_dir(), !real, utc_offset_secs)),
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
        "daemon started (v{}, {}, {})",
        env!("CARGO_PKG_VERSION"),
        if real { "real hardware" } else { "simulation" },
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

        // Record the tick into the strip history — the daemon is the resident
        // process, so this is what lets a freshly opened UI show the past
        // 10 minutes (and scroll back further) instead of starting empty.
        {
            let wall = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs_f64())
                .unwrap_or(0.0);
            let mut history = shared.history.lock().unwrap();
            history.resize_with(statuses.len(), Default::default);
            for (h, s) in history.iter_mut().zip(&statuses) {
                h.push(fan_core::history::HistorySample {
                    wall,
                    avg: s.effective_temp,
                    raw: s.raw_temp,
                    out: s.output_percent,
                });
            }
        }

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
