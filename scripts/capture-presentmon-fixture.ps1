<#
Captures a small REAL PresentMon CSV to commit as a test fixture (NOTES.md N39).

Why it exists: GitHub's Windows runners are headless, so PresentMon never sees a
frame there and PeakTweaks' CSV parser has only ever been checked against the
column names in PresentMon's README. One real file on a real desktop closes that.

Run it on a real Windows PC, in an elevated PowerShell (PresentMon needs admin
or the "Performance Log Users" group), while something is drawing frames in the
foreground: a game, or a video/animation in a browser window.

    .\scripts\capture-presentmon-fixture.ps1                       # busiest presenter
    .\scripts\capture-presentmon-fixture.ps1 -Process msedge.exe   # a named one

It downloads and verifies the pinned PresentMon (same checks as CI), records
for 10 seconds, keeps only the rows of ONE application (nothing about your other
programs is kept), caps the file at 600 rows, and writes
crates\engine\tests\fixtures\presentmon-real.csv. Open that file before you
commit it: the Application column holds the program's exe name.
#>
param(
    [string]$Process,
    [int]$Seconds = 10,
    [int]$MaxRows = 600
)
$ErrorActionPreference = 'Stop'

$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole(
    [Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $isAdmin) { throw "Run this from an elevated (Administrator) PowerShell." }

$root = Split-Path -Parent $PSScriptRoot
& (Join-Path $PSScriptRoot 'fetch-presentmon.ps1')
$pm = Join-Path $root 'vendor\presentmon\PresentMon-x64.exe'
$fixtures = Join-Path $root 'crates\engine\tests\fixtures'
New-Item -ItemType Directory -Force -Path $fixtures | Out-Null
$raw = Join-Path $env:TEMP 'peaktweaks-presentmon-raw.csv'
Remove-Item $raw -ErrorAction SilentlyContinue

Write-Host "Recording $Seconds s. Keep the program you want measured drawing in the foreground."
$pmArgs = @('--output_file', $raw, '--timed', $Seconds, '--terminate_after_timed',
    '--no_console_stats', '--session_name', 'PeakTweaksFixture', '--stop_existing_session')
if ($Process) { $pmArgs += @('--process_name', $Process) }
& $pm @pmArgs | Out-Host

if (-not (Test-Path $raw)) { throw "PresentMon wrote nothing: no program presented frames while it watched." }
$rows = @(Import-Csv $raw)
if ($rows.Count -eq 0) { throw "PresentMon's file has no rows." }

$top = $rows | Group-Object Application | Sort-Object Count -Descending | Select-Object -First 5
Write-Host "Presenters seen:"
$top | ForEach-Object { Write-Host ("  {0,-32} {1} rows" -f $_.Name, $_.Count) }
$app = if ($Process) { $Process } else { $top[0].Name }

# Keep the raw text of the header and of this application's rows, byte for byte.
$lines = Get-Content $raw
$header = $lines[0]
$appCol = ($header -split ',').IndexOf('Application')
if ($appCol -lt 0) { throw "No Application column in: $header" }
$keep = @($lines | Select-Object -Skip 1 | Where-Object { ($_ -split ',')[$appCol] -eq $app } | Select-Object -First $MaxRows)
if ($keep.Count -lt 60) { throw "Only $($keep.Count) rows for $app; record longer or draw more (need 60+)." }

$out = Join-Path $fixtures 'presentmon-real.csv'
Set-Content -Path $out -Value (@($header) + $keep) -Encoding utf8
Write-Host "Wrote $($keep.Count) rows for $app to $out"
Write-Host "Header: $header"
Write-Host "Now run:  cargo test -p peaktweaks-engine a_real_presentmon_file -- --nocapture"
