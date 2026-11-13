//! Headless fan-curves engine daemon.
//!
//! Runs `FanEngine` on a jittered ~1 s tick (850–1150 ms, so sampling never
//! phase-locks onto periodic system activity — all downstream time logic runs
//! on the monotonic clock, so the uneven spacing is harmless). On every exit
//! path the headers go back to the BIOS. Console output is CHANGES ONLY
//! (`--verbose` for the per-tick line); the durable record is the telemetry
//! CSV + behavior log. A local socket (see `ipc.rs`) serves the on-demand UI;
//! binding it doubles as the single-instance check.
//!
//! Backends: Windows drives the NCT6686D through PawnIO (admin required) plus
//! NVIDIA GPU fans through NVML; Linux uses hwmon/sysfs. Simulation is explicit
//! (`--sim`): a daemon that cannot reach real hardware exits rather than
//! pretending to control fans.

#[cfg(windows)]
mod autostart;
mod ipc;
#[cfg(windows)]
mod nct6686;
#[cfg(windows)]
mod nvml;
#[cfg(windows)]
mod pawnio;
mod telemetry;

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use fan_core::backend::{HwControl, HwSensor, SensorKind};
use fan_core::rng::Rng;
use fan_core::{ChannelStatus, FanEngine, HardwareBackend, OutputReason, Profile, SimulatedBackend};

use ipc::Shared;
use telemetry::TelemetryLog;

/// The daemon's one concrete backend type: enum dispatch keeps `FanEngine<B>`
/// static while letting the platform pick at runtime.
pub enum Backend {
    Sim(Box<SimulatedBackend>),
    #[cfg(target_os = "linux")]
    Hwmon(fan_core::hwmon::HwmonBackend),
    #[cfg(windows)]
    Windows(Box<WindowsBackend>),
}

/// Forward a `HardwareBackend` call to whichever backend is active.
macro_rules! dispatch {
    ($self:expr, $b:ident => $call:expr) => {
        match $self {
            Backend::Sim($b) => $call,
            #[cfg(target_os = "linux")]
            Backend::Hwmon($b) => $call,
            #[cfg(windows)]
            Backend::Windows($b) => $call,
        }
    };
}

impl HardwareBackend for Backend {
    fn description(&self) -> &str { dispatch!(self, b => b.description()) }
    fn is_simulated(&self) -> bool { dispatch!(self, b => b.is_simulated()) }
    fn sensors(&self) -> &[HwSensor] { dispatch!(self, b => b.sensors()) }
    fn controls(&self) -> &[HwControl] { dispatch!(self, b => b.controls()) }
    fn update(&mut self) { dispatch!(self, b => b.update()) }
    fn read_value(&self, id: &str) -> Option<f64> { dispatch!(self, b => b.read_value(id)) }
    fn set_control(&mut self, id: &str, percent: f64) { dispatch!(self, b => b.set_control(id, percent)) }
    fn control_write_succeeded(&self, id: &str) -> bool { dispatch!(self, b => b.control_write_succeeded(id)) }
    fn release_control(&mut self, id: &str) { dispatch!(self, b => b.release_control(id)) }
    fn read_control_rpm(&self, id: &str) -> Option<f64> { dispatch!(self, b => b.read_control_rpm(id)) }
}

/// The Windows machine: motherboard headers (NCT6686D) and NVIDIA GPU fans
/// behind one inventory. Detection tolerates either half being absent; the
/// control loop itself refuses to start without the motherboard half.
#[cfg(windows)]
pub struct WindowsBackend {
    nct: Option<nct6686::Nct6686Backend>,
    gpu: Option<nvml::NvmlBackend>,
    sensors: Vec<HwSensor>,
    controls: Vec<HwControl>,
    description: String,
}

