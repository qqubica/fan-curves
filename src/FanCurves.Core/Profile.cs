using System.Text.Json;

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
}

/// <summary>One channel's share of a <see cref="TuningSnapshot"/>: the tuning a preset
/// overwrites, i.e. everything in ChannelConfig except what describes the machine
/// (sensor/header assignments).</summary>
public sealed record ChannelTuning(
    List<CurvePoint> Points, double AveragingSeconds,
    double HysteresisC, double StepDownHoldSeconds, double SlewUpPercentPerSec,
    double SlewDownPercentPerSec, double MinPercent)
{
    /// <summary>Value equality — the compiler-generated one compares the point list by
    /// reference, so a snapshot would never match a re-captured copy of the same state.</summary>
    public bool Matches(ChannelTuning o) =>
        Points.SequenceEqual(o.Points) &&
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
    /// <summary>When false every channel's MinPercent is ignored — the fans are allowed
    /// all the way to a stop wherever the curve asks for less than the floor.
    /// App-level like the other feature switches; the per-channel value itself is kept.</summary>
    public bool SafetyFloorEnabled { get; set; } = true;
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
                    // barely-audible 10% start; the top steps carry sustained load
                    // now that the temperature staircase is the only driver.
                    new CurvePoint(20, 0),
                    new CurvePoint(50, 10),
                    new CurvePoint(55, 20),
                    new CurvePoint(62, 40),
                    new CurvePoint(70, 50),
                    new CurvePoint(76, 65),
                    new CurvePoint(84, 81),
                    new CurvePoint(88, 90),
                    new CurvePoint(92, 100),
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
            c.Points.ToList(), c.AveragingSeconds, c.HysteresisC,
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
        IncludeFields = true,
    };

    public static string ConfigDir =>
        Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData), "FanCurves");

    public static string ConfigPath => Path.Combine(ConfigDir, "profile.json");

    /// <summary>
    /// Dev flows (--sim / --screenshot) read the real profile but must never write it:
    /// AutoAssign prunes identifiers the simulated backend doesn't know, so a save would
    /// wipe the machine's real sensor/header assignments — including manual ones like a
    /// pump header, which is never auto-assigned and so never comes back.
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
                if (p != null && p.Channels.Count > 0) return p;
            }
        }
        catch { /* corrupted config → fall back to default */ }
        return MacBookLike();
    }
}
