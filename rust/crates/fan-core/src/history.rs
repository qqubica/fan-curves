//! Two-tier per-channel history: a small exact ring for the live window, and a
//! quantized spill file for everything older — the port of `ChannelHistory`.
//!
//! Lives in fan-core because BOTH processes keep one: the daemon records a
//! sample per engine tick around the clock (it is the resident part, so it is
//! the only place history can accumulate while no window is open), and the UI
//! keeps a local mirror it backfills from the daemon over IPC so the strip is
//! populated the moment the window opens.
//!
//! Keeping hours of samples in RAM is what made the C# app fat once already, so
//! only [`RING`] samples (the live window) stay in memory at full precision.
//! Everything is also appended to a fixed-size binary record on disk, which is
//! what scrollback reads. The disk copy is lossy — tenths of a degree/percent,
//! whole seconds — deliberately below display resolution. The spill is capped
//! at ~24 h by compaction (the WPF app's once-a-day-at-2× rule), which is what
//! keeps a daemon that runs for weeks from growing a file forever.
//!
//! The file vanishes with the process (delete-on-close on Windows, unlinked
//! immediately on Unix), so a crash cannot leave it behind. Any file error
//! silently degrades to the RAM-only ring: history is a convenience, and it
//! must never take the engine or the UI down.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};

/// Live window, ~10 min at the ~1 s tick.
pub const RING: usize = 600;
/// Spill retention, ~24 h at the ~1 s tick. Compaction keeps the newest this
/// many and runs when the file holds twice that, so it costs one rewrite a day.
pub const KEEP: usize = 86_400;
/// Bytes per spilled sample: u32 time + three i16 tenths.
const RECORD: usize = 10;
/// Seconds from the Unix epoch to 2020-01-01, so the u32 stamp lasts.
const EPOCH_2020: f64 = 1_577_836_800.0;
/// Sentinel for "no reading" in a quantized field.
const NULL_TENTHS: i16 = i16::MIN;

/// One strip sample. `wall` is unix seconds — differences give "how long ago",
/// and the hover chip formats it as a clock with the caller's UTC offset.
#[derive(Clone, Copy, Debug)]
pub struct HistorySample {
    pub wall: f64,
    /// Rolling average — NaN when the channel has no reading.
    pub avg: f64,
    pub raw: Option<f64>,
    pub out: f64,
}

fn to_tenths(v: f64) -> i16 {
    if v.is_nan() {
        NULL_TENTHS
    } else {
        (v * 10.0).round().clamp(i16::MIN as f64 + 1.0, i16::MAX as f64) as i16
    }
}

fn from_tenths(v: i16) -> f64 {
    if v == NULL_TENTHS {
        f64::NAN
    } else {
        v as f64 / 10.0
    }
}

/// One channel's history.
pub struct ChannelHistory {
    ring: VecDeque<HistorySample>,
    spill: Option<File>,
    /// Absolute index of the next sample — never rewinds, so a scrolled
    /// viewport keeps pointing at the same data across a compaction.
    total: usize,
    /// Absolute index of the oldest sample still on disk (CLEAR and compaction
    /// move this up).
    first: usize,
}

impl Default for ChannelHistory {
    fn default() -> Self {
        Self::new()
    }
}

impl ChannelHistory {
    pub fn new() -> Self {
        Self { ring: VecDeque::with_capacity(RING), spill: None, total: 0, first: 0 }
    }

    pub fn total(&self) -> usize {
        self.total
    }

    pub fn first_available(&self) -> usize {
        self.first
    }

    /// Samples on disk plus in the ring — how far back scrolling can go.
    pub fn available(&self) -> usize {
        self.total - self.first
    }

    fn ensure_spill(&mut self) -> Option<&mut File> {
        if self.spill.is_none() {
            self.spill = open_spill();
        }
        self.spill.as_mut()
    }

