' The FanCurvesDaemon logon task runs this script. Windows Script Host is a
' GUI-subsystem process, so its hidden Run starts the console-subsystem daemon
' without flashing a console window at logon.
Set fs = CreateObject("Scripting.FileSystemObject")
Set shell = CreateObject("WScript.Shell")
daemon = fs.BuildPath(fs.GetParentFolderName(WScript.ScriptFullName), "fan-daemon.exe")
WScript.Quit shell.Run(Chr(34) & daemon & Chr(34), 0, True)
