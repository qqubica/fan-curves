An NVIDIA fan range reported by the driver does not prove that manual fan writes work.

Read-only enumeration can return fan count and minimum/maximum speeds even when the driver rejects manual control. Confirm support with a bounded write test and automatic-policy restoration, then verify real RPM changes. Report control as applied only after the write succeeds; otherwise the UI can show changing commands while the hardware remains under firmware control.

The reported minimum also must not silently rewrite the user's request.
The RTX 5080 reports 30% as a minimum. Removing the application's clamp still
left about 1,400 RPM at a successfully accepted manual 0% request. Manual
write acknowledgement therefore does not prove zero-RPM operation. A 0% GPU
target hands control back to the driver's automatic idle policy and is labeled
automatic in the UI; positive targets are sent unchanged within 0–100%.
Reference: https://docs.nvidia.com/deploy/nvml-api/api/group__nvmlDeviceCommands.html

NVML enumeration reflects driver-visible fan endpoints, which can be fewer
than the physical fan blades on a multi-fan card. The RTX 5080 reports two
fan endpoints, so software using NVML cannot assign a separate curve to its
third physical fan. Enumerate current GPU core and hotspot temperatures by
probing the documented sensor types, and expose memory temperature only when
the driver's current `NVML_FI_DEV_MEMORY_TEMP` field succeeds; temperature
thresholds are limits, not current readings.

On this RTX 5080, read-only probes of NVML `GetTemperature`,
`GetTemperatureV`, `GetThermalSettings`, the memory-temperature field, and
NVAPI `GetThermalSettings` returned only the GPU core reading. The driver does
not expose a separate hotspot or memory reading through those APIs here, even
though the implementation can show them on hardware that does.
