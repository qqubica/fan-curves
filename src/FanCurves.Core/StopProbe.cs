namespace FanCurves.Core;

/// <summary>
/// Opportunistic fan stop ("is this fan actually needed?"): once a channel has run
/// continuously for RunSeconds with its temperature inside a StableRangeC band, the
/// fan is stopped as a trial. While stopped, a short-average of the temperature is
/// compared against the pre-stop baseline every tick — if it climbs more than
/// StableRangeC (or the curve demands more than it did at the stop), curve control
/// resumes immediately. A stop that does not survive its first
/// ProbeSeconds + RunSeconds delays the next trial by FailRetrySeconds, so the fan
/// never settles into an on/off cycle under load that genuinely needs it.
/// </summary>
public class StopProbe
{
    /// <summary>Continuous running (and temperature-stability) time required before a trial stop.</summary>
    public double RunSeconds { get; set; } = 30;
    /// <summary>Trial length — a rise inside this window (plus one RunSeconds) counts as a failed trial.</summary>
    public double ProbeSeconds { get; set; } = 30;
    /// <summary>"Stable" = temp within this band; a rise beyond it while stopped resumes the fan.</summary>
    public double StableRangeC { get; set; } = 3.5;
    /// <summary>Wait after a failed trial before trying again.</summary>
    public double FailRetrySeconds { get; set; } = 60;

    /// <summary>Rise detection averages the last few seconds so single-sample jitter can't trigger it.</summary>
    private const double RiseAvgSeconds = 5;

    private readonly List<(double time, double temp)> _window = new();
    private double _runningSince = double.NaN;
    private double _stoppedAt = double.NaN;   // NaN = not holding the fan off
    private double _baseline;                  // stable-window average at the moment of the stop
    private double _demandAtStop;
    private double _retryAfter = double.NegativeInfinity;

    /// <summary>True while the fan is being held at 0% (for UI/status).</summary>
    public bool Holding => !double.IsNaN(_stoppedAt);

    /// <param name="now">Monotonic time in seconds (same clock every call).</param>
    /// <param name="rawTemp">Raw (unaveraged) channel temperature.</param>
    /// <param name="demand">The output the curve/filter wants to write.</param>
    public double Step(double now, double rawTemp, double demand)
    {
        _window.Add((now, rawTemp));
        _window.RemoveAll(s => s.time < now - RunSeconds);

        if (Holding)
        {
            if (demand <= 0.01)
            {
                ToNormal(now); // the curve no longer wants the fan anyway
                return demand;
            }
            double recent = _window.Where(s => s.time >= now - RiseAvgSeconds).Average(s => s.temp);
            if (recent > _baseline + StableRangeC || demand > _demandAtStop + 0.01)
            {
                // Heat is building without the fan — hand back to the curve, and if the
                // stop didn't even survive to the first recheck horizon, back off.
                bool failed = now - _stoppedAt < ProbeSeconds + RunSeconds;
                ToNormal(now);
                if (failed) _retryAfter = now + FailRetrySeconds;
                return demand;
            }
            return 0;
        }

        if (demand <= 0.01)
        {
            _runningSince = double.NaN; // not running — nothing to trial-stop
            return demand;
        }
        if (double.IsNaN(_runningSince)) _runningSince = now;
        if (now - _runningSince < RunSeconds || now < _retryAfter) return demand;
        // The sample window must actually span the running period, and stay in-band.
        if (_window[^1].time - _window[0].time < RunSeconds - 2) return demand;
        if (_window.Max(s => s.temp) - _window.Min(s => s.temp) > StableRangeC) return demand;

        _stoppedAt = now;
        _baseline = _window.Average(s => s.temp);
        _demandAtStop = demand;
        return 0;
    }

    private void ToNormal(double now)
    {
        _stoppedAt = double.NaN;
        _runningSince = now; // the running clock restarts as the fan spins back up
    }

    public void Reset()
    {
        _window.Clear();
        _runningSince = double.NaN;
        _stoppedAt = double.NaN;
        _retryAfter = double.NegativeInfinity;
    }
}
