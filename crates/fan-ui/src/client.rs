//! IPC client + poll thread. One background thread owns the socket: it polls
//! `status` once a second, executes UI commands immediately when they arrive
//! (the channel's recv timeout IS the poll pacing), and wakes egui with
//! `request_repaint()` only when fresh data landed — the UI never animates on
//! its own, per the repaint rules.
//!
//! History is MIRRORED from the daemon, not collected here: the daemon records
//! a sample every engine tick around the clock, and on (re)connect this thread
//! backfills everything the daemon retains (~24 h) before following along one
//! sample per poll — so the strip is populated the moment the window opens
//! instead of starting from scratch.
//!
//! A normal launch waits for the hardware daemon. Only explicit `--sim` mode
//! may spawn a sibling simulated daemon, using its isolated socket.

use std::io::{BufRead, BufReader, BufWriter, Write};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use interprocess::local_socket::traits::Stream as _;
use interprocess::local_socket::{GenericNamespaced, RecvHalf, SendHalf, Stream, ToNsName};
use serde_json::{json, Value};

use fan_core::history::ChannelHistory;
pub use fan_core::history::HistorySample;
use fan_core::{ChannelStatus, Profile};

/// Samples per `history` request while backfilling (the daemon caps there too).
const HISTORY_CHUNK: usize = 4096;
/// Missing samples (all channels) above which a poll shows the live status
/// before fetching history. Steady state is one sample per channel.
const PUBLISH_BEFORE_BACKFILL: usize = 64;

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
    /// The start-with-Windows tasks are registered.
    pub autostart: bool,
    pub sensors: Vec<SourceItem>,
    pub controls: Vec<SourceItem>,
}

#[derive(Default)]
pub struct UiState {
    pub connected: bool,
    pub daemon_version: String,
    pub applying: bool,
    pub full_speed: bool,
    /// Clicks sent to the daemon but not yet acknowledged. The daemon remains
    /// authoritative; their parity only predicts the button's next state.
    pub pending_full_speed_toggles: usize,
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
    ToggleFullSpeed,
    /// Push edited settings; applied in place so engine state survives.
    Update(Box<Profile>),
    SetAutostart(bool),
    /// Wipe the recorded history, daemon-side and mirror alike (CLEAR).
    ClearHistory,
}

pub struct Link {
    pub state: Arc<Mutex<UiState>>,
    pub tx: Sender<Cmd>,
}

impl Link {
    /// Show the click immediately, while the worker keeps the daemon toggle
    /// atomic and reconciles its answer when the hardware write completes.
    pub fn toggle_full_speed(&self, ctx: &eframe::egui::Context) {
        let mut st = self.state.lock().unwrap();
        st.pending_full_speed_toggles += 1;
        if self.tx.send(Cmd::ToggleFullSpeed).is_err() {
            st.pending_full_speed_toggles -= 1;
        }
        drop(st);
        ctx.request_repaint();
    }
}

impl UiState {
    pub fn displayed_full_speed(&self) -> bool {
        self.full_speed ^ !self.pending_full_speed_toggles.is_multiple_of(2)
    }
}

pub fn start(ctx: eframe::egui::Context, sim: bool) -> Link {
    let state = Arc::new(Mutex::new(UiState::default()));
    let (tx, rx) = std::sync::mpsc::channel();
    {
        let state = Arc::clone(&state);
        std::thread::spawn(move || worker(ctx, state, rx, sim));
    }
    Link { state, tx }
}

struct Conn {
    reader: BufReader<RecvHalf>,
    writer: BufWriter<SendHalf>,
}

