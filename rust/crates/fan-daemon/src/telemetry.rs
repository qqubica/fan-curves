//! Port of `TelemetryLog.cs`: the review log — everything the engine saw and
//! everything it decided, written so a human can point at a moment and say
//! "this is the part I don't like". Two files under `<config>/logs/`:
//!
//! - `telemetry-YYYY-MM-DD.csv` — one row per channel per engine tick with every
//!   controller input and output. Rotated daily; the last 7 days are kept.
//! - `behavior.txt` — human-readable CHANGES only: fan start/stop, target steps,
//!   reason transitions, driving/released flips, `·· <event>` markers, and a full
//!   settings line whenever any tuning knob or the curve changes.
//!
//! Sim runs write `telemetry-sim-*.csv` / `behavior-sim.txt` so a simulation
//! never contaminates the real machine's record. Logging must never take fan
//! control down: every write is guarded, and the first failure disables the log
//! for the rest of the session. Files are UTF-8 WITH BOM — Windows PowerShell
//! 5.1 reads BOM-less UTF-8 as ANSI and mangles °/·/∞.
//!
//! The CSV schema, number formats and the behavior vocabulary are kept
//! IDENTICAL to the C# writer so existing analysis tooling parses both.

use std::fmt::Write as _;
use std::fs;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

use fan_core::{ChannelStatus, OutputReason, Profile};

/// Wall-clock timestamp pieces, derived from the UTC offset captured at startup.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Day {
    year: i32,
    month: u8,
    day: u8,
}

pub struct TelemetryLog {
    dir: PathBuf,
    csv_prefix: &'static str,
    behavior_path: PathBuf,
    utc_offset_secs: i64,

    csv: Option<BufWriter<fs::File>>,
    csv_day: Option<Day>,
    behavior: Option<BufWriter<fs::File>>,
    last_flush: Instant,
    dead: bool,

    prev: Vec<ChannelStatus>,
    last_settings: Option<String>,
}

const KEEP_DAYS: u64 = 7;
const BEHAVIOR_MAX_BYTES: u64 = 4 * 1024 * 1024;
const CSV_HEADER: &str = "time,channel,applied,out_pct,target_pct,reason,reason_level,reason_s,\
raw_c,avg_c,rpm,rpm_per_header";

impl TelemetryLog {
    /// `utc_offset_secs` comes from the caller because local-offset lookup is
    /// only safe before other threads exist; a DST flip mid-run shifts the log
    /// timestamps by an hour until restart (the C# writer tracks DST live —
    /// accepted divergence, noted in the README).
    pub fn new(config_dir: PathBuf, simulated: bool, utc_offset_secs: i64) -> Self {
        let dir = config_dir.join("logs");
        Self {
            behavior_path: dir.join(if simulated { "behavior-sim.txt" } else { "behavior.txt" }),
            csv_prefix: if simulated { "telemetry-sim-" } else { "telemetry-" },
            dir,
            utc_offset_secs,
            csv: None,
            csv_day: None,
            behavior: None,
            last_flush: Instant::now(),
            dead: false,
            prev: Vec::new(),
            last_settings: None,
        }
    }

    /// App-level marker (e.g. "daemon started") → behavior log.
    pub fn event(&mut self, message: &str) {
        if self.dead {
            return;
        }
        let stamp = self.timestamp();
        if self.write_behavior(&stamp, &format!("·· {message}")).is_err() {
            self.dead = true;
            return;
        }
        if let Some(b) = &mut self.behavior {
            if b.flush().is_err() {
                self.dead = true;
            }
        }
    }

    /// Called once per engine tick.
    pub fn record(&mut self, profile: &Profile, statuses: &[ChannelStatus]) {
        if self.dead {
            return;
        }
        if self.record_inner(profile, statuses).is_err() {
            self.dead = true; // logging must never take fan control down
        }
    }

    fn record_inner(&mut self, profile: &Profile, statuses: &[ChannelStatus]) -> std::io::Result<()> {
        let (day, stamp) = self.day_and_timestamp();
        self.rotate_csv(day)?;

        let settings = settings_line(profile);
        if self.last_settings.as_deref() != Some(&settings) {
            self.last_settings = Some(settings.clone());
            self.write_behavior(&stamp, &format!("settings  {settings}"))?;
        }

        for (i, s) in statuses.iter().enumerate() {
            let row = csv_row(&stamp, s);
            writeln!(self.csv.as_mut().expect("rotate_csv opened it"), "{row}")?;
            let prev = self.prev.get(i);
            for line in transition_lines(s, prev) {
                self.write_behavior(&stamp, &line)?;
            }
        }
        self.prev = statuses.to_vec();

        if let Some(b) = &mut self.behavior {
            b.flush()?;
        }
        if self.last_flush.elapsed() >= Duration::from_secs(5) {
            self.last_flush = Instant::now();
            self.csv.as_mut().expect("rotate_csv opened it").flush()?;
        }
        Ok(())
    }

