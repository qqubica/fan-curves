//! IPC endpoint for the on-demand UI (and CLI): a local socket — named pipe on
//! Windows, Unix socket elsewhere — speaking line-delimited JSON. One request
//! line in, one response line out; connections may pipeline.
//!
//! Commands:
//!   {"cmd":"ping"}                     → {"ok":true,"version":"…","simulated":true}
//!   {"cmd":"status"}                   → applying + profile name + latest tick statuses
//!   {"cmd":"profile"}                  → the full profile (PascalCase, same schema as profile.json)
//!   {"cmd":"set_profile","profile":…}  → replace + save (unless the daemon is read-only)
//!   {"cmd":"preset","name":"quiet"|"performance"} → adopt tuning, keep assignments
//!   {"cmd":"apply"} / {"cmd":"pause"}  → start/stop writing PWM (pause = BIOS control)
//!
//! Binding the socket also serves as the single-instance check: a second daemon
//! fails to create the listener and exits.

use std::io::{BufRead, BufReader, BufWriter, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use interprocess::local_socket::traits::{ListenerExt as _, Stream as _};
use interprocess::local_socket::{
    GenericNamespaced, Listener, ListenerOptions, Stream, ToNsName,
};
use serde::Deserialize;
use serde_json::json;

use fan_core::backend::SensorKind;
use fan_core::{ChannelStatus, FanEngine, HardwareBackend as _, Profile};

use crate::telemetry::TelemetryLog;
use crate::Backend;

pub const SOCKET_NAME: &str = "fan-curves-daemon.sock";

/// Everything the tick loop and the IPC handlers share.
pub struct Shared {
    pub engine: Mutex<FanEngine<Backend>>,
    pub latest: Mutex<Vec<ChannelStatus>>,
    pub telemetry: Mutex<TelemetryLog>,
    pub profile_path: std::path::PathBuf,
    pub read_only: bool,
    /// Set by Ctrl+C or the `shutdown` command; the tick loop exits on it and
    /// hands the headers back to the BIOS (the daemon's `exit.signal`).
    pub stop: AtomicBool,
}

/// The daemon runs ELEVATED (Super I/O access needs it) while the UI is a
/// normal user process, and a named pipe created by a high-integrity process
/// gets a default DACL a medium-integrity client cannot open — the UI would
/// see nothing but "access denied". So on Windows the pipe is created with an
/// explicit descriptor granting the interactive user full access:
///   D:(A;;GA;;;IU)  — allow, generic-all, Interactive Users
/// Interactive (rather than Everyone) keeps it to whoever is logged in at the
/// console; a service account or remote session cannot command the fans.
#[cfg(windows)]
fn listener_options<'a>(name: interprocess::local_socket::Name<'a>) -> ListenerOptions<'a> {
    use interprocess::os::windows::local_socket::ListenerOptionsExt;
    use interprocess::os::windows::security_descriptor::SecurityDescriptor;

    let opts = ListenerOptions::new().name(name);
    let sddl = widestring::U16CString::from_str("D:(A;;GA;;;IU)").expect("static SDDL");
    match SecurityDescriptor::deserialize(&sddl) {
        Ok(sd) => opts.security_descriptor(sd),
        // Without it the daemon still runs; only a non-elevated UI is locked out.
        Err(e) => {
            eprintln!("could not build the pipe security descriptor ({e}); \
                       a non-elevated UI will not be able to connect");
            opts
        }
    }
}

#[cfg(not(windows))]
fn listener_options<'a>(name: interprocess::local_socket::Name<'a>) -> ListenerOptions<'a> {
    ListenerOptions::new().name(name)
}

pub fn bind() -> std::io::Result<Listener> {
    let name = SOCKET_NAME.to_ns_name::<GenericNamespaced>()?;
    listener_options(name).create_sync()
}

pub fn serve(listener: Listener, shared: Arc<Shared>) {
    for conn in listener.incoming() {
        let Ok(conn) = conn else { continue };
        let shared = Arc::clone(&shared);
        std::thread::spawn(move || {
            let _ = handle(conn, &shared); // a dropped connection is not an error
        });
    }
}

fn handle(conn: Stream, shared: &Shared) -> std::io::Result<()> {
    let (recv, send) = conn.split();
    let mut reader = BufReader::new(recv);
    let mut writer = BufWriter::new(send);
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Ok(());
        }
        let response = respond(&line, shared);
        writer.write_all(response.to_string().as_bytes())?;
        writer.write_all(b"\n")?;
        writer.flush()?;
    }
}

#[derive(Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
enum Request {
    Ping,
    Status,
    Profile,
    SetProfile { profile: Box<Profile> },
    /// Apply an edited profile IN PLACE — the settings fingerprint picks the
    /// change up next tick and instant-applies it, exactly like a slider in
    /// the WPF app. Unlike `set_profile` this keeps engine state (averaging
    /// windows, kick/probe clocks) so a knob tweak is not a measurement reset.
    UpdateProfile { profile: Box<Profile> },
    /// Everything the SOURCES panel needs: each sensor/control with its live
    /// reading, so rows can lead with the temp/rpm like the WPF panel does.
    Inventory,
    Preset { name: String },
    Apply,
    Pause,
    Shutdown,
}

