namespace FanCurves.Core;

/// <summary>
/// Learned physics of one cooling channel — the knowledge that lets the engine spend
/// heatsink metal as a buffer instead of chasing temperature:
///
///   MassJPerC   thermal mass C in J/°C: how many joules the die+cooler absorb per
///               degree of warm-up. C · (ceiling − temp) is the energy credit left.
///   BaseTempC   temperature the channel settles to with (near) zero heat input.
///   R(fan%)     cooling resistance in °C/W at a fan speed: steady temp ≈ base + R·watts.
///               Stored as anchors at 0/20/40/60/80/100 %, linear in between.
///
/// All three refine online while the app runs (see PowerBudgetController) and persist
/// per channel in profile.json. Seeds are NH-D15-class, which errs on the safe side:
/// a smaller cooler ramps a little early on day one, then learns its way down.
/// </summary>
public class ThermalModel
{
    public static readonly double[] AnchorPercents = { 0, 20, 40, 60, 80, 100 };
    private static readonly double[] SeedR = { 0.55, 0.42, 0.34, 0.28, 0.24, 0.21 };
    public const double SeedMassJPerC = 450;
    public const double SeedBaseTempC = 30;

    public double MassJPerC { get; private set; } = SeedMassJPerC;
    public double BaseTempC { get; private set; } = SeedBaseTempC;
    private readonly double[] _r = (double[])SeedR.Clone();

    /// <summary>
    /// Learned-value plausibility bounds. Base is the ZERO-watt baseline (ambient +
    /// case air) — anything near real steady die temps (48°+) means the model has
    /// collapsed onto the (base, R) identifiability ridge: with samples from a single
    /// operating point base drifts up to eat the temperature level while R shrinks
    /// toward zero, until "dissipation" dwarfs the actual draw and every energy-side
    /// prediction goes blind (found on Kuba's machine 2026-07-26: base 52°, R flat
    /// ~0.08–0.10 °C/W at every fan speed). The R floor slides with fan speed: no sane
    /// air cooler beats 0.2 °C/W passively, while a big loop may reach 0.05 at full tilt.
    /// </summary>
    public const double MaxBaseTempC = 45;
    public static double MinR(double percent) => 0.20 - 0.0015 * Math.Clamp(percent, 0, 100);

    /// <summary>Adopt previously learned values when they pass sanity bounds (0 = unlearned).</summary>
    public void LoadFrom(ChannelConfig ch)
    {
        if (ch.LearnedThermalMassJPerC is > 20 and < 5000) MassJPerC = ch.LearnedThermalMassJPerC;
        if (ch.LearnedBaseTempC > 5 && ch.LearnedBaseTempC < MaxBaseTempC + 1) BaseTempC = ch.LearnedBaseTempC;
        if (ch.LearnedResistances.Count == _r.Length &&
            ch.LearnedResistances.Select((r, i) => r >= MinR(AnchorPercents[i]) && r < 4).All(ok => ok))
            for (int i = 0; i < _r.Length; i++) _r[i] = ch.LearnedResistances[i];
    }

    /// <summary>Throw the learning away and start again from the NH-D15-class seeds.</summary>
    public void Reset()
    {
        MassJPerC = SeedMassJPerC;
        BaseTempC = SeedBaseTempC;
        for (int i = 0; i < _r.Length; i++) _r[i] = SeedR[i];
    }

    /// <summary>The resistance anchors (°C/W at 0/20/…/100 %), for display.</summary>
    public IReadOnlyList<double> Resistances => _r;

    public void StoreTo(ChannelConfig ch)
    {
        ch.LearnedThermalMassJPerC = MassJPerC;
        ch.LearnedBaseTempC = BaseTempC;
        if (ch.LearnedResistances.Count != _r.Length) ch.LearnedResistances = _r.ToList();
        else for (int i = 0; i < _r.Length; i++) ch.LearnedResistances[i] = _r[i];
    }

    /// <summary>Cooling resistance at a fan speed (piecewise-linear over the anchors).</summary>
    public double R(double percent)
    {
        double p = Math.Clamp(percent, 0, 100);
        int i = Math.Min(AnchorPercents.Length - 2, (int)(p / 20));
        double f = (p - AnchorPercents[i]) / 20;
        return _r[i] + (_r[i + 1] - _r[i]) * f;
    }

