use crate::curve::FanCurve;

/// The "MacBook feel" (port of `ResponseFilter.cs`):
///
/// 1. The curve is a STAIRCASE — wide temperature bands, each with a fixed level.
///    The target may jump several levels at once when the average lands in a
///    higher band.
/// 2. The temperature fed to the curve is the rolling AVERAGE over the last
///    `averaging_seconds` — short spikes barely move it.
/// 3. Step-down hysteresis: the hold timer starts as soon as the average itself
///    maps to a lower level, but the drop only fires once the time has been
///    served AND even (average + `hysteresis_c`) maps lower — no flapping at a
///    band edge.
/// 4. Slew limit: the output glides toward the target level at a bounded %/s.
/// 5. Zero snap: target levels below `zero_snap_percent` collapse to 0 — the fan
///    either runs at a meaningful speed or stays fully stopped, never crawls.
#[derive(Debug, Clone)]
pub struct ResponseFilter {
    pub averaging_seconds: f64,
    pub hysteresis_c: f64,
    pub step_down_hold_seconds: f64,
    pub slew_up_percent_per_sec: f64,
    pub slew_down_percent_per_sec: f64,
    /// Levels above 0 but strictly below this snap to 0 (0 = disabled).
    pub zero_snap_percent: f64,

    samples: Vec<(f64, f64)>, // (time, temp)
    output: Option<f64>,
    target_level: Option<f64>,
    down_since: Option<f64>,
    apply_now: bool,

    /// Averaged temperature actually driving the curve (NaN before the first step).
    pub effective_temp: f64,

    // Diagnostics for the "why isn't the fan at the curve's %" readout —
    // refreshed on every step, valid until the next one.
    /// Unsnapped staircase level at the current average.
    pub curve_level: f64,
    /// The curve's level was above 0 but collapsed to 0 by the zero snap.
    pub snapped_to_zero: bool,
    /// Hold time served, but the average is not yet `hysteresis_c` clear of the band edge.
    pub hysteresis_holding: bool,
    /// Level a pending step-down will drop to (None when none is pending).
    pub pending_down_level: Option<f64>,
    /// Seconds of `step_down_hold_seconds` still to wait (None when none is pending).
    pub down_hold_remaining: Option<f64>,
}

impl Default for ResponseFilter {
    fn default() -> Self {
        Self {
            averaging_seconds: 20.0,
            hysteresis_c: 3.0,
            step_down_hold_seconds: 10.0,
            slew_up_percent_per_sec: 8.0,
            slew_down_percent_per_sec: 8.0,
            zero_snap_percent: 0.0,
            samples: Vec::new(),
            output: None,
            target_level: None,
            down_since: None,
            apply_now: false,
            effective_temp: f64::NAN,
            curve_level: 0.0,
            snapped_to_zero: false,
            hysteresis_holding: false,
            pending_down_level: None,
            down_hold_remaining: None,
        }
    }
}

impl ResponseFilter {
    pub fn new() -> Self {
        Self::default()
    }

    /// One-shot: the next `step` re-evaluates the curve at the current average
    /// and jumps straight to that level — no step-down hold, no hysteresis, no
    /// slew glide. The engine calls this when a tuning setting or curve point
    /// changes, so an edit is FELT the tick it lands instead of tens of seconds
    /// later. The averaging window is untouched: it is measurement, not settings.
    pub fn apply_now(&mut self) {
        self.apply_now = true;
    }

    /// The level the slew is gliding toward.
    pub fn target_level(&self) -> f64 {
        self.target_level.unwrap_or(0.0)
    }

