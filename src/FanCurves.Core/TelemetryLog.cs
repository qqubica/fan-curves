using System.Globalization;
using System.Text;

namespace FanCurves.Core;

/// <summary>
/// Review log: everything the engine saw and everything it decided, written so a human
/// can point at a moment and say "this is the part I don't like". Two files under
/// %AppData%\FanCurves\logs\:
///
///  - telemetry-YYYY-MM-DD.csv — one row per channel per engine tick with every input
///    the controller reads (raw/avg/trend temp, draw, sustained draw) and every output
///    it publishes (commanded %, target %, why-chip reason, headroom, demand, ceiling,
///    aim, learned model values). Rotated daily; the last 7 days are kept.
///  - behavior.txt — human-readable CHANGES only: fan start/stop, target ladder steps,
///    why-chip reason transitions, fuse latch/release, driving/released-to-BIOS flips,
///    and a full settings line whenever any tuning knob (or the curve) changes, so a
///    behavior complaint can always be matched to the settings in force at that moment.
///
/// Dev flows (--sim / --screenshot) write telemetry-sim-*.csv / behavior-sim.txt so a
/// simulation never contaminates the real machine's record (same rule as sensors.sim.txt).
/// Logging must never take fan control down with it: every write is wrapped, and the
/// first failure disables the log for the rest of the session.
/// </summary>
public class TelemetryLog : IDisposable
{
    private const int KeepDays = 7;
    private const long BehaviorMaxBytes = 4 * 1024 * 1024; // then rotated to *-old.txt

    private readonly string _dir;
    private readonly string _csvPrefix;    // "telemetry-" or "telemetry-sim-"
    private readonly string _behaviorPath;
    private readonly object _lock = new();

    private StreamWriter? _csv;
    private DateTime _csvDate;
    private StreamWriter? _behavior;
    private DateTime _lastFlush = DateTime.MinValue;
    private bool _dead; // a failed write turns logging off for the session

    // Previous tick, for change detection (indexed like Profile.Channels).
    private ChannelStatus[]? _prev;
    private string? _lastSettings;

    private static string Inv(FormattableString f) => FormattableString.Invariant(f);

    // With a BOM: Windows PowerShell and older editors otherwise read the °/·/∞ as ANSI mojibake.
    private static readonly UTF8Encoding Bom = new(encoderShouldEmitUTF8Identifier: true);

    public TelemetryLog(string configDir, bool simulated)
    {
        _dir = Path.Combine(configDir, "logs");
        _csvPrefix = simulated ? "telemetry-sim-" : "telemetry-";
        _behaviorPath = Path.Combine(_dir, simulated ? "behavior-sim.txt" : "behavior.txt");
    }

    /// <summary>App-level marker from the UI (e.g. "learned model reset") → behavior log.</summary>
    public void Event(string message)
    {
        lock (_lock) Guarded(() =>
        {
            WriteBehavior($"·· {message}");
            _behavior?.Flush();
        });
    }

    /// <summary>Called once per engine tick, on the engine's timer thread.</summary>
    public void Record(Profile profile, IReadOnlyList<ChannelStatus> statuses)
    {
        lock (_lock) Guarded(() =>
        {
            var now = DateTime.Now;
            RotateCsv(now);

            // Settings in force — one line whenever anything changes (preset adoption,
            // any slider, a curve edit, the control-mode switch) plus one at startup.
            string settings = SettingsLine(profile);
            if (settings != _lastSettings)
            {
                _lastSettings = settings;
                WriteBehavior("settings  " + settings);
            }

            for (int i = 0; i < statuses.Count; i++)
            {
                var s = statuses[i];
                _csv!.WriteLine(CsvRow(now, s));
                var prev = _prev != null && i < _prev.Length ? _prev[i] : null;
                LogTransitions(s, prev);
            }
            _prev = statuses.ToArray();

            _behavior?.Flush();
            if ((now - _lastFlush).TotalSeconds >= 5) { _lastFlush = now; _csv!.Flush(); }
        });
    }