#[cfg(windows)]
impl WindowsBackend {
    fn detect() -> Option<Self> {
        let nct = match nct6686::Nct6686Backend::detect() {
            Ok(v) => Some(v),
            Err(e) => { eprintln!("NCT6686D unavailable: {e}"); None }
        };
        let gpu = match nvml::NvmlBackend::detect() {
            Ok(v) => Some(v),
            Err(e) => { eprintln!("NVIDIA NVML unavailable: {e}"); None }
        };
        if nct.is_none() && gpu.is_none() { return None; }
        let mut sensors = Vec::new();
        let mut controls = Vec::new();
        let mut names = Vec::new();
        if let Some(b) = &nct {
            sensors.extend_from_slice(b.sensors());
            controls.extend_from_slice(b.controls());
            names.push(b.description());
        }
        if let Some(b) = &gpu {
            sensors.extend_from_slice(b.sensors());
            controls.extend_from_slice(b.controls());
            names.push(b.description());
        }
        let description = names.join(" + ");
        Some(Self { nct, gpu, sensors, controls, description })
    }
}

#[cfg(windows)]
impl HardwareBackend for WindowsBackend {
    fn description(&self) -> &str { &self.description }
    fn is_simulated(&self) -> bool { false }
    fn sensors(&self) -> &[HwSensor] { &self.sensors }
    fn controls(&self) -> &[HwControl] { &self.controls }
    fn update(&mut self) {
        if let Some(b) = &mut self.nct { b.update(); }
        if let Some(b) = &mut self.gpu { b.update(); }
    }
    fn read_value(&self, id: &str) -> Option<f64> {
        self.nct.as_ref().and_then(|b| b.read_value(id)).or_else(|| self.gpu.as_ref().and_then(|b| b.read_value(id)))
    }
    fn set_control(&mut self, id: &str, percent: f64) {
        if let Some(b) = &mut self.gpu {
            if b.has_control(id) { b.set_control(id, percent); return; }
        }
        if let Some(b) = &mut self.nct { b.set_control(id, percent); }
    }
    fn control_write_succeeded(&self, id: &str) -> bool {
        if let Some(b) = &self.gpu {
            if b.has_control(id) { return b.control_write_succeeded(id); }
        }
        self.nct.as_ref().is_some_and(|b| b.control_write_succeeded(id))
    }
    fn release_control(&mut self, id: &str) {
        if let Some(b) = &mut self.gpu {
            if b.has_control(id) { b.release_control(id); return; }
        }
        if let Some(b) = &mut self.nct { b.release_control(id); }
    }
    fn read_control_rpm(&self, id: &str) -> Option<f64> {
        self.nct.as_ref().and_then(|b| b.read_control_rpm(id)).or_else(|| self.gpu.as_ref().and_then(|b| b.read_control_rpm(id)))
    }
}

const USAGE: &str = "\
usage: fan-daemon [--sim] [--no-apply] [--verbose] [--ticks N] [--profile path]
       fan-daemon --probe                      dump sensors/controls read-only, then exit
       fan-daemon --selftest-write HEADER [%]  write one motherboard header, restore, exit
       fan-daemon --selftest-gpu FAN [%]       write one GPU fan for 2 s, restore, exit
       fan-daemon --install-autostart | --uninstall-autostart
       fan-daemon --send status                one IPC command to the running daemon
       fan-daemon --send '{\"cmd\":\"history\",\"channel\":0,\"start\":0,\"count\":10}'";

