namespace FanCurves.Core;

/// <summary>
/// Periodic spin-up for fully stopped fans: once the driven output has sat at 0%
/// for StoppedSeconds, the written PWM is forced to KickPercent for KickSeconds,
/// then the fan stops again and the cycle repeats. Any curve output above 0%
/// cancels the kick and restarts the stopped clock.
/// </summary>
public class IdleKick
{
    public double StoppedSeconds { get; set; } = 60;
    public double KickPercent { get; set; } = 20;
    public double KickSeconds { get; set; } = 20;

    private double _stoppedSince = double.NaN;
    private double _kickUntil = double.NaN;

    /// <summary>True while the output is being overridden (for UI/status).</summary>
    public bool Kicking => !double.IsNaN(_kickUntil);

    /// <param name="now">Monotonic time in seconds (same clock every call).</param>
    /// <param name="output">The output the curve/filter wants to write.</param>
    public double Step(double now, double output)
    {
        if (output > 0.01)
        {
            _stoppedSince = double.NaN;
            _kickUntil = double.NaN;
            return output;
        }

        if (Kicking)
        {
            if (now < _kickUntil) return KickPercent;
            _kickUntil = double.NaN;
            _stoppedSince = now; // stopped again — next kick in StoppedSeconds
            return output;
        }

        if (double.IsNaN(_stoppedSince))
        {
            _stoppedSince = now;
            return output;
        }

        if (now - _stoppedSince >= StoppedSeconds)
        {
            _kickUntil = now + KickSeconds;
            return KickPercent;
        }
        return output;
    }

    public void Reset()
    {
        _stoppedSince = double.NaN;
        _kickUntil = double.NaN;
    }
}
