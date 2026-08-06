//! IPC client + poll thread. One background thread owns the socket: it polls
//! `status` once a second, executes UI commands immediately when they arrive
//! (the channel's recv timeout IS the poll pacing), and wakes egui with
//! `request_repaint()` only when fresh data landed — the UI never animates on
//! its own, per the repaint rules.
//!
//! If no daemon answers at startup, it spawns a sibling `fan-daemon --sim`
//! once and keeps retrying.

use std::io::{BufRead, BufReader, BufWriter, Write};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use interprocess::local_socket::traits::Stream as _;
use interprocess::local_socket::{GenericNamespaced, RecvHalf, SendHalf, Stream, ToNsName};
use serde_json::{json, Value};

use fan_core::{ChannelStatus, Profile};

use crate::history::ChannelHistory;

pub const SOCKET_NAME: &str = "fan-curves-daemon.sock";

#[derive(Clone, Copy)]
pub struct HistorySample {
    /// Seconds since the UI connected — drives "how long ago".
    pub t: f64,
    /// Unix seconds, so the hover chip can show a wall clock.
    pub wall: f64,
    pub avg: f64,
    pub raw: Option<f64>,
    pub out: f64,
}

/// One row of the SOURCES panel.
#[derive(Clone, Default)]
pub struct SourceItem {
    pub id: String,
    pub name: String,
    /// Live temperature (sensors) or rpm (controls); None = unreadable.
    pub value: Option<f64>,
    pub is_rpm: bool,
}

#[derive(Clone, Default)]
pub struct Inventory {
    pub backend: String,
    pub simulated: bool,
    pub config_path: String,
    pub read_only: bool,
    /// The daemon's start-with-Windows task is registered.
    pub autostart: bool,
    /// The WPF app's task is ALSO registered — both would fight at logon.
    pub autostart_conflict: bool,
    pub sensors: Vec<SourceItem>,
    pub controls: Vec<SourceItem>,
}

#[derive(Default)]
pub struct UiState {
    pub connected: bool,
    pub daemon_version: String,
    pub applying: bool,
    pub profile_name: String,
    pub read_only: bool,
    pub channels: Vec<ChannelStatus>,
    /// The live profile as last seen from the daemon. The UI edits its own
    /// copy (`App::draft`) and pushes it; this is refreshed on connect, on a
    /// preset switch, and whenever a push reports back.
    pub profile: Option<Profile>,
    pub inventory: Inventory,
    pub history: Vec<ChannelHistory>,
    pub last_error: Option<String>,
    /// Bumped whenever `profile` is replaced from the daemon, so the UI knows
    /// to re-seed its draft instead of clobbering a fresh remote state.
    pub profile_epoch: u64,
}

pub enum Cmd {
    Preset(&'static str),
    Apply,
    Pause,
    /// Push edited settings; applied in place so engine state survives.
    Update(Box<Profile>),
    RefreshInventory,
    SetAutostart(bool),
}

pub struct Link {
    pub state: Arc<Mutex<UiState>>,
    pub tx: Sender<Cmd>,
}

pub fn start(ctx: eframe::egui::Context) -> Link {
    let state = Arc::new(Mutex::new(UiState::default()));
    let (tx, rx) = std::sync::mpsc::channel();
    {
        let state = Arc::clone(&state);
        std::thread::spawn(move || worker(ctx, state, rx));
    }
    Link { state, tx }
}

struct Conn {
    reader: BufReader<RecvHalf>,
    writer: BufWriter<SendHalf>,
}

impl Conn {
    fn open() -> std::io::Result<Self> {
        let name = SOCKET_NAME.to_ns_name::<GenericNamespaced>()?;
        let stream = Stream::connect(name)?;
        let (recv, send) = stream.split();
        Ok(Self { reader: BufReader::new(recv), writer: BufWriter::new(send) })
    }

    fn call(&mut self, request: Value) -> std::io::Result<Value> {
        self.writer.write_all(request.to_string().as_bytes())?;
        self.writer.write_all(b"\n")?;
        self.writer.flush()?;
        let mut line = String::new();
        if self.reader.read_line(&mut line)? == 0 {
            return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "daemon closed"));
        }
        serde_json::from_str(&line)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }
}