    /// <summary>Watts leaving through the cooler at this temperature and fan speed.</summary>
    public double DissipationWatts(double tempC, double percent) =>
        Math.Max(0, (tempC - BaseTempC) / R(percent));

    /// <summary>One C observation: this many surplus watts heated the mass at this slope.
    /// Caller guarantees the two share a sign; magnitudes are used.</summary>
    public void LearnMass(double surplusWatts, double slopeCPerSec)
    {
        double sample = Math.Clamp(Math.Abs(surplusWatts) / Math.Abs(slopeCPerSec), 20, 5000);
        MassJPerC = MassJPerC * 0.94 + sample * 0.06;
    }

    /// <summary>One steady-state observation (temp flat, power flat, fan settled).</summary>
    public void LearnSteady(double tempC, double watts, double percent)
    {
        // The base drifts toward what this operating point implies, then the observed
        // °C-over-base per watt is split across the two anchors bracketing the speed.
        // Both are clamped to the physical-plausibility bounds (see MaxBaseTempC):
        // single-operating-point data cannot separate base from R, so without the
        // bounds the pair drifts until the model is confidently absurd.
        BaseTempC = Math.Clamp(BaseTempC * 0.99 + (tempC - R(percent) * watts) * 0.01, 10, MaxBaseTempC);
        if (watts < 1) return;
        double p = Math.Clamp(percent, 0, 100);
        double sample = Math.Clamp((tempC - BaseTempC) / watts, MinR(p), 4);
        int i = Math.Min(AnchorPercents.Length - 2, (int)(p / 20));
        double f = (p - AnchorPercents[i]) / 20;
        _r[i] += (sample - _r[i]) * 0.08 * (1 - f);
        _r[i + 1] += (sample - _r[i + 1]) * 0.08 * f;
        // More fan can never cool worse: keep the anchor list non-increasing —
        // but never below each anchor's own floor, so one visited operating point
        // cannot flatten the unvisited rest of the ladder into nonsense.
        for (int k = 1; k < _r.Length; k++)
            _r[k] = Math.Max(MinR(AnchorPercents[k]), Math.Min(_r[k], _r[k - 1]));
    }
}

/// <summary>
/// Power-driven channel control ("control with power, not temperature"):
///
/// 1. The minute-scale average of package power is the true measure of how much heat
///    the cooler must ultimately move; the temperature staircase only supplies the
///    ladder of allowed speeds (and the fallback curve for the fuse).
/// 2. The heatsink's thermal mass is spent as credit: E = C · (ceiling − temp). A short
///    burst pours its joules into metal — the fan does not move.
/// 3. The fan steps UP only when the predicted time to exhaust that credit — the more
///    pessimistic of energy/surplus (model) and headroom/slope (measured) — drops under
///    RampLeadSeconds, and it goes directly to the ladder level that restores the lead.
///    ONE step per slope window: the measured slope is backward-looking, so right after
///    a step it still reports the pre-step warming and would otherwise climb the whole
///    ladder to 100% in as many ticks (Kuba's report, 2026-07-26). Within the settle
///    window a further step happens only if the chosen level has become objectively
///    insufficient — the draw rose enough that its predicted equilibrium no longer
///    clears the ceiling. The fuse is not rate-limited by any of this.
/// 4. It steps DOWN one ladder level per StepDownHoldSeconds once the sustained power
///    average no longer needs the current one — after a load ends this reacts to the
///    power collapse, minutes before a cooling temperature average would.
/// 5. The sustained aim is also enforced UPWARD: once the sink trend is past the steady
///    target and either the sustained average needs a higher ladder level or the temp
///    has settled there, it steps up one level per StepDownHoldSeconds. The tau trigger
///    alone goes blind at any equilibrium (surplus and slope both vanish, so both
///    predictions read ∞ — the better the model has learned, the more exactly), which
///    used to park the die anywhere below the ceiling with the fans never asked for more.
///    Each step up brands the level it left as measured-insufficient at that draw, so an
///    unlearned model cannot argue the fan straight back down into an on/off limit cycle;
///    the brand lifts once the sustained draw falls clearly below the draw that failed.
/// 6. Fuse: raw die temp at/over OverrideTempC snaps the output to the channel's own
///    temperature curve instantly, without slew, until the temp clears the threshold.
///    Silence is never bought at the price of throttling.
/// </summary>
public class PowerBudgetController
{
    // Pushed from the profile/channel before every Step.
    public double DisplayAveragingSeconds { get; set; } = 90;
    public double PowerAveragingSeconds { get; set; } = 60;
    public double RampLeadSeconds { get; set; } = 45;
    public double OverrideTempC { get; set; } = 90;
    public double StepDownHoldSeconds { get; set; } = 25;
    public double SlewUpPercentPerSec { get; set; } = 9;
    public double SlewDownPercentPerSec { get; set; } = 8;
    /// <summary>Levels above 0 but strictly below this snap to 0 (0 = disabled).</summary>
    public double ZeroSnapPercent { get; set; } = 0;
    /// <summary>The sink-trend temperature: a short rolling average that filters the die's
    /// instant jump on load onset but still tracks the heatsink warming.</summary>
    public double TrendAvgSeconds { get; set; } = 30;
    /// <summary>Window of the least-squares trend slope (°C/s).</summary>
    public double SlopeWindowSeconds { get; set; } = 25;
    /// <summary>Window of the "draw now" power average.</summary>
    public double ShortPowerSeconds { get; set; } = 10;
    /// <summary>The budget ceiling sits this far under the fuse.</summary>
    public double CeilingMarginC { get; set; } = 4;
    /// <summary>Sustained power aims this far under the fuse (never above the ceiling).</summary>
    public double SteadyTargetMarginC { get; set; } = 10;
    /// <summary>The fuse releases once the raw temp is this far below OverrideTempC…</summary>
    public double OverrideReleaseC { get; set; } = 3;
    /// <summary>…and has held there this long.</summary>
    public double OverrideReleaseSeconds { get; set; } = 10;
    /// <summary>When false the model stops refining itself (frozen at today's values).</summary>
    public bool LearningEnabled { get; set; } = true;

