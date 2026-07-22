using System.Diagnostics;

namespace FanCurves.Core;

/// <summary>Why the commanded % differs from the curve's configured level right now.</summary>
public enum OutputReason
{
    None,         // output matches the curve — nothing to explain
    RampUp,       // slew limit still gliding up toward the target level
    RampDown,     // slew limit still gliding down toward the target level
    StepDownHold, // lower band reached, waiting out StepDownHoldSeconds
    Hysteresis,   // avg maps lower but is not yet HysteresisC clear of the band edge
    ZeroSnap,     // curve level is under ZeroSnapPercent → running at 0 instead
    MinFloor,     // channel MinPercent overrides a lower curve level
    IdleKick,     // stopped fan briefly spun up by the idle kick
    StopProbe,    // trial stop — fan held at 0 to see if it is needed at all
}

public record ChannelStatus(
    string Name,
    double? RawTemp,
    double EffectiveTemp,
    double OutputPercent,
    double? Rpm,
    bool Applied,
    double TargetPercent = 0,
    OutputReason Reason = OutputReason.None,
    double ReasonLevel = 0,    // the "other" % involved (pending step / curve level)
    double ReasonSeconds = 0); // StepDownHold: seconds until the step-down lands

/// <summary>
/// Ticks roughly once a second (jittered ±15% so sampling never phase-locks onto
/// periodic system activity): reads temperatures, runs each channel's curve through its
/// ResponseFilter, writes PWM. Stop() (and Dispose) hand every control back to the BIOS.
/// All time-based logic downstream uses the monotonic clock, not tick counts, so the
/// uneven spacing is harmless.
/// </summary>
public class FanEngine : IDisposable
{
    private readonly IHardwareBackend _hw;
    private readonly Stopwatch _clock = Stopwatch.StartNew();
    private readonly Dictionary<ChannelConfig, ResponseFilter> _filters = new();
    private readonly Dictionary<ChannelConfig, IdleKick> _kicks = new();
    private readonly Dictionary<ChannelConfig, StopProbe> _probes = new();
    private readonly object _lock = new();
    private Timer? _timer;

    public Profile Profile { get; private set; }
    public bool Applying { get; private set; }
    public event Action<IReadOnlyList<ChannelStatus>>? Ticked;

    public FanEngine(IHardwareBackend hw, Profile profile)
    {
        _hw = hw;
        Profile = profile;
    }

    public void Start()
    {
        // One-shot; each Tick re-arms itself with a fresh jittered delay.
        _timer ??= new Timer(_ => Tick(), null, 0, Timeout.Infinite);
    }

    /// <summary>Begin writing PWM (until then the engine only monitors/previews).</summary>
    public void Apply() { lock (_lock) Applying = true; }

    public void StopApplying()
    {
        lock (_lock)
        {
            Applying = false;
            foreach (var ch in Profile.Channels)
                foreach (var id in ch.ControlIds)
                    _hw.ReleaseControl(id);
        }
    }

    public void ReplaceProfile(Profile p)
    {
        lock (_lock)
        {
            StopApplyingControlsNotIn(p);
            Profile = p;
            _filters.Clear();
            _kicks.Clear();
            _probes.Clear();
        }
    }

    private void StopApplyingControlsNotIn(Profile p)
    {
        var keep = p.Channels.SelectMany(c => c.ControlIds).ToHashSet();
        foreach (var ch in Profile.Channels)
            foreach (var id in ch.ControlIds.Where(id => !keep.Contains(id)))
                _hw.ReleaseControl(id);
    }

