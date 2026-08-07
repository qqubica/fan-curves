//! Rust port of `src/FanCurves.Core` (temperature-only feature set, 2026-08-06).
//!
//! Each module mirrors one C# file; behaviour parity is enforced by the unit
//! tests here plus golden traces generated from the C# implementation
//! (`tests/golden.rs`). All control logic is deterministic: every `step` takes
//! monotonic `now` seconds as a parameter, so the engine can be driven by a real
//! clock, a test script, or a replay identically.

pub mod backend;
pub mod curve;
pub mod engine;
pub mod filter;
pub mod history;
pub mod hwmon;
pub mod icon;
pub mod kick;
pub mod probe;
pub mod profile;
pub mod rng;
pub mod sim;

pub use backend::{HardwareBackend, HwControl, HwSensor};

/// Name of the daemon's local IPC socket (named pipe on Windows). One place,
/// shared by the daemon, the UI and the tray, so the three cannot drift apart.
/// `FAN_CURVES_SOCKET` overrides it — that is what lets a development
/// daemon+UI pair run beside the live stack without the two cross-talking
/// (the bind doubles as the single-instance lock, so a second daemon on the
/// DEFAULT name refuses to start at all).
pub fn ipc_socket_name() -> String {
    std::env::var("FAN_CURVES_SOCKET").unwrap_or_else(|_| "fan-curves-daemon.sock".into())
}
pub use curve::{CurvePoint, FanCurve};
pub use engine::{ChannelStatus, FanEngine, OutputReason};
pub use filter::ResponseFilter;
pub use kick::IdleKick;
pub use probe::StopProbe;
pub use profile::{ChannelConfig, ChannelTuning, Profile, TuningSnapshot};
pub use sim::SimulatedBackend;