    pub fn push(&mut self, s: HistorySample) {
        self.ring.push_back(s);
        while self.ring.len() > RING {
            self.ring.pop_front();
        }
        self.total += 1;

        let mut buf = [0u8; RECORD];
        let t = ((s.wall - EPOCH_2020).max(0.0)) as u32;
        buf[0..4].copy_from_slice(&t.to_le_bytes());
        buf[4..6].copy_from_slice(&to_tenths(s.avg).to_le_bytes());
        buf[6..8].copy_from_slice(&to_tenths(s.raw.unwrap_or(f64::NAN)).to_le_bytes());
        buf[8..10].copy_from_slice(&to_tenths(s.out).to_le_bytes());
        if let Some(f) = self.ensure_spill() {
            // A failed write disables the spill for the session; the ring lives on.
            if f.seek(SeekFrom::End(0)).is_err() || f.write_all(&buf).is_err() {
                self.spill = None;
            }
        }
        if self.total - self.first > 2 * KEEP {
            self.compact();
        }
    }

    /// Drop everything older than [`KEEP`] by rewriting the spill with only the
    /// newest records. Absolute indices are preserved — `first` moves up.
    fn compact(&mut self) {
        let keep_from = self.total - KEEP;
        let Some(f) = self.spill.as_mut() else {
            self.first = keep_from;
            return;
        };
        let offset = (keep_from - self.first) as u64 * RECORD as u64;
        let mut tail = Vec::new();
        let ok = f.seek(SeekFrom::Start(offset)).is_ok()
            && f.read_to_end(&mut tail).is_ok()
            && f.seek(SeekFrom::Start(0)).is_ok()
            && f.write_all(&tail).is_ok()
            && f.set_len(tail.len() as u64).is_ok();
        if !ok {
            self.spill = None; // degrade to the ring; never take the caller down
        }
        self.first = keep_from;
    }

    /// Wipe everything. Absolute indices keep counting so any live viewport
    /// anchor is simply left behind rather than pointing at different data.
    pub fn clear(&mut self) {
        self.ring.clear();
        self.first = self.total;
        if let Some(f) = self.spill.as_mut() {
            let _ = f.set_len(0);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.ring.is_empty()
    }

    /// The newest `count` samples — straight from the ring, exact.
    pub fn live_window(&self, count: usize) -> Vec<HistorySample> {
        let n = count.min(self.ring.len());
        self.ring.iter().skip(self.ring.len() - n).copied().collect()
    }

    /// `count` samples ending at absolute index `end` (exclusive), read from
    /// the spill file. Returns an empty vec if the file is gone.
    pub fn window_at(&mut self, end: usize, count: usize) -> Vec<HistorySample> {
        let end = end.clamp(self.first, self.total);
        let start = end.saturating_sub(count).max(self.first);
        let n = end - start;
        if n == 0 {
            return Vec::new();
        }
        let Some(f) = self.spill.as_mut() else { return Vec::new() };
        // The file starts at absolute index 0 of the CURRENT file (cleared
        // files restart), so offset by what CLEAR/compaction discarded.
        let offset = (start - self.first) as u64 * RECORD as u64;
        let mut buf = vec![0u8; n * RECORD];
        if f.seek(SeekFrom::Start(offset)).is_err() || f.read_exact(&mut buf).is_err() {
            return Vec::new();
        }
        buf.chunks_exact(RECORD)
            .map(|r| {
                let t = u32::from_le_bytes([r[0], r[1], r[2], r[3]]);
                let avg = from_tenths(i16::from_le_bytes([r[4], r[5]]));
                let raw = from_tenths(i16::from_le_bytes([r[6], r[7]]));
                let out = from_tenths(i16::from_le_bytes([r[8], r[9]]));
                HistorySample { wall: EPOCH_2020 + t as f64, avg, raw: (!raw.is_nan()).then_some(raw), out }
            })
            .collect()
    }

    /// `count` samples FROM absolute index `start` — the IPC serving read. The
    /// range is clamped to what is actually available (a caller asking for
    /// discarded or RAM-degraded territory is bumped forward, never stalled),
    /// and the returned index says where the samples really begin. Ranges
    /// inside the ring are served exact with no disk I/O — the steady-state
    /// one-sample catch-up never touches the file.
    pub fn read(&mut self, start: usize, count: usize) -> (usize, Vec<HistorySample>) {
        let ring_start = self.total - self.ring.len();
        let floor = if self.spill.is_some() { self.first } else { ring_start.max(self.first) };
        let start = start.max(floor).min(self.total);
        let count = count.min(self.total - start);
        if count == 0 {
            return (start, Vec::new());
        }
        if start >= ring_start {
            let off = start - ring_start;
            return (start, self.ring.iter().skip(off).take(count).copied().collect());
        }
        let w = self.window_at(start + count, count);
        if w.len() == count {
            return (start, w);
        }
        // The spill failed under us: serve whatever the ring still covers.
        let start = ring_start.min(self.total);
        let count = self.total - start;
        (start, self.ring.iter().take(count).copied().collect())
    }
}

/// Anchored view over a channel's history. `None` = following the live edge.
#[derive(Default, Clone, Copy)]
pub struct Viewport {
    /// Absolute index one past the newest visible sample.
    anchor: Option<usize>,
}

impl Viewport {
    pub fn is_live(&self) -> bool {
        self.anchor.is_none()
    }