    /// `now` is monotonic time in seconds (same clock every call).
    pub fn step(&mut self, now: f64, raw_temp: f64, curve: &FanCurve) -> f64 {
        self.samples.push((now, raw_temp));
        let cutoff = now - self.averaging_seconds;
        self.samples.retain(|s| s.0 >= cutoff);

        let avg = self.samples.iter().map(|s| s.1).sum::<f64>() / self.samples.len() as f64;
        self.effective_temp = avg;

        self.curve_level = curve.evaluate(avg);
        self.snapped_to_zero = self.curve_level > 0.0 && self.snap(self.curve_level) <= 0.0;
        self.hysteresis_holding = false;
        self.pending_down_level = None;
        self.down_hold_remaining = None;

        // Snap BOTH evaluations so the hysteresis/hold logic reasons about the level
        // the fan will actually run at; the slew still glides to/from real steps.
        let level_at_avg = self.snap(self.curve_level);
        if self.apply_now {
            // Settings just changed: adopt what the new curve/settings say at the
            // current average, immediately — target, holds and slew position alike.
            self.apply_now = false;
            self.target_level = Some(level_at_avg);
            self.down_since = None;
            self.output = Some(level_at_avg);
        } else {
            match self.target_level {
                None => self.target_level = Some(level_at_avg),
                Some(target) if level_at_avg > target => {
                    // Up (possibly several steps) as soon as the average says so.
                    self.target_level = Some(level_at_avg);
                    self.down_since = None;
                }
                Some(target) if level_at_avg < target => {
                    // Down: the hold timer runs from the moment the average itself maps
                    // lower; the drop additionally needs (average + hysteresis) to map
                    // lower at that moment — reaching the band starts the clock, clearing
                    // its edge by the margin permits the step.
                    let down_since = *self.down_since.get_or_insert(now);
                    let level_with_margin = self.snap(curve.evaluate(avg + self.hysteresis_c));
                    let held = now - down_since >= self.step_down_hold_seconds;
                    if held && level_with_margin < target {
                        self.target_level = Some(level_with_margin);
                        self.down_since = None;
                    } else if !held {
                        self.pending_down_level =
                            Some(if level_with_margin < target { level_with_margin } else { level_at_avg });
                        self.down_hold_remaining =
                            Some(self.step_down_hold_seconds - (now - down_since));
                    } else {
                        self.hysteresis_holding = true; // time served, offset not yet cleared
                    }
                }
                Some(_) => self.down_since = None,
            }
        }

        let target = self.target_level.unwrap_or(0.0);
        let mut output = self.output.unwrap_or(target);
        let dt = if self.samples.len() >= 2 {
            (now - self.samples[self.samples.len() - 2].0).clamp(0.05, 10.0)
        } else {
            1.0
        };
        if target > output {
            output = target.min(output + self.slew_up_percent_per_sec * dt);
        } else {
            output = target.max(output - self.slew_down_percent_per_sec * dt);
        }
        self.output = Some(output);

        output.clamp(0.0, 100.0)
    }

    fn snap(&self, level: f64) -> f64 {
        if level > 0.0 && level < self.zero_snap_percent {
            0.0
        } else {
            level
        }
    }

