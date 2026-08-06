use std::collections::HashMap;

use crate::backend::{HardwareBackend, HwControl, HwSensor, SensorKind};
use crate::rng::Rng;

/// Deterministic-ish fake PC with real thermodynamics (port of
/// `SimulatedBackend.cs`): the CPU heatsink integrates package watts through a
/// fan-dependent cooling resistance (so written PWM actually changes the
/// temperature), the die rides a power-proportional delta above the sink, and
/// fans' RPM follows the written PWM with lag. Spikes (browser tab, compile
/// burst) and sustained loads (game/AI) come and go.
pub struct SimulatedBackend {
    rng: Rng,
    t: f64,
    sink: f64,
    cpu: f64,
    gpu: f64,
    mb: f64,
    p_cpu: f64,
    spike_until: f64,
    load_until: f64,
    pwm: HashMap<String, f64>, // control id -> commanded %
    rpm: HashMap<String, f64>, // control id -> current rpm
    sensors: Vec<HwSensor>,
    controls: Vec<HwControl>,
}

impl Default for SimulatedBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl SimulatedBackend {
    pub fn new() -> Self {
        let s = |id: &str, name: &str| HwSensor { id: id.into(), name: name.into(), kind: SensorKind::Temp };
        Self {
            rng: Rng::new(7),
            t: 0.0,
            sink: 48.0,
            cpu: 50.0,
            gpu: 38.0,
            mb: 33.0,
            p_cpu: 38.0,
            spike_until: -1.0,
            load_until: -1.0,
            pwm: HashMap::new(),
            rpm: HashMap::new(),
            sensors: vec![
                s("sim/cpu", "CPU (Tctl/Tdie)"),
                s("sim/gpu", "GPU core"),
                s("sim/mb", "Motherboard"),
            ],
            controls: vec![
                HwControl { id: "sim/fan/cpu".into(), name: "CPU_FAN1 (NH-D15)".into() },
                HwControl { id: "sim/fan/case".into(), name: "CHA_FAN1 (NF-A14 ×4)".into() },
            ],
        }
    }
}

impl HardwareBackend for SimulatedBackend {
    fn description(&self) -> &str {
        "Simulation (no hardware control — demo sensors)"
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

    fn update(&mut self) {
        self.t += 1.0;

        // Random 2–6 s spikes (browser tab, compile burst) roughly every ~25 s.
        if self.t > self.spike_until && self.rng.next_f64() < 0.04 {
            self.spike_until = self.t + 2.0 + self.rng.next_f64() * 4.0;
        }
        // Occasional 60–120 s sustained load (game/AI) roughly every ~5 min.
        if self.t > self.load_until && self.rng.next_f64() < 0.004 {
            self.load_until = self.t + 60.0 + self.rng.next_f64() * 60.0;
        }

        let spike = self.t <= self.spike_until;
        let load = self.t <= self.load_until;

        // Package power responds instantly; the temperatures follow the physics below.
        self.p_cpu = 38.0
            + if spike { 62.0 } else { 0.0 }
            + if load { 150.0 } else { 0.0 }
            + (self.rng.next_f64() - 0.5) * 6.0;

        let f_cpu = self.pwm.get("sim/fan/cpu").copied().unwrap_or(35.0) / 100.0;
        let f_case = self.pwm.get("sim/fan/case").copied().unwrap_or(30.0) / 100.0;

        // Plant: sink mass ≈ 420 J/°C behind a fan-dependent resistance; case airflow
        // lowers the intake baseline a little. The die reads ~0.055 °C/W above the sink
        // (AM5 thermal density: Tctl jumps the moment load lands, whatever the fan does).
        let base_t = 27.0 + 5.0 * (1.0 - f_case) + (self.gpu - 40.0).max(0.0) * 0.05;
        let r = ((0.55 - 0.34 * f_cpu.sqrt()) * (1.0 - 0.10 * f_case)).max(0.16);
        self.sink += (self.p_cpu - (self.sink - base_t) / r) / 420.0;
        self.cpu = self.sink + (self.p_cpu - 38.0).max(0.0) * 0.055 + (self.rng.next_f64() - 0.5) * 0.8;

        let gpu_target = if load { 76.0 } else { 38.0 };
        self.gpu += (gpu_target - self.gpu) * 0.06 + (self.rng.next_f64() - 0.5) * 0.6;
        self.mb += ((self.cpu + self.gpu) / 2.0 * 0.55 + 12.0 - self.mb) * 0.01;

        for c in &self.controls {
            let target = self.pwm.get(&c.id).map_or(900.0, |p| p * 25.0); // % → rpm
            let cur = self.rpm.get(&c.id).copied().unwrap_or(target);
            self.rpm.insert(c.id.clone(), cur + (target - cur) * 0.3);
        }
    }

    fn read_value(&self, sensor_id: &str) -> Option<f64> {
        match sensor_id {
            "sim/cpu" => Some(self.cpu),
            "sim/gpu" => Some(self.gpu),
            "sim/mb" => Some(self.mb),
            _ => None,
        }
    }

    fn set_control(&mut self, control_id: &str, percent: f64) {
        self.pwm.insert(control_id.to_string(), percent);
    }

    fn release_control(&mut self, control_id: &str) {
        self.pwm.remove(control_id);
    }

    fn read_control_rpm(&self, control_id: &str) -> Option<f64> {
        self.rpm.get(control_id).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pwm_actually_cools_the_die() {
        // Two identical sims, one with fans pinned at 100%, one at 0%; under the
        // same random sequence the ventilated die must end up cooler.
        let mut hot = SimulatedBackend::new();
        let mut cool = SimulatedBackend::new();
        cool.set_control("sim/fan/cpu", 100.0);
        cool.set_control("sim/fan/case", 100.0);
        hot.set_control("sim/fan/cpu", 0.0);
        hot.set_control("sim/fan/case", 0.0);
        for _ in 0..600 {
            hot.update();
            cool.update();
        }
        let hot_t = hot.read_value("sim/cpu").unwrap();
        let cool_t = cool.read_value("sim/cpu").unwrap();
        assert!(cool_t + 5.0 < hot_t, "100% fan {cool_t}° should sit well under 0% fan {hot_t}°");
    }
}