    pub fn to_live(&mut self) {
        self.anchor = None;
    }

    pub fn can_scroll(&self, h: &ChannelHistory) -> bool {
        h.available() > RING
    }

    /// Pan by `delta` samples; negative goes into the past. Reaching the live
    /// edge drops the anchor, so the view resumes following new ticks.
    pub fn scroll_by(&mut self, delta: i64, h: &ChannelHistory) {
        if !self.can_scroll(h) && self.anchor.is_none() {
            return;
        }
        let current = self.anchor.unwrap_or(h.total()) as i64;
        let min_end = (h.first_available() + RING) as i64;
        let end = (current + delta).clamp(min_end, h.total() as i64);
        self.anchor = if end >= h.total() as i64 { None } else { Some(end as usize) };
    }

    /// The samples to draw, and whether we are at the live edge.
    pub fn samples(&mut self, h: &mut ChannelHistory) -> (Vec<HistorySample>, bool) {
        match self.anchor {
            None => (h.live_window(RING), true),
            Some(end) => {
                // A CLEAR can strand the anchor behind the available range.
                if end <= h.first_available() {
                    self.anchor = None;
                    return (h.live_window(RING), true);
                }
                let w = h.window_at(end, RING);
                if w.is_empty() {
                    self.anchor = None;
                    (h.live_window(RING), true)
                } else {
                    (w, false)
                }
            }
        }
    }
}

/// A private, self-deleting scratch file under the temp directory.
fn open_spill() -> Option<File> {
    let dir = std::env::temp_dir().join("FanCurves");
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(format!(
        "history-{}-{}.bin",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0)
    ));

    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_DELETE_ON_CLOSE: u32 = 0x0400_0000;
        std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(true)
            .custom_flags(FILE_FLAG_DELETE_ON_CLOSE)
            .open(&path)
            .ok()
    }
    #[cfg(not(windows))]
    {
        let f = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(true)
            .open(&path)
            .ok()?;
        // Unlink immediately: the handle keeps it alive, nothing survives us.
        let _ = std::fs::remove_file(&path);
        Some(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(i: usize) -> HistorySample {
        HistorySample {
            wall: EPOCH_2020 + i as f64,
            avg: 40.0 + (i % 10) as f64 / 10.0,
            raw: Some(41.0),
            out: (i % 100) as f64,
        }
    }

    #[test]
    fn spills_and_reads_back_older_samples() {
        let mut h = ChannelHistory::new();
        for i in 0..(RING + 250) {
            h.push(sample(i));
        }
        assert_eq!(h.total(), RING + 250);
        // The ring only holds the live window…
        assert_eq!(h.live_window(RING).len(), RING);
        // …but the spill still has the older ones, at their quantized values.
        let w = h.window_at(300, 100);
        assert_eq!(w.len(), 100);
        let expect = sample(299);
        assert!((w[99].avg - expect.avg).abs() < 0.05);
        assert!((w[99].out - expect.out).abs() < 0.05);
    }

    #[test]
    fn viewport_scrolls_back_and_snaps_to_live() {
        let mut h = ChannelHistory::new();
        for i in 0..(RING * 2) {
            h.push(sample(i));
        }
        let mut v = Viewport::default();
        assert!(v.is_live());
        assert!(v.can_scroll(&h));

        v.scroll_by(-120, &h);
        assert!(!v.is_live(), "scrolling back should leave the live edge");
        let (w, live) = v.samples(&mut h);
        assert!(!live);
        assert_eq!(w.len(), RING);

        // Scrolling forward past the end drops the anchor → follows live again.
        v.scroll_by(1000, &h);
        assert!(v.is_live());
        let (_, live) = v.samples(&mut h);
        assert!(live);
    }

    #[test]
    fn clear_strands_the_anchor_safely() {
        let mut h = ChannelHistory::new();
        for i in 0..(RING * 2) {
            h.push(sample(i));
        }
        let mut v = Viewport::default();
        v.scroll_by(-300, &h);
        assert!(!v.is_live());
        h.clear();
        // Everything the anchor pointed at is gone: fall back to live, no panic.
        let (w, live) = v.samples(&mut h);
        assert!(live);
        assert!(w.is_empty());
        assert!(v.is_live());
    }

    #[test]
    fn nan_survives_the_round_trip_as_a_gap() {
        let mut h = ChannelHistory::new();
        for i in 0..10 {
            let mut s = sample(i);
            if i == 5 {
                s.raw = None;
                s.avg = f64::NAN;
            }
            h.push(s);
        }
        let w = h.window_at(10, 10);
        assert_eq!(w.len(), 10);
        assert!(w[5].avg.is_nan(), "a missing average must stay missing");
        assert!(w[5].raw.is_none());
        assert!(!w[4].avg.is_nan());
    }

    #[test]
    fn read_serves_ranges_and_clamps_to_available() {
        let mut h = ChannelHistory::new();
        for i in 0..(RING + 100) {
            h.push(sample(i));
        }
        // Spill range, exact position honoured.
        let (start, w) = h.read(10, 50);
        assert_eq!(start, 10);
        assert_eq!(w.len(), 50);
        assert!((w[0].avg - sample(10).avg).abs() < 0.05);
        // Ring range comes back exact (not quantized).
        let (start, w) = h.read(RING + 50, 1000);
        assert_eq!(start, RING + 50);
        assert_eq!(w.len(), 50, "count is clamped to what exists");
        assert_eq!(w[0].avg, sample(RING + 50).avg);
        // Past the end: empty, positioned at the end.
        let (start, w) = h.read(h.total() + 5, 10);
        assert_eq!(start, h.total());
        assert!(w.is_empty());
    }

    #[test]
    fn read_skips_ranges_discarded_by_clear() {
        let mut h = ChannelHistory::new();
        for i in 0..100 {
            h.push(sample(i));
        }
        h.clear();
        for i in 100..130 {
            h.push(sample(i));
        }
        // The first 100 are gone; a request for them is bumped to what exists.
        let (start, w) = h.read(0, 4096);
        assert_eq!(start, 100);
        assert_eq!(w.len(), 30);
    }

    #[test]
    fn compaction_keeps_the_newest_day_and_absolute_indices() {
        let mut h = ChannelHistory::new();
        for i in 0..(2 * KEEP + 10) {
            h.push(sample(i));
        }
        assert_eq!(h.total(), 2 * KEEP + 10);
        assert!(h.available() <= 2 * KEEP, "compaction must have run");
        assert!(h.available() >= KEEP, "compaction must keep the newest day");
        // It fires once at 2×KEEP+1 and moves the floor in ONE daily jump.
        assert_eq!(h.first_available(), KEEP + 1);
        // A sample near the new floor still reads back at its absolute index.
        let idx = h.first_available() + 5;
        let (start, w) = h.read(idx, 1);
        assert_eq!(start, idx);
        assert_eq!(w.len(), 1);
        assert!((w[0].out - sample(idx).out).abs() < 0.05);
        // Discarded territory is bumped forward, not served stale.
        let (start, _) = h.read(0, 1);
        assert_eq!(start, h.first_available());
    }
}
