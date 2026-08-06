/// Opportunistic fan stop (port of `StopProbe.cs`): once a channel has run
/// continuously for `run_seconds` with its temperature inside a `stable_range_c`
/// band, the fan is stopped as a trial. While stopped, a short-average of the
/// temperature is compared against the pre-stop baseline every tick — if it
/// climbs more than `stable_range_c` (or the curve demands more than it did at
/// the stop), curve control resumes immediately. A stop that does not survive
/// its first `probe_seconds + run_seconds` delays the next trial by
/// `fail_retry_seconds`, so the fan never settles into an on/off cycle under
/// load that genuinely needs it.
#[derive(Debug, Clone)]
pub struct StopProbe {
    /// Continuous running (and temperature-stability) time required before a trial stop.
    pub run_seconds: f64,
    /// Trial length — a rise inside this window (plus one `run_seconds`) counts as a failed trial.
    pub probe_seconds: f64,
    /// "Stable" = temp within this band; a rise beyond it while stopped resumes the fan.
    pub stable_range_c: f64,
    /// Wait after a failed trial before trying again.
    pub fail_retry_seconds: f64,
    /// No trial starts while any recent sample is above this; a running trial
    /// crossing it resumes at once (a channel this hot should never lose its fan).
    pub max_temp_c: f64,

    window: Vec<(f64, f64)>, // (time, temp)
    running_since: Option<f64>,
    stopped_at: Option<f64>, // None = not holding the fan off
    baseline: f64,           // stable-window average at the moment of the stop
    demand_at_stop: f64,
    retry_after: f64,
}

/// Rise detection averages the last few seconds so single-sample jitter can't trigger it.
const RISE_AVG_SECONDS: f64 = 5.0;

impl Default for StopProbe {
    fn default() -> Self {
        Self {
            run_seconds: 30.0,
            probe_seconds: 30.0,
            stable_range_c: 3.5,
            fail_retry_seconds: 60.0,
            max_temp_c: 78.0,
            window: Vec::new(),
            running_since: None,
            stopped_at: None,
            baseline: 0.0,
            demand_at_stop: 0.0,
            retry_after: f64::NEG_INFINITY,
        }
    }
}

impl StopProbe {
    pub fn new() -> Self {
        Self::default()
    }

    /// True while the fan is being held at 0% (for UI/status).
    pub fn holding(&self) -> bool {
        self.stopped_at.is_some()
    }

    /// `now` is monotonic seconds; `raw_temp` the raw (unaveraged) channel
    /// temperature; `demand` the output the curve/filter wants to write.
    pub fn step(&mut self, now: f64, raw_temp: f64, demand: f64) -> f64 {
        self.window.push((now, raw_temp));
        let cutoff = now - self.run_seconds;
        self.window.retain(|s| s.0 >= cutoff);

        if let Some(stopped_at) = self.stopped_at {
            if demand <= 0.01 {
                self.to_normal(now); // the curve no longer wants the fan anyway
                return demand;
            }
            let recent: Vec<f64> = self
                .window
                .iter()
                .filter(|s| s.0 >= now - RISE_AVG_SECONDS)
                .map(|s| s.1)
                .collect();
            let recent = recent.iter().sum::<f64>() / recent.len() as f64;
            if recent > self.baseline + self.stable_range_c
                || recent > self.max_temp_c
                || demand > self.demand_at_stop + 0.01
            {
                // Heat is building without the fan — hand back to the curve, and if the
                // stop didn't even survive to the first recheck horizon, back off.
                let failed = now - stopped_at < self.probe_seconds + self.run_seconds;
                self.to_normal(now);
                if failed {
                    self.retry_after = now + self.fail_retry_seconds;
                }
                return demand;
            }
            return 0.0;
        }

        if demand <= 0.01 {
            self.running_since = None; // not running — nothing to trial-stop
            return demand;
        }
        let running_since = *self.running_since.get_or_insert(now);
        if now - running_since < self.run_seconds || now < self.retry_after {
            return demand;
        }
        // The sample window must actually span the running period, and stay in-band.
        if self.window[self.window.len() - 1].0 - self.window[0].0 < self.run_seconds - 2.0 {
            return demand;
        }
        let max = self.window.iter().map(|s| s.1).fold(f64::NEG_INFINITY, f64::max);
        let min = self.window.iter().map(|s| s.1).fold(f64::INFINITY, f64::min);
        if max - min > self.stable_range_c {
            return demand;
        }
        if max > self.max_temp_c {
            return demand; // too hot to gamble on a stop
        }

        self.stopped_at = Some(now);
        self.baseline = self.window.iter().map(|s| s.1).sum::<f64>() / self.window.len() as f64;
        self.demand_at_stop = demand;
        0.0
    }

