//! IPC endpoint for the on-demand UI (and CLI): a local socket — named pipe on
//! Windows, Unix socket elsewhere — speaking line-delimited JSON. One request
//! line in, one response line out; connections may pipeline.
//!
//! Commands:
//!   {"cmd":"ping"}                     → {"ok":true,"version":"…","simulated":true}
//!   {"cmd":"status"}                   → applying + profile name + latest tick statuses
//!                                        + per-channel history {first,total} so a client
//!                                        knows how far behind its mirror is
//!   {"cmd":"history","channel":N,"start":S,"count":C} → up to C samples from absolute
//!                                        index S (clamped to what is retained; the reply
//!                                        says where the samples really start)
//!   {"cmd":"clear_history"}            → wipe every channel's recorded history
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
use fan_core::history::ChannelHistory;
use fan_core::{ChannelStatus, FanEngine, HardwareBackend as _, Profile};

use crate::telemetry::TelemetryLog;
use crate::Backend;

/// The most samples one `history` reply will carry (~110 KB of JSON); a
/// backfilling client loops until it has caught up.
const HISTORY_CHUNK: usize = 4096;

/// Everything the tick loop and the IPC handlers share.
pub struct Shared {
    pub engine: Mutex<FanEngine<Backend>>,
    pub latest: Mutex<Vec<ChannelStatus>>,
    /// Per-channel strip history, recorded every tick. The daemon is the only
    /// resident process, so this is the only place a sample can survive while
    /// no UI window is open — the UI backfills its mirror from here on connect
    /// instead of starting the strip from scratch.
    pub history: Mutex<Vec<ChannelHistory>>,
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
    let name = fan_core::ipc_socket_name().to_ns_name::<GenericNamespaced>()?;
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
    /// A range of recorded strip samples, addressed by ABSOLUTE sample index
    /// (the same indices a scrolled viewport anchors on — they never rewind).
    History { channel: usize, start: usize, count: usize },
    /// Wipe the recorded history on every channel (the strip's CLEAR).
    ClearHistory,
    Preset { name: String },
    Apply,
    Pause,
    Shutdown,
    /// Register/remove the start-with-Windows task. The daemon already runs
    /// elevated, so the UI can ask for this without elevating itself.
    SetAutostart { enabled: bool },
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
            // Per-channel retention bounds, so a mirroring client can tell how
            // far behind it is and fetch exactly the missing range.
            let history: Vec<_> = shared
                .history
                .lock()
                .unwrap()
                .iter()
                .map(|h| json!({ "first": h.first_available(), "total": h.total() }))
                .collect();
            json!({
                "ok": true,
                "applying": engine.applying(),
                "profile_name": engine.profile().name,
                "channels": *latest,
                "history": history,
            })
        }
        Request::History { channel, start, count } => {
            let mut history = shared.history.lock().unwrap();
            let Some(h) = history.get_mut(channel) else {
                return json!({ "ok": false, "error": format!("no channel {channel}") });
            };
            let (start, samples) = h.read(start, count.min(HISTORY_CHUNK));
            // Compact rows, not objects: a 24 h backfill is ~86k samples and
            // the field names would triple the bytes on the pipe.
            let rows: Vec<_> = samples
                .iter()
                .map(|s| {
                    json!([
                        s.wall,
                        if s.avg.is_nan() { serde_json::Value::Null } else { json!(s.avg) },
                        s.raw,
                        s.out,
                    ])
                })
                .collect();
            json!({
                "ok": true,
                "start": start,
                "first": h.first_available(),
                "total": h.total(),
                "samples": rows,
            })
        }
        Request::ClearHistory => {
            for h in shared.history.lock().unwrap().iter_mut() {
                h.clear();
            }
            shared.telemetry.lock().unwrap().event("history cleared via IPC");
            json!({ "ok": true })
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
            #[cfg(windows)]
            let (autostart, autostart_conflict) =
                (crate::autostart::installed(), crate::autostart::conflicting_wpf_task());
            #[cfg(not(windows))]
            let (autostart, autostart_conflict) = (false, false);
            json!({
                "ok": true,
                "backend": hw.description(),
                "simulated": hw.is_simulated(),
                "config_path": shared.profile_path.to_string_lossy(),
                "read_only": shared.read_only,
                "autostart": autostart,
                "autostart_conflict": autostart_conflict,
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
        Request::SetAutostart { enabled } => {
            #[cfg(windows)]
            {
                let r = if enabled {
                    crate::autostart::install()
                } else {
                    crate::autostart::uninstall()
                };
                match r {
                    Ok(()) => {
                        shared.telemetry.lock().unwrap().event(&format!(
                            "autostart {} via IPC",
                            if enabled { "enabled" } else { "disabled" }
                        ));
                        json!({
                            "ok": true,
                            "autostart": crate::autostart::installed(),
                            "autostart_conflict": crate::autostart::conflicting_wpf_task(),
                        })
                    }
                    Err(e) => json!({ "ok": false, "error": e.to_string() }),
                }
            }
            #[cfg(not(windows))]
            {
                let _ = enabled;
                json!({ "ok": false, "error": "autostart is Windows-only for now" })
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use fan_core::history::HistorySample;
    use fan_core::SimulatedBackend;

    fn test_shared() -> Shared {
        let dir = std::env::temp_dir().join("FanCurvesIpcTest");
        Shared {
            engine: Mutex::new(FanEngine::new(
                Backend::Sim(SimulatedBackend::new()),
                Profile::mac_book_like(),
            )),
            latest: Mutex::new(Vec::new()),
            history: Mutex::new(Vec::new()),
            telemetry: Mutex::new(TelemetryLog::new(dir.clone(), true, 0)),
            profile_path: dir.join("profile.json"),
            read_only: true,
            stop: AtomicBool::new(false),
        }
    }

    fn sample(i: usize) -> HistorySample {
        HistorySample {
            wall: 1_700_000_000.0 + i as f64,
            avg: 40.0 + (i % 10) as f64 / 10.0,
            raw: Some(41.5),
            out: if i % 2 == 0 { 20.0 } else { 0.0 },
        }
    }

    #[test]
    fn status_history_and_clear_round_trip() {
        let s = test_shared();
        {
            let mut h = s.history.lock().unwrap();
            h.resize_with(2, Default::default);
            for i in 0..5000 {
                h[0].push(sample(i));
            }
            // A gap: channel 0 sample 100 has no readings at all.
            h[1].push(HistorySample { wall: 1_700_000_000.0, avg: f64::NAN, raw: None, out: 0.0 });
        }

        // status carries per-channel retention bounds.
        let r = respond(r#"{"cmd":"status"}"#, &s);
        assert_eq!(r["ok"], true);
        assert_eq!(r["history"][0]["first"], 0);
        assert_eq!(r["history"][0]["total"], 5000);
        assert_eq!(r["history"][1]["total"], 1);

        // Ranged fetch from the spill, exact position, compact rows.
        let r = respond(r#"{"cmd":"history","channel":0,"start":10,"count":50}"#, &s);
        assert_eq!(r["ok"], true);
        assert_eq!(r["start"], 10);
        let rows = r["samples"].as_array().unwrap();
        assert_eq!(rows.len(), 50);
        assert_eq!(rows[0][0].as_f64().unwrap(), 1_700_000_010.0);
        assert!((rows[0][1].as_f64().unwrap() - sample(10).avg).abs() < 0.051);
        assert_eq!(rows[0][3].as_f64().unwrap(), 20.0);

        // An oversized request is capped at the chunk size, not refused.
        let r = respond(r#"{"cmd":"history","channel":0,"start":0,"count":100000}"#, &s);
        assert_eq!(r["samples"].as_array().unwrap().len(), HISTORY_CHUNK);

        // Missing readings come back as JSON nulls.
        let r = respond(r#"{"cmd":"history","channel":1,"start":0,"count":10}"#, &s);
        let rows = r["samples"].as_array().unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0][1].is_null());
        assert!(rows[0][2].is_null());

        // Unknown channel is an error, not a panic.
        let r = respond(r#"{"cmd":"history","channel":9,"start":0,"count":1}"#, &s);
        assert_eq!(r["ok"], false);

        // clear_history moves every channel's floor to its total.
        let r = respond(r#"{"cmd":"clear_history"}"#, &s);
        assert_eq!(r["ok"], true);
        let r = respond(r#"{"cmd":"status"}"#, &s);
        assert_eq!(r["history"][0]["first"], 5000);
        assert_eq!(r["history"][0]["total"], 5000);
        let r = respond(r#"{"cmd":"history","channel":0,"start":0,"count":10}"#, &s);
        assert_eq!(r["start"], 5000);
        assert!(r["samples"].as_array().unwrap().is_empty());
    }
}

/// Client side: send one request line, print the one response line (`--send`).
pub fn send(request: &str) -> std::io::Result<String> {
    let name = fan_core::ipc_socket_name().to_ns_name::<GenericNamespaced>()?;
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
