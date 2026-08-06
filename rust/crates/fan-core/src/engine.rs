use std::collections::HashMap;
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use crate::backend::HardwareBackend;
use crate::curve::FanCurve;
use crate::filter::ResponseFilter;
use crate::kick::IdleKick;
use crate::probe::StopProbe;
use crate::profile::Profile;

/// Why the commanded % differs from the curve's configured level right now
/// (port of `OutputReason` in `FanEngine.cs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutputReason {
    None,         // output matches the curve — nothing to explain
    RampUp,       // slew limit still gliding up toward the target level
    RampDown,     // slew limit still gliding down toward the target level
    StepDownHold, // lower band reached, waiting out step_down_hold_seconds
    Hysteresis,   // avg maps lower but is not yet hysteresis_c clear of the band edge
    ZeroSnap,     // curve level is under zero_snap_percent → running at 0 instead
    MinFloor,     // channel min_percent overrides a lower curve level
    IdleKick,     // stopped fan briefly spun up by the idle kick
    StopProbe,    // trial stop — fan held at 0 to see if it is needed at all
}

impl std::fmt::Display for OutputReason {
    /// The C# enum names — the telemetry CSV's `reason` column vocabulary,
    /// kept identical so existing analysis scripts parse both apps' logs.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            OutputReason::None => "None",
            OutputReason::RampUp => "RampUp",
            OutputReason::RampDown => "RampDown",
            OutputReason::StepDownHold => "StepDownHold",
            OutputReason::Hysteresis => "Hysteresis",
            OutputReason::ZeroSnap => "ZeroSnap",
            OutputReason::MinFloor => "MinFloor",
            OutputReason::IdleKick => "IdleKick",
            OutputReason::StopProbe => "StopProbe",
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelStatus {
    pub name: String,
    pub raw_temp: Option<f64>,
    /// Rolling average driving the curve; NaN until the channel has a reading.
    pub effective_temp: f64,
    pub output_percent: f64,
    pub rpm: Option<f64>,
    pub applied: bool,
    pub target_percent: f64,
    pub reason: OutputReason,
    /// The "other" % involved (pending step / curve level).
    pub reason_level: f64,
    /// StepDownHold: time to the step.
    pub reason_seconds: f64,
    /// Every assigned header's rpm, in `control_ids` order — `rpm` is just the
    /// first. A channel driving several headers can have one fan stalled while
    /// the rest spin, and only a per-header reading makes that visible.
    pub rpms: Vec<Option<f64>>,
}

/// Port of `FanEngine.cs`, minus the timer: the daemon owns the clock and calls
/// `tick(now)` roughly once a second (jittered). All time-based logic downstream
/// uses the monotonic `now` parameter, so uneven spacing is harmless — and tests
/// can replay any timeline.
pub struct FanEngine<B: HardwareBackend> {
    hw: B,
    profile: Profile,
    applying: bool,
    filters: HashMap<usize, ResponseFilter>,
    kicks: HashMap<usize, IdleKick>,
    probes: HashMap<usize, StopProbe>,
    settings_sig: Option<String>,
    sensor_history_hours: Option<f64>, // None → applied on the first tick
}

impl<B: HardwareBackend> FanEngine<B> {
    pub fn new(hw: B, profile: Profile) -> Self {
        Self {
            hw,
            profile,
            applying: false,
            filters: HashMap::new(),
            kicks: HashMap::new(),
            probes: HashMap::new(),
            settings_sig: None,
            sensor_history_hours: None,
        }
    }

    pub fn backend(&self) -> &B {
        &self.hw
    }

    pub fn profile(&self) -> &Profile {
        &self.profile
    }

    /// Live edits go through here; the settings fingerprint picks them up on the
    /// next tick and instant-applies them, exactly like the C# UI editing the
    /// shared Profile object.
    pub fn profile_mut(&mut self) -> &mut Profile {
        &mut self.profile
    }

    pub fn applying(&self) -> bool {
        self.applying
    }

    /// Begin writing PWM (until then the engine only monitors/previews).
    pub fn apply(&mut self) {
        self.applying = true;
    }

    pub fn stop_applying(&mut self) {
        self.applying = false;
        for ch in &self.profile.channels {
            for id in &ch.control_ids {
                self.hw.release_control(id);
            }
        }
    }

    pub fn replace_profile(&mut self, p: Profile) {
        // Release controls the new profile no longer claims.
        let keep: std::collections::HashSet<&str> =
            p.channels.iter().flat_map(|c| c.control_ids.iter().map(String::as_str)).collect();
        for ch in &self.profile.channels {
            for id in &ch.control_ids {
                if !keep.contains(id.as_str()) {
                    self.hw.release_control(id);
                }
            }
        }
        self.profile = p;
        self.filters.clear();
        self.kicks.clear();
        self.probes.clear();
        self.settings_sig = None;
    }

