namespace FanCurves.Core;

/// <summary>
/// Deterministic-ish fake PC: an AM5-style CPU that idles warm and spikes hard on
/// light tasks, a GPU with load phases, a motherboard sensor that drifts slowly,
/// and fans whose RPM follows the written PWM with lag. Lets the whole app —
/// curve editing, engine, visualization — run on a machine with no controllable fans.
/// </summary>
public class SimulatedBackend : IHardwareBackend
{
    public string Description => "Simulation (no hardware control — demo sensors)";
    public bool IsSimulated => true;

    private readonly Random _rng = new(7);
    private double _t;
    private double _cpu = 46, _gpu = 38, _mb = 33;
    private double _spikeUntil = -1, _loadUntil = -1;
    private readonly Dictionary<string, double> _pwm = new();   // controlId -> commanded %
    private readonly Dictionary<string, double> _rpm = new();   // controlId -> current rpm

    public IReadOnlyList<HwSensor> Sensors { get; } = new List<HwSensor>
    {
        new("sim/cpu", "CPU (Tctl/Tdie)", "temp"),
        new("sim/gpu", "GPU core", "temp"),
        new("sim/mb",  "Motherboard", "temp"),
    };

    public IReadOnlyList<HwControl> Controls { get; } = new List<HwControl>
    {
        new("sim/fan/cpu",  "CPU_FAN1 (NH-D15)"),
        new("sim/fan/case", "CHA_FAN1 (P14 Pro ×4)"),
    };

    public void Update()
    {
        _t += 1.0;

        // Random 2–6 s spikes (browser tab, compile burst) roughly every ~25 s.
        if (_t > _spikeUntil && _rng.NextDouble() < 0.04)
            _spikeUntil = _t + 2 + _rng.NextDouble() * 4;
        // Occasional 60–120 s sustained load (game/AI) roughly every ~5 min.
        if (_t > _loadUntil && _rng.NextDouble() < 0.004)
            _loadUntil = _t + 60 + _rng.NextDouble() * 60;

        bool spike = _t <= _spikeUntil;
        bool load = _t <= _loadUntil;

        double cpuTarget = load ? 84 : spike ? 72 : 46;
        double gpuTarget = load ? 76 : 38;
        // CPU moves fast (AM5 thermal density), GPU slower, M/B very slow.
        _cpu += (cpuTarget - _cpu) * (spike || load ? 0.55 : 0.25) + (_rng.NextDouble() - 0.5) * 1.2;
        _gpu += (gpuTarget - _gpu) * 0.06 + (_rng.NextDouble() - 0.5) * 0.6;
        _mb += ((_cpu + _gpu) / 2 * 0.55 + 12 - _mb) * 0.01;

        foreach (var c in Controls)
        {
            double target = _pwm.TryGetValue(c.Id, out var p) ? p * 25 : 900; // %→rpm
            double cur = _rpm.TryGetValue(c.Id, out var r) ? r : target;
            _rpm[c.Id] = cur + (target - cur) * 0.3;
        }
    }

    public double? ReadValue(string sensorId) => sensorId switch
    {
        "sim/cpu" => _cpu,
        "sim/gpu" => _gpu,
        "sim/mb" => _mb,
        _ => null,
    };

    public void SetControl(string controlId, double percent) => _pwm[controlId] = percent;
    public void ReleaseControl(string controlId) => _pwm.Remove(controlId);
    public double? ReadControlRpm(string controlId) =>
        _rpm.TryGetValue(controlId, out var r) ? r : null;

    public void Dispose() { }
}
