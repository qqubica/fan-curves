//! Linux hardware backend: the kernel's hwmon sysfs interface. No library —
//! `k10temp` provides CPU Tctl, `nct6683` the board temps/tach/PWM (note: that
//! driver ships with PWM writes disabled unless loaded with its `force=1`
//! module option, because vendor EC firmwares vary), plain files everywhere:
//!
//!   /sys/class/hwmon/hwmonN/name          chip name
//!   …/tempX_input (m°C), tempX_label      temperature sensors
//!   …/fanX_input (rpm)                    tachometers
//!   …/pwmX (0–255), pwmX_enable           duty + mode (1 = manual)
//!
//! Handback contract: the first write to a pwm saves the original
//! `pwmX_enable` value; `release_control` restores it, returning the header to
//! the firmware's own curve — the sysfs equivalent of LHM's `SetDefault`.
#![cfg(target_os = "linux")]

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::backend::{HardwareBackend, HwControl, HwSensor, SensorKind};

pub struct HwmonBackend {
    description: String,
    sensors: Vec<HwSensor>,
    controls: Vec<HwControl>,
    temp_paths: HashMap<String, PathBuf>, // sensor id → tempX_input
    fan_paths: HashMap<String, PathBuf>,  // control id → fanX_input (same index as pwmX)
    pwm_paths: HashMap<String, PathBuf>,  // control id → pwmX
    enable_paths: HashMap<String, PathBuf>, // control id → pwmX_enable
    saved_enable: HashMap<String, String>, // original pwmX_enable, restored on release
    values: HashMap<String, f64>,         // refreshed each update()
}

fn read_trim(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

impl HwmonBackend {
    pub fn new() -> Self {
        let mut sensors = Vec::new();
        let mut controls = Vec::new();
        let mut temp_paths = HashMap::new();
        let mut fan_paths = HashMap::new();
        let mut pwm_paths = HashMap::new();
        let mut enable_paths = HashMap::new();
        let mut chips = Vec::new();

        let root = Path::new("/sys/class/hwmon");
        let entries = fs::read_dir(root).into_iter().flatten().flatten();
        for entry in entries {
            let dir = entry.path();
            let hwmon = entry.file_name().to_string_lossy().into_owned();
            let chip = read_trim(&dir.join("name")).unwrap_or_else(|| hwmon.clone());
            chips.push(chip.clone());

            for i in 1..=16u32 {
                let input = dir.join(format!("temp{i}_input"));
                if input.exists() {
                    let label = read_trim(&dir.join(format!("temp{i}_label")))
                        .unwrap_or_else(|| format!("temp{i}"));
                    let id = format!("{hwmon}:temp{i}");
                    temp_paths.insert(id.clone(), input);
                    sensors.push(HwSensor {
                        id,
                        name: format!("{chip} {label}"),
                        kind: SensorKind::Temp,
                    });
                }

                let pwm = dir.join(format!("pwm{i}"));
                let has_pwm = pwm.exists();
                if has_pwm {
                    let id = format!("{hwmon}:pwm{i}");
                    pwm_paths.insert(id.clone(), pwm);
                    enable_paths.insert(id.clone(), dir.join(format!("pwm{i}_enable")));
                    // Convention: fanN_input is the tach of pwmN on every chip
                    // this app targets (nct6683 included).
                    let fan = dir.join(format!("fan{i}_input"));
                    if fan.exists() {
                        fan_paths.insert(id.clone(), fan);
                    }
                    controls.push(HwControl { id, name: format!("{chip} pwm{i}") });
                }

                // Tach without a pwm (readable fan): expose as an rpm sensor.
                let fan_only = dir.join(format!("fan{i}_input"));
                if fan_only.exists() && !has_pwm {
                    let id = format!("{hwmon}:fan{i}");
                    temp_paths.insert(id.clone(), fan_only); // read path table reused
                    sensors.push(HwSensor {
                        id,
                        name: format!("{chip} fan{i}"),
                        kind: SensorKind::Rpm,
                    });
                }
            }
        }

        chips.sort();
        chips.dedup();
        Self {
            description: format!("Linux hwmon ({})", chips.join(", ")),
            sensors,
            controls,
            temp_paths,
            fan_paths,
            pwm_paths,
            enable_paths,
            saved_enable: HashMap::new(),
            values: HashMap::new(),
        }
    }
}

impl Default for HwmonBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl HardwareBackend for HwmonBackend {
    fn description(&self) -> &str {
        &self.description
    }

    fn is_simulated(&self) -> bool {
        false
    }

    fn sensors(&self) -> &[HwSensor] {
        &self.sensors
    }

    fn controls(&self) -> &[HwControl] {
        &self.controls
    }

    fn update(&mut self) {
        self.values.clear();
        for (id, path) in &self.temp_paths {
            if let Some(raw) = read_trim(path).and_then(|s| s.parse::<f64>().ok()) {
                // temp*_input is millidegrees; fan*_input (rpm sensors reusing
                // this table) is plain rpm — tell them apart by the id.
                let v = if id.contains(":temp") { raw / 1000.0 } else { raw };
                self.values.insert(id.clone(), v);
            }
        }
        for (id, path) in &self.fan_paths {
            if let Some(rpm) = read_trim(path).and_then(|s| s.parse::<f64>().ok()) {
                self.values.insert(format!("{id}#rpm"), rpm);
            }
        }
    }

    fn read_value(&self, sensor_id: &str) -> Option<f64> {
        self.values.get(sensor_id).copied()
    }

    fn set_control(&mut self, control_id: &str, percent: f64) {
        let Some(pwm) = self.pwm_paths.get(control_id) else { return };
        // First write claims the channel: remember the firmware's mode, go manual.
        if !self.saved_enable.contains_key(control_id) {
            if let Some(enable) = self.enable_paths.get(control_id) {
                let original = read_trim(enable).unwrap_or_else(|| "5".to_string());
                if fs::write(enable, "1").is_ok() {
                    self.saved_enable.insert(control_id.to_string(), original);
                }
            }
        }
        let duty = (percent.clamp(0.0, 100.0) * 2.55).round() as u32;
        let _ = fs::write(pwm, duty.to_string());
    }

    fn release_control(&mut self, control_id: &str) {
        if let Some(original) = self.saved_enable.remove(control_id) {
            if let Some(enable) = self.enable_paths.get(control_id) {
                let _ = fs::write(enable, original);
            }
        }
    }

    fn read_control_rpm(&self, control_id: &str) -> Option<f64> {
        self.values.get(&format!("{control_id}#rpm")).copied()
    }
}
