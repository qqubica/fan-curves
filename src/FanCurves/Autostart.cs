using System.Diagnostics;
using Microsoft.Win32;

namespace FanCurves;

/// <summary>
/// Start with Windows via a Task Scheduler logon task with highest privileges.
/// The app requires administrator (see app.manifest), and a plain HKCU Run entry
/// cannot launch elevated programs — the scheduled task is the only correct path.
/// </summary>
public static class Autostart
{
    private const string Name = "FanCurves";

    public static void Ensure()
    {
        var exe = Environment.ProcessPath;
        if (exe == null || exe.EndsWith("dotnet.exe", StringComparison.OrdinalIgnoreCase)) return;
        string command = $"\"{exe}\" --tray";

        try
        {
            var psi = new ProcessStartInfo("schtasks",
                $"/Create /F /TN \"{Name}\" /SC ONLOGON /RL HIGHEST /TR \"{command.Replace("\"", "\\\"")}\"")
            {
                CreateNoWindow = true,
                UseShellExecute = false,
            };
            Process.Start(psi)?.WaitForExit(5000);

            // Clean up the Run-key entry older builds may have left behind.
            using var run = Registry.CurrentUser.OpenSubKey(
                @"Software\Microsoft\Windows\CurrentVersion\Run", writable: true);
            run?.DeleteValue(Name, throwOnMissingValue: false);
        }
        catch { /* autostart is best-effort */ }
    }

    public static void Remove()
    {
        try
        {
            var psi = new ProcessStartInfo("schtasks", $"/Delete /F /TN \"{Name}\"")
            {
                CreateNoWindow = true,
                UseShellExecute = false,
            };
            Process.Start(psi)?.WaitForExit(5000);
        }
        catch { /* best-effort */ }
    }
}
