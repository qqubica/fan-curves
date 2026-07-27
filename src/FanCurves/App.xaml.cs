using System.IO;
using System.Security.Principal;
using System.Windows;
using System.Windows.Media;
using System.Windows.Media.Imaging;
using FanCurves.Core;

namespace FanCurves;

public partial class App : Application
{
    public static bool IsElevated
    {
        get
        {
            using var identity = WindowsIdentity.GetCurrent();
            return new WindowsPrincipal(identity).IsInRole(WindowsBuiltInRole.Administrator);
        }
    }

    private static bool _shuttingDown;
    public static bool ShuttingDown => _shuttingDown;

    public static void RequestShutdown()
    {
        if (_shuttingDown) return;
        _shuttingDown = true;
        Log("shutdown requested (window closed / tray exit)");
        Current.Shutdown();
    }

    private static System.Threading.Mutex? _instanceMutex;
    private static DateTime _lastUiCrashLog = DateTime.MinValue;

    /// <summary>Per-tick data + behavior-change review log (see TelemetryLog).</summary>
    public static TelemetryLog? Telemetry { get; private set; }

    /// <summary>Lifecycle/crash log → %AppData%\FanCurves\events.txt (best-effort).</summary>
    public static void Log(string message)
    {
        try
        {
            Directory.CreateDirectory(Profile.ConfigDir);
            File.AppendAllText(Path.Combine(Profile.ConfigDir, "events.txt"),
                $"{DateTime.Now:yyyy-MM-dd HH:mm:ss} [{Environment.ProcessId}] {message}{Environment.NewLine}");
        }
        catch { }
    }

