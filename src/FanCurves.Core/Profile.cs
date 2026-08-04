using System.Text.Json;
using System.Text.Json.Serialization;

namespace FanCurves.Core;

public class ChannelConfig
{
    public string Name { get; set; } = "Fan";
    /// <summary>Hardware control identifiers this channel drives (empty = unassigned).</summary>
    public List<string> ControlIds { get; set; } = new();
    /// <summary>Temperature sensor identifiers; the MAX of them feeds the curve.</summary>
    public List<string> SensorIds { get; set; } = new();
    public bool Enabled { get; set; } = true;
    /// <summary>Safety floor in percent — the engine never writes below this.</summary>
    public double MinPercent { get; set; } = 0;
    /// <summary>Step thresholds: at PointN.TempC and above, run at PointN.Percent (staircase).</summary>
    public List<CurvePoint> Points { get; set; } = new();
    /// <summary>The curve is driven by the rolling average over this window, not the live temp.</summary>
    public double AveragingSeconds { get; set; } = 20;
    /// <summary>A step is only left downward once (average + this) still maps to a lower step.</summary>
    public double HysteresisC { get; set; } = 3;
    /// <summary>The hysteresis condition must hold continuously this long before the step-down fires.</summary>
    public double StepDownHoldSeconds { get; set; } = 10;
    public double SlewUpPercentPerSec { get; set; } = 8;
    public double SlewDownPercentPerSec { get; set; } = 8;
    /// <summary>Power sensors (watts, summed) for thermal-budget control; empty = temperature control.</summary>
    public List<string> PowerSensorIds { get; set; } = new();
    /// <summary>The power curve (PowerCurve mode): at PointN.Watts sustained draw and above,
    /// run at PointN.Percent — the watts twin of Points. Ignored in the other modes.</summary>
    public List<PowerPoint> PowerPoints { get; set; } = new();
    /// <summary>Learned thermal mass in J/°C (0 = not learned yet; see ThermalModel).</summary>
    public double LearnedThermalMassJPerC { get; set; }
    /// <summary>Learned cool-idle baseline temperature (0 = not learned yet).</summary>
    public double LearnedBaseTempC { get; set; }
    /// <summary>Learned cooling resistances in °C/W at the ThermalModel anchor speeds.</summary>
    public List<double> LearnedResistances { get; set; } = new();
}

/// <summary>How fans are driven — see Profile.ControlMode.</summary>
public enum ControlMode
{
    /// <summary>Every channel follows its staircase curve through the temperature filter.</summary>
    Temperature,
    /// <summary>Channels with a power sensor use the thermal-budget controller instead.</summary>
    Power,
    /// <summary>Both run at once and the higher demand wins: the temperature filter's
    /// target rides into the budget controller as a floor, so the staircase is a
    /// guaranteed baseline while the power side may still ramp earlier (or push higher)
    /// when its predicted headroom runs out.</summary>
    Auto,
    /// <summary>Channels with a power sensor follow their own watts→% staircase
    /// (ChannelConfig.PowerPoints) directly: the sustained power average picks the step,
    /// run through the same averaging/hysteresis/hold/slew filter as the temperature
    /// side — deterministic, no predictive budget layer. The temperature staircase keeps
    /// running as a safety floor and the hard-override fuse still applies.</summary>
    PowerCurve,
}

/// <summary>One channel's share of a <see cref="TuningSnapshot"/>: the tuning a preset
/// overwrites, i.e. everything in ChannelConfig except what describes the machine
/// (sensor/header assignments) or was measured on it (the learned thermal model).</summary>
public sealed record ChannelTuning(
    List<CurvePoint> Points, List<PowerPoint> PowerPoints, double AveragingSeconds,
    double HysteresisC, double StepDownHoldSeconds, double SlewUpPercentPerSec,
    double SlewDownPercentPerSec, double MinPercent)
{
    /// <summary>Value equality — the compiler-generated one compares the two point lists by
    /// reference, so a snapshot would never match a re-captured copy of the same state.</summary>
    public bool Matches(ChannelTuning o) =>
        Points.SequenceEqual(o.Points) && PowerPoints.SequenceEqual(o.PowerPoints) &&
        AveragingSeconds == o.AveragingSeconds && HysteresisC == o.HysteresisC &&
        StepDownHoldSeconds == o.StepDownHoldSeconds &&
        SlewUpPercentPerSec == o.SlewUpPercentPerSec &&
        SlewDownPercentPerSec == o.SlewDownPercentPerSec && MinPercent == o.MinPercent;
}