    // ---- behavior (changes only) ----

    private void LogTransitions(ChannelStatus s, ChannelStatus? prev)
    {
        string ctx = Context(s);

        if (prev == null)
        {
            WriteBehavior($"{s.Name,-12} {(s.Applied ? "driving" : "released to BIOS")} — {ctx}");
            return;
        }

        if (s.Applied != prev.Applied)
            WriteBehavior($"{s.Name,-12} {(s.Applied ? "now driving" : "released to BIOS (paused)")} — {ctx}");

        bool wasOn = prev.OutputPercent > 0.01, isOn = s.OutputPercent > 0.01;
        if (isOn != wasOn)
            WriteBehavior($"{s.Name,-12} fan {(isOn ? "ON" : "OFF")} — {ctx}");

        if (Math.Abs(s.TargetPercent - prev.TargetPercent) > 0.5)
            WriteBehavior(Inv($"{s.Name,-12} target {prev.TargetPercent:0}% → {s.TargetPercent:0}% ") +
                          $"[{Describe(s)}] — {ctx}");

        // Reason transitions, minus the pure slew-ramp pairs (None↔Ramp*) — those are
        // implied by the target line that precedes every ramp.
        if (s.Reason != prev.Reason && !IsPureRampFlip(prev.Reason, s.Reason))
            WriteBehavior($"{s.Name,-12} {Describe(prev)} → {Describe(s)} — {ctx}");
    }

    private static bool IsPureRampFlip(OutputReason a, OutputReason b)
    {
        static bool Ramp(OutputReason r) => r is OutputReason.RampUp or OutputReason.RampDown;
        return (a == OutputReason.None && Ramp(b)) || (Ramp(a) && b == OutputReason.None);
    }

    /// <summary>The moment's vitals, appended to every behavior line.</summary>
    private static string Context(ChannelStatus s)
    {
        var sb = new StringBuilder();
        sb.Append(Inv($"out {s.OutputPercent:0}%"));
        if (!double.IsNaN(s.EffectiveTemp)) sb.Append(Inv($" · avg {s.EffectiveTemp:0.0}°"));
        if (s.RawTemp is double raw) sb.Append(Inv($" · now {raw:0.0}°"));
        if (s.WattsAvg is double wa) sb.Append(Inv($" · {wa:0} W avg"));
        if (!double.IsInfinity(s.TauSeconds)) sb.Append($" · headroom {Seconds(s.TauSeconds)}");
        return sb.ToString();
    }

    /// <summary>Same vocabulary as the why-chip, one compact phrase per reason.</summary>
    private static string Describe(ChannelStatus s) => s.Reason switch
    {
        OutputReason.RampUp => Inv($"ramping up to {s.TargetPercent:0}%"),
        OutputReason.RampDown => Inv($"ramping down to {s.TargetPercent:0}%"),
        OutputReason.StepDownHold => Inv($"step down to {s.ReasonLevel:0}% in {s.ReasonSeconds:0} s"),
        OutputReason.StepUpHold => Inv($"step up to {s.ReasonLevel:0}% in {s.ReasonSeconds:0} s"),
        OutputReason.Hysteresis => "hysteresis hold",
        OutputReason.ZeroSnap => Inv($"zero snap (curve asks {s.ReasonLevel:0}%)"),
        OutputReason.MinFloor => Inv($"safety floor (curve asks {s.ReasonLevel:0}%)"),
        OutputReason.IdleKick => "idle kick",
        OutputReason.StopProbe => "stop probe (trial stop)",
        OutputReason.BudgetHold => Inv($"budget hold (curve asks {s.ReasonLevel:0}%, headroom {Seconds(s.ReasonSeconds)})"),
        OutputReason.BudgetRamp => Inv($"budget ramp (curve asks {s.ReasonLevel:0}%, headroom {Seconds(s.ReasonSeconds)})"),
        OutputReason.HardOverride => Inv($"FUSE — raw-temp curve direct ({s.ReasonLevel:0}%)"),
        OutputReason.PowerCurve => Inv($"power curve (temp curve asks {s.ReasonLevel:0}%)"),
        _ => "steady on the curve",
    };

