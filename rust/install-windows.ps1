# Build and install the Rust app outside any source checkout. Run in an elevated
# PowerShell session as the Windows user who should receive the tray icon.
# The daemon task is elevated; the tray and its UI run at ordinary user level.
param([switch]$SkipBuild)
$ErrorActionPreference = 'Stop'
$root = $PSScriptRoot
$cargo = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'
$buildTarget = Join-Path $root 'target\install'
$source = Join-Path $buildTarget 'release'
$install = Join-Path $env:LOCALAPPDATA 'Programs\FanCurvesRust'
$daemon = Join-Path $install 'fan-daemon.exe'
$tray = Join-Path $install 'fan-tray.exe'
$principal = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Run install-windows.ps1 in an elevated PowerShell session.'
}
if (-not (Test-Path $cargo)) { throw "Cargo was not found at $cargo" }
if (-not (Test-Path "$env:WINDIR\System32\wscript.exe")) {
    throw 'Windows Script Host is required for the hidden daemon launcher.'
}
$logDir = Join-Path $root 'target\install\logs'
New-Item -ItemType Directory -Path $logDir -Force | Out-Null
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$logPath = Join-Path $logDir "install-$stamp.log"
Start-Transcript -Path $logPath -Force | Out-Null
Write-Output "Install log: $logPath"

if (-not $SkipBuild) {
    # A development daemon may be running from target\release, so build into
    # an independent output directory before touching any live process.
    & $cargo build --release -p fan-daemon -p fan-tray -p fan-ui --target-dir $buildTarget --manifest-path (Join-Path $root 'Cargo.toml')
    if ($LASTEXITCODE -ne 0) { throw 'Release build failed; existing installation was not changed.' }
}
foreach ($name in 'fan-daemon.exe','fan-tray.exe','fan-ui.exe') {
    if (-not (Test-Path (Join-Path $source $name))) { throw "Missing build: $name" }
}

$profile = Join-Path $env:APPDATA 'FanCurves\profile.json'
if (Test-Path -LiteralPath $profile -PathType Leaf) {
    $backup = "$profile.before-rust-install-$stamp.bak"
    Copy-Item -LiteralPath $profile -Destination $backup -ErrorAction Stop
    Write-Output "Profile backup: $backup"
}

# The legacy WPF controller also writes motherboard headers. Ask it to hand
# them back to the BIOS before the new daemon starts.
if (Get-Process -Name FanCurves -ErrorAction SilentlyContinue) {
    $signalDir = Join-Path $env:APPDATA 'FanCurves'
    New-Item -ItemType Directory -Path $signalDir -Force | Out-Null
    New-Item -ItemType File -Path (Join-Path $signalDir 'exit.signal') -Force | Out-Null
    for ($i = 0; $i -lt 20 -and (Get-Process -Name FanCurves -ErrorAction SilentlyContinue); $i++) {
        Start-Sleep -Milliseconds 500
    }
    if (Get-Process -Name FanCurves -ErrorAction SilentlyContinue) {
        throw 'The WPF FanCurves controller is still running; refusing to start a second controller.'
    }
}