    fn to_normal(&mut self, now: f64) {
        self.stopped_at = None;
        self.running_since = Some(now); // the running clock restarts as the fan spins back up
    }

    pub fn reset(&mut self) {
        self.window.clear();
        self.running_since = None;
        self.stopped_at = None;
        self.retry_after = f64::NEG_INFINITY;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe() -> StopProbe {
        StopProbe {
            run_seconds: 30.0,
            probe_seconds: 30.0,
            stable_range_c: 3.5,
            fail_retry_seconds: 60.0,
            max_temp_c: 78.0,
            ..Default::default()
        }
    }

    #[test]
    fn stable_run_earns_a_trial_stop() {
        let mut p = probe();
        let mut t = 0.0;
        let mut stopped_at = None;
        for _ in 0..40 {
            let out = p.step(t, 55.0, 30.0);
            if out == 0.0 && stopped_at.is_none() {
                stopped_at = Some(t);
            }
            t += 1.0;
        }
        // Trial begins once 30 s of stable running are on the clock.
        assert_eq!(stopped_at, Some(30.0));
        assert!(p.holding());
    }

    #[test]
    fn rise_resumes_and_arms_backoff() {
        let mut p = probe();
        let mut t = 0.0;
        for _ in 0..=30 {
            p.step(t, 55.0, 30.0);
            t += 1.0;
        }
        assert!(p.holding());
        // Temp climbs past baseline + band within the trial → resume, failed.
        let mut resumed_at = None;
        for _ in 0..10 {
            let out = p.step(t, 60.0, 30.0);
            if out > 0.0 && resumed_at.is_none() {
                resumed_at = Some(t);
            }
            t += 1.0;
        }
        assert!(resumed_at.is_some());
        assert!(!p.holding());
        // The failed trial delays the next one: 30 s of stable running from the
        // resume is NOT enough — the retry_after gate holds it back.
        let resume = resumed_at.unwrap();
        let mut next_stop = None;
        for _ in 0..80 {
            let out = p.step(t, 55.0, 30.0);
            if out == 0.0 && next_stop.is_none() {
                next_stop = Some(t);
            }
            t += 1.0;
        }
        let next = next_stop.expect("a later trial still happens");
        assert!(
            next >= resume + 60.0,
            "trial at {next} ignored the 60 s backoff from {resume}"
        );
    }

    #[test]
    fn demand_rise_while_holding_resumes() {
        let mut p = probe();
        let mut t = 0.0;
        for _ in 0..=30 {
            p.step(t, 55.0, 30.0);
            t += 1.0;
        }
        assert!(p.holding());
        assert_eq!(p.step(t, 55.0, 40.0), 40.0); // curve stepped up past the level at stop
        assert!(!p.holding());
    }

    #[test]
    fn too_hot_blocks_trials_and_aborts_a_running_one() {
        let mut p = probe();
        let mut t = 0.0;
        // Stable but ABOVE max_temp_c: no trial ever starts.
        for _ in 0..60 {
            assert_eq!(p.step(t, 80.0, 30.0), 30.0);
            t += 1.0;
        }
        // Cool and stable → trial; then the 5 s average crossing max resumes it
        // even though the rise stays inside the stable band relative to baseline.
        let mut p = probe();
        p.max_temp_c = 57.0;
        let mut t = 0.0;
        for _ in 0..=30 {
            p.step(t, 55.0, 30.0);
            t += 1.0;
        }
        assert!(p.holding());
        for _ in 0..8 {
            p.step(t, 58.0, 30.0); // > max 57, but 58 < 55 + 3.5 band
            t += 1.0;
        }
        assert!(!p.holding());
    }

    #[test]
    fn unstable_temperature_never_stops_the_fan() {
        let mut p = probe();
        let mut t = 0.0;
        for i in 0..120 {
            let temp = if i % 2 == 0 { 50.0 } else { 56.0 }; // 6° swing > 3.5 band
            assert_eq!(p.step(t, temp, 30.0), 30.0);
            t += 1.0;
        }
    }
}
