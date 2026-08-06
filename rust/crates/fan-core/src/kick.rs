/// Periodic spin-up for fully stopped fans (port of `IdleKick.cs`): once the
/// driven output has sat at 0% for `stopped_seconds`, the written PWM is forced
/// to `kick_percent` for `kick_seconds`, then the fan stops again and the cycle
/// repeats. Any curve output above 0% cancels the kick and restarts the clock.
#[derive(Debug, Clone)]
pub struct IdleKick {
    pub stopped_seconds: f64,
    pub kick_percent: f64,
    pub kick_seconds: f64,

    stopped_since: Option<f64>,
    kick_until: Option<f64>,
}

impl Default for IdleKick {
    fn default() -> Self {
        Self {
            stopped_seconds: 60.0,
            kick_percent: 20.0,
            kick_seconds: 20.0,
            stopped_since: None,
            kick_until: None,
        }
    }
}

impl IdleKick {
    pub fn new() -> Self {
        Self::default()
    }

    /// True while the output is being overridden (for UI/status).
    pub fn kicking(&self) -> bool {
        self.kick_until.is_some()
    }

    /// `now` is monotonic time in seconds; `output` is what the curve/filter
    /// wants to write.
    pub fn step(&mut self, now: f64, output: f64) -> f64 {
        if output > 0.01 {
            self.stopped_since = None;
            self.kick_until = None;
            return output;
        }

        if let Some(until) = self.kick_until {
            if now < until {
                return self.kick_percent;
            }
            self.kick_until = None;
            self.stopped_since = Some(now); // stopped again — next kick in stopped_seconds
            return output;
        }

        let Some(since) = self.stopped_since else {
            self.stopped_since = Some(now);
            return output;
        };

        if now - since >= self.stopped_seconds {
            self.kick_until = Some(now + self.kick_seconds);
            return self.kick_percent;
        }
        output
    }

    pub fn reset(&mut self) {
        self.stopped_since = None;
        self.kick_until = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kick_cycle_fires_lasts_and_repeats() {
        let mut k = IdleKick { stopped_seconds: 10.0, kick_percent: 5.0, kick_seconds: 5.0, ..Default::default() };
        // t=0 registers the stop; the kick fires once 10 s have passed.
        for t in 0..10 {
            assert_eq!(k.step(t as f64, 0.0), 0.0, "t={t}");
        }
        assert_eq!(k.step(10.0, 0.0), 5.0); // kick starts (until t=15)
        assert!(k.kicking());
        assert_eq!(k.step(14.0, 0.0), 5.0);
        assert_eq!(k.step(15.0, 0.0), 0.0); // over; stopped clock restarts at 15
        assert!(!k.kicking());
        assert_eq!(k.step(24.0, 0.0), 0.0);
        assert_eq!(k.step(25.0, 0.0), 5.0); // next cycle
    }

    #[test]
    fn demand_cancels_kick_and_restarts_clock() {
        let mut k = IdleKick { stopped_seconds: 10.0, kick_percent: 5.0, kick_seconds: 5.0, ..Default::default() };
        for t in 0..=10 {
            k.step(t as f64, 0.0);
        }
        assert!(k.kicking());
        assert_eq!(k.step(11.0, 30.0), 30.0); // curve wants the fan — kick yields
        assert!(!k.kicking());
        // Back to 0: the stillness period starts over.
        assert_eq!(k.step(12.0, 0.0), 0.0);
        assert_eq!(k.step(21.0, 0.0), 0.0);
        assert_eq!(k.step(22.0, 0.0), 5.0);
    }
}
