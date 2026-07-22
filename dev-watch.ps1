# dev-watch.ps1 — continuous rebuild + relaunch for REAL-hardware development.
#
# Unlike `dotnet watch` (which hard-kills the app, leaving the Super I/O frozen at
# the last written PWM), this uses the app's clean restart cycle:
#   exit.signal -> app exits cleanly (fans back to BIOS) -> dotnet build -> relaunch.
# If the build fails, the app stays stopped and the fans stay under BIOS control.
#
# Run from an ELEVATED terminal (the exe manifest requires admin):
#   .\dev-watch.ps1
# Ctrl+C stops watching and leaves the app running.

$ErrorActionPreference = 'Stop'

$principal = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Write-Host 'Run this from an elevated terminal — the app requires administrator.' -ForegroundColor Red
    exit 1
}

$root    = $PSScriptRoot
$src     = Join-Path $root 'src'
$project = Join-Path $root 'src\FanCurves\FanCurves.csproj'
$exe     = Join-Path $root 'src\FanCurves\bin\Debug\net8.0-windows\FanCurves.exe'
$signal  = Join-Path $env:APPDATA 'FanCurves\exit.signal'

function Get-SourceStamp {
    (Get-ChildItem $src -Recurse -File -Include *.cs, *.xaml, *.csproj, *.manifest |
        Where-Object { $_.FullName -notmatch '\\(bin|obj)\\' } |
        Measure-Object LastWriteTimeUtc -Maximum).Maximum
}

function Stop-App {
    $proc = Get-Process FanCurves -ErrorAction SilentlyContinue
    if (-not $proc) { return }
    New-Item -ItemType File -Force $signal | Out-Null
    try { $proc | Wait-Process -Timeout 8 -ErrorAction Stop } catch {}
    $left = Get-Process FanCurves -ErrorAction SilentlyContinue
    if ($left) {
        Write-Host '  app ignored exit.signal — killing (fans may stay at last PWM!)' -ForegroundColor Yellow
        $left | Stop-Process -Force
    }
}

function Invoke-Cycle {
    Stop-App
    dotnet build $project --nologo -v minimal
    if ($LASTEXITCODE -ne 0) {
        Write-Host '  BUILD FAILED — app stopped, fans under BIOS control. Fix and save to retry.' -ForegroundColor Yellow
        return
    }
    Start-Process $exe
    Write-Host ('  rebuilt and started  [{0:HH:mm:ss}]' -f (Get-Date)) -ForegroundColor Green
}

Write-Host "Watching $src  (Ctrl+C stops watching, leaves the app running)"
$stamp = Get-SourceStamp
Invoke-Cycle

while ($true) {
    Start-Sleep -Seconds 1
    $now = Get-SourceStamp
    if ($now -le $stamp) { continue }
    do { $stamp = $now; Start-Sleep -Seconds 1; $now = Get-SourceStamp } while ($now -gt $stamp)
    Write-Host ('change detected [{0:HH:mm:ss}] — restarting...' -f (Get-Date))
    Invoke-Cycle
}
