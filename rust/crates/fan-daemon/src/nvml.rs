//! NVIDIA GPU monitoring and fan control through the documented NVML driver API.
//! A fan is advertised as a control only when the driver reports its writable
//! range. Actual writes can still fail; a failed write immediately restores
//! automatic control and disables that fan for the rest of this session.
#![cfg(windows)]

use std::collections::{HashMap, HashSet};
use std::ffi::{c_char, c_void, CStr};
use std::io;
use std::path::PathBuf;

use fan_core::backend::{HardwareBackend, HwControl, HwSensor, SensorKind};
use libloading::Library;

type Device = *mut c_void;
type Init = unsafe extern "C" fn() -> i32;
type Shutdown = unsafe extern "C" fn() -> i32;
type Count = unsafe extern "C" fn(*mut u32) -> i32;
type Handle = unsafe extern "C" fn(u32, *mut Device) -> i32;
type Name = unsafe extern "C" fn(Device, *mut c_char, u32) -> i32;
type Read = unsafe extern "C" fn(Device, *mut u32) -> i32;
type Limits = unsafe extern "C" fn(Device, *mut u32, *mut u32) -> i32;
type Write = unsafe extern "C" fn(Device, u32, u32) -> i32;
type Reset = unsafe extern "C" fn(Device, u32) -> i32;
type Temperature = unsafe extern "C" fn(Device, u32, *mut u32) -> i32;
type FanRpm = unsafe extern "C" fn(Device, *mut FanSpeedInfo) -> i32;
type GetFieldValues = unsafe extern "C" fn(Device, u32, *mut FieldValue) -> i32;

#[repr(C)]
struct FanSpeedInfo { version: u32, fan: u32, speed: u32 }

#[repr(C)]
#[derive(Clone, Copy)]
union FieldValueData { unsigned_int: u32, padding: u64 }

#[repr(C)]
#[derive(Clone, Copy)]
struct FieldValue {
    field_id: u32,
    scope_id: u32,
    timestamp: i64,
    latency_usec: i64,
    value_type: i32,
    nvml_return: i32,
    value: FieldValueData,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetSystemDirectoryW(buffer: *mut u16, size: u32) -> u32;
}

fn nvml_path() -> io::Result<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    let mut wide = [0u16; 32768];
    let len = unsafe { GetSystemDirectoryW(wide.as_mut_ptr(), wide.len() as u32) } as usize;
    if len == 0 || len >= wide.len() { return Err(io::Error::last_os_error()); }
    Ok(PathBuf::from(std::ffi::OsString::from_wide(&wide[..len])).join("nvml.dll"))
}

struct Api {
    _library: Library,
    shutdown: Shutdown,
    get_temperature: Temperature,
    get_field_values: Option<GetFieldValues>,
    get_fan_rpm: Option<FanRpm>,
    set_fan_speed: Option<Write>,
    reset_fan_speed: Option<Reset>,
}

struct Gpu {
    handle: Device,
    fan_count: u32,
    temperature_sensors: Vec<TemperatureSensor>,
}

#[derive(Clone, Copy)]
enum TemperatureSource { NvmlSensor(u32), FieldValue(u32) }

#[derive(Clone, Copy)]
struct TemperatureSensor { id: &'static str, label: &'static str, source: TemperatureSource }

// NVML's documented nvmlTemperatureSensors_t currently defines GPU (0) and
// GPU_MAX (1). GPU_MAX is the hottest current die reading, not a threshold.
const TEMPERATURE_SENSORS: &[TemperatureSensor] = &[
    TemperatureSensor { id: "0", label: "GPU Core", source: TemperatureSource::NvmlSensor(0) },
    TemperatureSensor { id: "1", label: "GPU Hotspot", source: TemperatureSource::NvmlSensor(1) },
    // NVML_FI_DEV_MEMORY_TEMP (82) is a current reading. Threshold fields are
    // deliberately not treated as live temperatures.
    TemperatureSensor { id: "memory", label: "GPU Memory", source: TemperatureSource::FieldValue(82) },
];

fn read_field_value(api: &Api, device: Device, field_id: u32) -> Option<f64> {
    let get = api.get_field_values?;
    let mut field = FieldValue {
        field_id, scope_id: 0, timestamp: 0, latency_usec: 0,
        value_type: 0, nvml_return: -1,
        value: FieldValueData { padding: 0 },
    };
    let call_status = unsafe { get(device, 1, &mut field) };
    if call_status != 0 || field.nvml_return != 0 || field.value_type != 1 { return None; }
    Some(unsafe { field.value.unsigned_int } as f64)
}