    pub fn flush(&mut self) {
        if let Some(c) = &mut self.csv {
            let _ = c.flush();
        }
        if let Some(b) = &mut self.behavior {
            let _ = b.flush();
        }
    }

    // ---- files ----

    fn rotate_csv(&mut self, day: Day) -> std::io::Result<()> {
        if self.csv.is_some() && self.csv_day == Some(day) {
            return Ok(());
        }
        if let Some(mut old) = self.csv.take() {
            let _ = old.flush();
        }
        fs::create_dir_all(&self.dir)?;
        self.csv_day = Some(day);
        let path = self.dir.join(format!(
            "{}{:04}-{:02}-{:02}.csv",
            self.csv_prefix, day.year, day.month, day.day
        ));
        let fresh = !path.exists();
        let mut file = BufWriter::new(fs::OpenOptions::new().create(true).append(true).open(&path)?);
        if fresh {
            file.write_all("\u{FEFF}".as_bytes())?; // BOM
            writeln!(file, "{CSV_HEADER}")?;
        }
        self.csv = Some(file);
        self.prune();
        Ok(())
    }

    fn prune(&self) {
        // A locked stale file must not stop today's log — everything best-effort.
        let Ok(entries) = fs::read_dir(&self.dir) else { return };
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if !(name.starts_with(self.csv_prefix) && name.ends_with(".csv")) {
                continue;
            }
            let stale = e
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| SystemTime::now().duration_since(t).ok())
                .is_some_and(|age| age > Duration::from_secs(KEEP_DAYS * 24 * 3600));
            if stale {
                let _ = fs::remove_file(e.path());
            }
        }
    }

    fn write_behavior(&mut self, stamp: &str, line: &str) -> std::io::Result<()> {
        if self.behavior.is_none() {
            fs::create_dir_all(&self.dir)?;
            // Rotation is best-effort.
            if let Ok(meta) = fs::metadata(&self.behavior_path) {
                if meta.len() > BEHAVIOR_MAX_BYTES {
                    let stem = self.behavior_path.file_stem().unwrap_or_default().to_string_lossy();
                    let old = self.behavior_path.with_file_name(format!("{stem}-old.txt"));
                    let _ = fs::remove_file(&old);
                    let _ = fs::rename(&self.behavior_path, &old);
                }
            }
            let fresh = !self.behavior_path.exists();
            let mut file =
                BufWriter::new(fs::OpenOptions::new().create(true).append(true).open(&self.behavior_path)?);
            if fresh {
                file.write_all("\u{FEFF}".as_bytes())?;
            }
            self.behavior = Some(file);
        }
        writeln!(self.behavior.as_mut().unwrap(), "{stamp}  {line}")
    }

    // ---- wall clock ----

    fn day_and_timestamp(&self) -> (Day, String) {
        let unix = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let (day, h, m, s) = civil_from_unix(unix + self.utc_offset_secs);
        let stamp = format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            day.year, day.month, day.day, h, m, s
        );
        (day, stamp)
    }

    fn timestamp(&self) -> String {
        self.day_and_timestamp().1
    }
}

impl Drop for TelemetryLog {
    fn drop(&mut self) {
        self.flush();
    }
}

/// Days-from-civil inverse (Howard Hinnant's algorithm): unix seconds → local
/// civil date + time-of-day, given the seconds already offset to local.
fn civil_from_unix(local_secs: i64) -> (Day, u8, u8, u8) {
    let days = local_secs.div_euclid(86_400);
    let secs = local_secs.rem_euclid(86_400);
    let (h, m, s) = ((secs / 3600) as u8, ((secs % 3600) / 60) as u8, (secs % 60) as u8);

    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let mth = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
    let year = (if mth <= 2 { y + 1 } else { y }) as i32;
    (Day { year, month: mth, day: d }, h, m, s)
}

// ---- formatting (C#-identical) ----

