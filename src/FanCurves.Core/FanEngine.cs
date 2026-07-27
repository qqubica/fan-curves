using System.Diagnostics;

namespace FanCurves.Core;

/// <summary>Why the commanded % differs from the curve's configured level right now.</summary>
public enum OutputReason
{
    None,         // output matches the curve — nothing to explain
    RampUp,       // slew limit still gliding up toward the target level
    RampDown,     // slew limit still gliding down toward the target level
    StepDownHold, // lower band reached, waiting out StepDownHoldSeconds
    StepUpHold,   // power control: sustained demand needs a higher level, hold running
    Hysteresis,   // avg maps lower but is not yet HysteresisC clear of the band edge
    ZeroSnap,     // curve level is under ZeroSnapPercent → running at 0 instead
    MinFloor,     // channel MinPercent overrides a lower curve level
    IdleKick,     // stopped fan briefly spun up by the idle kick
    StopProbe,    // trial stop — fan held at 0 to see if it is needed at all
    BudgetHold,   // power control: the thermal buffer lets the fan run below the temp curve
    BudgetRamp,   // power control: the buffer/power demands more than the temp curve asks
    HardOverride, // fuse: die at/over the override temp — temp curve direct, no slew
    PowerCurve,   // power-curve mode: the watts staircase demands more than the temp curve
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
    double MassJPerC = 0,      // learned thermal mass
    // ---- the rest is thermal-budget telemetry for the developer panel ----
    double TauSeconds = double.PositiveInfinity, // predicted headroom before the guarded line (aim, then ceiling)
    double DemandLevel = 0,    // ladder level the sustained power average asks for
    double TrendTempC = double.NaN, // sink-trend temp the budget is measured from
    double CeilingC = 0,       // budget ceiling in force (override − margin)
    double AimC = 0,           // sustained aim in force — the line headroom is measured to
    double SlopeCPerSec = 0,   // measured warming/cooling slope of the trend
    double BaseTempC = 0,      // learned cool-idle baseline
    double ResistanceCPerW = 0); // learned cooling resistance at the commanded %

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
    private readonly Dictionary<ChannelConfig, ResponseFilter> _powerFilters = new();
    private readonly Dictionary<ChannelConfig, PowerBudgetController> _budgets = new();
    private readonly Dictionary<ChannelConfig, IdleKick> _kicks = new();
    private readonly Dictionary<ChannelConfig, StopProbe> _probes = new();
    // PowerCurve mode runs without the budget controller, so it needs its own fuse
    // latch per channel (same contract: raw temp ≥ override → temp staircase on the
    // RAW temp, instantly, and the output never decays while latched).
    private sealed class FuseState { public bool Active; public double OkSince = double.NaN; public double Held; }
    private readonly Dictionary<ChannelConfig, FuseState> _fuses = new();
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
            _powerFilters.Clear();
            _budgets.Clear();
            _kicks.Clear();
            _probes.Clear();
            _fuses.Clear();
        }
    }

    /// <summary>
    /// Throw away everything the thermal models have learned (live controllers and the
    /// values persisted in the profile) and start again from the seeds. Control state —
    /// current level, slew position, fuse — is untouched, so the fans do not jump.
    /// </summary>
    public void ResetThermalModels()
    {
        lock (_lock)
        {
            foreach (var b in _budgets.Values) b.Model.Reset();
            foreach (var ch in Profile.Channels)
            {
                ch.LearnedThermalMassJPerC = 0;
                ch.LearnedBaseTempC = 0;
                ch.LearnedResistances.Clear();
            }
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
                if (Profile.ControlMode != ControlMode.Temperature && ch.PowerSensorIds.Count > 0)
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
                double tauSeconds = double.PositiveInfinity, demandLevel = 0;
                double trendTempC = double.NaN, ceilingC = 0, aimC = 0, slopeCPerSec = 0;
                double baseTempC = 0, resistanceCPerW = 0;
                if (temp.HasValue)
                {
                    var curve = new FanCurve(ch.Points);
                    double filtered;

                    // The temperature filter runs whenever it has a say: alone when the
                    // channel has no live power side, in Auto mode, where its target
                    // rides into the budget controller as a floor — whichever side asks
                    // for more fan wins, and the budget's physics stay honest because its
                    // slewed output IS the physical output — and in PowerCurve mode,
                    // where it is the safety floor under the watts staircase.
                    ResponseFilter? filter = null;
                    double tempFiltered = 0;
                    if (!watts.HasValue ||
                        Profile.ControlMode is ControlMode.Auto or ControlMode.PowerCurve)
                    {
                        if (!_filters.TryGetValue(ch, out filter))
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
                        tempFiltered = filter.Step(now, temp.Value, curve);
                    }

                    if (watts.HasValue && Profile.ControlMode == ControlMode.PowerCurve)
                    {
                        // Power-curve mode: the same MacBook filter, run in the WATTS
                        // dimension — sustained draw average → watts staircase → slewed
                        // output. Deterministic: what the curve says is what runs. The
                        // temperature side above stays as a safety floor, and the fuse
                        // still snaps to the temp staircase on the raw die temp; the
                        // predictive budget layer is off entirely.
                        if (!_powerFilters.TryGetValue(ch, out var pfilter))
                        {
                            pfilter = new ResponseFilter();
                            _powerFilters[ch] = pfilter;
                        }
                        pfilter.AveragingSeconds = Profile.PowerAveragingSeconds;
                        pfilter.HysteresisC = Profile.PowerCurveHysteresisW; // watts here
                        pfilter.StepDownHoldSeconds = ch.StepDownHoldSeconds;
                        pfilter.SlewUpPercentPerSec = ch.SlewUpPercentPerSec;
                        pfilter.SlewDownPercentPerSec = ch.SlewDownPercentPerSec;
                        pfilter.ZeroSnapPercent = Profile.ZeroSnapEnabled ? Profile.ZeroSnapPercent : 0;

                        bool hasPowerCurve = ch.PowerPoints.Count > 0;
                        double pFiltered = hasPowerCurve
                            ? pfilter.Step(now, watts.Value, FanCurve.FromPower(ch.PowerPoints))
                            : 0;
                        double tTarget = filter!.TargetLevel;
                        double pTarget = hasPowerCurve ? pfilter.TargetLevel : 0;
                        effectiveTemp = filter.EffectiveTemp;
                        wattsAvg = hasPowerCurve ? pfilter.EffectiveTemp : watts;
                        // Gates the stop probe exactly like the budget's DemandLevel: a
                        // die-limited load reads "stable" to the probe, but its draw
                        // keeps the watts staircase asking for fan.
                        demandLevel = pTarget;

                        if (!_fuses.TryGetValue(ch, out var fuse))
                        {
                            fuse = new FuseState();
                            _fuses[ch] = fuse;
                        }
                        if (temp.Value >= Profile.OverrideTempC)
                        {
                            fuse.Active = true;
                            fuse.OkSince = double.NaN;
                        }
                        else if (fuse.Active)
                        {
                            if (temp.Value <= Profile.OverrideTempC - Profile.OverrideReleaseC)
                            {
                                if (double.IsNaN(fuse.OkSince)) fuse.OkSince = now;
                                if (now - fuse.OkSince >= Profile.OverrideReleaseSeconds)
                                    fuse.Active = false;
                            }
                            else fuse.OkSince = double.NaN;
                        }
                        overrideActive = fuse.Active;

                        filtered = Math.Max(tempFiltered, pFiltered);
                        if (fuse.Active)
                        {
                            fuse.Held = Math.Max(fuse.Held,
                                Math.Max(curve.Evaluate(temp.Value), filtered));
                            filtered = fuse.Held;
                        }
                        else fuse.Held = 0;

                        output = Math.Max(ch.MinPercent, filtered);
                        targetPct = Math.Max(ch.MinPercent, Math.Max(tTarget, pTarget));

                        // Most specific explanation wins; later checks override earlier ones.
                        if (overrideActive)
                        {
                            reason = OutputReason.HardOverride;
                            reasonLevel = curve.Evaluate(temp.Value);
                        }
                        else
                        {
                            bool powerLeads = pTarget > tTarget + 0.5;
                            if (powerLeads)
                            {
                                reason = OutputReason.PowerCurve;
                                reasonLevel = tTarget; // what the temp curve alone asks
                            }
                            // A pending step-down only matters on the side whose level is
                            // in force, and only while it would actually lower the max.
                            var leadF = powerLeads ? pfilter : filter;
                            double otherTarget = powerLeads ? tTarget : pTarget;
                            if (!double.IsNaN(leadF.DownHoldRemaining) &&
                                leadF.PendingDownLevel >= otherTarget - 0.5)
                            {
                                reason = OutputReason.StepDownHold;
                                reasonLevel = Math.Max(ch.MinPercent,
                                    Math.Max(otherTarget, leadF.PendingDownLevel));
                                reasonSeconds = leadF.DownHoldRemaining;
                            }
                            if ((powerLeads ? pfilter.SnappedToZero : filter.SnappedToZero)
                                && output <= 0.01)
                            {
                                reason = OutputReason.ZeroSnap;
                                reasonLevel = powerLeads ? pfilter.CurveLevel : filter.CurveLevel;
                            }
                            if (Math.Abs(output - targetPct) > 0.5)
                                reason = output < targetPct ? OutputReason.RampUp : OutputReason.RampDown;
                            if (ch.MinPercent > 0 && filtered < ch.MinPercent - 0.01)
                            {
                                reason = OutputReason.MinFloor;
                                reasonLevel = Math.Max(filter.CurveLevel,
                                    hasPowerCurve ? pfilter.CurveLevel : 0);
                            }
                        }
                    }
                    else if (watts.HasValue)
                    {
                        if (!_budgets.TryGetValue(ch, out var budget))
                        {
                            budget = new PowerBudgetController();
                            budget.Model.LoadFrom(ch);
                            _budgets[ch] = budget;
                        }
                        budget.DisplayAveragingSeconds = ch.AveragingSeconds;
                        budget.StepDownHoldSeconds = ch.StepDownHoldSeconds;
                        budget.HysteresisC = ch.HysteresisC;
                        budget.SlewUpPercentPerSec = ch.SlewUpPercentPerSec;
                        budget.SlewDownPercentPerSec = ch.SlewDownPercentPerSec;
                        budget.ZeroSnapPercent = Profile.ZeroSnapEnabled ? Profile.ZeroSnapPercent : 0;
                        budget.PowerAveragingSeconds = Profile.PowerAveragingSeconds;
                        budget.RampLeadSeconds = Profile.RampLeadSeconds;
                        budget.ReliefMaxWatts = Profile.ReliefMaxWatts;
                        budget.PowerFloorPercentAt100W = Profile.PowerFloorPercentAt100W;
                        budget.PowerFloorPercentAt200W = Profile.PowerFloorPercentAt200W;
                        budget.OverrideTempC = Profile.OverrideTempC;
                        budget.CeilingMarginC = Profile.BudgetCeilingMarginC;
                        budget.SteadyTargetMarginC = Profile.SteadyTargetMarginC;
                        budget.TrendAvgSeconds = Profile.PowerTrendSeconds;
                        budget.SlopeWindowSeconds = Profile.PowerSlopeSeconds;
                        budget.ShortPowerSeconds = Profile.PowerNowSeconds;
                        budget.OverrideReleaseC = Profile.OverrideReleaseC;
                        budget.OverrideReleaseSeconds = Profile.OverrideReleaseSeconds;
                        budget.LearningEnabled = Profile.ThermalLearningEnabled;
                        // Auto mode: the temperature side's demand is a hard floor, and
                        // the staircase's next step is a line the buffer drains toward.
                        budget.FloorPercent = filter?.TargetLevel ?? 0;
                        budget.GuardFloor = filter != null;

                        filtered = budget.Step(now, temp.Value, watts.Value, curve);
                        budget.Model.StoreTo(ch); // learned values ride along in the profile
                        effectiveTemp = budget.EffectiveTemp;
                        wattsAvg = budget.PowerAvg;
                        budgetJoules = budget.BudgetJoules;
                        massJPerC = budget.Model.MassJPerC;
                        tauSeconds = budget.TauSeconds;
                        demandLevel = budget.DemandLevel;
                        trendTempC = budget.TrendTempC;
                        ceilingC = budget.CeilingC;
                        aimC = budget.SteadyTargetC;
                        slopeCPerSec = budget.SlopeCPerSec;
                        baseTempC = budget.Model.BaseTempC;
                        resistanceCPerW = budget.Model.R(filtered);
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
                            else if (!double.IsNaN(budget.UpHoldRemaining))
                            {
                                reason = OutputReason.StepUpHold;
                                reasonLevel = Math.Max(ch.MinPercent, budget.PendingUpLevel);
                                reasonSeconds = budget.UpHoldRemaining;
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
                        filtered = tempFiltered;
                        effectiveTemp = filter!.EffectiveTemp;
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
                            // Nor is a channel whose sustained power draw demands any fan: the
                            // probe's stability test is blind to a die-limited load — a CPU
                            // clamping its own temperature reads perfectly "stable" at 180 W,
                            // and a trial stop then holds the fan off indefinitely while the
                            // die self-throttles (chess-engine report, 2026-07-27: fan held
                            // at 0% at 178 W because the clamped die could never "rise").
                            if (Profile.StopProbeEnabled && ch.MinPercent <= 0 &&
                                demandLevel <= 0.01)
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
                                probe.MaxTempC = Profile.StopProbeMaxTempC;
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
                    watts, wattsAvg, budgetJoules, massJPerC,
                    tauSeconds, demandLevel, trendTempC, ceilingC, aimC, slopeCPerSec,
                    baseTempC, resistanceCPerW));
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