    private static string Seconds(double s) =>
        double.IsInfinity(s) || double.IsNaN(s) ? "∞" : Inv($"{s:0} s");

    private void WriteBehavior(string line)
    {
        EnsureBehavior();
        _behavior!.WriteLine($"{DateTime.Now:yyyy-MM-dd HH:mm:ss}  {line}");
    }

    // ---- settings snapshot ----

    /// <summary>Every knob that shapes behavior, one line; differences trigger a log entry.</summary>
    private static string SettingsLine(Profile p)
    {
        var sb = new StringBuilder();
        sb.Append(Inv($"profile \"{p.Name}\" · mode {p.ControlMode}"));
        sb.Append(Inv($" · snap {(p.ZeroSnapEnabled ? Inv($"<{p.ZeroSnapPercent:0}%") : "off")}"));
        sb.Append(p.IdleKickEnabled
            ? Inv($" · kick {p.IdleKickStoppedSeconds:0}s/{p.IdleKickPercent:0}%/{p.IdleKickSeconds:0}s")
            : " · kick off");
        sb.Append(p.StopProbeEnabled
            ? Inv($" · probe {p.StopProbeRunSeconds:0}s/{p.StopProbeSeconds:0}s/{p.StopProbeStableRangeC:0.#}°/{p.StopProbeRetrySeconds:0}s/<{p.StopProbeMaxTempC:0}°")
            : " · probe off");
        sb.Append(Inv($" · pwrAvg {p.PowerAveragingSeconds:0}s · pwrHyst {p.PowerCurveHysteresisW:0}W · lead {p.RampLeadSeconds:0}s"));
        sb.Append(p.ReliefEnabled ? Inv($" · relief <{p.ReliefMaxWatts:0}W") : " · relief off");
        sb.Append(p.PowerFloorEnabled
            ? Inv($" · pfloor {p.PowerFloorPercentAt100W:0}%@100W/{p.PowerFloorPercentAt200W:0}%@200W")
            : " · pfloor off");
        sb.Append(Inv($" · fuse {p.OverrideTempC:0}° (release −{p.OverrideReleaseC:0}°/{p.OverrideReleaseSeconds:0}s)"));
        sb.Append(Inv($" · ceilM {p.BudgetCeilingMarginC:0}° · aimM {p.SteadyTargetMarginC:0}°"));
        sb.Append(Inv($" · trend {p.PowerTrendSeconds:0}s · slope {p.PowerSlopeSeconds:0}s · now {p.PowerNowSeconds:0}s"));
        sb.Append($" · learn {(p.ThermalLearningEnabled ? "on" : "off")}");
        foreach (var ch in p.Channels)
        {
            sb.Append(Inv($" | {ch.Name}: min {ch.MinPercent:0}% · avg {ch.AveragingSeconds:0}s"));
            sb.Append(Inv($" · hyst {ch.HysteresisC:0.#}° · hold {ch.StepDownHoldSeconds:0}s"));
            sb.Append(Inv($" · slew {ch.SlewUpPercentPerSec:0}/{ch.SlewDownPercentPerSec:0}"));
            sb.Append(" · curve ");
            sb.Append(string.Join(' ', ch.Points.Select(pt => Inv($"{pt.TempC:0}:{pt.Percent:0}"))));
            if (ch.PowerPoints.Count > 0)
            {
                sb.Append(" · pwrCurve ");
                sb.Append(string.Join(' ', ch.PowerPoints.Select(pt => Inv($"{pt.Watts:0}:{pt.Percent:0}"))));
            }
            if (ch.PowerSensorIds.Count > 0) sb.Append(Inv($" · pwr[{ch.PowerSensorIds.Count}]"));
        }
        return sb.ToString();
    }

