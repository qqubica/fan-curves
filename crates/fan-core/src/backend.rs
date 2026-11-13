//! Abstraction over the machine, so the engine runs identically on real
//! hardware and in simulation.

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

    /// Whether the most recent control write is known to have reached the
    /// device. Backends without an acknowledgement mechanism report success.
    fn control_write_succeeded(&self, _control_id: &str) -> bool { true }

    /// Give control back to the BIOS/firmware curve.
    fn release_control(&mut self, control_id: &str);

    /// Fan RPM sensor associated with a control, if the backend knows it.
    fn read_control_rpm(&self, control_id: &str) -> Option<f64>;
}