    protected override void OnStartup(StartupEventArgs e)
    {
        base.OnStartup(e);
        ShutdownMode = ShutdownMode.OnExplicitShutdown; // the window hides to tray, app lives on

        // A second instance would fight over PWM writes and profile.json — refuse.
        _instanceMutex = new System.Threading.Mutex(true, "FanCurvesSingleInstance", out bool first);
        if (!first)
        {
            Log($"duplicate instance refused (args: {string.Join(" ", e.Args)})");
            Shutdown();
            return;
        }

        Log($"startup (args: {string.Join(" ", e.Args)}, elevated: {IsElevated})");
        AppDomain.CurrentDomain.UnhandledException += (_, ex) =>
            Log($"CRASH (AppDomain): {ex.ExceptionObject}");
        DispatcherUnhandledException += (_, ex) =>
        {
            // A UI fault must not take fan control down with it: a dead process leaves
            // the Super I/O frozen at the last written PWM with nothing watching the
            // die (2026-07-27: a resize-layout crash parked the fans at 0% while a
            // load pushed the CPU through 80°). Log it, swallow it, keep the engine
            // ticking. Repeated faults log at most once every few seconds.
            if ((DateTime.UtcNow - _lastUiCrashLog).TotalSeconds >= 5)
            {
                _lastUiCrashLog = DateTime.UtcNow;
                Log($"CRASH (Dispatcher, suppressed — app kept alive): {ex.Exception}");
            }
            ex.Handled = true;
        };
        AppDomain.CurrentDomain.ProcessExit += (_, _) => Log("process exit");

        bool forceSim = e.Args.Contains("--sim");
        bool devMode = e.Args.Contains("--dev");
        bool trayStart = e.Args.Contains("--tray"); // autostart entry point: hidden, applying
        string? screenshotPath = null;
        double screenshotAfter = 4;   // seconds of ticking before the capture
        int idx = Array.IndexOf(e.Args, "--screenshot");
        if (idx >= 0 && idx + 1 < e.Args.Length) screenshotPath = e.Args[idx + 1];
        // Optional third token: how long to let the sim run first — the history strips
        // need minutes of samples to show anything worth capturing.
        if (screenshotPath != null && idx + 2 < e.Args.Length &&
            double.TryParse(e.Args[idx + 2], System.Globalization.NumberStyles.Float,
                System.Globalization.CultureInfo.InvariantCulture, out var after))
            screenshotAfter = Math.Clamp(after, 1, 1800);

        // Keyed off the flags, not off hw.IsSimulated: an elevated launch whose kernel
        // driver failed also lands on the simulated backend, and that profile — the
        // user's real one — must still save its edits.
        Profile.ReadOnly = forceSim || screenshotPath != null;

        var profile = Profile.LoadOrDefault();

        // Register start-with-Windows, except in dev flows (--sim / --screenshot)
        // or when the user opted out in the close dialog.
        if (!forceSim && screenshotPath == null && profile.AutostartEnabled) Autostart.Ensure();

        IHardwareBackend hw = CreateBackend(forceSim);
        Log($"backend: {hw.Description}{(hw.IsSimulated ? " (SIMULATED)" : "")}");
        DumpSensors(hw);
        AutoAssign(profile, hw);
        var engine = new FanEngine(hw, profile);

        // Review log: per-tick CSV + behavior transitions → %AppData%\FanCurves\logs\.
        // Hooked on the engine's own thread so it keeps recording even if the UI faults.
        Telemetry = new TelemetryLog(Profile.ConfigDir, simulated: Profile.ReadOnly);
        engine.Ticked += statuses => Telemetry?.Record(engine.Profile, statuses);
        AppDomain.CurrentDomain.ProcessExit += (_, _) => Telemetry?.Dispose();

        var win = new MainWindow(hw, engine, profile, devMode);
        MainWindow = win;
        if (!trayStart) win.Show();

        // Update aid: creating this file makes the running elevated instance exit
        // cleanly (fans back to BIOS), so a non-elevated shell can swap in a new build.
        // Ryzen's Tctl/Tdie can read 0 for the first update cycles after the kernel
        // driver loads, so the startup AutoAssign may find no plausible CPU sensor.
        // Keep retrying while any channel is sensor-less; the engine reads the live
        // Profile objects, so an assignment takes effect on its next tick.
        var healTimer = new System.Windows.Threading.DispatcherTimer { Interval = TimeSpan.FromSeconds(3) };
        healTimer.Tick += (_, _) =>
        {
            bool powerAvailable = hw.Sensors.Any(s => s.Kind == "power");
            if (profile.Channels.All(c => c.SensorIds.Count > 0 &&
                    (!powerAvailable || c.PowerSensorIds.Count > 0))) { healTimer.Stop(); return; }
            AssignEmptyChannels(profile, hw);
        };
        healTimer.Start();

        var dumpTimer = new System.Windows.Threading.DispatcherTimer { Interval = TimeSpan.FromSeconds(15) };
        dumpTimer.Tick += (_, _) => { dumpTimer.Stop(); DumpSensors(hw, append: true); };
        dumpTimer.Start();

        string exitSignal = Path.Combine(Profile.ConfigDir, "exit.signal");
        try { File.Delete(exitSignal); } catch { }
        var signalTimer = new System.Windows.Threading.DispatcherTimer { Interval = TimeSpan.FromSeconds(1) };
        signalTimer.Tick += (_, _) =>
        {
            if (!File.Exists(exitSignal)) return;
            try { File.Delete(exitSignal); } catch { }
            Log("exit.signal received");
            win.ForceExit();
        };
        signalTimer.Start();

        if (screenshotPath != null)
        {
            // Dev/verify aid: let the sim engine tick a few times, capture, exit.
            var t = new System.Windows.Threading.DispatcherTimer
                { Interval = TimeSpan.FromSeconds(screenshotAfter) };
            t.Tick += (_, _) =>
            {
                t.Stop();
                CaptureWindow(win, screenshotPath);
                RequestShutdown();
            };
            t.Start();
        }
    }