/// <summary>A profile's curve + behaviour tuning, detached from the live objects
/// (<see cref="Profile.CaptureTuning"/> / <see cref="Profile.ApplyTuning"/>).</summary>
public sealed record TuningSnapshot(string Name, List<ChannelTuning> Channels)
{
    public bool Matches(TuningSnapshot o) =>
        Name == o.Name && Channels.Count == o.Channels.Count &&
        Channels.Zip(o.Channels).All(p => p.First.Matches(p.Second));
}

public class Profile
{
    public string Name { get; set; } = "Profile";
    /// <summary>When false the app never (re)registers its start-with-Windows task.</summary>
    public bool AutostartEnabled { get; set; } = true;
    /// <summary>When true the close button hides to the tray instead of exiting.</summary>
    public bool MinimizeToTrayOnClose { get; set; } = true;
    /// <summary>When false a settings edit is only felt after the normal holds/slew
    /// instead of being adopted on the tick it lands (see FanEngine.SettingsSignature).</summary>
    public bool InstantApplyEnabled { get; set; } = true;
    /// <summary>When false nothing is written to logs\ (per-tick CSV + behavior.txt).</summary>
    public bool TelemetryLoggingEnabled { get; set; } = true;
    /// <summary>When false the process runs at normal priority instead of High — High
    /// keeps the engine tick and the UI scheduled under a fully loaded CPU.</summary>
    public bool HighPriorityEnabled { get; set; } = true;
    /// <summary>How long the hardware library keeps per-sensor value history (LHM
    /// defaults to 1 day for EVERY internal sensor — ~150+ on this machine, tens of
    /// MB of RAM the app never reads). 0 = none; the engine applies it on tick.</summary>
    public double SensorHistoryHours { get; set; } = 0;
    /// <summary>When false, fans sitting at 0% are never periodically spun up.</summary>
    public bool IdleKickEnabled { get; set; } = false;
    /// <summary>Seconds a driven fan must sit at 0% before a kick fires.</summary>
    public double IdleKickStoppedSeconds { get; set; } = 10;
    /// <summary>PWM percent written while a kick runs.</summary>
    public double IdleKickPercent { get; set; } = 5;
    /// <summary>How long each kick lasts, in seconds.</summary>
    public double IdleKickSeconds { get; set; } = 5;
    /// <summary>When true, steadily running fans with stable temps are trial-stopped (see StopProbe).</summary>
    public bool StopProbeEnabled { get; set; } = true;
    /// <summary>Continuous running time (with stable temp) required before a trial stop.</summary>
    public double StopProbeRunSeconds { get; set; } = 30;
    /// <summary>Trial length — a temperature rise within it means the fan was needed.</summary>
    public double StopProbeSeconds { get; set; } = 30;
    /// <summary>"Stable" band width; a rise beyond it while stopped resumes the fan.</summary>
    public double StopProbeStableRangeC { get; set; } = 3.5;
    /// <summary>Wait after a failed trial before probing again.</summary>
    public double StopProbeRetrySeconds { get; set; } = 60;
    /// <summary>No trial stop starts (and a running one aborts) above this raw temp.</summary>
    public double StopProbeMaxTempC { get; set; } = 78;
    /// <summary>Legacy pre-2026-07-27 switch, kept so older profiles load (false →
    /// Temperature, true → leave the mode alone — the default already uses power).
    /// Declared BEFORE ControlMode: serialization follows declaration order and
    /// deserialization document order, so in a new-format file ControlMode is applied
    /// after this bridge and always wins.</summary>
    public bool PowerControlEnabled
    {
        get => ControlMode != ControlMode.Temperature;
        set
        {
            if (!value) ControlMode = ControlMode.Temperature;
            else if (ControlMode == ControlMode.Temperature) ControlMode = ControlMode.Power;
        }
    }
    /// <summary>How channels with a power sensor are driven: the temperature filter, the
    /// thermal-budget controller (power in, heatsink mass as credit), or Auto — both at
    /// once with the higher demand winning. Channels without a power sensor always stay
    /// on the temperature filter.</summary>
    public ControlMode ControlMode { get; set; } = ControlMode.Auto;
    /// <summary>Window for the sustained power average — "how much heat must actually leave".</summary>
    public double PowerAveragingSeconds { get; set; } = 60;
    /// <summary>PowerCurve mode: a step is only left downward once the sustained draw sits
    /// this many watts clear of the band edge — the watts twin of ChannelConfig.HysteresisC.</summary>
    public double PowerCurveHysteresisW { get; set; } = 10;
    /// <summary>Fans step up once the predicted time to exhaust the thermal buffer drops under this.</summary>
    public double RampLeadSeconds { get; set; } = 45;
    /// <summary>When false every channel's MinPercent is ignored — the fans are allowed
    /// all the way to a stop wherever the curve/budget asks for less than the floor.
    /// App-level like the other feature switches; the per-channel value itself is kept.</summary>
    public bool SafetyFloorEnabled { get; set; } = true;
    /// <summary>When false the futility probe never runs: no up-step is taken back, no
    /// futility latch is set, and demand/ramp fall back to "the top of the ladder" when
    /// no level can hold the aim. Downward relief needs the latch, so it never arms
    /// either. On is the 2026-07-27 behaviour (a die-limited load is not marched to
    /// 100% for fractions of a degree).</summary>
    public bool FutilityProbeEnabled { get; set; } = true;
    /// <summary>Auto mode only: when false the budget stops watching the buffer drain
    /// toward the temperature staircase's next step, and a floor step that out-ranks its
    /// target is no longer treated as measurement (pre-2026-07-27 Auto).</summary>
    public bool FloorGuardEnabled { get; set; } = true;
    /// <summary>When false, downward relief never probes below the staircase floor
    /// (a standing waiver is withdrawn on the next tick).</summary>
    public bool ReliefEnabled { get; set; } = true;
    /// <summary>Downward relief (probing below the staircase floor once the up-march is
    /// proven futile) only runs while the sustained draw is under this many watts.</summary>
    public double ReliefMaxWatts { get; set; } = 190;
    /// <summary>When false the power floor line is not applied at all.</summary>
    public bool PowerFloorEnabled { get; set; } = true;
    /// <summary>Power floor calibration: minimum fan % at 100 W and at 200 W sustained
    /// draw (linear through the two points, clamped outside; both 0 = off).</summary>
    public double PowerFloorPercentAt100W { get; set; } = 30;
    public double PowerFloorPercentAt200W { get; set; } = 80;
    /// <summary>Fuse: at/above this raw die temp the channel's temp curve takes over instantly, no slew.</summary>
    public double OverrideTempC { get; set; } = 90;
    /// <summary>Budget ceiling = OverrideTempC − this. The credit E = C·(ceiling − temp) is
    /// measured against it, so it is the temperature the buffer is allowed to reach.</summary>
    public double BudgetCeilingMarginC { get; set; } = 4;
    /// <summary>Sustained target = OverrideTempC − this: where the power average aims to settle.</summary>
    public double SteadyTargetMarginC { get; set; } = 10;
    /// <summary>Sink-trend window: short average that hides the die's instant jump but tracks the metal.</summary>
    public double PowerTrendSeconds { get; set; } = 30;
    /// <summary>Window of the least-squares slope (°C/s) used for the measured time-to-ceiling.</summary>
    public double PowerSlopeSeconds { get; set; } = 25;
    /// <summary>"Draw now" window — the short power average the surplus/ramp decisions use.</summary>
    public double PowerNowSeconds { get; set; } = 10;
    /// <summary>The fuse releases only once the raw temp sits this far under OverrideTempC.</summary>
    public double OverrideReleaseC { get; set; } = 3;
    /// <summary>…and has stayed there this long.</summary>
    public double OverrideReleaseSeconds { get; set; } = 10;
    /// <summary>When false the thermal model is frozen at its current values (no online learning).</summary>
    public bool ThermalLearningEnabled { get; set; } = true;
    /// <summary>When true, curve targets below ZeroSnapPercent run the fan at 0% instead.</summary>
    public bool ZeroSnapEnabled { get; set; } = true;
    /// <summary>Targets above 0% but below this stop the fan — meaningful speed or nothing.</summary>
    public double ZeroSnapPercent { get; set; } = 20;
    public List<ChannelConfig> Channels { get; set; } = new();