/// C# "0.#": up to one decimal, no trailing zero. NaN/None → empty, ∞ → "inf".
fn n01(v: Option<f64>) -> String {
    match v {
        Some(d) if d.is_nan() => String::new(),
        Some(d) if d.is_infinite() => "inf".into(),
        Some(d) => {
            let r = (d * 10.0).round() / 10.0;
            if r == r.trunc() {
                format!("{:.0}", r)
            } else {
                format!("{:.1}", r)
            }
        }
        None => String::new(),
    }
}

/// C# "0.0": always one decimal.
fn n10(v: Option<f64>) -> String {
    match v {
        Some(d) if d.is_nan() => String::new(),
        Some(d) if d.is_infinite() => "inf".into(),
        Some(d) => format!("{d:.1}"),
        None => String::new(),
    }
}

/// C# "0": rounded integer.
fn n0(v: Option<f64>) -> String {
    match v {
        Some(d) if d.is_nan() => String::new(),
        Some(d) if d.is_infinite() => "inf".into(),
        Some(d) => format!("{:.0}", d),
        None => String::new(),
    }
}

fn csv_row(stamp: &str, s: &ChannelStatus) -> String {
    let rpms = s
        .rpms
        .iter()
        .map(|r| n0(*r))
        .collect::<Vec<_>>()
        .join("|");
    format!(
        "{},{},{},{},{},{},{},{},{},{},{},{}",
        stamp,
        s.name.replace(',', " "),
        if s.applied { "1" } else { "0" },
        n01(Some(s.output_percent)),
        n01(Some(s.target_percent)),
        s.reason,
        n01(Some(s.reason_level)),
        n01(Some(s.reason_seconds)),
        n10(s.raw_temp),
        n10(Some(s.effective_temp)),
        n0(s.rpm),
        rpms
    )
}

/// Same vocabulary as the why-chip, one compact phrase per reason.
fn describe(s: &ChannelStatus) -> String {
    match s.reason {
        OutputReason::RampUp => format!("ramping up to {}%", n0(Some(s.target_percent))),
        OutputReason::RampDown => format!("ramping down to {}%", n0(Some(s.target_percent))),
        OutputReason::StepDownHold => format!(
            "step down to {}% in {} s",
            n0(Some(s.reason_level)),
            n0(Some(s.reason_seconds))
        ),
        OutputReason::Hysteresis => "hysteresis hold".into(),
        OutputReason::ZeroSnap => format!("zero snap (curve asks {}%)", n0(Some(s.reason_level))),
        OutputReason::MinFloor => format!("safety floor (curve asks {}%)", n0(Some(s.reason_level))),
        OutputReason::IdleKick => "idle kick".into(),
        OutputReason::StopProbe => "stop probe (trial stop)".into(),
        OutputReason::None => "steady on the curve".into(),
    }
}

/// The moment's vitals, appended to every behavior line.
fn context(s: &ChannelStatus) -> String {
    let mut out = format!("out {}%", n0(Some(s.output_percent)));
    if !s.effective_temp.is_nan() {
        let _ = write!(out, " · avg {}°", n10(Some(s.effective_temp)));
    }
    if let Some(raw) = s.raw_temp {
        let _ = write!(out, " · now {}°", n10(Some(raw)));
    }
    out
}

fn is_pure_ramp_flip(a: OutputReason, b: OutputReason) -> bool {
    let ramp = |r| matches!(r, OutputReason::RampUp | OutputReason::RampDown);
    (a == OutputReason::None && ramp(b)) || (ramp(a) && b == OutputReason::None)
}

fn transition_lines(s: &ChannelStatus, prev: Option<&ChannelStatus>) -> Vec<String> {
    let ctx = context(s);
    let Some(p) = prev else {
        return vec![format!(
            "{:<12} {} — {}",
            s.name,
            if s.applied { "driving" } else { "released to BIOS" },
            ctx
        )];
    };
    let mut lines = Vec::new();
    if s.applied != p.applied {
        lines.push(format!(
            "{:<12} {} — {}",
            s.name,
            if s.applied { "now driving" } else { "released to BIOS (paused)" },
            ctx
        ));
    }
    let (was_on, is_on) = (p.output_percent > 0.01, s.output_percent > 0.01);
    if is_on != was_on {
        lines.push(format!("{:<12} fan {} — {}", s.name, if is_on { "ON" } else { "OFF" }, ctx));
    }
    if (s.target_percent - p.target_percent).abs() > 0.5 {
        lines.push(format!(
            "{:<12} target {}% → {}% [{}] — {}",
            s.name,
            n0(Some(p.target_percent)),
            n0(Some(s.target_percent)),
            describe(s),
            ctx
        ));
    }
    if s.reason != p.reason && !is_pure_ramp_flip(p.reason, s.reason) {
        lines.push(format!("{:<12} {} → {} — {}", s.name, describe(p), describe(s), ctx));
    }
    lines
}