fn respond(line: &str, shared: &Shared) -> serde_json::Value {
    let request = match serde_json::from_str::<Request>(line) {
        Ok(r) => r,
        Err(e) => return json!({ "ok": false, "error": format!("bad request: {e}") }),
    };
    match request {
        Request::Ping => {
            let engine = shared.engine.lock().unwrap();
            json!({
                "ok": true,
                "version": env!("CARGO_PKG_VERSION"),
                "simulated": engine.backend().is_simulated(),
                "backend": engine.backend().description(),
            })
        }
        Request::Status => {
            let engine = shared.engine.lock().unwrap();
            let latest = shared.latest.lock().unwrap();
            json!({
                "ok": true,
                "applying": engine.applying(),
                "profile_name": engine.profile().name,
                "channels": *latest,
            })
        }
        Request::Profile => {
            let engine = shared.engine.lock().unwrap();
            json!({ "ok": true, "read_only": shared.read_only, "profile": engine.profile() })
        }
        Request::SetProfile { profile } => {
            if profile.channels.is_empty() {
                return json!({ "ok": false, "error": "profile has no channels" });
            }
            let mut engine = shared.engine.lock().unwrap();
            engine.replace_profile(*profile);
            let saved = save_if_allowed(shared, engine.profile());
            shared.telemetry.lock().unwrap().event("profile replaced via IPC");
            json!({ "ok": true, "saved": saved })
        }
        Request::UpdateProfile { profile } => {
            if profile.channels.is_empty() {
                return json!({ "ok": false, "error": "profile has no channels" });
            }
            let mut engine = shared.engine.lock().unwrap();
            engine.profile_mut().apply_settings(&profile);
            let saved = save_if_allowed(shared, engine.profile());
            json!({ "ok": true, "saved": saved })
        }
        Request::Inventory => {
            let engine = shared.engine.lock().unwrap();
            let hw = engine.backend();
            let sensors: Vec<_> = hw
                .sensors()
                .iter()
                .map(|s| {
                    json!({
                        "id": s.id,
                        "name": s.name,
                        "kind": match s.kind { SensorKind::Temp => "temp", SensorKind::Rpm => "rpm" },
                        "value": hw.read_value(&s.id),
                    })
                })
                .collect();
            let controls: Vec<_> = hw
                .controls()
                .iter()
                .map(|c| json!({ "id": c.id, "name": c.name, "rpm": hw.read_control_rpm(&c.id) }))
                .collect();
            json!({
                "ok": true,
                "backend": hw.description(),
                "simulated": hw.is_simulated(),
                "config_path": shared.profile_path.to_string_lossy(),
                "read_only": shared.read_only,
                "sensors": sensors,
                "controls": controls,
            })
        }
        Request::Preset { name } => {
            let preset = match name.as_str() {
                "quiet" => Profile::mac_book_like(),
                "performance" => Profile::performance(),
                other => return json!({ "ok": false, "error": format!("unknown preset: {other}") }),
            };
            let mut engine = shared.engine.lock().unwrap();
            engine.profile_mut().adopt_tuning(&preset);
            // The changed settings fingerprint instant-applies on the next tick,
            // exactly like a preset click in the WPF UI.
            let saved = save_if_allowed(shared, engine.profile());
            shared.telemetry.lock().unwrap().event(&format!("preset adopted via IPC: {}", preset.name));
            json!({ "ok": true, "profile_name": engine.profile().name, "saved": saved })
        }
        Request::Apply => {
            shared.engine.lock().unwrap().apply();
            shared.telemetry.lock().unwrap().event("apply via IPC");
            json!({ "ok": true })
        }
        Request::Pause => {
            shared.engine.lock().unwrap().stop_applying();
            shared.telemetry.lock().unwrap().event("paused via IPC (headers to BIOS)");
            json!({ "ok": true })
        }
        Request::Shutdown => {
            shared.telemetry.lock().unwrap().event("shutdown requested via IPC");
            shared.stop.store(true, Ordering::SeqCst);
            json!({ "ok": true, "stopping": true })
        }
    }
}

fn save_if_allowed(shared: &Shared, profile: &Profile) -> bool {
    if shared.read_only {
        return false;
    }
    profile.save(&shared.profile_path).is_ok()
}

/// Client side: send one request line, print the one response line (`--send`).
pub fn send(request: &str) -> std::io::Result<String> {
    let name = SOCKET_NAME.to_ns_name::<GenericNamespaced>()?;
    let conn = Stream::connect(name)?;
    let (recv, send_half) = conn.split();
    let mut writer = BufWriter::new(send_half);
    writer.write_all(request.as_bytes())?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    let mut reply = String::new();
    BufReader::new(recv).read_line(&mut reply)?;
    Ok(reply.trim_end().to_string())
}
