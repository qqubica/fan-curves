namespace FanCurves.Core;

/// <summary>
/// Deterministic-ish fake PC with real thermodynamics: the CPU heatsink integrates
/// package watts through a fan-dependent cooling resistance (so written PWM actually
/// changes the temperature), the die rides a power-proportional delta above the sink,
/// and fans' RPM follows the written PWM with lag. Spikes (browser tab, compile burst)
/// and sustained loads (game/AI) come and go. Lets the whole app — curve editing,
/// engine, visualization — run honestly on a machine with no controllable fans.
/// </summary>
public class SimulatedBackend : IHardwareBackend
{
    public string Description => "Simulation (no hardware control — demo sensors)";
    public bool IsSimulated => true;

    private readonly Random _rng = new(7);
    private double _t;
    private double _sink = 48, _cpu = 50, _gpu = 38, _mb = 33;
    private double _pCpu = 38, _pGpu = 14;
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

    // No library behind this backend keeps value history — the setting is a no-op,
    // but the sensor count keeps the dev panel's RAM estimate honest in --sim.
    public int InternalSensorCount => Sensors.Count;
    public void SetSensorHistoryWindow(TimeSpan window) { }

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

        // Package power responds instantly; the temperatures follow the physics below.
        _pCpu = 38 + (spike ? 62 : 0) + (load ? 150 : 0) + (_rng.NextDouble() - 0.5) * 6;
        _pGpu += ((load ? 230 : 14) - _pGpu) * 0.25 + (_rng.NextDouble() - 0.5) * 4;

        double fCpu = (_pwm.TryGetValue("sim/fan/cpu", out var fc) ? fc : 35) / 100.0;
        double fCase = (_pwm.TryGetValue("sim/fan/case", out var fx) ? fx : 30) / 100.0;

        // Plant: sink mass ≈ 420 J/°C behind a fan-dependent resistance; case airflow
        // lowers the intake baseline a little. The die reads ~0.055 °C/W above the sink
        // (AM5 thermal density: Tctl jumps the moment load lands, whatever the fan does).
        double baseT = 27 + 5 * (1 - fCase) + Math.Max(0, _gpu - 40) * 0.05;
        double r = Math.Max(0.16, (0.55 - 0.34 * Math.Sqrt(fCpu)) * (1 - 0.10 * fCase));
        _sink += (_pCpu - (_sink - baseT) / r) / 420.0;
        _cpu = _sink + Math.Max(0, _pCpu - 38) * 0.055 + (_rng.NextDouble() - 0.5) * 0.8;

        double gpuTarget = load ? 76 : 38;
        _gpu += (gpuTarget - _gpu) * 0.06 + (_rng.NextDouble() - 0.5) * 0.6;
        _mb += ((_cpu + _gpu) / 2 * 0.55 + 12 - _mb) * 0.01;

        foreach (var c in Controls)
        {
            double target = _pwm.TryGetValue(c.Id, out var p) ? p * 25 : 900; // %→rpm
            double cur = _rpm.TryGetValue(c.Id, out var rp) ? rp : target;
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
