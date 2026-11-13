# Build Fan Curves and install it outside the source checkout, then (re)start it.
#
#   First install:  run from an ELEVATED PowerShell. Registering the two logon
#                   tasks (hidden elevated daemon, ordinary-privilege tray)
#                   needs administrator.
#   Update:         no elevation needed. The tasks already point at the install
#                   directory, so the script only swaps the files and restarts
#                   the tasks.
#
# Between the daemon's shutdown and its restart the fans are on the BIOS curve,
# the same safe state as Pause. The profile in %APPDATA%\FanCurves is not touched.
param([switch]$SkipBuild)
$ErrorActionPreference = 'Stop'

$repo = Split-Path -Parent $PSScriptRoot
$cargo = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'
$built = Join-Path $repo 'target\release'
$install = Join-Path $env:LOCALAPPDATA 'Programs\FanCurvesRust'
$binaries = 'fan-daemon.exe', 'fan-tray.exe', 'fan-ui.exe'
$launchers = 'register-autostart.ps1', 'start-daemon.vbs'
$tasks = 'FanCurvesDaemon', 'FanCurvesTray'

# True when both logon tasks exist and start something inside $install.
function Test-TasksRegistered {
    foreach ($name in $tasks) {
        $task = Get-ScheduledTask -TaskName $name -ErrorAction SilentlyContinue
        if (-not $task) { return $false }
        $action = $task.Actions | Select-Object -First 1
        $commandLine = "$($action.Execute) $($action.Arguments)"
        if ($commandLine.IndexOf($install + '\', [StringComparison]::OrdinalIgnoreCase) -lt 0) {
            return $false
        }
    }
    return $true
}

# One IPC command to the running daemon; true when a daemon answered.
# Start-Process keeps the client's stderr away from PowerShell 5.1, which
# turns redirected native stderr into terminating errors.
function Send-Daemon([string]$client, [string]$command) {
    $process = Start-Process -FilePath $client -ArgumentList '--send', $command `
        -WindowStyle Hidden -Wait -PassThru
    return $process.ExitCode -eq 0
}

$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$elevated = ([Security.Principal.WindowsPrincipal]$identity).IsInRole(
    [Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $elevated -and -not (Test-TasksRegistered)) {
    throw 'The logon tasks are not registered yet. Run this script once from an elevated PowerShell.'
}

if (-not $SkipBuild) {
    if (-not (Test-Path $cargo)) { throw "Cargo was not found at $cargo" }
    & $cargo build --release --manifest-path (Join-Path $repo 'Cargo.toml')
    if ($LASTEXITCODE -ne 0) { throw 'Release build failed; the installation was not touched.' }
}
foreach ($name in $binaries) {
    if (-not (Test-Path (Join-Path $built $name))) { throw "Missing build output: $name" }
}

# Stop the running daemon over IPC so it hands the fans back to the BIOS
# before its executable is replaced.
$client = Join-Path $built 'fan-daemon.exe'
if (Send-Daemon $client 'ping') {
    $null = Send-Daemon $client 'shutdown'
    $running = $true
    for ($i = 0; $i -lt 20 -and $running; $i++) {
        Start-Sleep -Milliseconds 500
        $running = Send-Daemon $client 'ping'
    }
    if ($running) { throw 'The running daemon did not shut down; nothing was replaced.' }
}

# Stop the tray and UI, but only the ones running from the install directory.
Get-CimInstance Win32_Process | Where-Object {
    $_.Name -in @('fan-tray.exe', 'fan-ui.exe') -and
    $_.ExecutablePath -and
    $_.ExecutablePath.StartsWith($install + '\', [StringComparison]::OrdinalIgnoreCase)
} | ForEach-Object {
    $process = Get-Process -Id $_.ProcessId -ErrorAction SilentlyContinue
    if ($process) {
        Stop-Process -Id $process.Id
        if (-not $process.WaitForExit(10000)) {
            throw "$($_.Name) (pid $($process.Id)) did not exit; nothing was replaced."
        }
    }
}

New-Item -ItemType Directory -Path $install -Force | Out-Null
$files = @($binaries | ForEach-Object { Join-Path $built $_ }) +
         @($launchers | ForEach-Object { Join-Path $PSScriptRoot $_ })
foreach ($file in $files) {
    # A just-exited process can hold its image locked for a moment.
    for ($attempt = 1; ; $attempt++) {
        try {
            Copy-Item -LiteralPath $file -Destination $install -Force
            break
        } catch [System.IO.IOException] {
            if ($attempt -ge 20) { throw }
            Start-Sleep -Milliseconds 250
        }
    }
}

$daemon = Join-Path $install 'fan-daemon.exe'
if ($elevated) {
    & $daemon --install-autostart
    if ($LASTEXITCODE -ne 0) { throw 'Could not register the logon tasks.' }
}

foreach ($name in $tasks) {
    & schtasks.exe /Run /TN $name | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "Installed, but the $name task did not start." }
}

# The install is only done once the new daemon answers from real hardware.
$up = $false
for ($i = 0; $i -lt 40 -and -not $up; $i++) {
    Start-Sleep -Milliseconds 500
    $up = Send-Daemon $daemon 'ping'
}
if (-not $up) { throw 'Installed, but the daemon did not come up. Run fan-daemon.exe from an elevated shell to see why.' }
$ping = & $daemon --send ping | ConvertFrom-Json
if ($ping.simulated) { throw 'A simulated daemon answered on the production socket; the hardware daemon is not running.' }
Write-Output "Fan Curves $($ping.version) installed at $install"
Write-Output "Backend: $($ping.backend)"