    public ThermalModel Model { get; } = new();

    private readonly List<(double time, double v)> _temps = new();
    private readonly List<(double time, double v)> _powers = new();
    private readonly List<(double time, double v)> _trend = new();
    private double _target = double.NaN;
    private double _output = double.NaN;
    private double _downSince = double.NaN;
    private double _upSince = double.NaN;
    private double _overrideOkSince = double.NaN;
    private double _lastTime = double.NaN;
    private double _lastRampStep = double.NaN;
    // A demand step-up is measured proof the level it left cannot hold the aim at that
    // sustained draw. Remembered so the model (possibly unlearned or frozen) cannot
    // immediately argue the fan back down — that would be a slow on/off limit cycle.
    // Forgiven once the sustained draw falls clearly below the draw that failed it.
    private double _failedLevel = double.NaN;
    private double _failedPowerAvg;

    // Diagnostics for the engine/UI — refreshed on every Step, valid until the next one.
    /// <summary>Fuse engaged: the temperature curve is being written directly, no slew.</summary>
    public bool OverrideActive { get; private set; }
    /// <summary>Rolling average temp over DisplayAveragingSeconds (hero numeral parity).</summary>
    public double EffectiveTemp { get; private set; } = double.NaN;
    /// <summary>What the plain temperature staircase would ask at the display average.</summary>
    public double CurveLevelAtAvg { get; private set; }
    /// <summary>The level the slew is gliding toward (zero-snap applied).</summary>
    public double TargetLevel => double.IsNaN(_target) ? 0 : Snap(_target);
    /// <summary>The chosen ladder level before the zero snap.</summary>
    public double PreSnapTarget => double.IsNaN(_target) ? 0 : _target;
    /// <summary>The chosen level was above 0 but collapsed to 0 by the zero snap.</summary>
    public bool SnappedToZero { get; private set; }
    /// <summary>Package power, short average (last ~10 s).</summary>
    public double PowerNow { get; private set; }
    /// <summary>Package power averaged over PowerAveragingSeconds — the demand signal.</summary>
    public double PowerAvg { get; private set; }
    /// <summary>Energy credit left before the budget ceiling: C · (ceiling − trend temp).</summary>
    public double BudgetJoules { get; private set; }
    /// <summary>Predicted seconds until the credit runs out (∞ when temps hold or fall).</summary>
    public double TauSeconds { get; private set; } = double.PositiveInfinity;
    /// <summary>Ladder level the sustained power average asks for (the step-down goal).</summary>
    public double DemandLevel { get; private set; }
    /// <summary>Level a pending step-down will drop to (NaN when none is pending).</summary>
    public double PendingDownLevel { get; private set; } = double.NaN;
    /// <summary>Seconds of StepDownHoldSeconds still to wait (NaN when none is pending).</summary>
    public double DownHoldRemaining { get; private set; } = double.NaN;
    /// <summary>Level a pending demand step-up will rise to (NaN when none is pending).</summary>
    public double PendingUpLevel { get; private set; } = double.NaN;
    /// <summary>Seconds until the pending step-up lands (NaN when none is pending).</summary>
    public double UpHoldRemaining { get; private set; } = double.NaN;
    /// <summary>Sink-trend temperature — the temperature the budget is measured from.</summary>
    public double TrendTempC { get; private set; } = double.NaN;
    /// <summary>Measured trend slope in °C/s (positive = warming).</summary>
    public double SlopeCPerSec { get; private set; }
    /// <summary>The budget ceiling in force: OverrideTempC − CeilingMarginC.</summary>
    public double CeilingC { get; private set; }

