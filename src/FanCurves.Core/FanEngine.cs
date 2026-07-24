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
    BudgetHold,   // power control: the thermal buffer lets the fan run below the temp curve
    BudgetRamp,   // power control: the buffer/power demands more than the temp curve asks
    HardOverride, // fuse: die at/over the override temp — temp curve direct, no slew
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
    double ReasonSeconds = 0,  // StepDownHold: time to the step; Budget*: predicted headroom
    double? Watts = null,      // instantaneous channel power draw (power control only)
    double? WattsAvg = null,   // sustained power average — the demand signal
    double BudgetJoules = 0,   // energy credit left before the budget ceiling
    double MassJPerC = 0);     // learned thermal mass

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
    private readonly Dictionary<ChannelConfig, PowerBudgetController> _budgets = new();
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
            _budgets.Clear();
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
                double? temp = ch.SensorIds
                    .Select(_hw.ReadValue)
                    .Where(v => v.HasValue)
                    .Select(v => v!.Value)
                    .DefaultIfEmpty(double.NaN)
                    .Max();
                if (double.IsNaN(temp.Value)) temp = null;

                // Sum of the channel's power sensors — non-null switches the channel
                // from the temperature filter to the thermal-budget controller.
                double? watts = null;
                if (Profile.PowerControlEnabled && ch.PowerSensorIds.Count > 0)
                {
                    double sum = 0;
                    bool any = false;
                    foreach (var id in ch.PowerSensorIds)
                        if (_hw.ReadValue(id) is double w and >= 0) { sum += w; any = true; }
                    if (any) watts = sum;
                }

                double output = 0;
                bool applied = false;
                bool overrideActive = false;
                var reason = OutputReason.None;
                double targetPct = 0, reasonLevel = 0, reasonSeconds = 0;
                double effectiveTemp = double.NaN;
                double? wattsAvg = null;
                double budgetJoules = 0, massJPerC = 0;
                if (temp.HasValue)
                {
                    var curve = new FanCurve(ch.Points);
                    double filtered;
                    if (watts.HasValue)
                    {
                        if (!_budgets.TryGetValue(ch, out var budget))
                        {
                            budget = new PowerBudgetController();
                            budget.Model.LoadFrom(ch);
                            _budgets[ch] = budget;
                        }
                        budget.DisplayAveragingSeconds = ch.AveragingSeconds;
                        budget.StepDownHoldSeconds = ch.StepDownHoldSeconds;
                        budget.SlewUpPercentPerSec = ch.SlewUpPercentPerSec;
                        budget.SlewDownPercentPerSec = ch.SlewDownPercentPerSec;
                        budget.ZeroSnapPercent = Profile.ZeroSnapEnabled ? Profile.ZeroSnapPercent : 0;
                        budget.PowerAveragingSeconds = Profile.PowerAveragingSeconds;
                        budget.RampLeadSeconds = Profile.RampLeadSeconds;
                        budget.OverrideTempC = Profile.OverrideTempC;

                        filtered = budget.Step(now, temp.Value, watts.Value, curve);
                        budget.Model.StoreTo(ch); // learned values ride along in the profile
                        effectiveTemp = budget.EffectiveTemp;
                        wattsAvg = budget.PowerAvg;
                        budgetJoules = budget.BudgetJoules;
                        massJPerC = budget.Model.MassJPerC;
                        overrideActive = budget.OverrideActive;
                        output = Math.Max(ch.MinPercent, filtered);
                        targetPct = Math.Max(ch.MinPercent, budget.TargetLevel);

                        // Most specific explanation wins; later checks override earlier ones.
                        if (overrideActive)
                        {
                            reason = OutputReason.HardOverride;
                            reasonLevel = curve.Evaluate(temp.Value);
                        }
                        else
                        {
                            if (!double.IsNaN(budget.DownHoldRemaining))
                            {
                                reason = OutputReason.StepDownHold;
                                reasonLevel = Math.Max(ch.MinPercent, budget.PendingDownLevel);
                                reasonSeconds = budget.DownHoldRemaining;
                            }
                            else if (budget.SnappedToZero && output <= 0.01)
                            {
                                reason = OutputReason.ZeroSnap;
                                reasonLevel = budget.PreSnapTarget;
                            }
                            else if (budget.TargetLevel < budget.CurveLevelAtAvg - 0.5)
                            {
                                reason = OutputReason.BudgetHold;
                                reasonLevel = budget.CurveLevelAtAvg;
                                reasonSeconds = budget.TauSeconds;
                            }
                            else if (budget.TargetLevel > budget.CurveLevelAtAvg + 0.5)
                            {
                                reason = OutputReason.BudgetRamp;
                                reasonLevel = budget.CurveLevelAtAvg;
                                reasonSeconds = budget.TauSeconds;
                            }
                            if (Math.Abs(output - targetPct) > 0.5 &&
                                reason is OutputReason.None or OutputReason.BudgetHold)
                                reason = output < targetPct ? OutputReason.RampUp : OutputReason.RampDown;
                            if (ch.MinPercent > 0 && filtered < ch.MinPercent - 0.01)
                            {
                                reason = OutputReason.MinFloor;
                                reasonLevel = budget.PreSnapTarget;
                            }
                        }
                    }
                    else
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

                        filtered = filter.Step(now, temp.Value, curve);
                        effectiveTemp = filter.EffectiveTemp;
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
                    }
                    if (Applying && ch.Enabled && ch.ControlIds.Count > 0)
                    {
                        if (overrideActive)
                        {
                            // The fuse outranks the courtesy features: no trial stops or
                            // kicks while the die is over the override temperature.
                            _probes.Remove(ch);
                            _kicks.Remove(ch);
                        }
                        else
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
                        }
                        foreach (var id in ch.ControlIds) _hw.SetControl(id, output);
                        applied = true;
                    }
                }
                // While the BIOS (or nothing) drives the fan, the stopped/running clocks must not run.
                if (!applied) { _kicks.Remove(ch); _probes.Remove(ch); }

                double? rpm = ch.ControlIds.Count > 0 ? _hw.ReadControlRpm(ch.ControlIds[0]) : null;
                statuses.Add(new ChannelStatus(ch.Name, temp, effectiveTemp, output, rpm, applied,
                    targetPct, reason, reasonLevel, reasonSeconds,
                    watts, wattsAvg, budgetJoules, massJPerC));
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