/// Every knob that shapes behavior, one line; differences trigger a log entry.
fn settings_line(p: &Profile) -> String {
    let mut s = String::with_capacity(256);
    let _ = write!(s, "profile \"{}\"", p.name);
    if p.zero_snap_enabled {
        let _ = write!(s, " · snap <{}%", n0(Some(p.zero_snap_percent)));
    } else {
        s.push_str(" · snap off");
    }
    if p.idle_kick_enabled {
        let _ = write!(
            s,
            " · kick {}s/{}%/{}s",
            n0(Some(p.idle_kick_stopped_seconds)),
            n0(Some(p.idle_kick_percent)),
            n0(Some(p.idle_kick_seconds))
        );
    } else {
        s.push_str(" · kick off");
    }
    if p.stop_probe_enabled {
        let _ = write!(
            s,
            " · probe {}s/{}s/{}°/{}s/<{}°",
            n0(Some(p.stop_probe_run_seconds)),
            n0(Some(p.stop_probe_seconds)),
            n01(Some(p.stop_probe_stable_range_c)),
            n0(Some(p.stop_probe_retry_seconds)),
            n0(Some(p.stop_probe_max_temp_c))
        );
    } else {
        s.push_str(" · probe off");
    }
    let _ = write!(s, " · instantApply {}", if p.instant_apply_enabled { "on" } else { "off" });
    for ch in &p.channels {
        if p.safety_floor_enabled {
            let _ = write!(s, " | {}: min {}%", ch.name, n0(Some(ch.min_percent)));
        } else {
            let _ = write!(s, " | {}: min off", ch.name);
        }
        let _ = write!(
            s,
            " · avg {}s · hyst {}° · hold {}s · slew {}/{}",
            n0(Some(ch.averaging_seconds)),
            n01(Some(ch.hysteresis_c)),
            n0(Some(ch.step_down_hold_seconds)),
            n0(Some(ch.slew_up_percent_per_sec)),
            n0(Some(ch.slew_down_percent_per_sec))
        );
        s.push_str(" · curve ");
        let pts = ch
            .points
            .iter()
            .map(|pt| format!("{}:{}", n0(Some(pt.temp_c)), n0(Some(pt.percent))))
            .collect::<Vec<_>>()
            .join(" ");
        s.push_str(&pts);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates_are_correct() {
        // 2026-08-06 00:00:00 UTC = 1785974400 (20 671 days × 86 400)
        let (d, h, m, s) = civil_from_unix(1_785_974_400);
        assert_eq!((d.year, d.month, d.day, h, m, s), (2026, 8, 6, 0, 0, 0));
        // Epoch and a leap-year date.
        let (d, ..) = civil_from_unix(0);
        assert_eq!((d.year, d.month, d.day), (1970, 1, 1));
        let (d, ..) = civil_from_unix(1_709_164_800); // 2024-02-29
        assert_eq!((d.year, d.month, d.day), (2024, 2, 29));
    }

    #[test]
    fn number_formats_match_csharp() {
        assert_eq!(n01(Some(30.0)), "30"); // "0.#" drops the trailing zero
        assert_eq!(n01(Some(3.5)), "3.5");
        assert_eq!(n01(Some(3.46)), "3.5");
        assert_eq!(n01(Some(f64::NAN)), "");
        assert_eq!(n01(None), "");
        assert_eq!(n01(Some(f64::INFINITY)), "inf");
        assert_eq!(n10(Some(48.0)), "48.0"); // "0.0" keeps it
        assert_eq!(n0(Some(89.6)), "90");
    }

    #[test]
    fn settings_line_matches_csharp_shape() {
        let line = settings_line(&Profile::mac_book_like());
        assert!(line.starts_with("profile \"Quiet (MacBook-like)\" · snap <20% · kick off · probe 30s/30s/3.5°/60s/<78° · instantApply on | CPU cooler: min 0% · avg 90s · hyst 1.5° · hold 25s · slew 9/8 · curve 20:0 50:10"), "{line}");
        assert!(line.contains("| Case fans: min 0% · avg 25s · hyst 4° · hold 10s · slew 7/7 · curve 20:0 40:5"), "{line}");
    }
}