    private static IHardwareBackend CreateBackend(bool forceSim)
    {
        if (!forceSim && IsElevated)
        {
            try
            {
                // Real hardware wins as long as it reads ANYTHING — real temps with
                // "no fan header" beats pretty fake numbers. Sim only when the
                // driver gives us nothing at all.
                var lhm = new LhmBackend();
                if (lhm.Controls.Count > 0 || lhm.Sensors.Any(s => s.Kind == "temp")) return lhm;
                lhm.Dispose();
            }
            catch { /* driver failed → simulation */ }
        }
        return new SimulatedBackend();
    }

    /// <summary>
    /// Make the profile's assignments valid for THIS backend: drop identifiers the
    /// backend doesn't know (e.g. a profile saved in simulation reopened on real
    /// hardware), then wire any empty channel to plausible sensors/controls.
    /// </summary>
    private static void AutoAssign(Profile profile, IHardwareBackend hw)
    {
        var knownControls = hw.Controls.Select(c => c.Id).ToHashSet();
        bool changed = false;

        foreach (var ch in profile.Channels)
        {
            // Drop sensors this backend doesn't know or that read garbage right now
            // (unknown IDs read null), so a bad pick heals on the next launch.
            changed |= ch.SensorIds.RemoveAll(id => !Plausible(hw.ReadValue(id))) > 0;
            changed |= ch.PowerSensorIds.RemoveAll(id => !PlausibleWatts(hw.ReadValue(id))) > 0;
            changed |= ch.ControlIds.RemoveAll(id => !knownControls.Contains(id)) > 0;

            if (ch.ControlIds.Count == 0)
            {
                bool isCpu = ch.Name.Contains("CPU", StringComparison.OrdinalIgnoreCase);
                // Pump headers are never auto-assigned (slowing a pump is a hazard);
                // the case channel only takes explicitly chassis-named headers —
                // "first header that isn't the CPU's" grabbed a pump once.
                static bool Named(HwControl c, params string[] needles) =>
                    needles.Any(n => c.Name.Contains(n, StringComparison.OrdinalIgnoreCase));
                var control = hw.Controls
                    .Where(c => !Named(c, "Pump"))
                    .FirstOrDefault(c => isCpu ? Named(c, "CPU")
                                               : Named(c, "System", "Case", "Chassis", "CHA"));
                if (control != null) { ch.ControlIds.Add(control.Id); changed = true; }
            }
        }
        if (changed) profile.Save();
        AssignEmptyChannels(profile, hw);
    }

    // Boards expose dead thermistor inputs that report 0 °C (or 127 °C for
    // "invalid") under names like "CPU" — never trust a sensor by name alone.
    private static bool Plausible(double? v) => v is > 5 and < 115;

    // Package power can read 0 W for the first update cycles (like Tctl) — the heal
    // timer keeps retrying until a real draw shows up.
    private static bool PlausibleWatts(double? v) => v is > 0.5 and < 2000;

    // The CPU package sensor is "Package" (AMD) / "CPU Package" (Intel); graphics
    // cards report "GPU Package"/"GPU Power". Matching the GPU marker first keeps
    // a bare "Package" needle from grabbing the graphics card.
    private static string? FindPowerSensor(IHardwareBackend hw, bool gpu)
    {
        var candidates = hw.Sensors.Where(s => s.Kind == "power" &&
            s.Name.Contains("GPU", StringComparison.OrdinalIgnoreCase) == gpu &&
            PlausibleWatts(hw.ReadValue(s.Id))).ToList();
        return (candidates.FirstOrDefault(s => s.Name.Contains("Package", StringComparison.OrdinalIgnoreCase))
            ?? candidates.FirstOrDefault())?.Id;
    }

