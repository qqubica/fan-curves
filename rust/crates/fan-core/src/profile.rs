use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::curve::CurvePoint;

/// Port of `Profile.cs` / `ChannelConfig`. Field names serialize PascalCase so
/// the Rust daemon reads (and writes) the exact `profile.json` the WPF app
/// uses; unknown fields from older schemas are ignored, missing fields take the
/// same defaults as the C# property initializers.
///
/// `TuningSnapshot` (preset undo) is UI machinery and deliberately not ported here.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct ChannelConfig {
    pub name: String,
    /// Hardware control identifiers this channel drives (empty = unassigned).
    pub control_ids: Vec<String>,
    /// Temperature sensor identifiers; the MAX of them feeds the curve.
    pub sensor_ids: Vec<String>,
    pub enabled: bool,
    /// Safety floor in percent — the engine never writes below this.
    pub min_percent: f64,
    /// Step thresholds: at point N's temp and above, run at point N's percent (staircase).
    pub points: Vec<CurvePoint>,
    /// The curve is driven by the rolling average over this window, not the live temp.
    pub averaging_seconds: f64,
    /// A step is only left downward once (average + this) still maps to a lower step.
    pub hysteresis_c: f64,
    /// The hysteresis condition must hold continuously this long before the step-down fires.
    pub step_down_hold_seconds: f64,
    pub slew_up_percent_per_sec: f64,
    pub slew_down_percent_per_sec: f64,
}