    // ---- CSV ----

    private const string CsvHeader =
        "time,channel,applied,out_pct,target_pct,reason,reason_level,reason_s," +
        "raw_c,avg_c,trend_c,rpm,watts,watts_avg,budget_kj,headroom_s,demand_pct," +
        "ceiling_c,aim_c,slope_cps,base_c,r_cpw,mass_jpc";

    private static string CsvRow(DateTime now, ChannelStatus s) => string.Join(',',
        now.ToString("yyyy-MM-dd HH:mm:ss", CultureInfo.InvariantCulture),
        s.Name.Replace(',', ' '),
        s.Applied ? "1" : "0",
        N(s.OutputPercent, "0.#"),
        N(s.TargetPercent, "0.#"),
        s.Reason.ToString(),
        N(s.ReasonLevel, "0.#"),
        N(s.ReasonSeconds, "0.#"),
        N(s.RawTemp, "0.0"),
        N(s.EffectiveTemp, "0.0"),
        N(s.TrendTempC, "0.0"),
        N(s.Rpm, "0"),
        N(s.Watts, "0.0"),
        N(s.WattsAvg, "0.0"),
        N(s.BudgetJoules / 1000, "0.00"),
        N(s.TauSeconds, "0"),
        N(s.DemandLevel, "0.#"),
        N(s.CeilingC, "0.#"),
        N(s.AimC, "0.#"),
        N(s.SlopeCPerSec, "0.####"),
        N(s.BaseTempC, "0.0"),
        N(s.ResistanceCPerW, "0.###"),
        N(s.MassJPerC, "0"));

    /// <summary>NaN/null → empty cell, ∞ → "inf", else invariant with the given format.</summary>
    private static string N(double? v, string fmt) =>
        v is not double d || double.IsNaN(d) ? "" :
        double.IsInfinity(d) ? "inf" :
        d.ToString(fmt, CultureInfo.InvariantCulture);

    // ---- files ----

    private void RotateCsv(DateTime now)
    {
        if (_csv != null && now.Date == _csvDate) return;
        _csv?.Dispose();
        Directory.CreateDirectory(_dir);
        _csvDate = now.Date;
        string path = Path.Combine(_dir, $"{_csvPrefix}{now:yyyy-MM-dd}.csv");
        bool fresh = !File.Exists(path);
        _csv = new StreamWriter(path, append: true, Bom) { AutoFlush = false };
        if (fresh) _csv.WriteLine(CsvHeader);
        Prune(now);
    }

    private void Prune(DateTime now)
    {
        try
        {
            foreach (var f in Directory.GetFiles(_dir, _csvPrefix + "*.csv"))
                if (now - File.GetLastWriteTime(f) > TimeSpan.FromDays(KeepDays))
                    File.Delete(f);
        }
        catch { /* a locked stale file must not stop today's log */ }
    }

    private void EnsureBehavior()
    {
        if (_behavior != null) return;
        Directory.CreateDirectory(_dir);
        try
        {
            var info = new FileInfo(_behaviorPath);
            if (info.Exists && info.Length > BehaviorMaxBytes)
            {
                string old = Path.ChangeExtension(_behaviorPath, null) + "-old.txt";
                File.Delete(old);
                File.Move(_behaviorPath, old);
            }
        }
        catch { /* rotation is best-effort */ }
        _behavior = new StreamWriter(_behaviorPath, append: true, Bom) { AutoFlush = false };
    }

    private void Guarded(Action write)
    {
        if (_dead) return;
        try { write(); }
        catch { _dead = true; } // logging must never take fan control down
    }

    public void Dispose()
    {
        lock (_lock)
        {
            try { _csv?.Dispose(); } catch { }
            try { _behavior?.Dispose(); } catch { }
            _csv = null;
            _behavior = null;
            _dead = true;
        }
    }
}