    /// <summary>
    /// The default: silent flat floor at idle, dead zone against spikes,
    /// smooth ramp only under sustained load. Case fans fully stop at idle.
    /// </summary>
    public static Profile MacBookLike() => new()
    {
        Name = "Quiet (MacBook-like)",
        Channels =
        {
            new ChannelConfig
            {
                Name = "CPU cooler",
                MinPercent = 0, // true MacBook behaviour: fans fully stop at idle
                Points =
                {
                    // Silent (stopped) through everything up to 50°C avg, then a
                    // barely-audible 10% start and a 50% cap — sustained-load speed
                    // comes from the power side (Auto), not the temp staircase.
                    new CurvePoint(20, 0),
                    new CurvePoint(50, 10),
                    new CurvePoint(55, 20),
                    new CurvePoint(62, 40),
                    new CurvePoint(84, 50),
                },
                // The watts twin (PowerCurve mode): sized for a ~200 W-class CPU —
                // silence through desktop draw, the same ladder of levels as the
                // temp staircase so the two sides agree on the allowed speeds.
                PowerPoints =
                {
                    new PowerPoint(0, 0),
                    new PowerPoint(95, 20),
                    new PowerPoint(125, 40),
                    new PowerPoint(150, 50),
                    new PowerPoint(175, 65),
                    new PowerPoint(195, 81),
                    new PowerPoint(215, 90),
                    new PowerPoint(235, 100),
                },
                // 90 s averaging window: only genuinely sustained load moves the fans.
                AveragingSeconds = 90, HysteresisC = 1.5, StepDownHoldSeconds = 25,
                SlewUpPercentPerSec = 9, SlewDownPercentPerSec = 8,
            },
            new ChannelConfig
            {
                Name = "Case fans",
                MinPercent = 0, // 0% = fans stopped
                Points =
                {
                    new CurvePoint(20, 0),  // fully stopped through idle
                    new CurvePoint(40, 5),  // whisper bands (NF-A14s run at any duty >0%)
                    new CurvePoint(42, 10),
                    new CurvePoint(60, 20),
                    new CurvePoint(70, 40),
                    new CurvePoint(75, 60),
                    new CurvePoint(82, 82),
                    new CurvePoint(86, 100),
                },
                // Case channel sums CPU+GPU draw, so the steps sit higher.
                PowerPoints =
                {
                    new PowerPoint(0, 0),
                    new PowerPoint(140, 25),
                    new PowerPoint(220, 40),
                    new PowerPoint(300, 55),
                    new PowerPoint(380, 70),
                },
                AveragingSeconds = 25, HysteresisC = 4, StepDownHoldSeconds = 10,
                SlewUpPercentPerSec = 7, SlewDownPercentPerSec = 7,
            },
        },
    };