impl Default for ChannelConfig {
    fn default() -> Self {
        Self {
            name: "Fan".into(),
            control_ids: Vec::new(),
            sensor_ids: Vec::new(),
            enabled: true,
            min_percent: 0.0,
            points: Vec::new(),
            averaging_seconds: 20.0,
            hysteresis_c: 3.0,
            step_down_hold_seconds: 10.0,
            slew_up_percent_per_sec: 8.0,
            slew_down_percent_per_sec: 8.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Profile {
    pub name: String,
    /// When false the app never (re)registers its start-with-OS task.
    pub autostart_enabled: bool,
    /// When true the close button hides to the tray instead of exiting.
    pub minimize_to_tray_on_close: bool,
    /// When false a settings edit is only felt after the normal holds/slew
    /// instead of being adopted on the tick it lands.
    pub instant_apply_enabled: bool,
    /// When false nothing is written to the logs (per-tick CSV + behavior.txt).
    pub telemetry_logging_enabled: bool,
    /// When false the process runs at normal priority instead of High.
    pub high_priority_enabled: bool,
    /// How long the hardware library keeps per-sensor value history (hours; 0 = none).
    pub sensor_history_hours: f64,
    /// When false, fans sitting at 0% are never periodically spun up.
    pub idle_kick_enabled: bool,
    /// Seconds a driven fan must sit at 0% before a kick fires.
    pub idle_kick_stopped_seconds: f64,
    /// PWM percent written while a kick runs.
    pub idle_kick_percent: f64,
    /// How long each kick lasts, in seconds.
    pub idle_kick_seconds: f64,
    /// When true, steadily running fans with stable temps are trial-stopped.
    pub stop_probe_enabled: bool,
    pub stop_probe_run_seconds: f64,
    pub stop_probe_seconds: f64,
    pub stop_probe_stable_range_c: f64,
    pub stop_probe_retry_seconds: f64,
    pub stop_probe_max_temp_c: f64,
    /// When false every channel's MinPercent is ignored — the fans are allowed
    /// all the way to a stop wherever the curve asks for less than the floor.
    pub safety_floor_enabled: bool,
    /// When true, curve targets below ZeroSnapPercent run the fan at 0% instead.
    pub zero_snap_enabled: bool,
    /// Targets above 0% but below this stop the fan — meaningful speed or nothing.
    pub zero_snap_percent: f64,
    pub channels: Vec<ChannelConfig>,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            name: "Profile".into(),
            autostart_enabled: true,
            minimize_to_tray_on_close: true,
            instant_apply_enabled: true,
            telemetry_logging_enabled: true,
            high_priority_enabled: true,
            sensor_history_hours: 0.0,
            idle_kick_enabled: false,
            idle_kick_stopped_seconds: 10.0,
            idle_kick_percent: 5.0,
            idle_kick_seconds: 5.0,
            stop_probe_enabled: true,
            stop_probe_run_seconds: 30.0,
            stop_probe_seconds: 30.0,
            stop_probe_stable_range_c: 3.5,
            stop_probe_retry_seconds: 60.0,
            stop_probe_max_temp_c: 78.0,
            safety_floor_enabled: true,
            zero_snap_enabled: true,
            zero_snap_percent: 20.0,
            channels: Vec::new(),
        }
    }
}

impl Profile {
    /// The default: silent flat floor at idle, dead zone against spikes,
    /// smooth ramp only under sustained load. Case fans fully stop at idle.
    pub fn mac_book_like() -> Self {
        let pt = CurvePoint::new;
        Self {
            name: "Quiet (MacBook-like)".into(),
            channels: vec![
                ChannelConfig {
                    name: "CPU cooler".into(),
                    min_percent: 0.0, // true MacBook behaviour: fans fully stop at idle
                    points: vec![
                        // Silent (stopped) through everything up to 50°C avg, then a
                        // barely-audible 10% start; the top steps carry sustained load.
                        pt(20.0, 0.0),
                        pt(50.0, 10.0),
                        pt(55.0, 20.0),
                        pt(62.0, 40.0),
                        pt(70.0, 50.0),
                        pt(76.0, 65.0),
                        pt(84.0, 81.0),
                        pt(88.0, 90.0),
                        pt(92.0, 100.0),
                    ],
                    // 90 s averaging window: only genuinely sustained load moves the fans.
                    averaging_seconds: 90.0,
                    hysteresis_c: 1.5,
                    step_down_hold_seconds: 25.0,
                    slew_up_percent_per_sec: 9.0,
                    slew_down_percent_per_sec: 8.0,
                    ..Default::default()
                },
                ChannelConfig {
                    name: "Case fans".into(),
                    min_percent: 0.0, // 0% = fans stopped
                    points: vec![
                        pt(20.0, 0.0), // fully stopped through idle
                        pt(40.0, 5.0), // whisper bands (NF-A14s run at any duty >0%)
                        pt(42.0, 10.0),
                        pt(60.0, 20.0),
                        pt(70.0, 40.0),
                        pt(75.0, 60.0),
                        pt(82.0, 82.0),
                        pt(86.0, 100.0),
                    ],
                    averaging_seconds: 25.0,
                    hysteresis_c: 4.0,
                    step_down_hold_seconds: 10.0,
                    slew_up_percent_per_sec: 7.0,
                    slew_down_percent_per_sec: 7.0,
                    ..Default::default()
                },
            ],
            ..Default::default()
        }
    }

    /// Louder alternative: earlier, faster ramps that keep components cooler.
    pub fn performance() -> Self {
        let pt = CurvePoint::new;
        Self {
            name: "Performance".into(),
            channels: vec![
                ChannelConfig {
                    name: "CPU cooler".into(),
                    min_percent: 30.0,
                    points: vec![pt(20.0, 45.0), pt(50.0, 60.0), pt(65.0, 75.0), pt(80.0, 90.0), pt(90.0, 100.0)],
                    averaging_seconds: 8.0,
                    hysteresis_c: 2.0,
                    step_down_hold_seconds: 5.0,
                    slew_up_percent_per_sec: 6.0,
                    slew_down_percent_per_sec: 2.0,
                    ..Default::default()
                },
                ChannelConfig {
                    name: "Case fans".into(),
                    min_percent: 0.0,
                    points: vec![pt(20.0, 25.0), pt(45.0, 40.0), pt(60.0, 55.0), pt(75.0, 75.0), pt(85.0, 95.0)],
                    averaging_seconds: 10.0,
                    hysteresis_c: 2.0,
                    step_down_hold_seconds: 5.0,
                    slew_up_percent_per_sec: 4.0,
                    slew_down_percent_per_sec: 2.0,
                    ..Default::default()
                },
            ],
            ..Default::default()
        }
    }