# A previous task may point into a temporary worktree. Shut down whichever
# daemon owns the production socket before swapping the installed binary.
$sourceDaemon = Join-Path $source 'fan-daemon.exe'
function Invoke-DaemonRequest([string]$request, [switch]$ReturnReply) {
    # Use an explicit native command line: PowerShell 5.1 otherwise strips
    # JSON quotes, and the old CLI exits zero even for an invalid JSON reply.
    $info = New-Object System.Diagnostics.ProcessStartInfo
    $info.FileName = $sourceDaemon
    $info.Arguments = '--send "' + $request.Replace('"', '\"') + '"'
    $info.UseShellExecute = $false
    $info.CreateNoWindow = $true
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true
    $process = [System.Diagnostics.Process]::Start($info)
    $output = $process.StandardOutput.ReadToEndAsync()
    $errors = $process.StandardError.ReadToEndAsync()
    try {
        if (-not $process.WaitForExit(5000)) {
            $process.Kill()
            throw 'Daemon IPC timed out; refusing an uncertain migration.'
        }
        if ($process.ExitCode -ne 0) { return $false }
        $reply = $output.GetAwaiter().GetResult() | ConvertFrom-Json
        if (-not $reply.ok) { throw 'Daemon rejected the migration request.' }
        if ($ReturnReply) { return $reply }
        return $true
    } finally { $process.Dispose() }
}
# Older daemons held edits only in memory. Preserve that live profile, including
# manual header assignments, rather than reverting to stale on-disk defaults.
$liveProfile = Invoke-DaemonRequest '{"cmd":"profile"}' -ReturnReply
if ($liveProfile -and $liveProfile.profile) {
    $json = $liveProfile.profile | ConvertTo-Json -Depth 50
    $profileDirectory = Split-Path -Parent $profile
    New-Item -ItemType Directory -Path $profileDirectory -Force | Out-Null
    [System.IO.File]::WriteAllText($profile, $json, (New-Object System.Text.UTF8Encoding($false)))
    Write-Output 'Preserved the running daemon profile before migration.'
}
$null = Invoke-DaemonRequest '{"cmd":"shutdown"}'
$daemonReachable = $false
for ($i = 0; $i -lt 20; $i++) {
    $daemonReachable = Invoke-DaemonRequest '{"cmd":"ping"}'
    if (-not $daemonReachable) { break }
    Start-Sleep -Milliseconds 500
}
if ($daemonReachable) { throw 'The running daemon did not shut down; refusing to replace binaries.' }

# Stop only GUI processes running from this install directory. A development
# tray or UI in another checkout is left alone.
Get-CimInstance Win32_Process | Where-Object {
    $_.Name -in @('fan-tray.exe','fan-ui.exe') -and
    $_.ExecutablePath -and
    $_.ExecutablePath.StartsWith($install + '\', [StringComparison]::OrdinalIgnoreCase)
} | Sort-Object @{Expression = { if ($_.Name -eq 'fan-tray.exe') { 0 } else { 1 } }} | ForEach-Object {
    $guiProcess = Get-Process -Id $_.ProcessId -ErrorAction SilentlyContinue
    if ($guiProcess) {
        Stop-Process -Id $guiProcess.Id -ErrorAction Stop
        if (-not $guiProcess.WaitForExit(10000)) {
            throw "UI process $($guiProcess.Id) did not exit; refusing to replace its executable."
        }
    }
}

New-Item -ItemType Directory -Path $install -Force | Out-Null
foreach ($name in 'fan-daemon.exe','fan-tray.exe','fan-ui.exe') {
    for ($copyAttempt = 0; $copyAttempt -lt 20; $copyAttempt++) {
        try {
            Copy-Item -LiteralPath (Join-Path $source $name) -Destination (Join-Path $install $name) -Force
            break
        } catch [System.IO.IOException] {
            if ($copyAttempt -eq 19) { throw }
            Start-Sleep -Milliseconds 250
        }
    }
}
Copy-Item -LiteralPath (Join-Path $root 'start-daemon.ps1') -Destination (Join-Path $install 'start-daemon.ps1') -Force
Copy-Item -LiteralPath (Join-Path $root 'start-daemon.vbs') -Destination (Join-Path $install 'start-daemon.vbs') -Force
Copy-Item -LiteralPath (Join-Path $root 'register-autostart.ps1') -Destination (Join-Path $install 'register-autostart.ps1') -Force

# Registration uses the installed copy. This replaces a task that captured an
# executable path under .claude\worktrees or rust\target\release.
& $daemon --install-autostart
if ($LASTEXITCODE -ne 0) { throw 'Could not register FanCurvesDaemon.' }

# Remove a Run entry left by a prior installer revision, if present.
$runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
Remove-ItemProperty -Path $runKey -Name FanCurvesRust -ErrorAction SilentlyContinue

# The former WPF task would write the same fan headers if it remained enabled.
if (Get-ScheduledTask -TaskName FanCurves -ErrorAction SilentlyContinue) {
    Disable-ScheduledTask -TaskName FanCurves | Out-Null
}

# Activate the newly registered task and a tray in this logon session. The UI
# opens only when requested through the tray.
& schtasks.exe /Run /TN FanCurvesDaemon
if ($LASTEXITCODE -ne 0) { throw 'Installed, but the daemon task did not start.' }
Start-ScheduledTask -TaskName FanCurvesTray
Write-Output "Installed Fan Curves at $install; daemon task and tray started."
Stop-Transcript | Out-Null
