' Windows Script Host is a GUI-subsystem process. Its hidden Run starts the
' console-subsystem daemon without flashing a console at desktop logon.
Set fs = CreateObject("Scripting.FileSystemObject")
Set shell = CreateObject("WScript.Shell")
daemon = fs.BuildPath(fs.GetParentFolderName(WScript.ScriptFullName), "fan-daemon.exe")
command = Chr(34) & daemon & Chr(34) & " --save-profile"
WScript.Quit shell.Run(command, 0, True)
