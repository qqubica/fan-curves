/// Abstraction over the machine so the app runs identically in simulation
/// (port of `IHardwareBackend.cs`).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SensorKind {
    Temp,
    Rpm,
}

#[derive(Debug, Clone)]
pub struct HwSensor {
    pub id: String,
    pub name: String,
    pub kind: SensorKind,
}

#[derive(Debug, Clone)]
pub struct HwControl {
    pub id: String,
    pub name: String,
}

pub trait HardwareBackend {
    fn description(&self) -> &str;
    fn is_simulated(&self) -> bool;

    fn sensors(&self) -> &[HwSensor];
    fn controls(&self) -> &[HwControl];

    /// Refresh hardware readings; call once per engine tick.
    fn update(&mut self);

    fn read_value(&self, sensor_id: &str) -> Option<f64>;

    /// Write a PWM duty cycle 0–100 to a control.
    fn set_control(&mut self, control_id: &str, percent: f64);

    /// Give control back to the BIOS/firmware curve.
    fn release_control(&mut self, control_id: &str);

    /// Fan RPM sensor associated with a control, if the backend knows it.
    fn read_control_rpm(&self, control_id: &str) -> Option<f64>;

    /// Every sensor the backend's library tracks internally — not just the
    /// temp/rpm ones exposed in `sensors()`. Feeds the history RAM estimate.
    fn internal_sensor_count(&self) -> usize {
        self.sensors().len()
    }

    /// How long the backend's own library may keep per-sensor value history,
    /// in hours; zero = none. Call from the same thread as `update()`.
    fn set_sensor_history_window(&mut self, _hours: f64) {}
}
