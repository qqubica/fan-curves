# This script is the hidden, elevated Task Scheduler action. Keep it beside
# fan-daemon.exe in the stable install directory so source worktrees can move.
$ErrorActionPreference = 'Stop'
& (Join-Path $PSScriptRoot 'fan-daemon.exe') --save-profile
exit $LASTEXITCODE