#[derive(Debug, PartialEq, Eq)]
enum FanRequest { Automatic, Manual(u32) }

fn fan_request(percent: f64) -> Option<FanRequest> {
    let speed = percent.is_finite().then(|| percent.round().clamp(0.0, 100.0) as u32)?;
    Some(if speed == 0 { FanRequest::Automatic } else { FanRequest::Manual(speed) })
}

pub struct NvmlBackend {
    api: Api,
    gpus: Vec<Gpu>,
    sensors: Vec<HwSensor>,
    controls: Vec<HwControl>,
    values: HashMap<String, f64>,
    active: HashSet<(usize, u32)>,
    auto_reset_attempted: HashSet<(usize, u32)>,
    automatic_requested: HashSet<(usize, u32)>,
    failed: HashSet<(usize, u32)>,
}

// NVML device handles are opaque, used only by the daemon's single engine thread.
unsafe impl Send for NvmlBackend {}

impl NvmlBackend {
    pub fn detect() -> io::Result<Self> {
        // An elevated daemon must not search the current directory for a DLL.
        let library = unsafe { Library::new(nvml_path()?) }.map_err(io::Error::other)?;
        unsafe {
            fn required<T: Copy>(lib: &Library, name: &[u8]) -> io::Result<T> {
                unsafe { lib.get::<T>(name) }.map(|s| *s).map_err(io::Error::other)
            }
            fn optional<T: Copy>(lib: &Library, name: &[u8]) -> Option<T> {
                unsafe { lib.get::<T>(name) }.ok().map(|s| *s)
            }
            let init: Init = required(&library, b"nvmlInit_v2")?;
            let shutdown: Shutdown = required(&library, b"nvmlShutdown")?;
            let count: Count = required(&library, b"nvmlDeviceGetCount_v2")?;
            let handle: Handle = required(&library, b"nvmlDeviceGetHandleByIndex_v2")?;
            let name: Name = required(&library, b"nvmlDeviceGetName")?;
            let get_temperature: Temperature = required(&library, b"nvmlDeviceGetTemperature")?;
            let get_field_values = optional(&library, b"nvmlDeviceGetFieldValues");
            let get_fan_count: Read = required(&library, b"nvmlDeviceGetNumFans")?;
            let get_fan_rpm: Option<FanRpm> = optional(&library, b"nvmlDeviceGetFanSpeedRPM");
            let limits: Option<Limits> = optional(&library, b"nvmlDeviceGetMinMaxFanSpeed");
            let set_fan_speed: Option<Write> = optional(&library, b"nvmlDeviceSetFanSpeed_v2");
            let reset_fan_speed: Option<Reset> = optional(&library, b"nvmlDeviceSetDefaultFanSpeed_v2");
            if init() != 0 { return Err(io::Error::other("NVML initialization failed")); }
            let mut device_count = 0;
            if count(&mut device_count) != 0 {
                shutdown();
                return Err(io::Error::other("NVML GPU enumeration failed"));
            }
            let api = Api { _library: library, shutdown, get_temperature, get_field_values, get_fan_rpm, set_fan_speed, reset_fan_speed };
            let mut backend = Self { api, gpus: Vec::new(), sensors: Vec::new(), controls: Vec::new(), values: HashMap::new(), active: HashSet::new(), auto_reset_attempted: HashSet::new(), automatic_requested: HashSet::new(), failed: HashSet::new() };
            for index in 0..device_count {
                let mut dev = std::ptr::null_mut();
                if handle(index, &mut dev) != 0 || dev.is_null() { continue; }
                let index = backend.gpus.len();
                let mut label = [0 as c_char; 96];
                let gpu_name = if name(dev, label.as_mut_ptr(), label.len() as u32) == 0 {
                    CStr::from_ptr(label.as_ptr()).to_string_lossy().into_owned()
                } else { format!("NVIDIA GPU {index}") };
                let mut fan_count = 0;
                let _ = get_fan_count(dev, &mut fan_count);
                let (mut min_speed, mut max_speed) = (0, 0);
                let writable = matches!(limits, Some(f) if f(dev, &mut min_speed, &mut max_speed) == 0)
                    && min_speed <= max_speed && max_speed <= 100
                    && backend.api.set_fan_speed.is_some() && backend.api.reset_fan_speed.is_some();
                let temperature_sensors: Vec<_> = TEMPERATURE_SENSORS.iter().copied()
                    .filter(|sensor| match sensor.source {
                        // Always monitor the core: losing this reading must
                        // trigger the automatic-control safety handback.
                        TemperatureSource::NvmlSensor(0) => true,
                        TemperatureSource::NvmlSensor(sensor_type) => {
                            let mut current = 0;
                            get_temperature(dev, sensor_type, &mut current) == 0
                        }
                        TemperatureSource::FieldValue(field_id) => read_field_value(&backend.api, dev, field_id).is_some(),
                    })
                    .collect();
                for sensor in &temperature_sensors {
                    backend.sensors.push(HwSensor {
                        id: format!("/nvidiagpu/{index}/temperature/{}", sensor.id),
                        name: format!("{gpu_name} — {}", sensor.label),
                        kind: SensorKind::Temp,
                    });
                }
                for fan in 0..fan_count.min(16) {
                    backend.sensors.push(HwSensor { id: format!("/nvidiagpu/{index}/fan/{fan}"), name: format!("{gpu_name} — GPU Fan #{}", fan + 1), kind: SensorKind::Rpm });
                    if writable {
                        backend.controls.push(HwControl { id: format!("/nvidiagpu/{index}/control/{fan}"), name: format!("{gpu_name} — GPU Fan #{}", fan + 1) });
                    }
                }
                backend.gpus.push(Gpu { handle: dev, fan_count: fan_count.min(16), temperature_sensors });
            }
            if backend.gpus.is_empty() { return Err(io::Error::other("NVML found no NVIDIA GPU")); }
            Ok(backend)
        }
    }

