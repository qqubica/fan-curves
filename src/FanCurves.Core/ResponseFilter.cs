namespace FanCurves.Core;

/// <summary>
/// The "MacBook feel":
///
/// 1. The curve is a STAIRCASE — wide temperature bands, each with a fixed level
///    (see FanCurve.Evaluate). The target may jump several levels at once when the
///    average lands in a higher band.
/// 2. The temperature fed to the curve is the rolling AVERAGE over the last
///    AveragingSeconds (default 20 s) — short spikes barely move it.
/// 3. Step-down hysteresis: a level is only left downward once even
///    (average + HysteresisC) maps to a lower level AND that has held continuously
///    for StepDownHoldSeconds — no flapping at a band edge.
/// 4. Slew limit: the output glides toward the target level at a bounded %/s.
/// 5. Zero snap: target levels below ZeroSnapPercent collapse to 0 — the fan either
///    runs at a meaningful speed or stays fully stopped, never crawls.
/// </summary>
public class ResponseFilter
{
    public double AveragingSeconds { get; set; } = 20;
    public double HysteresisC { get; set; } = 3;
    public double StepDownHoldSeconds { get; set; } = 10;
    public double SlewUpPercentPerSec { get; set; } = 8;
    public double SlewDownPercentPerSec { get; set; } = 8;
    /// <summary>Levels above 0 but strictly below this snap to 0 (0 = disabled).</summary>
    public double ZeroSnapPercent { get; set; } = 0;

    private readonly List<(double time, double temp)> _samples = new();
    private double _output = double.NaN;
    private double _targetLevel = double.NaN;
    private double _downSince = double.NaN;
    private bool _applyNow;

    /// <summary>One-shot: the next Step re-evaluates the curve at the current average
    /// and jumps straight to that level — no step-down hold, no hysteresis, no slew
    /// glide. The engine calls this when a tuning setting or curve point changes, so
    /// an edit is FELT the tick it lands instead of tens of seconds later. The
    /// averaging window is untouched: it is measurement, not settings.</summary>
    public void ApplyNow() => _applyNow = true;

    /// <summary>Averaged temperature actually driving the curve (for UI display).</summary>
    public double EffectiveTemp { get; private set; } = double.NaN;

    // Diagnostics for the UI's "why isn't the fan at the curve's %" readout —
    // refreshed on every Step, valid until the next one.
    /// <summary>Unsnapped staircase level at the current average.</summary>
    public double CurveLevel { get; private set; }
    /// <summary>The level the slew is gliding toward.</summary>
    public double TargetLevel => double.IsNaN(_targetLevel) ? 0 : _targetLevel;
    /// <summary>The curve's level was above 0 but collapsed to 0 by the zero snap.</summary>
    public bool SnappedToZero { get; private set; }
    /// <summary>Average maps to a lower level, but not yet HysteresisC clear of the band edge.</summary>
    public bool HysteresisHolding { get; private set; }
    /// <summary>Level a pending step-down will drop to (NaN when none is pending).</summary>
    public double PendingDownLevel { get; private set; } = double.NaN;
    /// <summary>Seconds of StepDownHoldSeconds still to wait (NaN when none is pending).</summary>
    public double DownHoldRemaining { get; private set; } = double.NaN;

    /// <param name="now">Monotonic time in seconds.</param>
    public double Step(double now, double rawTemp, FanCurve curve)
    {
        _samples.Add((now, rawTemp));
        _samples.RemoveAll(s => s.time < now - AveragingSeconds);

        double avg = _samples.Average(s => s.temp);
        EffectiveTemp = avg;

        CurveLevel = curve.Evaluate(avg);
        SnappedToZero = CurveLevel > 0 && Snap(CurveLevel) <= 0;
        HysteresisHolding = false;
        PendingDownLevel = double.NaN;
        DownHoldRemaining = double.NaN;

        // Snap BOTH evaluations so the hysteresis/hold logic reasons about the level
        // the fan will actually run at; the slew still glides to/from real steps.
        double levelAtAvg = Snap(CurveLevel);
        if (_applyNow)
        {
            // Settings just changed: adopt what the new curve/settings say at the
            // current average, immediately — target, holds and slew position alike.
            _applyNow = false;
            _targetLevel = levelAtAvg;
            _downSince = double.NaN;
            _output = _targetLevel;
        }
        else if (double.IsNaN(_targetLevel))
        {
            _targetLevel = levelAtAvg;
        }
        else if (levelAtAvg > _targetLevel)
        {
            _targetLevel = levelAtAvg; // up (possibly several steps) as soon as the average says so
            _downSince = double.NaN;
        }
        else
        {
            // Down only when the average has fallen clearly below the band edge
            // (temperature margin) AND stayed there for StepDownHoldSeconds (time margin).
            double levelWithMargin = Snap(curve.Evaluate(avg + HysteresisC));
            if (levelWithMargin < _targetLevel)
            {
                if (double.IsNaN(_downSince)) _downSince = now;
                if (now - _downSince >= StepDownHoldSeconds)
                {
                    _targetLevel = levelWithMargin;
                    _downSince = double.NaN;
                }
                else
                {
                    PendingDownLevel = levelWithMargin;
                    DownHoldRemaining = StepDownHoldSeconds - (now - _downSince);
                }
            }
            else
            {
                _downSince = double.NaN;
                HysteresisHolding = levelAtAvg < _targetLevel;
            }
        }

        if (double.IsNaN(_output)) _output = _targetLevel;
        double dt = _samples.Count >= 2 ? now - _samples[^2].time : 1.0;
        dt = Math.Clamp(dt, 0.05, 10);
        if (_targetLevel > _output)
            _output = Math.Min(_targetLevel, _output + SlewUpPercentPerSec * dt);
        else
            _output = Math.Max(_targetLevel, _output - SlewDownPercentPerSec * dt);

        return Math.Clamp(_output, 0, 100);
    }

    private double Snap(double level) =>
        level > 0 && level < ZeroSnapPercent ? 0 : level;

    public void Reset()
    {
        _samples.Clear();
        _output = double.NaN;
        _targetLevel = double.NaN;
        _downSince = double.NaN;
        _applyNow = false;
        EffectiveTemp = double.NaN;
        CurveLevel = 0;
        SnappedToZero = false;
        HysteresisHolding = false;
        PendingDownLevel = double.NaN;
        DownHoldRemaining = double.NaN;
    }
}
