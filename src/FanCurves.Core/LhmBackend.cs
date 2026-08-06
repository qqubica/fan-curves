using LibreHardwareMonitor.Hardware;

namespace FanCurves.Core;

/// <summary>
/// Real hardware via LibreHardwareMonitor. Needs administrator rights (kernel driver)
/// to see the Super I/O chip; on the ASRock X870 Steel Legend that's a Nuvoton NCT67xx
/// exposing CPU_FAN/CHA_FAN PWM controls. Without admin or without a supported chip,
/// Controls comes back empty and the app falls back to simulation.
/// </summary>
public class LhmBackend : IHardwareBackend
{
    private readonly Computer _computer;
    private readonly List<HwSensor> _sensors = new();
    private readonly List<HwControl> _controls = new();
    private readonly Dictionary<string, ISensor> _byId = new();
    private readonly Dictionary<string, ISensor> _controlById = new();
    private readonly Dictionary<string, ISensor?> _rpmForControl = new();

    public string Description { get; }
    public bool IsSimulated => false;
    public IReadOnlyList<HwSensor> Sensors => _sensors;
    public IReadOnlyList<HwControl> Controls => _controls;

    public LhmBackend()
    {
        _computer = new Computer
        {
            IsCpuEnabled = true,
            IsGpuEnabled = true,
            IsMotherboardEnabled = true,
            IsControllerEnabled = true,
        };
        _computer.Open();
        _computer.Accept(new UpdateVisitor());
        Enumerate();
        Description = $"Hardware ({_controls.Count} controllable fan header(s))";
    }

    private void Enumerate()
    {
        void Walk(IHardware hw)
        {
            // Late-appearing sensors must inherit the configured history window too.
            hw.SensorAdded += s => s.ValuesTimeWindow = _historyWindow;
            // GPU fans belong to the GPU driver/firmware, never to this app — only
            // motherboard (Super I/O) headers are offered as controls. GPU temps are
            // still exposed as sensors (case-fan curves key off them).
            bool isGpu = hw.HardwareType is HardwareType.GpuNvidia or HardwareType.GpuAmd or HardwareType.GpuIntel;
            foreach (var sensor in hw.Sensors)
            {
                string id = sensor.Identifier.ToString();
                switch (sensor.SensorType)
                {
                    case SensorType.Temperature:
                        _sensors.Add(new HwSensor(id, $"{hw.Name} — {sensor.Name}", "temp"));
                        _byId[id] = sensor;
                        break;
                    case SensorType.Fan:
                        _sensors.Add(new HwSensor(id, $"{hw.Name} — {sensor.Name}", "rpm"));
                        _byId[id] = sensor;
                        break;
                    case SensorType.Control when sensor.Control != null && !isGpu:
                        _controls.Add(new HwControl(id, $"{hw.Name} — {sensor.Name}"));
                        _controlById[id] = sensor;
                        // Pair "Fan Control #N" with "Fan #N" on the same hardware.
                        _rpmForControl[id] = hw.Sensors.FirstOrDefault(s =>
                            s.SensorType == SensorType.Fan &&
                            s.Index == sensor.Index);
                        break;
                }
            }
            foreach (var sub in hw.SubHardware) Walk(sub);
        }
        foreach (var hw in _computer.Hardware) Walk(hw);
    }

    public void Update() => _computer.Accept(new UpdateVisitor());

    public double? ReadValue(string sensorId) =>
        _byId.TryGetValue(sensorId, out var s) ? s.Value : null;

    public void SetControl(string controlId, double percent)
    {
        if (_controlById.TryGetValue(controlId, out var s) && s.Control != null)
            s.Control.SetSoftware((float)Math.Clamp(percent, 0, 100));
    }

    public void ReleaseControl(string controlId)
    {
        if (_controlById.TryGetValue(controlId, out var s) && s.Control != null)
            s.Control.SetDefault();
    }

    public double? ReadControlRpm(string controlId) =>
        _rpmForControl.TryGetValue(controlId, out var s) ? s?.Value : null;

    // LHM's own default: every sensor keeps a rolling 1-day List<SensorValue>,
    // appended ~every 4th Update(). This app never reads it — at a ~1 s tick and
    // 150+ internal sensors it is tens of MB of pure ballast, so the engine
    // overrides it with Profile.SensorHistoryHours on the first tick.
    private TimeSpan _historyWindow = TimeSpan.FromDays(1);

    public int InternalSensorCount
    {
        get { int n = 0; ForEachSensor(_ => n++); return n; }
    }

    public void SetSensorHistoryWindow(TimeSpan window)
    {
        _historyWindow = window;
        // Zero clears the list and stops future appends (Sensor.Value setter skips
        // history entirely at TimeSpan.Zero); live Value reads are unaffected.
        ForEachSensor(s => s.ValuesTimeWindow = window);
    }

    private void ForEachSensor(Action<ISensor> act)
    {
        void Walk(IHardware hw)
        {
            foreach (var s in hw.Sensors) act(s);
            foreach (var sub in hw.SubHardware) Walk(sub);
        }
        foreach (var hw in _computer.Hardware) Walk(hw);
    }

    public void Dispose()
    {
        foreach (var id in _controlById.Keys) ReleaseControl(id);
        _computer.Close();
    }

    private class UpdateVisitor : IVisitor
    {
        public void VisitComputer(IComputer computer) => computer.Traverse(this);
        public void VisitHardware(IHardware hardware)
        {
            hardware.Update();
            foreach (var sub in hardware.SubHardware) sub.Accept(this);
        }
        public void VisitSensor(ISensor sensor) { }
        public void VisitParameter(IParameter parameter) { }
    }
}