struct Args {
    sim: bool,
    apply: bool,
    verbose: bool,
    probe: bool,
    /// (header index, percent) — write-path self-test on one header, then exit.
    selftest: Option<(usize, f64)>,
    /// (fan index, percent): bounded two-second GPU write/restore test.
    gpu_selftest: Option<(usize, f64)>,
    /// Register (true) or remove (false) the start-with-Windows tasks, then exit.
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
        selftest: None,
        gpu_selftest: None,
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
            "--selftest-write" => {
                args.selftest = it.next().and_then(|v| v.parse().ok()).map(|i| {
                    (i, it.next().and_then(|v| v.parse().ok()).unwrap_or(40.0))
                });
            }
            "--selftest-gpu" => {
                args.gpu_selftest = it.next().and_then(|v| v.parse().ok()).map(|i| {
                    (i, it.next().and_then(|v| v.parse().ok()).unwrap_or(50.0))
                });
            }
            "--ticks" => args.ticks = it.next().and_then(|v| v.parse().ok()),
            "--profile" => args.profile = it.next().map(PathBuf::from),
            "--send" => args.send = it.next(),
            "--install-autostart" => args.autostart = Some(true),
            "--uninstall-autostart" => args.autostart = Some(false),
            other => {
                eprintln!("unknown argument: {other}\n{USAGE}");
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

/// `%AppData%\FanCurves` on Windows,
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

/// Wire the profile to this machine: preserve identifiers
/// this backend cannot currently see (a transient backend failure must not erase them),
/// de-duplicate headers across channels (a header obeys ONE PWM value — the
/// last writer silently wins otherwise), and wire empty channels to plausible
/// defaults. Pump headers are never auto-assigned; the case channel only takes
/// explicitly chassis-named headers.
fn auto_assign<B: HardwareBackend>(profile: &mut Profile, hw: &B) -> bool {
    let gpu_controls: Vec<String> = hw.controls().iter()
        .filter(|c| c.id.starts_with("/nvidiagpu/") && c.id.contains("/control/"))
        .map(|c| c.id.clone()).collect();
    let already_assigned: std::collections::HashSet<&str> = profile.channels.iter()
        .flat_map(|ch| ch.control_ids.iter().map(String::as_str)).collect();
    let unclaimed_gpu_controls: Vec<String> = gpu_controls.iter()
        .filter(|id| !already_assigned.contains(id.as_str())).cloned().collect();
    let mut added_gpu_channel = false;
    if !unclaimed_gpu_controls.is_empty() && !profile.channels.iter().any(|c| contains_ci(&c.name, "GPU")) {
        use fan_core::{ChannelConfig, CurvePoint};
        profile.channels.push(ChannelConfig {
            name: "GPU fans".into(),
            control_ids: unclaimed_gpu_controls,
            // Never ask a GPU fan to stop under load. The driver's reported
            // minimum is enforced again by NVML at the write boundary.
            min_percent: 30.0,
            points: vec![CurvePoint::new(20.0, 30.0), CurvePoint::new(50.0, 40.0),
                         CurvePoint::new(65.0, 60.0), CurvePoint::new(75.0, 80.0),
                         CurvePoint::new(83.0, 100.0)],
            averaging_seconds: 10.0,
            ..Default::default()
        });
        added_gpu_channel = true;
    }
    let mut changed = added_gpu_channel;

    let mut claimed = std::collections::HashSet::new();
    for ch in &mut profile.channels {
        let before = ch.control_ids.len();
        ch.control_ids.retain(|id| claimed.insert(id.clone()));
        changed |= ch.control_ids.len() != before;
    }

    for ch in &mut profile.channels {
        // Keep missing IDs in the saved profile. The engine ignores unavailable
        // readings and the backend rejects unknown controls for this session.

        if ch.control_ids.is_empty() {
            if contains_ci(&ch.name, "GPU") {
                ch.control_ids = gpu_controls.iter().filter(|id| !claimed.contains(*id)).cloned().collect();
                changed |= !ch.control_ids.is_empty();
                continue;
            }
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
        if contains_ci(&ch.name, "GPU") {
            if let Some(t) = &gpu_temp { ch.sensor_ids.push(t.clone()); changed = true; }
            continue;
        }
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
/// ramp — logging them too is noise.
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
        OutputReason::FullSpeed => "full speed override".into(),
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
                    "start-with-Windows {} (tasks \"{}\" and \"{}\")",
                    if install { "ENABLED" } else { "disabled" },
                    autostart::TASK_NAME,
                    autostart::TRAY_TASK_NAME
                );
            }
            Err(e) => {
                eprintln!("could not change autostart: {e}\n  (needs an elevated shell)");
                std::process::exit(1);
            }
        }
        return;
    }