    fn parse_control(id: &str) -> Option<(usize, u32)> {
        let parts: Vec<_> = id.split('/').collect();
        if parts.len() != 5 || parts[1] != "nvidiagpu" || parts[3] != "control" { return None; }
        Some((parts[2].parse().ok()?, parts[4].parse().ok()?))
    }

    pub fn has_control(&self, id: &str) -> bool {
        self.controls.iter().any(|c| c.id == id)
    }

    pub fn is_manual(&self, id: &str) -> bool {
        Self::parse_control(id).is_some_and(|key| self.active.contains(&key))
    }

    fn restore_automatic(&mut self, index: usize, fan: u32) {
        let key = (index, fan);
        if !self.active.contains(&key) && self.auto_reset_attempted.contains(&key) { return; }
        let Some(gpu) = self.gpus.get(index) else { return; };
        let result = unsafe { (self.api.reset_fan_speed.unwrap())(gpu.handle, fan) };
        if result == 0 {
            self.active.remove(&key);
            self.auto_reset_attempted.insert(key);
        } else {
            eprintln!("GPU fan {index}/{fan} automatic-control restore failed: NVML code {result}");
        }
    }
}

impl HardwareBackend for NvmlBackend {
    fn description(&self) -> &str { "NVIDIA NVML" }
    fn is_simulated(&self) -> bool { false }
    fn sensors(&self) -> &[HwSensor] { &self.sensors }
    fn controls(&self) -> &[HwControl] { &self.controls }
    fn update(&mut self) {
        self.values.clear();
        let mut missing = Vec::new();
        for (index, gpu) in self.gpus.iter().enumerate() {
            let mut value = 0;
            unsafe {
                for sensor in &gpu.temperature_sensors {
                    let current = match sensor.source {
                        TemperatureSource::NvmlSensor(sensor_type) => {
                            if (self.api.get_temperature)(gpu.handle, sensor_type, &mut value) == 0 { Some(value as f64) } else { None }
                        }
                        TemperatureSource::FieldValue(field_id) => read_field_value(&self.api, gpu.handle, field_id),
                    };
                    if let Some(current) = current {
                        self.values.insert(format!("/nvidiagpu/{index}/temperature/{}", sensor.id), current);
                    } else if matches!(sensor.source, TemperatureSource::NvmlSensor(0)) {
                        // Core temperature drives the existing GPU safety behavior.
                        missing.extend(self.active.iter().filter(|(i, _)| *i == index).copied());
                    }
                }
                for fan in 0..gpu.fan_count {
                    if let Some(get_rpm) = self.api.get_fan_rpm {
                        let mut info = FanSpeedInfo { version: std::mem::size_of::<FanSpeedInfo>() as u32 | (1 << 24), fan, speed: 0 };
                        if get_rpm(gpu.handle, &mut info) == 0 {
                            self.values.insert(format!("/nvidiagpu/{index}/fan/{fan}"), info.speed as f64);
                        }
                    }
                }
            }
        }
        for (gpu, fan) in missing {
            eprintln!("GPU {gpu} temperature unavailable; restoring fan {fan} to automatic control");
            self.failed.insert((gpu, fan));
            self.release_control(&format!("/nvidiagpu/{gpu}/control/{fan}"));
        }
    }
    fn read_value(&self, sensor_id: &str) -> Option<f64> { self.values.get(sensor_id).copied() }
    fn set_control(&mut self, control_id: &str, percent: f64) {
        let Some((index, fan)) = Self::parse_control(control_id) else { return; };
        if !self.has_control(control_id) { return; }
        let Some(request) = fan_request(percent) else {
            self.failed.insert((index, fan));
            self.release_control(control_id);
            return;
        };
        if request == FanRequest::Automatic {
            // Manual 0% is accepted by some drivers but this card continues at
            // its 30% hardware minimum. The driver owns zero-RPM idle policy.
            // Reset at least once even when this process has not written yet:
            // a previous daemon can have left the GPU in manual mode.
            self.automatic_requested.insert((index, fan));
            self.restore_automatic(index, fan);
            return;
        }
        self.automatic_requested.remove(&(index, fan));
        if self.failed.contains(&(index, fan)) { return; }
        let Some(gpu) = self.gpus.get(index) else { return; };
        let FanRequest::Manual(speed) = request else { unreachable!() };
        let result = unsafe { (self.api.set_fan_speed.unwrap())(gpu.handle, fan, speed) };
        if result == 0 { self.active.insert((index, fan)); }
        else {
            eprintln!("GPU fan control {control_id} failed with NVML code {result}; restoring automatic control");
            self.failed.insert((index, fan));
            // The driver may have changed mode before returning an error.
            // Try the documented default-policy handback even if this was the
            // first write and the fan is absent from `active`.
            self.active.insert((index, fan));
            self.release_control(control_id);
        }
    }
    fn control_write_succeeded(&self, control_id: &str) -> bool {
        Self::parse_control(control_id).is_some_and(|key| self.active.contains(&key) && !self.failed.contains(&key) && !self.automatic_requested.contains(&key))
    }
    fn release_control(&mut self, control_id: &str) {
        let Some((index, fan)) = Self::parse_control(control_id) else { return; };
        if self.active.contains(&(index, fan)) { self.restore_automatic(index, fan); }
    }
    fn read_control_rpm(&self, control_id: &str) -> Option<f64> {
        let (gpu, fan) = Self::parse_control(control_id)?;
        self.values.get(&format!("/nvidiagpu/{gpu}/fan/{fan}")).copied()
    }
}