    /// Fingerprint of every setting that shapes the control path: zero snap,
    /// per-channel tuning, the curve and the sensor assignments. When it changes
    /// between ticks the change came from a user edit — and every live filter is
    /// told to apply it INSTANTLY, skipping step-down holds, hysteresis and the
    /// slew glide. Idle-kick/stop-probe params and UI-only settings stay out:
    /// they need no snap, and a false positive here jumps the fan for no reason.
    fn settings_signature(&self) -> String {
        let p = &self.profile;
        let mut s = String::with_capacity(256);
        let n = |s: &mut String, v: f64| {
            let _ = write!(s, "{v} ");
        };
        n(&mut s, if p.zero_snap_enabled { p.zero_snap_percent } else { 0.0 });
        for ch in &p.channels {
            s.push('|');
            s.push(if ch.enabled { '+' } else { '-' });
            n(&mut s, if p.safety_floor_enabled { ch.min_percent } else { 0.0 });
            n(&mut s, ch.averaging_seconds);
            n(&mut s, ch.hysteresis_c);
            n(&mut s, ch.step_down_hold_seconds);
            n(&mut s, ch.slew_up_percent_per_sec);
            n(&mut s, ch.slew_down_percent_per_sec);
            for pt in &ch.points {
                n(&mut s, pt.temp_c);
                n(&mut s, pt.percent);
            }
            s.push('/');
            s.push_str(&ch.sensor_ids.join(","));
        }
        s
    }