    /// <summary>Louder alternative: earlier, faster ramps that keep components cooler.</summary>
    public static Profile Performance() => new()
    {
        Name = "Performance",
        Channels =
        {
            new ChannelConfig
            {
                Name = "CPU cooler",
                MinPercent = 30,
                Points =
                {
                    new CurvePoint(20, 45),
                    new CurvePoint(50, 60),
                    new CurvePoint(65, 75),
                    new CurvePoint(80, 90),
                    new CurvePoint(90, 100),
                },
                PowerPoints =
                {
                    new PowerPoint(0, 45),
                    new PowerPoint(80, 60),
                    new PowerPoint(130, 75),
                    new PowerPoint(180, 90),
                    new PowerPoint(220, 100),
                },
                AveragingSeconds = 8, HysteresisC = 2, StepDownHoldSeconds = 5,
                SlewUpPercentPerSec = 6, SlewDownPercentPerSec = 2,
            },
            new ChannelConfig
            {
                Name = "Case fans",
                MinPercent = 0,
                Points =
                {
                    new CurvePoint(20, 25),
                    new CurvePoint(45, 40),
                    new CurvePoint(60, 55),
                    new CurvePoint(75, 75),
                    new CurvePoint(85, 95),
                },
                PowerPoints =
                {
                    new PowerPoint(0, 25),
                    new PowerPoint(100, 40),
                    new PowerPoint(180, 55),
                    new PowerPoint(260, 75),
                    new PowerPoint(340, 95),
                },
                AveragingSeconds = 10, HysteresisC = 2, StepDownHoldSeconds = 5,
                SlewUpPercentPerSec = 4, SlewDownPercentPerSec = 2,
            },
        },
    };

