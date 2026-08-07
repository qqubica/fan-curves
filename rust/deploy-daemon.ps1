# Swap the running fan daemon for a freshly built one. Run from an ELEVATED
# shell - the daemon needs administrator for PawnIO / Super I/O access, and a
# non-elevated session cannot start it usefully (it would fall back to
# simulation and control nothing).
#
# Order matters: the daemon must exit BEFORE the build - its exe is locked
# while it runs, and the IPC socket doubles as the single-instance lock.
# Between the shutdown and the start the fans sit on the BIOS curve, the same
# safe state as Pause. The UI and tray are non-elevated and are NOT touched
# here; restart them by hand if they changed too.
$ErrorActionPreference = "Stop"
$root = $PSScriptRoot
$exe = Join-Path $root "target\release\fan-daemon.exe"
$cargo = Join-Path $env:USERPROFILE ".cargo\bin\cargo.exe"

# Ask whichever daemon owns the socket to exit; "could not reach the daemon"
# just means none was running.
if (Test-Path $exe) {
    try { & $exe --send '{"cmd":"shutdown"}' | Out-Null } catch {}
    Start-Sleep -Seconds 3
}

# Old images renamed aside by a previous in-place swap are unlocked now.
Get-ChildItem (Join-Path $root "target\release") -Filter "fan-daemon-old*.exe" -ErrorAction SilentlyContinue |
    ForEach-Object { try { Remove-Item $_.FullName -Force } catch {} }

& $cargo build --release -p fan-daemon --manifest-path (Join-Path $root "Cargo.toml")
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

Start-Process $exe
Write-Output "daemon started: $exe"