    // Needle order is a preference ranking: the real CPU die sensor first
    // (AMD "Tctl", Intel "CPU Package"), generic "CPU"-named ones last.
    private static string? FindSensor(IHardwareBackend hw, params string[] needles)
    {
        foreach (var n in needles)
        {
            var hit = hw.Sensors.FirstOrDefault(s => s.Kind == "temp" &&
                s.Name.Contains(n, StringComparison.OrdinalIgnoreCase) &&
                Plausible(hw.ReadValue(s.Id)));
            if (hit != null) return hit.Id;
        }
        return null;
    }

    /// <summary>
    /// Wire sensor-less channels to plausible temps. Runs at startup and from the
    /// heal timer, so it swaps in a fresh list instead of mutating — the engine's
    /// tick thread may be enumerating the old one.
    /// </summary>
    private static void AssignEmptyChannels(Profile profile, IHardwareBackend hw)
    {
        string? cpuTemp = FindSensor(hw, "Tctl", "CPU Package", "CPU");
        string? gpuTemp = FindSensor(hw, "GPU Core", "GPU Hot Spot", "GPU");
        string? cpuPower = FindPowerSensor(hw, gpu: false);
        string? gpuPower = FindPowerSensor(hw, gpu: true);
        bool changed = false;

        foreach (var ch in profile.Channels.Where(c => c.SensorIds.Count == 0))
        {
            bool isCpu = ch.Name.Contains("CPU", StringComparison.OrdinalIgnoreCase);
            var ids = new List<string>();
            if (cpuTemp != null) ids.Add(cpuTemp);
            if (!isCpu && gpuTemp != null && !ids.Contains(gpuTemp)) ids.Add(gpuTemp);
            if (ids.Count > 0) { ch.SensorIds = ids; changed = true; }
        }
        // CPU channel budgets on package power; the case channel on the whole
        // system's draw (CPU + GPU) — that is the heat its fans must exhaust.
        foreach (var ch in profile.Channels.Where(c => c.PowerSensorIds.Count == 0))
        {
            bool isCpu = ch.Name.Contains("CPU", StringComparison.OrdinalIgnoreCase);
            var ids = new List<string>();
            if (cpuPower != null) ids.Add(cpuPower);
            if (!isCpu && gpuPower != null && !ids.Contains(gpuPower)) ids.Add(gpuPower);
            if (ids.Count > 0) { ch.PowerSensorIds = ids; changed = true; }
        }
        if (changed) profile.Save();
    }

    /// <summary>Debug aid: what the backend sees → %AppData%\FanCurves\sensors.txt
    /// (written at launch, appended again after 15 s of engine updates).</summary>
    private static void DumpSensors(IHardwareBackend hw, bool append = false)
    {
        try
        {
            Directory.CreateDirectory(Profile.ConfigDir);
            // Dev flows dump beside the real one: overwriting sensors.txt with the sim's
            // fake IDs throws away the only record of what the machine actually exposes.
            string path = Path.Combine(Profile.ConfigDir,
                Profile.ReadOnly ? "sensors.sim.txt" : "sensors.txt");
            var lines = new[] { $"{DateTime.Now:yyyy-MM-dd HH:mm:ss}  {hw.Description}" }
                .Concat(hw.Sensors.Select(s =>
                    $"{s.Kind,-5} {s.Id,-45} {s.Name,-55} = {hw.ReadValue(s.Id)?.ToString("0.##") ?? "null"}"))
                .Concat(hw.Controls.Select(c => $"ctrl  {c.Id,-45} {c.Name}"));
            if (append) File.AppendAllLines(path, new[] { "" }.Concat(lines));
            else File.WriteAllLines(path, lines);
        }
        catch { /* diagnostics must never block startup */ }
    }

    private static void CaptureWindow(Window win, string path)
    {
        var bmp = new RenderTargetBitmap(
            (int)win.ActualWidth, (int)win.ActualHeight, 96, 96, PixelFormats.Pbgra32);
        bmp.Render(win);
        var enc = new PngBitmapEncoder();
        enc.Frames.Add(BitmapFrame.Create(bmp));
        using var fs = File.Create(path);
        enc.Save(fs);
    }
}