    /// Load `profile.json`, falling back to the default preset on a missing,
    /// corrupted, or channel-less file — same contract as the C# `LoadOrDefault`.
    pub fn load_or_default(path: &Path) -> Self {
        if let Ok(text) = std::fs::read_to_string(path) {
            if let Ok(p) = serde_json::from_str::<Profile>(&text) {
                if !p.channels.is_empty() {
                    return p;
                }
            }
        }
        Self::mac_book_like()
    }

    /// Write the profile as pretty JSON. The caller owns read-only policy
    /// (dev/sim flows must never write the real config — see the C# `Profile.ReadOnly`).
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_string_pretty(self).expect("profile serializes");
        std::fs::write(path, json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_preserves_everything() {
        let p = Profile::mac_book_like();
        let json = serde_json::to_string_pretty(&p).unwrap();
        let back: Profile = serde_json::from_str(&json).unwrap();
        assert_eq!(back.name, p.name);
        assert_eq!(back.channels.len(), 2);
        assert_eq!(back.channels[0].points, p.channels[0].points);
        assert_eq!(back.channels[0].averaging_seconds, 90.0);
        assert_eq!(back.channels[1].hysteresis_c, 4.0);
    }

    #[test]
    fn reads_csharp_shaped_json_with_unknown_fields() {
        // Shaped like a System.Text.Json save from the WPF app, including a field
        // from the removed power-mode schema that must be silently ignored, and
        // missing fields that must take the C# defaults.
        let json = r#"{
            "Name": "Custom",
            "ZeroSnapPercent": 28,
            "PowerControlEnabled": true,
            "Channels": [
                {
                    "Name": "CPU cooler",
                    "ControlIds": ["/lpc/nct6686d/0/control/0"],
                    "SensorIds": ["/amdcpu/0/temperature/2"],
                    "MinPercent": 0,
                    "Points": [
                        { "TempC": 20, "Percent": 0 },
                        { "TempC": 50, "Percent": 10 }
                    ],
                    "AveragingSeconds": 90,
                    "HysteresisC": 1.5,
                    "PowerSensorIds": ["/amdcpu/0/power/0"]
                }
            ]
        }"#;
        let p: Profile = serde_json::from_str(json).unwrap();
        assert_eq!(p.name, "Custom");
        assert_eq!(p.zero_snap_percent, 28.0);
        assert!(p.zero_snap_enabled); // missing → C# default true
        assert!(!p.idle_kick_enabled); // missing → C# default false
        assert_eq!(p.stop_probe_max_temp_c, 78.0);
        let ch = &p.channels[0];
        assert_eq!(ch.sensor_ids, vec!["/amdcpu/0/temperature/2"]);
        assert_eq!(ch.points[1], CurvePoint::new(50.0, 10.0));
        assert_eq!(ch.hysteresis_c, 1.5);
        assert_eq!(ch.step_down_hold_seconds, 10.0); // missing → default
        assert!(ch.enabled);
    }

    #[test]
    fn serialized_field_names_are_pascal_case() {
        let json = serde_json::to_string(&Profile::mac_book_like()).unwrap();
        for key in [
            "\"Name\"",
            "\"Channels\"",
            "\"SensorIds\"",
            "\"ControlIds\"",
            "\"MinPercent\"",
            "\"TempC\"",
            "\"Percent\"",
            "\"AveragingSeconds\"",
            "\"HysteresisC\"",
            "\"StepDownHoldSeconds\"",
            "\"SlewUpPercentPerSec\"",
            "\"ZeroSnapPercent\"",
            "\"StopProbeMaxTempC\"",
            "\"SensorHistoryHours\"",
            "\"MinimizeToTrayOnClose\"",
        ] {
            assert!(json.contains(key), "missing {key} in {json}");
        }
    }
}