    /// <param name="now">Monotonic time in seconds (same clock every call).</param>
    /// <param name="rawTemp">Raw channel temperature (max of assigned sensors).</param>
    /// <param name="watts">Sum of the channel's power sensors.</param>
    /// <param name="curve">The channel's staircase — ladder of levels + fuse fallback.</param>
    public double Step(double now, double rawTemp, double watts, FanCurve curve)
    {
        _temps.Add((now, rawTemp));
        _powers.Add((now, watts));
        _temps.RemoveAll(s => s.time < now - Math.Max(DisplayAveragingSeconds, TrendAvgSeconds));
        _powers.RemoveAll(s => s.time < now - Math.Max(PowerAveragingSeconds, ShortPowerSeconds));

        EffectiveTemp = Mean(_temps, now - DisplayAveragingSeconds);
        double trendTemp = Mean(_temps, now - TrendAvgSeconds);
        _trend.Add((now, trendTemp));
        _trend.RemoveAll(s => s.time < now - SlopeWindowSeconds);

        PowerNow = Mean(_powers, now - ShortPowerSeconds);
        PowerAvg = _powers.Average(s => s.v);
        CurveLevelAtAvg = curve.Evaluate(EffectiveTemp);

        double dt = double.IsNaN(_lastTime) ? 1 : Math.Clamp(now - _lastTime, 0.05, 10);
        _lastTime = now;

        if (double.IsNaN(_target)) _target = curve.Evaluate(rawTemp);
        if (double.IsNaN(_output)) _output = Snap(_target);

        // ---- Fuse (checked on the RAW temp — the die must never wait for an average).
        if (rawTemp >= OverrideTempC) { OverrideActive = true; _overrideOkSince = double.NaN; }
        else if (OverrideActive)
        {
            if (rawTemp <= OverrideTempC - OverrideReleaseC)
            {
                if (double.IsNaN(_overrideOkSince)) _overrideOkSince = now;
                if (now - _overrideOkSince >= OverrideReleaseSeconds) OverrideActive = false;
            }
            else _overrideOkSince = double.NaN;
        }

        double ceiling = OverrideTempC - CeilingMarginC;
        BudgetJoules = Model.MassJPerC * Math.Max(0, ceiling - trendTemp);
        TrendTempC = trendTemp;
        CeilingC = ceiling;

        double slope = TrendSlope();
        SlopeCPerSec = slope;
        double surplus = PowerNow - Model.DissipationWatts(trendTemp, _output);
        double tauEnergy = surplus > 1 ? BudgetJoules / surplus : double.PositiveInfinity;
        double tauSlope = slope > 0.005 ? (ceiling - trendTemp) / slope : double.PositiveInfinity;
        TauSeconds = Math.Min(tauEnergy, tauSlope);
        // A spent budget with the trend not clearly falling is ZERO headroom, not
        // infinite: parked at the ceiling, surplus and slope both vanish and the two
        // predictions above go blind exactly when the credit is gone.
        if (BudgetJoules <= 0 && slope > -0.002) TauSeconds = 0;

        var ladder = Ladder(curve);
        // The sustained aim can never sit above the transient ceiling, whatever the
        // two margins are set to — otherwise "settled" would mean "already over budget".
        double steadyTarget = OverrideTempC - Math.Max(SteadyTargetMarginC, CeilingMarginC);
        DemandLevel = ladder[^1];
        foreach (var l in ladder)
            if (Model.BaseTempC + Model.R(l) * PowerAvg <= steadyTarget) { DemandLevel = l; break; }
        // Measured insufficiency outranks the model's estimate: while a level stands
        // proven too weak for today's draw, demand cannot fall to it or below.
        if (!double.IsNaN(_failedLevel) &&
            PowerAvg < _failedPowerAvg - Math.Max(5, _failedPowerAvg * 0.1))
            _failedLevel = double.NaN;
        if (!double.IsNaN(_failedLevel))
            foreach (var l in ladder)
                if (l > _failedLevel + 0.01) { DemandLevel = Math.Max(DemandLevel, l); break; }

        PendingDownLevel = double.NaN;
        DownHoldRemaining = double.NaN;
        PendingUpLevel = double.NaN;
        UpHoldRemaining = double.NaN;

        if (OverrideActive)
        {
            // The channel's own staircase, evaluated on the raw temp, wins outright —
            // output jumps up instantly and never decays while the fuse is latched.
            double must = curve.Evaluate(rawTemp);
            _target = Math.Max(_target, must);
            _output = Math.Max(_output, must);
            _downSince = double.NaN;
            _upSince = double.NaN;
        }
        else if (TauSeconds < RampLeadSeconds && _target < ladder[^1] - 0.01 &&
                 (double.IsNaN(_lastRampStep) || now - _lastRampStep >= SlopeWindowSeconds ||
                  Model.BaseTempC + Model.R(_target) * PowerNow > ceiling - 1))
        {
            // Credit is running out: jump the target to the lowest ladder level whose
            // equilibrium temperature at today's draw sits back under the ceiling —
            // at that level the surplus decays to zero before the credit does. Then
            // hold: the slope that keeps TauSeconds low is measured over the past
            // window and cannot yet know about this step, so another one is allowed
            // only after the window turns over — or immediately if the draw has risen
            // enough that the level just chosen no longer clears the ceiling.
            double up = ladder[^1];
            foreach (var l in ladder)
            {
                if (l <= _target + 0.01) continue;
                if (Model.BaseTempC + Model.R(l) * PowerNow <= ceiling - 1) { up = l; break; }
            }
            _target = up;
            _lastRampStep = now;
            _downSince = double.NaN;
            _upSince = double.NaN;
        }
        else if (trendTemp > steadyTarget && _target < ladder[^1] - 0.01 &&
                 (DemandLevel > _target + 0.01 || slope > -0.002))
        {
            // The sink is already past the sustained aim and either the power average
            // needs a level this one cannot hold, or the temp has settled there (not
            // cooling — a frozen or unlearned model can claim the level is sufficient
            // while the measured trend proves otherwise): one ladder step up per hold.
            // The tau trigger never fires at a settled equilibrium (surplus and slope
            // both vanish), so without this the die would park anywhere below the
            // ceiling with the fans never asked for more. Gating on the trend keeps
            // burst immunity: short spikes never drag the trend past the aim.
            double above = ladder[^1];
            foreach (var l in ladder) if (l > _target + 0.01) { above = l; break; }
            if (double.IsNaN(_upSince)) _upSince = now;
            _downSince = double.NaN;
            if (now - _upSince >= StepDownHoldSeconds)
            {
                _failedLevel = _target;      // measured: this level cannot hold the aim
                _failedPowerAvg = PowerAvg;  // at this sustained draw
                _target = above;
                _upSince = double.NaN;
            }
            else
            {
                PendingUpLevel = Snap(above);
                UpHoldRemaining = StepDownHoldSeconds - (now - _upSince);
            }
        }
        else if (TauSeconds >= RampLeadSeconds && DemandLevel < _target - 0.01 &&
                 trendTemp < ceiling - 1)
        {
            // Sustained power no longer needs this level: one ladder step down per hold.
            double below = ladder[0];
            foreach (var l in ladder) if (l < _target - 0.01) below = l;
            if (double.IsNaN(_downSince)) _downSince = now;
            _upSince = double.NaN;
            if (now - _downSince >= StepDownHoldSeconds)
            {
                _target = below;
                _downSince = double.NaN;
            }
            else
            {
                PendingDownLevel = Snap(below);
                DownHoldRemaining = StepDownHoldSeconds - (now - _downSince);
            }
        }
        else { _downSince = double.NaN; _upSince = double.NaN; }

        double snapped = Snap(_target);
        SnappedToZero = !OverrideActive && _target > 0 && snapped <= 0;
        if (!OverrideActive)
        {
            if (snapped > _output)
                _output = Math.Min(snapped, _output + SlewUpPercentPerSec * dt);
            else
                _output = Math.Max(snapped, _output - SlewDownPercentPerSec * dt);
        }

        // ---- Online learning — only with the fan settled, so the plant response
        //      isn't a mix of fan changes and power changes.
        if (LearningEnabled && !OverrideActive && Math.Abs(_output - snapped) < 0.5 && PowerStable(now))
        {
            if (Math.Abs(surplus) > 15 && Math.Abs(slope) > 0.015 &&
                Math.Sign(surplus) == Math.Sign(slope))
                Model.LearnMass(surplus, slope);
            else if (Math.Abs(slope) < 0.008 && TempSpan(now - TrendAvgSeconds) < 1.2)
                Model.LearnSteady(trendTemp, PowerNow, _output);
        }

        return Math.Clamp(_output, 0, 100);
    }

