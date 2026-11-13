# Register or remove both logon tasks from the stable install directory.
# Called by fan-daemon --install-autostart / --uninstall-autostart.
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('Install', 'Uninstall')]
    [string]$Action
)
$ErrorActionPreference = 'Stop'
$daemonTask = 'FanCurvesDaemon'
$trayTask = 'FanCurvesTray'

if ($Action -eq 'Uninstall') {
    foreach ($name in $daemonTask, $trayTask) {
        if (Get-ScheduledTask -TaskName $name -ErrorAction SilentlyContinue) {
            Unregister-ScheduledTask -TaskName $name -Confirm:$false
        }
    }
    return
}

$daemon = Join-Path $PSScriptRoot 'fan-daemon.exe'
$tray = Join-Path $PSScriptRoot 'fan-tray.exe'
$launcher = Join-Path $PSScriptRoot 'start-daemon.vbs'
foreach ($path in $daemon, $tray, $launcher) {
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        throw "Installed startup file is missing: $path"
    }
}
$userSid = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
$settings = New-ScheduledTaskSettingsSet `
    -ExecutionTimeLimit (New-TimeSpan -Seconds 0) `
    -AllowStartIfOnBatteries `
    -DontStopIfGoingOnBatteries `
    -MultipleInstances IgnoreNew
$trigger = New-ScheduledTaskTrigger -AtLogOn -User $userSid
$daemonAction = New-ScheduledTaskAction -Execute 'wscript.exe' -Argument (
    '//B //Nologo "' + $launcher + '"'
)
$daemonPrincipal = New-ScheduledTaskPrincipal -UserId $userSid -LogonType Interactive -RunLevel Highest
Register-ScheduledTask -TaskName $daemonTask -Action $daemonAction -Trigger $trigger `
    -Principal $daemonPrincipal -Settings $settings -Force | Out-Null

$trayAction = New-ScheduledTaskAction -Execute $tray
$trayPrincipal = New-ScheduledTaskPrincipal -UserId $userSid -LogonType Interactive -RunLevel Limited
Register-ScheduledTask -TaskName $trayTask -Action $trayAction -Trigger $trigger `
    -Principal $trayPrincipal -Settings $settings -Force | Out-Null