impl Conn {
    fn open() -> std::io::Result<Self> {
        let name = fan_core::ipc_socket_name().to_ns_name::<GenericNamespaced>()?;
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

fn worker(ctx: eframe::egui::Context, state: Arc<Mutex<UiState>>, rx: Receiver<Cmd>, sim: bool) {
    let mut conn: Option<Conn> = None;
    let mut spawned_daemon = false;
    // Daemon-side absolute index each channel's mirror has caught up to.
    // Daemon-relative, so it resets with the mirror on every (re)connect.
    let mut synced: Vec<usize> = Vec::new();

    loop {
        if conn.is_none() {
            match Conn::open() {
                Ok(mut c) => {
                    let hello = c.call(json!({"cmd": "ping"})).ok();
                    let profile = c.call(json!({"cmd": "profile"})).ok();
                    // No inventory here: the status poll right below fetches
                    // it anyway, before the history backfill.
                    let mut st = state.lock().unwrap();
                    st.connected = true;
                    st.last_error = None;
                    if let Some(h) = hello {
                        st.daemon_version =
                            h["version"].as_str().unwrap_or_default().to_string();
                    }
                    apply_profile_reply(&mut st, profile);
                    // Start the mirror over: sample indices are daemon-relative
                    // and this may be a different daemon than last time. What
                    // the daemon retains (~24 h) comes back in the backfill.
                    st.history.clear();
                    drop(st);
                    synced.clear();
                    conn = Some(c);
                    ctx.request_repaint();
                    // First status RIGHT NOW: the recv_timeout below would
                    // otherwise sit out a full second before the first poll,
                    // and the freshly opened window would show an empty hero
                    // and no channels for exactly that long.
                    poll_status(&ctx, &mut conn, &state, &mut synced);
                }
                Err(e) => {
                    {
                        let mut st = state.lock().unwrap();
                        st.connected = false;
                        st.last_error = Some(format!("daemon unreachable: {e}"));
                    }
                    ctx.request_repaint();
                    if sim && !spawned_daemon {
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
                    matches!(cmd, Cmd::SetAutostart(_));
                let clear_history = matches!(cmd, Cmd::ClearHistory);
                let full_speed_toggle = matches!(cmd, Cmd::ToggleFullSpeed);
                let request = match cmd {
                    Cmd::Preset(name) => json!({"cmd": "preset", "name": name}),
                    Cmd::Apply => json!({"cmd": "apply"}),
                    Cmd::Pause => json!({"cmd": "pause"}),
                    Cmd::ToggleFullSpeed => json!({"cmd": "toggle_full_speed"}),
                    Cmd::Update(profile) => json!({"cmd": "update_profile", "profile": profile}),
                    Cmd::SetAutostart(on) => json!({"cmd": "set_autostart", "enabled": on}),
                    Cmd::ClearHistory => json!({"cmd": "clear_history"}),
                };
                let reply = run_call(&mut conn, &state, request);
                if full_speed_toggle {
                    let mut st = state.lock().unwrap();
                    if let Some(enabled) = reply.as_ref().and_then(|r| r["full_speed"].as_bool()) {
                        st.full_speed = enabled;
                    }
                    st.pending_full_speed_toggles = st.pending_full_speed_toggles.saturating_sub(1);
                    drop(st);
                    ctx.request_repaint();
                }
                if clear_history {
                    // `synced` is daemon-side position — a clear moves the
                    // daemon's floor, not its total — so it stands.
                    let mut st = state.lock().unwrap();
                    for h in st.history.iter_mut() {
                        h.clear();
                    }
                    drop(st);
                    ctx.request_repaint();
                } else if refresh_inventory {
                    apply_inventory_reply(&mut state.lock().unwrap(), reply);
                } else if reply.is_some() && refetch_profile {
                    let reply = run_call(&mut conn, &state, json!({"cmd": "profile"}));
                    apply_profile_reply(&mut state.lock().unwrap(), reply);
                }
                // The toggle reply already contains its authoritative state.
                // Do not queue another hardware inventory read ahead of a
                // second click; the regular poll will refresh channel data.
                if !full_speed_toggle {
                    poll_status(&ctx, &mut conn, &state, &mut synced);
                }
            }
            Err(RecvTimeoutError::Timeout) => poll_status(&ctx, &mut conn, &state, &mut synced),
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
    synced: &mut Vec<usize>,
) {
    let Some(reply) = run_call(conn, state, json!({"cmd": "status"})) else { return };
    let channels: Vec<ChannelStatus> =
        serde_json::from_value(reply["channels"].clone()).unwrap_or_default();
    // Source rows include every discovered temperature sensor and fan RPM.
    // Refresh their readings with status so the developer panel stays live.
    let mut inventory = run_call(conn, state, json!({"cmd": "inventory"}));

    // Mirror the daemon's history: fetch whatever it has that we do not. On
    // first connect that is the whole backfill (up to ~24 h, chunked); in
    // steady state it is the one sample the last tick appended, served from
    // the daemon's RAM ring with no disk I/O on either side. All IPC happens
    // BEFORE taking the state lock.
    let mut fetched: Vec<(usize, Vec<HistorySample>)> = Vec::new();
    if let Some(infos) = reply["history"].as_array() {
        synced.resize(channels.len(), 0);
        // Where each channel's fetch starts. Anything before the daemon's
        // floor (CLEAR, compaction) is gone.
        let ranges: Vec<(usize, usize)> = infos
            .iter()
            .take(channels.len())
            .enumerate()
            .map(|(i, info)| {
                let first = info["first"].as_u64().unwrap_or(0) as usize;
                let total = info["total"].as_u64().unwrap_or(0) as usize;
                (synced[i].min(total).max(first), total)
            })
            .collect();
        // A first-connect backfill is up to ~86k samples per channel and
        // takes up to a second to replay. Show the live readings first, so the
        // window is not empty meanwhile; the strip fills in after.
        let backlog: usize = ranges.iter().map(|(have, total)| total - have).sum();
        if backlog > PUBLISH_BEFORE_BACKFILL {
            apply_status(&mut state.lock().unwrap(), &reply, &channels, inventory.take());
            ctx.request_repaint();
        }
        for (i, (mut have, total)) in ranges.into_iter().enumerate() {
            while have < total {
                let want = (total - have).min(HISTORY_CHUNK);
                let request = json!({"cmd": "history", "channel": i, "start": have, "count": want});
                let Some(r) = run_call(conn, state, request) else {
                    synced[i] = have;
                    return; // connection dropped; position is kept for the retry
                };
                let got = r["samples"].as_array().map_or(0, |a| a.len());
                if r["ok"].as_bool() != Some(true) || got == 0 {
                    have = total; // nothing more retrievable — do not spin
                    break;
                }
                // The daemon may bump the start past discarded territory.
                let start = r["start"].as_u64().map_or(have, |s| s as usize);
                fetched.push((i, parse_samples(&r["samples"])));
                have = start + got;
            }
            synced[i] = have;
        }
    }

    let mut st = state.lock().unwrap();
    // `inventory` is None here when it already went out before the backfill.
    apply_status(&mut st, &reply, &channels, inventory);
    for (i, samples) in fetched {
        for s in samples {
            st.history[i].push(s);
        }
    }
    drop(st);
    // Fresh data: the one place the steady-state poll asks to paint (a big
    // backfill also paints once before it starts).
    ctx.request_repaint();
}

/// Everything a `status` + `inventory` pair carries, except history samples.
fn apply_status(st: &mut UiState, reply: &Value, channels: &[ChannelStatus], inventory: Option<Value>) {
    apply_inventory_reply(st, inventory);
    st.applying = reply["applying"].as_bool().unwrap_or(false);
    st.full_speed = reply["full_speed"].as_bool().unwrap_or(false);
    if let Some(name) = reply["profile_name"].as_str() {
        st.profile_name = name.to_string();
    }
    st.history.resize_with(channels.len(), ChannelHistory::new);
    st.channels = channels.to_vec();
}

/// Decode the `history` reply's compact rows: `[wall, avg|null, raw|null, out]`.
fn parse_samples(rows: &Value) -> Vec<HistorySample> {
    rows.as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|r| {
                    let r = r.as_array()?;
                    Some(HistorySample {
                        wall: r.first()?.as_f64()?,
                        avg: r.get(1).and_then(Value::as_f64).unwrap_or(f64::NAN),
                        raw: r.get(2).and_then(Value::as_f64),
                        out: r.get(3).and_then(Value::as_f64).unwrap_or(0.0),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
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
        sensors: items("sensors", false),
        controls: items("controls", true),
    };
}

/// Stop the daemon behind the current socket (the simulation's own).
pub fn shutdown_daemon() {
    if let Ok(mut conn) = Conn::open() {
        let _ = conn.call(json!({"cmd": "shutdown"}));
    }
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