    // Client mode: one request line to a running daemon, print the reply, done.
    // A bare word is shorthand for a command without arguments (`--send status`),
    // which also spares scripts the JSON quoting.
    if let Some(request) = &args.send {
        let request = if request.trim_start().starts_with('{') {
            request.clone()
        } else {
            serde_json::json!({ "cmd": request }).to_string()
        };
        match ipc::send(&request) {
            Ok(reply) => println!("{reply}"),
            Err(e) => {
                eprintln!("could not reach the daemon: {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    // Simulation is explicit. A missing hardware backend must not turn a
    // scheduled production controller into a convincing simulated process.
    let mut hw = None;
    if !args.sim {
        #[cfg(windows)]
        {
            hw = WindowsBackend::detect().map(|b| Backend::Windows(Box::new(b)));
        }
        #[cfg(target_os = "linux")]
        {
            hw = Some(Backend::Hwmon(fan_core::hwmon::HwmonBackend::new()));
        }
    }
    if !args.sim && hw.is_none() {
        eprintln!("no supported hardware backend available; use --sim explicitly for a demo");
        std::process::exit(1);
    }
    let real = !args.sim;

    // Local-offset lookup is only reliable before other threads exist; captured
    // once, so a DST flip mid-run shifts log timestamps until restart.
    let utc_offset_secs = time::UtcOffset::current_local_offset()
        .map(|o| o.whole_seconds() as i64)
        .unwrap_or(0);

    let mut hw = hw.unwrap_or_else(|| Backend::Sim(Box::new(SimulatedBackend::new())));
    if args.probe {
        probe(&mut hw);
        return;
    }
    #[cfg(windows)]
    if let Some((index, percent)) = args.selftest {
        match &mut hw {
            Backend::Windows(b) if b.nct.is_some() => {
                let nct = b.nct.as_mut().unwrap();
                nct.update(); // populate rpm readings for the report
                if let Err(e) = nct.selftest_write(index, percent) {
                    eprintln!("self-test failed: {e}");
                    std::process::exit(1);
                }
            }
            _ => eprintln!("--selftest-write needs the real backend (run elevated)"),
        }
        return;
    }
    #[cfg(windows)]
    if let Some((fan, percent)) = args.gpu_selftest {
        let Backend::Windows(b) = &mut hw else {
            eprintln!("--selftest-gpu needs the NVIDIA NVML backend");
            std::process::exit(1);
        };
        let Some(gpu) = &mut b.gpu else {
            eprintln!("--selftest-gpu: NVIDIA NVML is unavailable");
            std::process::exit(1);
        };
        let id = format!("/nvidiagpu/0/control/{fan}");
        if !gpu.has_control(&id) {
            eprintln!("--selftest-gpu: {id} is not a supported control");
            std::process::exit(1);
        }
        gpu.update();
        println!("before: {id} = {:?} rpm", gpu.read_control_rpm(&id));
        gpu.set_control(&id, percent.clamp(30.0, 100.0));
        if !gpu.control_write_succeeded(&id) {
            eprintln!("--selftest-gpu: driver refused the fan write");
            drop(hw); // retry any failed automatic-policy handback before exit
            std::process::exit(1);
        }
        std::thread::sleep(Duration::from_secs(2));
        gpu.update();
        println!("manual: {id} = {:?} rpm", gpu.read_control_rpm(&id));
        gpu.release_control(&id);
        if gpu.is_manual(&id) {
            eprintln!("--selftest-gpu: automatic-policy restore failed; retrying on drop");
            drop(hw);
            std::process::exit(1);
        }
        println!("restored: {id} = automatic");
        return;
    }

    #[cfg(windows)]
    if let Backend::Windows(b) = &hw {
        if b.nct.is_none() {
            eprintln!("NCT6686D motherboard control is unavailable; refusing GPU-only startup");
            std::process::exit(1);
        }
    }

    // Edits are saved to the profile, with one exception: a simulation never
    // touches the real `profile.json`. It could not use it anyway (the real
    // sensor and header ids mean nothing to the simulated machine), and
    // saving would overwrite manual assignments that do not come back on
    // their own. So a simulation runs the built-in defaults, unsaved, unless
    // `--profile` points it at a file of its own.
    let read_only = args.sim && args.profile.is_none();
    let profile_path = args.profile.clone().unwrap_or_else(|| config_dir().join("profile.json"));
    let mut profile = if read_only {
        println!("profile: built-in defaults (simulation, nothing is saved)");
        Profile::mac_book_like()
    } else {
        let profile = Profile::load_or_default(&profile_path);
        println!("profile: \"{}\" from {}", profile.name, profile_path.display());
        profile
    };
    println!("backend: {}", hw.description());
    #[cfg(windows)]
    let autostart_state = {
        // Say the residency situation out loud: a controller that does not
        // come back after a reboot silently leaves the fans to the BIOS.
        let installed = autostart::installed();
        println!(
            "autostart: {}",
            if installed { "ON" } else { "off (--install-autostart to enable)" }
        );
        ipc::AutostartCache::checked(installed)
    };
    #[cfg(not(windows))]
    let autostart_state = ipc::AutostartCache::checked(false);
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

    let shared = Arc::new(Shared::new(
        engine,
        // Sim runs get the -sim log filenames, so they never mix into real logs.
        TelemetryLog::new(config_dir(), !real, utc_offset_secs),
        profile_path,
        read_only,
        autostart_state,
    ));
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
            shared.publish(&engine);
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

#[cfg(test)]
mod assignment_tests {
    use super::*;

    struct InventoryHw { sensors: Vec<HwSensor>, controls: Vec<HwControl> }
    impl InventoryHw {
        fn gpu() -> Self {
            Self {
                sensors: vec![HwSensor { id: "/nvidiagpu/0/temperature/0".into(), name: "RTX 5080 — GPU Core".into(), kind: SensorKind::Temp }],
                controls: (0..2).map(|i| HwControl { id: format!("/nvidiagpu/0/control/{i}"), name: format!("GPU Fan #{i}") }).collect(),
            }
        }
    }
    impl HardwareBackend for InventoryHw {
        fn description(&self) -> &str { "test" }
        fn is_simulated(&self) -> bool { false }
        fn sensors(&self) -> &[HwSensor] { &self.sensors }
        fn controls(&self) -> &[HwControl] { &self.controls }
        fn update(&mut self) {}
        fn read_value(&self, id: &str) -> Option<f64> { (id == "/nvidiagpu/0/temperature/0").then_some(42.0) }
        fn set_control(&mut self, _: &str, _: f64) {}
        fn release_control(&mut self, _: &str) {}
        fn read_control_rpm(&self, _: &str) -> Option<f64> { None }
    }

    #[test]
    fn gpu_channel_uses_gpu_temperature_and_only_unclaimed_fans() {
        let mut profile = Profile::mac_book_like();
        let manually_assigned = "/nvidiagpu/0/control/0".to_string();
        profile.channels[1].control_ids.push(manually_assigned.clone());
        assert!(auto_assign(&mut profile, &InventoryHw::gpu()));
        let gpu = profile.channels.iter().find(|ch| ch.name == "GPU fans").unwrap();
        assert_eq!(profile.channels[1].control_ids, vec![manually_assigned]);
        assert_eq!(gpu.control_ids, vec!["/nvidiagpu/0/control/1"]);
        assert_eq!(gpu.sensor_ids, vec!["/nvidiagpu/0/temperature/0"]);
        assert!(gpu.min_percent >= 30.0);
    }

    #[test]
    fn unavailable_motherboard_ids_survive_gpu_only_inventory() {
        let mut profile = Profile::mac_book_like();
        let old_control = "/lpc/nct6686d/0/control/1".to_string();
        let old_sensor = "/amdcpu/0/temperature/2".to_string();
        profile.channels[0].control_ids.push(old_control.clone());
        profile.channels[0].sensor_ids.push(old_sensor.clone());
        auto_assign(&mut profile, &InventoryHw::gpu());
        assert_eq!(profile.channels[0].control_ids, vec![old_control]);
        assert_eq!(profile.channels[0].sensor_ids, vec![old_sensor]);
    }

    #[test]
    fn cpu_case_preset_does_not_overwrite_gpu_tuning() {
        let mut profile = Profile::mac_book_like();
        auto_assign(&mut profile, &InventoryHw::gpu());
        let gpu_points = profile.channels[2].points.clone();
        let gpu_sensors = profile.channels[2].sensor_ids.clone();
        profile.adopt_tuning(&Profile::performance());
        assert_eq!(profile.channels[2].points, gpu_points);
        assert_eq!(profile.channels[2].sensor_ids, gpu_sensors);
    }
}
