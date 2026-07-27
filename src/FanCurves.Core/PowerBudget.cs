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
/// 3. The fan steps UP only when the predicted headroom — seconds until the sink trend
///    crosses the sustained aim (or the ceiling, once already past the aim), the more
///    pessimistic of the model's equilibrium prediction and the measured slope — drops
///    under RampLeadSeconds, and it goes directly to the ladder level whose equilibrium
///    holds the aim. Headroom is referenced to the AIM, not the ceiling: nearly every
///    real state on a big cooler equilibrates under the ceiling, so a ceiling ETA read
///    ∞ regardless of temperature or fan speed and recovered mid-warm-up with the fans
///    still parked (2026-07-26); aim-referenced, it only recovers when the fans step up
///    or the load ends.
///    ONE step per slope window: the measured slope is backward-looking, so right after
///    a step it still reports the pre-step warming and would otherwise climb the whole
///    ladder to 100% in as many ticks (Kuba's report, 2026-07-26). Within the settle
///    window a further step happens only if the chosen level has become objectively
///    insufficient — the draw rose enough that its predicted equilibrium no longer
///    clears the ceiling. The fuse is not rate-limited by any of this.
/// 4. It steps DOWN one ladder level per StepDownHoldSeconds once the sustained power
///    average no longer needs the current one — and only onto a level predicted to hold
///    the aim with the channel's HysteresisC to spare (the budget twin of the temp
///    filter's step-down hysteresis; without it a level that equilibrates right AT the
///    aim is stepped onto and back off in a slow hunt). After a load ends this reacts
///    to the power collapse, minutes before a cooling temperature average would.
/// 5. The sustained aim is also enforced UPWARD: once the sink trend is past the steady
///    target and either the sustained average needs a higher ladder level or the temp
///    has settled there, it steps up one level per StepDownHoldSeconds. The tau trigger
///    alone goes blind at any equilibrium (surplus and slope both vanish, so both
///    predictions read ∞ — the better the model has learned, the more exactly), which
///    used to park the die anywhere below the ceiling with the fans never asked for more.
///    Each step up brands the level it left as measured-insufficient at that draw, so an
///    unlearned model cannot argue the fan straight back down into an on/off limit cycle;
///    the brand lifts once the sustained draw falls clearly below the draw that failed.
///    Both upward paths stop at the FUTILITY EDGE: when no ladder level can hold the aim
///    (die-limited load — the conduction gradient under the die dwarfs what airflow can
///    touch, so the die self-clamps at the same temperature at every fan speed), demand
///    and the step-up logic go no further than the lowest level within HysteresisC of
///    the best equilibrium the ladder offers. Treating the unreachable aim as
///    demand = max marched the fans to 100% for fractions of a degree (a chess engine
///    pinned the die at 85.3° at 81, 90 and 100% alike, 2026-07-27). The model band
///    alone cannot hold the line — LearnSteady keeps teaching the visited anchor the
///    measured R while unvisited anchors stay stale-optimistic — so every model-driven
///    step from a settled state doubles as an EXPERIMENT: if the trend is flat again at
///    unchanged draw without having dropped HysteresisC, the step is taken back and
///    further steps above the returned-to level are latched off until the draw or trend
///    leaves the proven neighbourhood. One short probe instead of a march; the fuse
///    stays the hard stop.
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
    /// <summary>Step-down clearance: the level below must hold the aim this much to spare
    /// (the channel's HysteresisC — same anti-flap knob as the temperature filter).</summary>
    public double HysteresisC { get; set; } = 1.5;
    public double SlewUpPercentPerSec { get; set; } = 9;
    public double SlewDownPercentPerSec { get; set; } = 8;
    /// <summary>Levels above 0 but strictly below this snap to 0 (0 = disabled).</summary>
    public double ZeroSnapPercent { get; set; } = 0;
    /// <summary>External demand floor (Auto mode): the temperature filter's target level.
    /// The published target and the slewed output never fall below it, so the output the
    /// physics read — surplus, equilibrium, learning — is the level the fan actually runs;
    /// feeding the model the controller's own lower choice while an outside hand drives
    /// the fan harder would teach it that little fan cools this well. 0 outside Auto.</summary>
    public double FloorPercent { get; set; }
    /// <summary>Auto mode: the buffer is also being drained toward the staircase floor's
    /// next step — the floor fires there on the display average whatever the model
    /// believes. When set, the measured headroom prong watches the trend drain toward
    /// that line (so a settled creep reads finite instead of ∞ and the ramp glides in
    /// ahead of the floor's snap), and a floor step that out-ranks the budget's target
    /// brands the level it beat as measured-insufficient and is adopted as the budget's
    /// own — the staircase IS a measurement. False outside Auto.</summary>
    public bool GuardFloor { get; set; }
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
    private double _forgiveSince = double.NaN;
    private double _lastFloor;
    // A model-driven up-step from a settled state is an EXPERIMENT: it promised the
    // settled trend would come down. If the trend is flat again at unchanged draw and
    // has not dropped by HysteresisC, the promise is proven false — the die is limited
    // by something airflow cannot touch (boost clamp, conduction gradient) — so the
    // step is taken back and further model-driven steps above the returned-to level
    // are LATCHED off until the draw or the trend leaves the proven neighbourhood.
    // The model band alone cannot catch this: LearnSteady keeps pulling the visited
    // anchor up to the measured R while unvisited anchors keep stale lower values, so
    // the model perpetually believes the next step buys degrees it cannot.
    private double _expFrom = double.NaN;      // level the experiment left
    private double _expTrend;                  // settled trend before the step
    private double _expDraw;                   // sustained draw before the step
    private double _expTime;
    private double _futileAbove = double.NaN;  // latch: no measured benefit above this
    private double _futileDraw;
    private double _futileTrend;

    // Diagnostics for the engine/UI — refreshed on every Step, valid until the next one.
    /// <summary>Fuse engaged: the temperature curve is being written directly, no slew.</summary>
    public bool OverrideActive { get; private set; }
    /// <summary>Rolling average temp over DisplayAveragingSeconds (hero numeral parity).</summary>
    public double EffectiveTemp { get; private set; } = double.NaN;
    /// <summary>What the plain temperature staircase would ask at the display average.</summary>
    public double CurveLevelAtAvg { get; private set; }
    /// <summary>The level the slew is gliding toward (floor and zero-snap applied).</summary>
    public double TargetLevel => double.IsNaN(_target) ? 0 : Snap(Math.Max(_target, FloorPercent));
    /// <summary>The chosen ladder level (floor applied) before the zero snap.</summary>
    public double PreSnapTarget => double.IsNaN(_target) ? 0 : Math.Max(_target, FloorPercent);
    /// <summary>The chosen level was above 0 but collapsed to 0 by the zero snap.</summary>
    public bool SnappedToZero { get; private set; }
    /// <summary>Package power, short average (last ~10 s).</summary>
    public double PowerNow { get; private set; }
    /// <summary>Package power averaged over PowerAveragingSeconds — the demand signal.</summary>
    public double PowerAvg { get; private set; }
    /// <summary>Energy credit left before the budget ceiling: C · (ceiling − trend temp).</summary>
    public double BudgetJoules { get; private set; }
    /// <summary>Predicted headroom: seconds until the trend crosses the guarded line —
    /// the sustained aim while below it, the ceiling once past it. ∞ only while the
    /// sustained draw is holdable at the current fan level.</summary>
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
    /// <summary>The sustained aim in force: OverrideTempC − max(SteadyTargetMarginC, CeilingMarginC).</summary>
    public double SteadyTargetC { get; private set; }

    /// <param name="now">Monotonic time in seconds (same clock every call).</param>
    /// <param name="rawTemp">Raw channel temperature (max of assigned sensors).</param>
    /// <param name="watts">Sum of the channel's power sensors.</param>
    /// <param name="curve">The channel's staircase — ladder of levels + fuse fallback.</param>
    public double Step(double now, double rawTemp, double watts, FanCurve curve)
    {
        _temps.Add((now, rawTemp));
        _powers.Add((now, watts));
        _temps.RemoveAll(s => s.time < now - Math.Max(DisplayAveragingSeconds, TrendAvgSeconds));
        // Retention covers the learning-quietness horizon below (slope window + trend
        // window + power averaging), which dominates every other power-window need.
        _powers.RemoveAll(s => s.time < now -
            (SlopeWindowSeconds + TrendAvgSeconds + PowerAveragingSeconds));

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

        var ladder = Ladder(curve);
        // The sustained aim can never sit above the transient ceiling, whatever the
        // two margins are set to — otherwise "settled" would mean "already over budget".
        double steadyTarget = OverrideTempC - Math.Max(SteadyTargetMarginC, CeilingMarginC);
        SteadyTargetC = steadyTarget;

        // ---- Auto's floor is a measurement, not an opinion: a floor step that out-ranks
        // the budget's target is the staircase proving that level could not keep the sink
        // under the step's own temperature at today's draw. Brand it like a StepUpHold
        // step and adopt the floor's level as the budget's own target — otherwise the
        // budget keeps insisting 0% is sustainable, the floor cycles the fan on/off
        // around its step temperature every few minutes, and headroom reads ∞ the whole
        // time the buffer is visibly being drained (Kuba's report, 2026-07-27). The
        // output doesn't jump: the floor was already binding it. The display average the
        // floor fires on is itself a sustained measure, so no draw-settled gate here.
        if (GuardFloor && !OverrideActive && FloorPercent > _lastFloor + 0.01 &&
            FloorPercent > _target + 0.01)
        {
            _failedLevel = _target;
            _failedPowerAvg = PowerAvg;
            _forgiveSince = double.NaN;
            _target = FloorPercent;
            _expFrom = double.NaN; // the jump confounds any experiment in flight
            _downSince = double.NaN;
            _upSince = double.NaN;
        }
        _lastFloor = FloorPercent;

        double slope = TrendSlope();
        SlopeCPerSec = slope;
        double surplus = PowerNow - Model.DissipationWatts(trendTemp, _output); // for learning

        // ---- Headroom: predicted seconds until the trend crosses the guarded line — the
        // sustained aim while below it, the hard ceiling once past the aim. Referenced to
        // the AIM, not the ceiling: on a big cooler nearly every real state equilibrates
        // under the ceiling, so a ceiling ETA read ∞ "no matter what" and even recovered
        // to ∞ mid-warm-up as the climb decelerated toward a hotter-than-wanted
        // equilibrium with the fans still parked (Kuba's report, 2026-07-26). Headroom
        // may only recover because the fans stepped up or the load went away — never
        // because the system is settling somewhere it should not be.
        //   Model prong: exact first-order time for the trend to reach the line given the
        //   equilibrium the sustained draw implies at the CURRENT fan level — finite and
        //   still shrinking however gently the temp moves, as long as that equilibrium
        //   sits past the line (the case the old surplus/slope pair went blind on).
        //   Measured prong: distance over the observed warming slope — model-free, keeps
        //   the headroom honest while the model is unlearned, frozen or wrong.
        double guarded = trendTemp < steadyTarget ? steadyTarget : ceiling;
        double eq = Model.BaseTempC + Model.R(_output) * PowerAvg;
        // Quietness horizons. The slope fit is contaminated by any burst younger than
        // the span it can see (SlopeWindowSeconds of trend values, each itself an
        // average over TrendAvgSeconds of die temps) — the measured prong must stay
        // silent that long after a burst, or an 8 s spike's die jump reads as a rush
        // toward the aim (harness S3). LEARNING needs quiet over the sink's much longer
        // thermal memory: quasi-steady sampling inside a spike train pairs the
        // still-elevated trend with trough power and inflates R (harness S3b taught
        // itself R(0)=0.78 through windows that were 30 s "stable"), and a poisoned R
        // is what turns the model prong into a false accuser.
        double recentPeak = 0, learnPeak = 0;
        double slopeQuiet = SlopeWindowSeconds + TrendAvgSeconds;
        double learnQuiet = slopeQuiet + PowerAveragingSeconds;
        foreach (var (t, v) in _powers)
        {
            if (t >= now - slopeQuiet && v > recentPeak) recentPeak = v;
            if (t >= now - learnQuiet && v > learnPeak) learnPeak = v;
        }
        bool drawSettled = recentPeak - PowerAvg <= Math.Max(10, PowerAvg * 0.25);
        bool drawLearnable = learnPeak - PowerAvg <= Math.Max(10, PowerAvg * 0.25);
        // Below the aim the equilibrium must clear the line by the hysteresis margin —
        // the twin of the step-down clearance, so the two form a dead band around the
        // aim inside which only settled evidence (the StepUpHold path) moves the fan.
        // At the ceiling the margin stays minimal: conservative there means firing
        // earlier, not later.
        double clearBy = guarded == steadyTarget ? HysteresisC : 0.05;
        // The model prong runs UNGATED: it reads only sustained quantities (PowerAvg,
        // the trend) and the clearance margin absorbs power-window aliasing. Gating it
        // on burst-quietness silenced the whole predictive layer under a real game —
        // a live draw never stops fluctuating, so the gate never opened, headroom sat
        // pinned at ∞ and the die climbed through 80° with the fans parked until the
        // reactive path finally caught it above the aim (Kuba's report, 2026-07-27).
        double tauModel = trendTemp < guarded && eq > guarded + clearBy
            ? Model.R(_output) * Model.MassJPerC * Math.Log((eq - trendTemp) / (eq - guarded))
            : double.PositiveInfinity;
        double tauSlope = drawSettled && slope > 0.005
            ? Math.Max(0, guarded - trendTemp) / slope
            : double.PositiveInfinity;
        TauSeconds = Math.Min(tauModel, tauSlope);
        // The temperature floor's next step is a guarded line too (Auto): the staircase
        // fires there on the display average whatever the model believes, so headroom
        // watches the trend drain toward it. This is what makes "the buffer is being
        // driven down by the temperature" visible below the aim — a slow settled creep
        // toward the floor's step reads as finite, falling headroom instead of ∞, and
        // the ramp branch glides to a level ahead of the floor's snap. MEASURED prong
        // only: the model arm was tried and cut the same day — near an unvisited
        // operating point the learned R is at its least trustworthy, and one transient
        // of ridge-inflated R(0) turned the 57° line's small clearance into a false
        // fire during a spike (harness F4); the floor's own reactive step (adopted and
        // branded above) is already the safety net the model arm pretended to be.
        if (GuardFloor && drawSettled && slope > 0.005)
        {
            double line = NextFloorStep(curve, Math.Max(_target, FloorPercent));
            if (!double.IsPositiveInfinity(line))
                TauSeconds = Math.Min(TauSeconds, Math.Max(0, line - trendTemp) / slope);
        }
        // A spent budget with the trend not clearly falling is ZERO headroom, not
        // infinite: parked at the ceiling, equilibrium gap and slope both vanish and
        // the two predictions above go blind exactly when the credit is gone.
        if (BudgetJoules <= 0 && slope > -0.002) TauSeconds = 0;

        // ---- Futility latch release: the proven neighbourhood is left once the draw
        // moves clearly away (lighter load unwinds normally; heavier load has earned a
        // fresh experiment) or the trend moves past HysteresisC of the proven-flat
        // temperature (cooler: the limit lifted; hotter: the verdict no longer covers
        // this state and the fan must be allowed to try again).
        if (!double.IsNaN(_futileAbove))
        {
            double band = Math.Max(5, _futileDraw * 0.1);
            if (PowerAvg < _futileDraw - band || PowerAvg > _futileDraw + band ||
                Math.Abs(trendTemp - _futileTrend) > HysteresisC)
                _futileAbove = double.NaN;
        }
        // ---- Experiment verdict, only once the plant has settled again (flat slope,
        // draw still quiet): a fallen trend proves the step helped; a flat trend at
        // unchanged draw proves it bought nothing — take the step back and latch. A
        // clearly grown draw makes the flat trend inconclusive (a boost-clamped CPU
        // converts fan into watts at constant temperature, but so does a genuinely
        // heavier load) — re-baseline and keep watching.
        if (!double.IsNaN(_expFrom))
        {
            double band = Math.Max(5, _expDraw * 0.1);
            if (PowerAvg < _expDraw - band) _expFrom = double.NaN; // load shrank — moot
            else if (now - _expTime >= StepDownHoldSeconds && drawSettled &&
                     Math.Abs(slope) < 0.005)
            {
                if (trendTemp <= _expTrend - HysteresisC)
                    _expFrom = double.NaN;                         // measured: it helped
                else if (PowerAvg > _expDraw + band)
                {
                    _expTrend = trendTemp;                         // inconclusive: rebase
                    _expDraw = PowerAvg;
                    _expTime = now;
                }
                else
                {
                    _futileAbove = _expFrom;
                    _futileDraw = PowerAvg;
                    _futileTrend = trendTemp;
                    _target = Math.Min(_target, _expFrom);
                    // The level was not too weak — the aim is unreachable here; keeping
                    // the brand would march the demand right back up over the latch.
                    _failedLevel = double.NaN;
                    _forgiveSince = double.NaN;
                    _expFrom = double.NaN;
                }
            }
        }

        DemandLevel = UsefulLevel(ladder, PowerAvg, steadyTarget);
        // Measured insufficiency outranks the model's estimate: while a level stands
        // proven too weak for today's draw, demand cannot fall to it or below. The
        // brand lifts only once the sustained draw has stayed clearly below the draw
        // that failed for a full hold — a fluctuating load's lulls dip under any
        // instantaneous threshold, and instant forgiveness re-ran the same
        // forgive → step down → re-warm → re-brand loop on every swell (harness S7).
        if (!double.IsNaN(_failedLevel))
        {
            if (PowerAvg < _failedPowerAvg - Math.Max(5, _failedPowerAvg * 0.1))
            {
                if (double.IsNaN(_forgiveSince)) _forgiveSince = now;
                else if (now - _forgiveSince >= StepDownHoldSeconds)
                {
                    _failedLevel = double.NaN;
                    _forgiveSince = double.NaN;
                }
            }
            else _forgiveSince = double.NaN;
        }
        if (!double.IsNaN(_failedLevel))
            foreach (var l in ladder)
                if (l > _failedLevel + 0.01) { DemandLevel = Math.Max(DemandLevel, l); break; }
        // Measured futility outranks both the model and the brand: while the latch
        // stands, no level above it is worth asking for.
        if (!double.IsNaN(_futileAbove))
            DemandLevel = Math.Min(DemandLevel, _futileAbove);

        // First ladder level above the current target, and whether the settled upward
        // enforcement stepping onto it would do measurable good: yes while the model
        // claims THIS level holds the aim (a settled trend above the aim proves it
        // wrong, and the step is what teaches it), or while the next level is expected
        // to buy at least the dead band — and never past the futility latch. A
        // die-limited load fails these, so the settled march stops at the futility
        // edge instead of running to max fan.
        double stepAbove = ladder[^1];
        foreach (var l in ladder) if (l > _target + 0.01) { stepAbove = l; break; }
        bool stepUpUseful = (double.IsNaN(_futileAbove) || stepAbove <= _futileAbove + 0.01) &&
                            (Eq(_target, PowerAvg) <= steadyTarget ||
                             Eq(stepAbove, PowerAvg) <= Eq(_target, PowerAvg) - HysteresisC);

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
            _expFrom = double.NaN;
            _downSince = double.NaN;
            _upSince = double.NaN;
        }
        else if (TauSeconds < RampLeadSeconds && _target < ladder[^1] - 0.01 &&
                 (double.IsNaN(_lastRampStep) || now - _lastRampStep >= SlopeWindowSeconds ||
                  Model.BaseTempC + Model.R(_target) * PowerNow > steadyTarget))
        {
            // Headroom is running out: jump the target to the lowest ladder level whose
            // equilibrium temperature at today's draw holds the sustained aim — at that
            // level the warming dies out before the line is crossed (same line the
            // demand estimate and the upward enforcement defend, so the predictive
            // step no longer undershoots what the settle logic would ask for). Then
            // hold: the slope that keeps TauSeconds low is measured over the past
            // window and cannot yet know about this step, so another one is allowed
            // only after the window turns over — or immediately if the draw has risen
            // enough that the level just chosen no longer holds the aim.
            double up = ladder[^1];
            bool holdsAim = false;
            foreach (var l in ladder)
            {
                if (l <= _target + 0.01) continue;
                if (Eq(l, PowerNow) <= steadyTarget) { up = l; holdsAim = true; break; }
            }
            if (!holdsAim)
            {
                // Aim unreachable at any higher speed — die-limited territory. Step only
                // to the futility edge (lowest level within HysteresisC of the ladder's
                // best equilibrium); if this level already sits inside that band, more
                // fan buys fractions of a degree and the step is skipped outright.
                double best = Eq(ladder[^1], PowerNow);
                if (Eq(_target, PowerNow) <= best + HysteresisC) up = _target;
                else
                    foreach (var l in ladder)
                    {
                        if (l <= _target + 0.01) continue;
                        if (Eq(l, PowerNow) <= best + HysteresisC) { up = l; break; }
                    }
            }
            if (!double.IsNaN(_futileAbove)) up = Math.Min(up, _futileAbove);
            if (up > _target + 0.01)
            {
                // With the draw settled this step is measured evidence, same as a
                // StepUpHold step: brand the level being left as insufficient at this
                // sustained draw. Without the brand an optimistically-wrong model argues
                // the fan straight back down after every predictive step and the pair
                // hunts in a slow on/off cycle (harness S5). Model-prong fires during an
                // unsettled draw (burst still in flight) stay unbranded — no measured
                // corroboration yet. A step from a genuinely settled state also opens
                // a futility experiment: the promised trend drop is now on the record.
                if (drawSettled)
                {
                    _failedLevel = _target;
                    _failedPowerAvg = PowerAvg;
                    if (Math.Abs(slope) < 0.005)
                    {
                        _expFrom = _target;
                        _expTrend = trendTemp;
                        _expDraw = PowerAvg;
                        _expTime = now;
                    }
                }
                _target = up;
                _lastRampStep = now;
            }
            _downSince = double.NaN;
            _upSince = double.NaN;
        }
        else if (trendTemp > steadyTarget && _target < ladder[^1] - 0.01 &&
                 (DemandLevel > _target + 0.01 || (slope > -0.002 && stepUpUseful)))
        {
            // The sink is already past the sustained aim and either the power average
            // needs a level this one cannot hold, or the temp has settled there (not
            // cooling — a frozen or unlearned model can claim the level is sufficient
            // while the measured trend proves otherwise): one ladder step up per hold.
            // The tau trigger never fires at a settled equilibrium (surplus and slope
            // both vanish), so without this the die would park anywhere below the
            // ceiling with the fans never asked for more. Gating on the trend keeps
            // burst immunity: short spikes never drag the trend past the aim; gating
            // the settled arm on stepUpUseful keeps a die-limited load from being
            // marched to max fan for fractions of a degree.
            if (double.IsNaN(_upSince)) _upSince = now;
            _downSince = double.NaN;
            if (now - _upSince >= StepDownHoldSeconds)
            {
                _failedLevel = _target;      // measured: this level cannot hold the aim
                _failedPowerAvg = PowerAvg;  // at this sustained draw
                if (Math.Abs(slope) < 0.005)
                {
                    _expFrom = _target;      // futility experiment: settled state left
                    _expTrend = trendTemp;
                    _expDraw = PowerAvg;
                    _expTime = now;
                }
                _target = stepAbove;
                _upSince = double.NaN;
            }
            else
            {
                PendingUpLevel = Snap(stepAbove);
                UpHoldRemaining = StepDownHoldSeconds - (now - _upSince);
            }
        }
        else if (TauSeconds >= RampLeadSeconds && DemandLevel < _target - 0.01 &&
                 trendTemp < ceiling - 1)
        {
            // Sustained power no longer needs this level: one ladder step down per hold.
            double below = ladder[0];
            foreach (var l in ladder) if (l < _target - 0.01) below = l;
            // The level below must hold the aim even at 5/4 of today's sustained draw —
            // hysteresis in the draw dimension, where the swings actually live. The
            // ±HysteresisC temperature band alone is worth only ~4 W at a 20% fan
            // level, so a game's minute-scale power swells crossed it constantly and
            // the fan chased them up and down (harness S7). Deliberately NOT stacked
            // with the temperature band: the double margin blocked the final step to
            // silence at warm idle, with 0%'s predicted equilibrium under the aim but
            // inside the stacked band (harness S5b).
            if (Model.BaseTempC + Model.R(below) * (PowerAvg * 1.25) > steadyTarget)
            {
                _downSince = double.NaN;
                _upSince = double.NaN;
            }
            else
            {
                if (double.IsNaN(_downSince)) _downSince = now;
                _upSince = double.NaN;
                if (now - _downSince >= StepDownHoldSeconds)
                {
                    _target = below;
                    _expFrom = double.NaN;
                    _downSince = double.NaN;
                }
                else
                {
                    PendingDownLevel = Snap(below);
                    DownHoldRemaining = StepDownHoldSeconds - (now - _downSince);
                }
            }
        }
        else { _downSince = double.NaN; _upSince = double.NaN; }

        // The external floor binds the OUTPUT, not _target: the budget's own choice keeps
        // evolving underneath (step-downs included), and when the floor drops the output
        // simply slews back to it. The floor arrives already zero-snapped by its filter.
        double floored = Math.Max(_target, FloorPercent);
        double snapped = Snap(floored);
        SnappedToZero = !OverrideActive && floored > 0 && snapped <= 0;
        if (!OverrideActive)
        {
            if (snapped > _output)
                _output = Math.Min(snapped, _output + SlewUpPercentPerSec * dt);
            else
                _output = Math.Max(snapped, _output - SlewDownPercentPerSec * dt);
        }
        else _output = Math.Max(_output, FloorPercent); // the fuse never yields below the temp side

        // ---- Online learning — only with the fan settled, so the plant response
        //      isn't a mix of fan changes and power changes.
        if (LearningEnabled && !OverrideActive && drawLearnable &&
            Math.Abs(_output - snapped) < 0.5 && PowerStable(now))
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

    /// <summary>Predicted settled temperature at a fan level and sustained draw.</summary>
    private double Eq(double level, double watts) =>
        Model.BaseTempC + Model.R(level) * watts;

    /// <summary>The lowest ladder level whose equilibrium at this draw holds the aim —
    /// or, when NO level can, the lowest within HysteresisC of the best equilibrium the
    /// ladder offers (the futility edge). A die-limited load used to read as
    /// demand = max because every level "failed" the unreachable aim, marching the fans
    /// to 100% for fractions of a degree (chess-engine report, 2026-07-27: die clamped
    /// at 85.3° at 81, 90 and 100% fan alike, heatsink barely warm).</summary>
    private double UsefulLevel(List<double> ladder, double watts, double aim)
    {
        foreach (var l in ladder)
            if (Eq(l, watts) <= aim) return l;
        double best = Eq(ladder[^1], watts);
        foreach (var l in ladder)
            if (Eq(l, watts) <= best + HysteresisC) return l;
        return ladder[^1];
    }

    /// <summary>Lowest staircase temperature at which the Auto floor would command more
    /// than this level (zero-snapped like the floor itself); +∞ when nothing can.</summary>
    private double NextFloorStep(FanCurve curve, double level)
    {
        double line = double.PositiveInfinity;
        foreach (var p in curve.Points)
            if (Snap(p.Percent) > level + 0.01 && p.TempC < line) line = p.TempC;
        return line;
    }

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
        _forgiveSince = double.NaN;
        _expFrom = double.NaN;
        _futileAbove = double.NaN;
        _lastFloor = 0;
        OverrideActive = false;
        EffectiveTemp = double.NaN;
        TrendTempC = double.NaN;
        SlopeCPerSec = 0;
        TauSeconds = double.PositiveInfinity;
    }
}