impl Drop for NvmlBackend {
    fn drop(&mut self) {
        let mut pending = self.active.clone();
        pending.extend(self.automatic_requested.iter().filter(|key| !self.auto_reset_attempted.contains(key)).copied());
        for (gpu, fan) in pending { self.restore_automatic(gpu, fan); }
        unsafe { (self.api.shutdown)(); }
    }
}

#[cfg(test)]
mod tests {
    use super::{fan_request, FanRequest, NvmlBackend};
    #[test]
    fn control_ids_are_strict() {
        assert_eq!(NvmlBackend::parse_control("/nvidiagpu/2/control/1"), Some((2, 1)));
        assert_eq!(NvmlBackend::parse_control("/nvidiagpu/2/temperature/1"), None);
        assert_eq!(NvmlBackend::parse_control("/nvidiagpu/2/control/no"), None);
    }
    #[test]
    fn zero_uses_driver_automatic_and_positive_requests_stay_manual() {
        assert_eq!(fan_request(0.0), Some(FanRequest::Automatic));
        assert_eq!(fan_request(27.0), Some(FanRequest::Manual(27)));
        assert_eq!(fan_request(30.0), Some(FanRequest::Manual(30)));
        assert_eq!(fan_request(-8.0), Some(FanRequest::Automatic));
        assert_eq!(fan_request(150.0), Some(FanRequest::Manual(100)));
        assert_eq!(fan_request(f64::NAN), None);
    }
}