    private double Snap(double level) =>
        level > 0 && level < ZeroSnapPercent ? 0 : level;

    private static double Mean(List<(double time, double v)> samples, double since)
    {
        double sum = 0; int n = 0;
        foreach (var (t, v) in samples)
            if (t >= since) { sum += v; n++; }
        return n > 0 ? sum / n : double.NaN;
    }

    /// <summary>°C/s of the sink trend, least-squares over the slope window.</summary>
    private double TrendSlope()
    {
        if (_trend.Count < 5 || _trend[^1].time - _trend[0].time < 10) return 0;
        double t0 = _trend[0].time;
        double n = _trend.Count, sx = 0, sy = 0, sxx = 0, sxy = 0;
        foreach (var (t, v) in _trend)
        {
            double x = t - t0;
            sx += x; sy += v; sxx += x * x; sxy += x * v;
        }
        double denom = n * sxx - sx * sx;
        return denom < 1e-9 ? 0 : (n * sxy - sx * sy) / denom;
    }

    private bool PowerStable(double now)
    {
        double min = double.MaxValue, max = double.MinValue;
        foreach (var (t, v) in _powers)
        {
            if (t < now - TrendAvgSeconds) continue;
            if (v < min) min = v;
            if (v > max) max = v;
        }
        return min <= max && max - min <= Math.Max(10, PowerNow * 0.25);
    }

    private double TempSpan(double since)
    {
        double min = double.MaxValue, max = double.MinValue;
        foreach (var (t, v) in _temps)
        {
            if (t < since) continue;
            if (v < min) min = v;
            if (v > max) max = v;
        }
        return min <= max ? max - min : 0;
    }

    /// <summary>The distinct staircase levels, ascending — the allowed speeds.</summary>
    private static List<double> Ladder(FanCurve curve)
    {
        var l = curve.Points.Select(p => p.Percent).Distinct().OrderBy(p => p).ToList();
        if (l.Count == 0) l.Add(50);
        return l;
    }

    public void Reset()
    {
        _temps.Clear();
        _powers.Clear();
        _trend.Clear();
        _target = double.NaN;
        _output = double.NaN;
        _downSince = double.NaN;
        _upSince = double.NaN;
        _overrideOkSince = double.NaN;
        _lastTime = double.NaN;
        _lastRampStep = double.NaN;
        _failedLevel = double.NaN;
        OverrideActive = false;
        EffectiveTemp = double.NaN;
        TrendTempC = double.NaN;
        SlopeCPerSec = 0;
        TauSeconds = double.PositiveInfinity;
    }
}