    pub fn reset(&mut self) {
        self.samples.clear();
        self.output = None;
        self.target_level = None;
        self.down_since = None;
        self.apply_now = false;
        self.effective_temp = f64::NAN;
        self.curve_level = 0.0;
        self.snapped_to_zero = false;
        self.hysteresis_holding = false;
        self.pending_down_level = None;
        self.down_hold_remaining = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::CurvePoint;

    fn curve() -> FanCurve {
        FanCurve::new(&[
            CurvePoint::new(20.0, 0.0),
            CurvePoint::new(50.0, 10.0),
            CurvePoint::new(55.0, 20.0),
            CurvePoint::new(62.0, 40.0),
        ])
    }

    /// Drive with 1 Hz samples of a constant temp for `secs`, return last output.
    fn run_flat(f: &mut ResponseFilter, c: &FanCurve, from: f64, secs: u32, temp: f64) -> f64 {
        let mut out = 0.0;
        for i in 0..secs {
            out = f.step(from + i as f64, temp, c);
        }
        out
    }

    #[test]
    fn short_spike_barely_moves_the_average() {
        let mut f = ResponseFilter { averaging_seconds: 90.0, ..Default::default() };
        let c = curve();
        run_flat(&mut f, &c, 0.0, 90, 45.0);
        assert_eq!(f.target_level(), 0.0);
        // A 5 s / 90 °C spike on a 90 s window moves the average ~2.5° — no step.
        for i in 0..5 {
            f.step(90.0 + i as f64, 90.0, &c);
        }
        assert!(f.effective_temp < 48.0, "avg jumped: {}", f.effective_temp);
        assert_eq!(f.target_level(), 0.0);
    }

    #[test]
    fn sustained_load_steps_up_and_slews_at_rate() {
        let mut f = ResponseFilter { averaging_seconds: 20.0, slew_up_percent_per_sec: 8.0, ..Default::default() };
        let c = curve();
        run_flat(&mut f, &c, 0.0, 20, 45.0); // settle at 0%
        // Hold 63° until the 20 s average itself crosses band edges.
        let mut out_prev = 0.0;
        let mut stepped = false;
        for i in 0..40 {
            let out = f.step(20.0 + i as f64, 63.0, &c);
            assert!(out - out_prev <= 8.0 + 1e-9, "slew exceeded: {} -> {}", out_prev, out);
            out_prev = out;
            stepped = stepped || f.target_level() > 0.0;
        }
        assert_eq!(f.target_level(), 40.0); // avg fully inside the top band
        assert!(stepped);
        assert!((out_prev - 40.0).abs() < 1e-9, "output should have reached the target");
    }

    #[test]
    fn step_down_waits_hold_and_hysteresis() {
        let mut f = ResponseFilter {
            averaging_seconds: 10.0,
            hysteresis_c: 3.0,
            step_down_hold_seconds: 10.0,
            slew_down_percent_per_sec: 100.0, // isolate the hold logic from the glide
            ..Default::default()
        };
        let c = curve();
        run_flat(&mut f, &c, 0.0, 30, 57.0); // settled in the 20% band
        assert_eq!(f.target_level(), 20.0);

        // Drop to 53°: the 10 s average maps to the 10% band quickly, starting the
        // hold clock — but 53 + 3 hysteresis still maps to 20%, so after the hold
        // is served the filter sits in hysteresis_holding, target unchanged.
        let mut t = 30.0;
        for _ in 0..25 {
            f.step(t, 53.0, &c);
            t += 1.0;
        }
        assert_eq!(f.target_level(), 20.0);
        assert!(f.hysteresis_holding);

        // Now 45°: (avg + 3) maps below 20% too; the hold was already served, so
        // once the average itself catches up the step fires.
        for _ in 0..15 {
            f.step(t, 45.0, &c);
            t += 1.0;
        }
        assert_eq!(f.target_level(), 0.0);
    }

    #[test]
    fn pending_down_diagnostics_run_during_hold() {
        let mut f = ResponseFilter {
            averaging_seconds: 5.0,
            hysteresis_c: 1.0,
            step_down_hold_seconds: 10.0,
            ..Default::default()
        };
        let c = curve();
        run_flat(&mut f, &c, 0.0, 10, 57.0);
        assert_eq!(f.target_level(), 20.0);
        // 40°: maps to 0 even with margin — a pending drop with a countdown.
        f.step(10.0, 40.0, &c);
        let mut remaining_seen = f64::NAN;
        for i in 0..9 {
            f.step(11.0 + i as f64, 40.0, &c);
            if let Some(r) = f.down_hold_remaining {
                remaining_seen = r;
            }
        }
        assert!(remaining_seen > 0.0 && remaining_seen < 10.0);
        f.step(21.0, 40.0, &c); // hold served (started at t=10)
        assert_eq!(f.target_level(), 0.0);
    }

    #[test]
    fn returning_into_the_band_resets_the_hold() {
        let mut f = ResponseFilter {
            averaging_seconds: 2.0,
            hysteresis_c: 1.0,
            step_down_hold_seconds: 10.0,
            ..Default::default()
        };
        let c = curve();
        run_flat(&mut f, &c, 0.0, 5, 57.0);
        assert_eq!(f.target_level(), 20.0);
        // Dip below for 6 s (short averaging → the avg follows fast)…
        let mut t = 5.0;
        for _ in 0..6 {
            f.step(t, 45.0, &c);
            t += 1.0;
        }
        assert!(f.down_hold_remaining.is_some());
        // …pop back into the band (down_since resets)…
        for _ in 0..5 {
            f.step(t, 57.0, &c);
            t += 1.0;
        }
        assert!(f.down_hold_remaining.is_none());
        // …then below again: the full hold must be served afresh.
        for _ in 0..6 {
            f.step(t, 45.0, &c);
            t += 1.0;
        }
        assert_eq!(f.target_level(), 20.0);
        assert!(f.down_hold_remaining.expect("hold restarted") > 3.0);
    }

    #[test]
    fn zero_snap_collapses_low_bands() {
        let mut f = ResponseFilter { averaging_seconds: 5.0, zero_snap_percent: 20.0, ..Default::default() };
        let c = curve();
        run_flat(&mut f, &c, 0.0, 10, 52.0); // curve says 10%, snap says 0
        assert_eq!(f.curve_level, 10.0);
        assert!(f.snapped_to_zero);
        assert_eq!(f.target_level(), 0.0);
        // The 20% band is NOT snapped (strictly-below rule).
        run_flat(&mut f, &c, 10.0, 20, 58.0);
        assert_eq!(f.target_level(), 20.0);
        assert!(!f.snapped_to_zero);
    }

    #[test]
    fn apply_now_jumps_without_glide_or_hold() {
        let mut f = ResponseFilter { averaging_seconds: 5.0, ..Default::default() };
        let c = curve();
        run_flat(&mut f, &c, 0.0, 10, 63.0);
        assert_eq!(f.target_level(), 40.0);
        // Pretend the user just edited the curve: with apply_now armed the filter
        // re-adopts the (new) verdict at the current average instantly.
        let flat = FanCurve::new(&[CurvePoint::new(20.0, 5.0)]);
        f.apply_now();
        let out = f.step(10.0, 63.0, &flat);
        assert_eq!(out, 5.0); // no 8 %/s glide down from 40 — a jump
        assert_eq!(f.target_level(), 5.0);
    }
}