    /// <summary>
    /// Copy curve + behaviour tuning from a preset while keeping this profile's
    /// sensor/header assignments (those describe the machine, not the preset).
    /// </summary>
    public void AdoptTuning(Profile preset) => ApplyTuning(preset.CaptureTuning());

    /// <summary>Everything a preset switch overwrites, as a detached copy — so the UI can
    /// undo one by handing the pre-switch snapshot back to <see cref="ApplyTuning"/>.</summary>
    public TuningSnapshot CaptureTuning() => new(Name, Channels
        .Select(c => new ChannelTuning(
            c.Points.ToList(), c.PowerPoints.ToList(), c.AveragingSeconds, c.HysteresisC,
            c.StepDownHoldSeconds, c.SlewUpPercentPerSec, c.SlewDownPercentPerSec, c.MinPercent))
        .ToList());

    /// <summary>Restore a <see cref="CaptureTuning"/> snapshot. Sensor/header assignments and
    /// every app-level setting are untouched — a snapshot only carries curve + behaviour tuning.</summary>
    public void ApplyTuning(TuningSnapshot snapshot)
    {
        Name = snapshot.Name;
        for (int i = 0; i < Channels.Count && i < snapshot.Channels.Count; i++)
        {
            var mine = Channels[i];
            var src = snapshot.Channels[i];
            mine.Points = src.Points.ToList();
            mine.PowerPoints = src.PowerPoints.ToList();
            mine.AveragingSeconds = src.AveragingSeconds;
            mine.HysteresisC = src.HysteresisC;
            mine.StepDownHoldSeconds = src.StepDownHoldSeconds;
            mine.SlewUpPercentPerSec = src.SlewUpPercentPerSec;
            mine.SlewDownPercentPerSec = src.SlewDownPercentPerSec;
            mine.MinPercent = src.MinPercent;
        }
    }

    private static readonly JsonSerializerOptions JsonOpts = new()
    {
        WriteIndented = true,
        Converters = { new JsonStringEnumConverter() },
        IncludeFields = true,
    };

    public static string ConfigDir =>
        Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData), "FanCurves");

    public static string ConfigPath => Path.Combine(ConfigDir, "profile.json");

    /// <summary>
    /// Dev flows (--sim / --screenshot) read the real profile but must never write it:
    /// AutoAssign prunes identifiers the simulated backend doesn't know, so a save would
    /// wipe the machine's real sensor/header assignments — including manual ones like a
    /// pump header, which is never auto-assigned and so never comes back. It would also
    /// park sim-learned thermal models on real hardware.
    /// </summary>
    public static bool ReadOnly { get; set; }

    public void Save()
    {
        if (ReadOnly) return;
        Directory.CreateDirectory(ConfigDir);
        File.WriteAllText(ConfigPath, JsonSerializer.Serialize(this, JsonOpts));
    }

    public static Profile LoadOrDefault()
    {
        try
        {
            if (File.Exists(ConfigPath))
            {
                var p = JsonSerializer.Deserialize<Profile>(File.ReadAllText(ConfigPath), JsonOpts);
                if (p != null && p.Channels.Count > 0)
                {
                    // Profiles saved before the power curve existed carry no watts
                    // points; seed them from the default preset so PowerCurve mode
                    // has a ladder to stand on. (The editor never leaves a curve
                    // with fewer than two points, so empty means "never had one".)
                    var seed = MacBookLike();
                    for (int i = 0; i < p.Channels.Count && i < seed.Channels.Count; i++)
                        if (p.Channels[i].PowerPoints.Count == 0)
                            p.Channels[i].PowerPoints = seed.Channels[i].PowerPoints.ToList();
                    return p;
                }
            }
        }
        catch { /* corrupted config → fall back to default */ }
        return MacBookLike();
    }
}