    /// One engine tick at monotonic time `now` (seconds). Reads temperatures,
    /// runs each channel's curve through its filter chain, writes PWM when
    /// applying, and returns the per-channel status for UI/telemetry.
    pub fn tick(&mut self, now: f64) -> Vec<ChannelStatus> {
        // Applied before update(), on the same thread, so a launch with 0 h never
        // accumulates a single history entry (mirrors the C# ordering).
        if self.sensor_history_hours != Some(self.profile.sensor_history_hours) {
            self.sensor_history_hours = Some(self.profile.sensor_history_hours);
            self.hw
                .set_sensor_history_window(self.profile.sensor_history_hours.clamp(0.0, 24.0));
        }
        self.hw.update();

        // A changed fingerprint = a user edit landed since the last tick →
        // apply it on THIS tick, skipping holds and the slew glide.
        let sig = self.settings_signature();
        if let Some(prev) = &self.settings_sig {
            if *prev != sig && self.profile.instant_apply_enabled {
                for f in self.filters.values_mut() {
                    f.apply_now();
                }
            }
        }
        self.settings_sig = Some(sig);

        let mut statuses = Vec::with_capacity(self.profile.channels.len());
        for i in 0..self.profile.channels.len() {
            let ch = self.profile.channels[i].clone(); // snapshot, as the C# tick snapshots lists

            let temp = ch
                .sensor_ids
                .iter()
                .filter_map(|id| self.hw.read_value(id))
                .fold(None, |acc: Option<f64>, v| Some(acc.map_or(v, |a| a.max(v))));

            // The safety floor degrades to "no floor" when the feature is off — a
            // disabled floor must not block a stop, gate the trial stops, or claim
            // the MinFloor why-chip.
            let min_pct = if self.profile.safety_floor_enabled { ch.min_percent } else { 0.0 };

            let mut output = 0.0;
            let mut applied = false;
            let mut reason = OutputReason::None;
            let mut target_pct = 0.0;
            let mut reason_level = 0.0;
            let mut reason_seconds = 0.0;
            let mut effective_temp = f64::NAN;

            if let Some(temp) = temp {
                let curve = FanCurve::new(&ch.points);
                let filter = self.filters.entry(i).or_default();
                filter.averaging_seconds = ch.averaging_seconds;
                filter.hysteresis_c = ch.hysteresis_c;
                filter.step_down_hold_seconds = ch.step_down_hold_seconds;
                filter.slew_up_percent_per_sec = ch.slew_up_percent_per_sec;
                filter.slew_down_percent_per_sec = ch.slew_down_percent_per_sec;
                filter.zero_snap_percent =
                    if self.profile.zero_snap_enabled { self.profile.zero_snap_percent } else { 0.0 };

                let filtered = filter.step(now, temp, &curve);
                effective_temp = filter.effective_temp;
                output = min_pct.max(filtered);
                target_pct = min_pct.max(filter.target_level());

                // Most specific explanation wins; later checks override earlier ones.
                if let Some(remaining) = filter.down_hold_remaining {
                    reason = OutputReason::StepDownHold;
                    reason_level = min_pct.max(filter.pending_down_level.unwrap_or(f64::NAN));
                    reason_seconds = remaining;
                } else if filter.hysteresis_holding {
                    reason = OutputReason::Hysteresis;
                } else if filter.snapped_to_zero && output <= 0.01 {
                    reason = OutputReason::ZeroSnap;
                    reason_level = filter.curve_level;
                }
                if (output - target_pct).abs() > 0.5 {
                    reason = if output < target_pct { OutputReason::RampUp } else { OutputReason::RampDown };
                }
                if min_pct > 0.0 && filtered < min_pct - 0.01 {
                    reason = OutputReason::MinFloor;
                    reason_level = filter.curve_level;
                }

                if self.applying && ch.enabled && !ch.control_ids.is_empty() {
                    // Trial stop runs BEFORE the kick, so a probe-stopped channel counts
                    // as stopped for the idle kick (same choice as zero snap). Channels
                    // with a safety floor are never trial-stopped — the floor wins.
                    if self.profile.stop_probe_enabled && min_pct <= 0.0 {
                        let probe = self.probes.entry(i).or_default();
                        probe.run_seconds = self.profile.stop_probe_run_seconds;
                        probe.probe_seconds = self.profile.stop_probe_seconds;
                        probe.stable_range_c = self.profile.stop_probe_stable_range_c;
                        probe.fail_retry_seconds = self.profile.stop_probe_retry_seconds;
                        probe.max_temp_c = self.profile.stop_probe_max_temp_c;
                        output = probe.step(now, temp, output);
                        if probe.holding() {
                            reason = OutputReason::StopProbe;
                        }
                    } else {
                        self.probes.remove(&i); // re-enabling starts with a fresh window
                    }
                    if self.profile.idle_kick_enabled {
                        let kick = self.kicks.entry(i).or_default();
                        kick.stopped_seconds = self.profile.idle_kick_stopped_seconds;
                        kick.kick_percent = self.profile.idle_kick_percent;
                        kick.kick_seconds = self.profile.idle_kick_seconds;
                        output = kick.step(now, output);
                        if kick.kicking() {
                            reason = OutputReason::IdleKick;
                        }
                    } else {
                        self.kicks.remove(&i); // re-enabling starts the stopped clock fresh
                    }
                    for id in &ch.control_ids {
                        self.hw.set_control(id, output);
                    }
                    applied = true;
                }
            }
            // While the BIOS (or nothing) drives the fan, the stopped/running clocks must not run.
            if !applied {
                self.kicks.remove(&i);
                self.probes.remove(&i);
            }

            let rpms: Vec<Option<f64>> =
                ch.control_ids.iter().map(|id| self.hw.read_control_rpm(id)).collect();
            statuses.push(ChannelStatus {
                name: ch.name.clone(),
                raw_temp: temp,
                effective_temp,
                output_percent: output,
                rpm: rpms.first().copied().flatten(),
                applied,
                target_percent: target_pct,
                reason,
                reason_level,
                reason_seconds,
                rpms,
            });
        }
        statuses
    }
}

impl<B: HardwareBackend> Drop for FanEngine<B> {
    /// Hand every header back to the BIOS — the same guarantee as the C# Dispose.
    fn drop(&mut self) {
        self.stop_applying();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{HwControl, HwSensor, SensorKind};
    use crate::curve::CurvePoint;
    use crate::profile::ChannelConfig;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;

    type PwmLog = Rc<RefCell<HashMap<String, Option<f64>>>>; // None = released

    /// Scriptable fake: the test sets the temperature; writes are recorded in a
    /// shared log the test can keep after the engine consumes the backend.
    struct FakeHw {
        temp: f64,
        pwm: PwmLog,
        sensors: Vec<HwSensor>,
        controls: Vec<HwControl>,
    }

    impl FakeHw {
        fn new() -> Self {
            Self {
                temp: 40.0,
                pwm: Rc::new(RefCell::new(HashMap::new())),
                sensors: vec![HwSensor { id: "t0".into(), name: "CPU".into(), kind: SensorKind::Temp }],
                controls: vec![HwControl { id: "c0".into(), name: "CPU_FAN".into() }],
            }
        }

        fn written(&self) -> Option<f64> {
            self.pwm.borrow().get("c0").copied().flatten()
        }
    }

    impl HardwareBackend for FakeHw {
        fn description(&self) -> &str {
            "fake"
        }
        fn is_simulated(&self) -> bool {
            true
        }
        fn sensors(&self) -> &[HwSensor] {
            &self.sensors
        }
        fn controls(&self) -> &[HwControl] {
            &self.controls
        }
        fn update(&mut self) {}
        fn read_value(&self, id: &str) -> Option<f64> {
            (id == "t0").then_some(self.temp)
        }
        fn set_control(&mut self, id: &str, percent: f64) {
            self.pwm.borrow_mut().insert(id.into(), Some(percent));
        }
        fn release_control(&mut self, id: &str) {
            self.pwm.borrow_mut().insert(id.into(), None);
        }
        fn read_control_rpm(&self, _id: &str) -> Option<f64> {
            Some(1000.0)
        }
    }

    fn profile() -> Profile {
        Profile {
            stop_probe_enabled: false, // exercised separately
            channels: vec![ChannelConfig {
                name: "CPU cooler".into(),
                control_ids: vec!["c0".into()],
                sensor_ids: vec!["t0".into()],
                points: vec![CurvePoint::new(20.0, 0.0), CurvePoint::new(50.0, 40.0)],
                averaging_seconds: 5.0,
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn only_writes_pwm_while_applying() {
        let mut e = FanEngine::new(FakeHw::new(), profile());
        e.tick(0.0);
        assert_eq!(e.backend().written(), None);
        e.apply();
        e.tick(1.0);
        assert!(e.backend().written().is_some());
    }

    #[test]
    fn drop_releases_controls_to_bios() {
        let hw = FakeHw::new();
        let log = Rc::clone(&hw.pwm);
        let mut e = FanEngine::new(hw, profile());
        e.apply();
        e.tick(0.0);
        assert!(log.borrow().get("c0").copied().flatten().is_some());
        drop(e); // the Drop impl must hand the header back (None = released)
        assert_eq!(log.borrow().get("c0").copied().flatten(), None);
    }

    #[test]
    fn min_floor_reason_and_output() {
        let mut p = profile();
        p.channels[0].min_percent = 30.0;
        let mut e = FanEngine::new(FakeHw::new(), p);
        e.apply();
        let st = e.tick(0.0); // 40° avg → curve says 0%, floor lifts to 30
        assert_eq!(st[0].output_percent, 30.0);
        assert_eq!(st[0].reason, OutputReason::MinFloor);
        // Switching the safety floor off drops the floor (and the reason).
        e.profile_mut().safety_floor_enabled = false;
        let st = e.tick(1.0);
        assert_eq!(st[0].output_percent, 0.0);
        assert_ne!(st[0].reason, OutputReason::MinFloor);
    }

    #[test]
    fn settings_edit_applies_instantly() {
        let mut e = FanEngine::new(FakeHw::new(), profile());
        e.apply();
        for t in 0..5 {
            e.tick(t as f64);
        }
        // Curve says 0 at 40°; now edit the curve so 40° maps to 80% — with
        // instant apply the very next tick jumps, no 8 %/s glide from 0.
        e.profile_mut().channels[0].points = vec![CurvePoint::new(20.0, 80.0)];
        let st = e.tick(5.0);
        assert_eq!(st[0].output_percent, 80.0);
    }

    #[test]
    fn instant_apply_off_keeps_the_glide() {
        let mut e = FanEngine::new(FakeHw::new(), profile());
        e.profile_mut().instant_apply_enabled = false;
        e.apply();
        for t in 0..5 {
            e.tick(t as f64);
        }
        e.profile_mut().channels[0].points = vec![CurvePoint::new(20.0, 80.0)];
        let st = e.tick(5.0);
        // Read within the tick, but felt through the slew: one 8 %/s step.
        assert!(st[0].output_percent <= 8.0 + 1e-9);
        assert_eq!(st[0].reason, OutputReason::RampUp);
    }

    #[test]
    fn stop_probe_holds_and_reports() {
        let mut p = profile();
        p.stop_probe_enabled = true;
        p.stop_probe_run_seconds = 10.0;
        p.channels[0].points = vec![CurvePoint::new(20.0, 30.0)]; // always wants 30%
        let mut e = FanEngine::new(FakeHw::new(), p);
        e.apply();
        let mut held = false;
        for t in 0..=12 {
            let st = e.tick(t as f64);
            if st[0].reason == OutputReason::StopProbe {
                assert_eq!(st[0].output_percent, 0.0);
                held = true;
            }
        }
        assert!(held, "a stable 10 s run should have earned a trial stop");
    }
}
