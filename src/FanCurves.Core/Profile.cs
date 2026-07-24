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
    /// <summary>Learned thermal mass in J/°C (0 = not learned yet; see ThermalModel).</summary>
    public double LearnedThermalMassJPerC { get; set; }
    /// <summary>Learned cool-idle baseline temperature (0 = not learned yet).</summary>
    public double LearnedBaseTempC { get; set; }
    /// <summary>Learned cooling resistances in °C/W at the ThermalModel anchor speeds.</summary>
    public List<double> LearnedResistances { get; set; } = new();
}

public class Profile
{
    public string Name { get; set; } = "Profile";
    /// <summary>When false the app never (re)registers its start-with-Windows task.</summary>
    public bool AutostartEnabled { get; set; } = true;
    /// <summary>When true the close button hides to the tray instead of exiting.</summary>
    public bool MinimizeToTrayOnClose { get; set; } = true;
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
    /// <summary>When true, channels with a power sensor are driven by the thermal-budget
    /// controller (power in, heatsink mass as credit) instead of the temperature filter.</summary>
    public bool PowerControlEnabled { get; set; } = true;
    /// <summary>Window for the sustained power average — "how much heat must actually leave".</summary>
    public double PowerAveragingSeconds { get; set; } = 60;
    /// <summary>Fans step up once the predicted time to exhaust the thermal buffer drops under this.</summary>
    public double RampLeadSeconds { get; set; } = 45;
    /// <summary>Fuse: at/above this raw die temp the channel's temp curve takes over instantly, no slew.</summary>
    public double OverrideTempC { get; set; } = 90;
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
                    // Silent (stopped) through everything up to 57°C avg,
                    // then a gentle 20% start and fine-grained steps under load.
                    new CurvePoint(20, 0),
                    new CurvePoint(57, 20),
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
                MinPercent = 0, // 0% = fans stopped (Arctic P-series stop below 5% PWM)
                Points =
                {
                    new CurvePoint(20, 0),  // fully stopped through idle and light load
                    new CurvePoint(58, 25),
                    new CurvePoint(70, 40),
                    new CurvePoint(80, 55),
                    new CurvePoint(88, 70),
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
    public void AdoptTuning(Profile preset)
    {
        Name = preset.Name;
        for (int i = 0; i < Channels.Count && i < preset.Channels.Count; i++)
        {
            var mine = Channels[i];
            var src = preset.Channels[i];
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
        Converters = { new JsonStringEnumConverter() },
        IncludeFields = true,
    };

    public static string ConfigDir =>
        Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData), "FanCurves");

    public static string ConfigPath => Path.Combine(ConfigDir, "profile.json");

    public void Save()
    {
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
