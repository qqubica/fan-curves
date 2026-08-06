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
pub mod hwmon;
pub mod icon;
pub mod kick;
pub mod probe;
pub mod profile;
pub mod rng;
pub mod sim;

pub use backend::{HardwareBackend, HwControl, HwSensor};
pub use curve::{CurvePoint, FanCurve};
pub use engine::{ChannelStatus, FanEngine, OutputReason};
pub use filter::ResponseFilter;
pub use kick::IdleKick;
pub use probe::StopProbe;
pub use profile::{ChannelConfig, ChannelTuning, Profile, TuningSnapshot};
pub use sim::SimulatedBackend;