fn worker(ctx: eframe::egui::Context, state: Arc<Mutex<UiState>>, rx: Receiver<Cmd>) {
    let start = Instant::now();
    let mut conn: Option<Conn> = None;
    let mut spawned_daemon = false;

    loop {
        if conn.is_none() {
            match Conn::open() {
                Ok(mut c) => {
                    let hello = c.call(json!({"cmd": "ping"})).ok();
                    let profile = c.call(json!({"cmd": "profile"})).ok();
                    let inventory = c.call(json!({"cmd": "inventory"})).ok();
                    let mut st = state.lock().unwrap();
                    st.connected = true;
                    st.last_error = None;
                    if let Some(h) = hello {
                        st.daemon_version =
                            h["version"].as_str().unwrap_or_default().to_string();
                    }
                    apply_profile_reply(&mut st, profile);
                    apply_inventory_reply(&mut st, inventory);
                    drop(st);
                    conn = Some(c);
                    ctx.request_repaint();
                    // First status RIGHT NOW: the recv_timeout below would
                    // otherwise sit out a full second before the first poll,
                    // and the freshly opened window would show an empty hero
                    // and no channels for exactly that long.
                    poll_status(&ctx, &mut conn, &state, start);
                }
                Err(e) => {
                    {
                        let mut st = state.lock().unwrap();
                        st.connected = false;
                        st.last_error = Some(format!("daemon unreachable: {e}"));
                    }
                    ctx.request_repaint();
                    if !spawned_daemon {
                        spawned_daemon = true;
                        spawn_sibling_daemon();
                    }
                }
            }
        }

        // The recv timeout is the 1 Hz status pacing; a command wakes us early.
        match rx.recv_timeout(Duration::from_millis(1000)) {
            Ok(cmd) => {
                // A preset rewrites the whole profile daemon-side, so the UI
                // must re-read it; an edit push is authoritative already.
                let refetch_profile = matches!(cmd, Cmd::Preset(_));
                // Autostart changes what the inventory reports, so re-read it.
                let refresh_inventory =
                    matches!(cmd, Cmd::RefreshInventory | Cmd::SetAutostart(_));
                let request = match cmd {
                    Cmd::Preset(name) => json!({"cmd": "preset", "name": name}),
                    Cmd::Apply => json!({"cmd": "apply"}),
                    Cmd::Pause => json!({"cmd": "pause"}),
                    Cmd::Update(profile) => json!({"cmd": "update_profile", "profile": profile}),
                    Cmd::RefreshInventory => json!({"cmd": "inventory"}),
                    Cmd::SetAutostart(on) => json!({"cmd": "set_autostart", "enabled": on}),
                };
                let reply = run_call(&mut conn, &state, request);
                if refresh_inventory {
                    apply_inventory_reply(&mut state.lock().unwrap(), reply);
                } else if reply.is_some() && refetch_profile {
                    let reply = run_call(&mut conn, &state, json!({"cmd": "profile"}));
                    apply_profile_reply(&mut state.lock().unwrap(), reply);
                }
                poll_status(&ctx, &mut conn, &state, start);
            }
            Err(RecvTimeoutError::Timeout) => poll_status(&ctx, &mut conn, &state, start),
            Err(RecvTimeoutError::Disconnected) => return, // UI is gone
        }
    }
}

/// Issue one request; on I/O failure drop the connection so the next loop
/// reconnects. Returns the reply when the daemon answered.
fn run_call(conn: &mut Option<Conn>, state: &Arc<Mutex<UiState>>, request: Value) -> Option<Value> {
    let c = conn.as_mut()?;
    match c.call(request) {
        Ok(v) => Some(v),
        Err(e) => {
            *conn = None;
            let mut st = state.lock().unwrap();
            st.connected = false;
            st.last_error = Some(format!("lost daemon: {e}"));
            None
        }
    }
}

fn poll_status(
    ctx: &eframe::egui::Context,
    conn: &mut Option<Conn>,
    state: &Arc<Mutex<UiState>>,
    start: Instant,
) {
    let Some(reply) = run_call(conn, state, json!({"cmd": "status"})) else { return };
    let channels: Vec<ChannelStatus> =
        serde_json::from_value(reply["channels"].clone()).unwrap_or_default();
    let t = start.elapsed().as_secs_f64();
    let wall = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    let mut st = state.lock().unwrap();
    st.applying = reply["applying"].as_bool().unwrap_or(false);
    if let Some(name) = reply["profile_name"].as_str() {
        st.profile_name = name.to_string();
    }
    st.history.resize_with(channels.len(), ChannelHistory::new);
    for (i, ch) in channels.iter().enumerate() {
        // A missing reading is RECORDED, not skipped: the strip draws it as a
        // break in the trace, which is how a dead sensor should read.
        st.history[i].push(HistorySample {
            t,
            wall,
            avg: ch.effective_temp,
            raw: ch.raw_temp,
            out: ch.output_percent,
        });
    }
    st.channels = channels;
    drop(st);
    ctx.request_repaint(); // fresh data — the ONE place the UI asks to paint
}

fn apply_profile_reply(st: &mut UiState, reply: Option<Value>) {
    if let Some(r) = reply {
        st.read_only = r["read_only"].as_bool().unwrap_or(false);
        if let Ok(p) = serde_json::from_value::<Profile>(r["profile"].clone()) {
            st.profile = Some(p);
            st.profile_epoch = st.profile_epoch.wrapping_add(1);
        }
    }
}

fn apply_inventory_reply(st: &mut UiState, reply: Option<Value>) {
    let Some(r) = reply else { return };
    let items = |key: &str, is_rpm: bool| -> Vec<SourceItem> {
        r[key]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|v| SourceItem {
                        id: v["id"].as_str().unwrap_or_default().to_string(),
                        name: v["name"].as_str().unwrap_or_default().to_string(),
                        value: v[if is_rpm { "rpm" } else { "value" }].as_f64(),
                        is_rpm: is_rpm || v["kind"].as_str() == Some("rpm"),
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    st.inventory = Inventory {
        backend: r["backend"].as_str().unwrap_or_default().to_string(),
        simulated: r["simulated"].as_bool().unwrap_or(true),
        config_path: r["config_path"].as_str().unwrap_or_default().to_string(),
        read_only: r["read_only"].as_bool().unwrap_or(false),
        autostart: r["autostart"].as_bool().unwrap_or(false),
        autostart_conflict: r["autostart_conflict"].as_bool().unwrap_or(false),
        sensors: items("sensors", false),
        controls: items("controls", true),
    };
}

/// Best-effort: start `fan-daemon --sim` from the UI binary's directory.
fn spawn_sibling_daemon() {
    let Ok(me) = std::env::current_exe() else { return };
    let Some(dir) = me.parent() else { return };
    let name = if cfg!(windows) { "fan-daemon.exe" } else { "fan-daemon" };
    let path = dir.join(name);
    if path.exists() {
        let _ = std::process::Command::new(path).arg("--sim").spawn();
    }
}
