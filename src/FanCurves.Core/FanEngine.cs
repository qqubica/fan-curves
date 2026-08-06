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
    double ReasonSeconds = 0,  // StepDownHold: time to the step
    // Every assigned header's rpm, in ControlIds order — Rpm above is just the first.
    // A channel driving several headers can have one fan stalled while the rest spin,
    // and only a per-header reading makes that visible.
    IReadOnlyList<double?>? Rpms = null);

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
    private string? _settingsSig;
    private double _sensorHistoryHours = double.NaN; // NaN → applied on the first tick

    public Profile Profile { get; private set; }
    public bool Applying { get; private set; }
    public event Action<IReadOnlyList<ChannelStatus>>? Ticked;
    /// <summary>A tick threw and was skipped (see Tick) — for the event log.</summary>
    public event Action<Exception>? Faulted;

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
            _settingsSig = null;
        }
    }

    /// <summary>
    /// Fingerprint of every setting that shapes the control path: zero snap, per-channel
    /// tuning, the curve and the sensor assignments. When it changes between ticks the
    /// change came from a user edit — a slider, a curve drag, a preset — and every live
    /// filter is told to apply it INSTANTLY: re-evaluate and jump, skipping step-down
    /// holds, hysteresis and the slew glide (Kuba's ask, 2026-07-29 — an edit used to be
    /// felt only after the 25 s hold + ramp). Idle-kick/stop-probe params and UI-only
    /// settings stay out: they need no snap, and a false positive here jumps the fan
    /// for no reason.
    /// </summary>
    private string SettingsSignature()
    {
        var sb = new System.Text.StringBuilder(256);
        var inv = System.Globalization.CultureInfo.InvariantCulture;
        void N(double v) { sb.Append(v.ToString("R", inv)).Append(' '); }
        var p = Profile;
        N(p.ZeroSnapEnabled ? p.ZeroSnapPercent : 0);
        foreach (var ch in p.Channels)
        {
            sb.Append('|').Append(ch.Enabled ? '+' : '-');
            N(p.SafetyFloorEnabled ? ch.MinPercent : 0);
            N(ch.AveragingSeconds); N(ch.HysteresisC);
            N(ch.StepDownHoldSeconds); N(ch.SlewUpPercentPerSec); N(ch.SlewDownPercentPerSec);
            foreach (var pt in ch.Points) { N(pt.TempC); N(pt.Percent); }
            sb.Append('/').Append(string.Join(",", ch.SensorIds));
        }
        return sb.ToString();
    }

    private void StopApplyingControlsNotIn(Profile p)
    {
        var keep = p.Channels.SelectMany(c => c.ControlIds).ToHashSet();
        foreach (var ch in Profile.Channels)
            foreach (var id in ch.ControlIds.Where(id => !keep.Contains(id)))
                _hw.ReleaseControl(id);
    }

    /// <summary>
    /// One tick, guarded. The profile's collections are edited on the UI thread (sensor
    /// and header checkboxes, curve drags) without taking this lock, so a tick can catch
    /// a list mid-edit — "Collection was modified" killed the process twice on
    /// 2026-07-29, which is the worst possible failure: it leaves the Super I/O frozen
    /// at the last written PWM with nothing watching the die. Skipping one tick costs a
    /// second of control; dying costs the machine. The re-arm sits in the finally
    /// because this is a ONE-SHOT timer — an escape before it would stop fan control
    /// for the rest of the session even if the process survived.
    /// </summary>
    private void Tick()
    {
        lock (_lock)
        {
            try { TickCore(); }
            catch (Exception ex) { Faulted?.Invoke(ex); }
            finally { _timer?.Change(Random.Shared.Next(850, 1151), Timeout.Infinite); }
        }
    }

    private void TickCore()
    {
        // Applied here, before Update(), so it runs on the same thread that appends
        // to the library's per-sensor history lists (a UI-thread set would race the
        // append) and so a launch with 0 h never accumulates a single entry.
        if (_sensorHistoryHours != Profile.SensorHistoryHours)
        {
            _sensorHistoryHours = Profile.SensorHistoryHours;
            _hw.SetSensorHistoryWindow(TimeSpan.FromHours(Math.Clamp(_sensorHistoryHours, 0, 24)));
        }
        _hw.Update();
        double now = _clock.Elapsed.TotalSeconds;
        var statuses = new List<ChannelStatus>();

        // A changed fingerprint = a user edit landed since the last tick →
        // apply it on THIS tick, skipping holds and the slew glide.
        string sig = SettingsSignature();
        if (_settingsSig != null && sig != _settingsSig && Profile.InstantApplyEnabled)
            foreach (var f in _filters.Values) f.ApplyNow();
        _settingsSig = sig;

        foreach (var ch in Profile.Channels.ToArray())
        {
            // Snapshot everything the UI thread can edit under our feet (assignment
            // checkboxes, curve drags, undo/redo). Copying is not atomic either, so
            // the guard in Tick is still the backstop — but a snapshot turns the
            // common case from "tick skipped" into "tick runs on last-known lists".
            var sensorIds = ch.SensorIds.ToArray();
            var controlIds = ch.ControlIds.ToArray();
            var points = ch.Points.ToArray();

            double? temp = sensorIds
                .Select(_hw.ReadValue)
                .Where(v => v.HasValue)
                .Select(v => v!.Value)
                .DefaultIfEmpty(double.NaN)
                .Max();
            if (double.IsNaN(temp.Value)) temp = null;

            // The safety floor degrades to "no floor" when the feature is off — a
            // disabled floor must not block a stop, gate the trial stops, or claim
            // the MinFloor why-chip.
            double minPct = Profile.SafetyFloorEnabled ? ch.MinPercent : 0;

            double output = 0;
            bool applied = false;
            var reason = OutputReason.None;
            double targetPct = 0, reasonLevel = 0, reasonSeconds = 0;
            double effectiveTemp = double.NaN;
            if (temp.HasValue)
            {
                var curve = new FanCurve(points);
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

                double filtered = filter.Step(now, temp.Value, curve);
                effectiveTemp = filter.EffectiveTemp;
                output = Math.Max(minPct, filtered);
                targetPct = Math.Max(minPct, filter.TargetLevel);

                // Most specific explanation wins; later checks override earlier ones.
                if (!double.IsNaN(filter.DownHoldRemaining))
                {
                    reason = OutputReason.StepDownHold;
                    reasonLevel = Math.Max(minPct, filter.PendingDownLevel);
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
                if (minPct > 0 && filtered < minPct - 0.01)
                {
                    reason = OutputReason.MinFloor;
                    reasonLevel = filter.CurveLevel;
                }

                if (Applying && ch.Enabled && controlIds.Length > 0)
                {
                    // Trial stop runs BEFORE the kick, so a probe-stopped channel counts
                    // as stopped for the idle kick (same choice as zero snap). Channels
                    // with a safety floor are never trial-stopped — the floor wins.
                    if (Profile.StopProbeEnabled && minPct <= 0)
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
                    foreach (var id in controlIds) _hw.SetControl(id, output);
                    applied = true;
                }
            }
            // While the BIOS (or nothing) drives the fan, the stopped/running clocks must not run.
            if (!applied) { _kicks.Remove(ch); _probes.Remove(ch); }

            var rpms = Array.ConvertAll(controlIds, _hw.ReadControlRpm);
            double? rpm = rpms.Length > 0 ? rpms[0] : null;
            statuses.Add(new ChannelStatus(ch.Name, temp, effectiveTemp, output, rpm, applied,
                targetPct, reason, reasonLevel, reasonSeconds, rpms));
        }

        Ticked?.Invoke(statuses);
    }

    public void Dispose()
    {
        lock (_lock) { _timer?.Dispose(); _timer = null; }
        StopApplying();
    }
}
