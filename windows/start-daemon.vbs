' The FanCurvesDaemon logon task runs this script, elevated.
'
' Windows Script Host is a GUI-subsystem process, so its hidden Run starts the
' console-subsystem daemon without flashing a console window at logon.
'
' It also restarts the daemon after a crash. A fault inside a driver library
' (nvml.dll after a GPU driver reset, 2026-11-13) kills the process without
' unwinding, so the fan headers stay frozen at the last written duty and
' nothing manages them until someone notices. Exit codes 0 (shutdown over
' IPC), 1 (refused to start) and 2 (bad arguments) are final; anything else
' is a crash and gets a restart, up to five in quick succession.
Const QUICK_SECONDS = 60
Const MAX_QUICK_CRASHES = 5

Set fs = CreateObject("Scripting.FileSystemObject")
Set shell = CreateObject("WScript.Shell")
daemon = fs.BuildPath(fs.GetParentFolderName(WScript.ScriptFullName), "fan-daemon.exe")

quickCrashes = 0
Do
    started = Now
    code = shell.Run(Chr(34) & daemon & Chr(34), 0, True)
    If code = 0 Or code = 1 Or code = 2 Then Exit Do
    If DateDiff("s", started, Now) < QUICK_SECONDS Then
        quickCrashes = quickCrashes + 1
    Else
        quickCrashes = 1
    End If
    If quickCrashes > MAX_QUICK_CRASHES Then Exit Do
    WScript.Sleep 2000
Loop
WScript.Quit code