    private void Tick()
    {
        lock (_lock)
        {
            _hw.Update();
            double now = _clock.Elapsed.TotalSeconds;
            var statuses = new List<ChannelStatus>();

            foreach (var ch in Profile.Channels)
            {
                if (!_filters.TryGetValue(ch, out var filter))
                {
                    filter = new ResponseFilter();
                    _filters[ch] = filter;
                }
                filter.AveragingSeconds = ch.AveragingSeconds;
                filter.HysteresisC = ch.HysteresisC;
                filter.StepDownHoldSeconds = ch.StepDownHoldSeconds;
                filter.SlewUpPercentPerSec = ch.SlewUpPercentPerSec;
                filter.SlewDownPercentPerSec = ch.SlewDownPercentPerSec;
                filter.ZeroSnapPercent = Profile.ZeroSnapEnabled ? Profile.ZeroSnapPercent : 0;

                double? temp = ch.SensorIds
                    .Select(_hw.ReadValue)
                    .Where(v => v.HasValue)
                    .Select(v => v!.Value)
                    .DefaultIfEmpty(double.NaN)
                    .Max();
                if (double.IsNaN(temp.Value)) temp = null;

                double output = 0;
                bool applied = false;
                var reason = OutputReason.None;
                double targetPct = 0, reasonLevel = 0, reasonSeconds = 0;
                if (temp.HasValue)
                {
                    var curve = new FanCurve(ch.Points);
                    double filtered = filter.Step(now, temp.Value, curve);
                    output = Math.Max(ch.MinPercent, filtered);
                    targetPct = Math.Max(ch.MinPercent, filter.TargetLevel);

                    // Most specific explanation wins; later checks override earlier ones.
                    if (!double.IsNaN(filter.DownHoldRemaining))
                    {
                        reason = OutputReason.StepDownHold;
                        reasonLevel = Math.Max(ch.MinPercent, filter.PendingDownLevel);
                        reasonSeconds = filter.DownHoldRemaining;
                    }
                    else if (filter.HysteresisHolding)
                    {
                        reason = OutputReason.Hysteresis;
                    }
                    else if (filter.SnappedToZero && output <= 0.01)
                    {
                        reason = OutputReason.ZeroSnap;
                        reasonLevel = filter.CurveLevel;
                    }
                    if (Math.Abs(output - targetPct) > 0.5)
                        reason = output < targetPct ? OutputReason.RampUp : OutputReason.RampDown;
                    if (ch.MinPercent > 0 && filtered < ch.MinPercent - 0.01)
                    {
                        reason = OutputReason.MinFloor;
                        reasonLevel = filter.CurveLevel;
                    }
                    if (Applying && ch.Enabled && ch.ControlIds.Count > 0)
                    {
                        // Trial stop runs BEFORE the kick, so a probe-stopped channel counts
                        // as stopped for the idle kick (same choice as zero snap). Channels
                        // with a safety floor are never trial-stopped — the floor wins.
                        if (Profile.StopProbeEnabled && ch.MinPercent <= 0)
                        {
                            if (!_probes.TryGetValue(ch, out var probe))
                            {
                                probe = new StopProbe();
                                _probes[ch] = probe;
                            }
                            probe.RunSeconds = Profile.StopProbeRunSeconds;
                            probe.ProbeSeconds = Profile.StopProbeSeconds;
                            probe.StableRangeC = Profile.StopProbeStableRangeC;
                            probe.FailRetrySeconds = Profile.StopProbeRetrySeconds;
                            output = probe.Step(now, temp.Value, output);
                            if (probe.Holding) reason = OutputReason.StopProbe;
                        }
                        else
                        {
                            _probes.Remove(ch); // re-enabling starts with a fresh window
                        }
                        if (Profile.IdleKickEnabled)
                        {
                            if (!_kicks.TryGetValue(ch, out var kick))
                            {
                                kick = new IdleKick();
                                _kicks[ch] = kick;
                            }
                            kick.StoppedSeconds = Profile.IdleKickStoppedSeconds;
                            kick.KickPercent = Profile.IdleKickPercent;
                            kick.KickSeconds = Profile.IdleKickSeconds;
                            output = kick.Step(now, output);
                            if (kick.Kicking) reason = OutputReason.IdleKick;
                        }
                        else
                        {
                            _kicks.Remove(ch); // re-enabling starts the stopped clock fresh
                        }
                        foreach (var id in ch.ControlIds) _hw.SetControl(id, output);
                        applied = true;
                    }
                }
                // While the BIOS (or nothing) drives the fan, the stopped/running clocks must not run.
                if (!applied) { _kicks.Remove(ch); _probes.Remove(ch); }

                double? rpm = ch.ControlIds.Count > 0 ? _hw.ReadControlRpm(ch.ControlIds[0]) : null;
                statuses.Add(new ChannelStatus(ch.Name, temp, filter.EffectiveTemp, output, rpm, applied,
                    targetPct, reason, reasonLevel, reasonSeconds));
            }

            Ticked?.Invoke(statuses);

            _timer?.Change(Random.Shared.Next(850, 1151), Timeout.Infinite);
        }
    }

    public void Dispose()
    {
        lock (_lock) { _timer?.Dispose(); _timer = null; }
        StopApplying();
    }
}
