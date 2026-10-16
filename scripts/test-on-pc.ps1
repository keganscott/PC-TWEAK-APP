<#
The remaining real-PC checks in one run, behind one admin prompt. Start it
with scripts\test-on-pc.cmd (double-click), which asks Windows for admin once.

What it does, in order (docs/NOTES.md items in brackets):
  1. Closes PeakTweaks if it is open, then a read-only field check  [N22, N43]
  2. Makes a real restore point with the app's own code           [N24, N26]
     (turns System Protection on for C: if it is off and lifts Windows'
     one-point-per-24-hours limit, a journalled change Undo all reverses)
  3. Records frames from a game you start, with NVIDIA readings     [N33, N34, N39]
  4. Opens PeakTweaks for the checks only a person can do: Gaming Mode,
     Clean memory, Record while I play and the game tools            [N114, N115, N120, N121]
  5. Closes it and takes a second field check, whose journal shows what
     step 4 changed
  6. Asks whether to undo everything PeakTweaks applied (default: no)
  7. Puts everything in one zip on the Desktop

Steps 2 and 6 change the PC; it asks before each. Nothing else changes anything
except what you do yourself in the app in step 4.

It needs peaktweaks-tester.exe and peaktweaks-field-check.exe: from
tester-build\ (scripts\build-tester.ps1), from the folder this script is in,
or from the folder given as -From (an unzipped CI artifact).
#>
param([string]$From)

$ErrorActionPreference = 'Stop'

function Find-Exe([string]$name) {
    $root = Split-Path -Parent $PSScriptRoot
    $places = @()
    if ($From) { $places += $From }
    $places += (Join-Path $root 'tester-build'), $PSScriptRoot, (Join-Path $env:USERPROFILE 'Downloads')
    foreach ($p in $places) {
        $hit = Get-ChildItem -Path $p -Filter $name -Recurse -Depth 2 -ErrorAction SilentlyContinue |
            Sort-Object LastWriteTime -Descending | Select-Object -First 1
        if ($hit) { return $hit.FullName }
    }
    throw "Cannot find $name. Build it with scripts\build-tester.ps1, or pass -From <folder with the downloaded exe>."
}

function Ask([string]$question) {
    $a = Read-Host "$question (yes/no)"
    return $a.Trim() -match '^(y|yes)$'
}

function Stop-App {
    $p = Get-Process -Name 'peaktweaks-tester', 'peaktweaks' -ErrorAction SilentlyContinue
    if ($p) {
        Write-Host 'Closing PeakTweaks so the checks and the app do not write the journal at the same time.'
        $p | Stop-Process -Force
        Start-Sleep -Seconds 2
    }
}

function Field-Check([string]$label, [string[]]$extra) {
    Write-Host ""
    Write-Host "== $label" -ForegroundColor Cyan
    $before = @(Get-ChildItem -Path $out -Directory -Filter 'peaktweaks-field-check-*' | ForEach-Object Name)
    Push-Location $out
    # Windows PowerShell turns a native program's stderr into errors; with
    # 'Stop' the first line would end the whole script.
    $ErrorActionPreference = 'Continue'
    try {
        & $fieldCheck @extra 2>&1 | ForEach-Object { "$_" } | Tee-Object -FilePath (Join-Path $out "$label.log")
        $code = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = 'Stop'
        Pop-Location
    }
    $new = Get-ChildItem -Path $out -Directory -Filter 'peaktweaks-field-check-*' |
        Where-Object { $before -notcontains $_.Name } | Select-Object -First 1
    if ($new) { Rename-Item $new.FullName (Join-Path $out $label) }
    if ($code -ne 0) { Write-Host "  (exit code $code; the log says why)" -ForegroundColor Yellow }
}

$principal = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Start this through scripts\test-on-pc.cmd, which asks Windows for admin once.'
}

$tester = Find-Exe 'peaktweaks-tester.exe'
$fieldCheck = Find-Exe 'peaktweaks-field-check.exe'
$stamp = Get-Date -Format 'yyyy-MM-dd_HHmm'
$out = Join-Path ([Environment]::GetFolderPath('Desktop')) "peaktweaks-pc-test-$stamp"
New-Item -ItemType Directory -Force $out | Out-Null
Start-Transcript -Path (Join-Path $out 'transcript.txt') | Out-Null

Write-Host "PeakTweaks PC test. Results go to $out"
Write-Host "App:         $tester"
Write-Host "Field check: $fieldCheck"

# 1. Read-only snapshot
Stop-App
Field-Check '1-before' @()

# 2. Restore point
Write-Host ""
Write-Host '== 2. Restore point' -ForegroundColor Cyan
Write-Host 'This makes a real restore point: it turns System Protection on for C: if it is off'
Write-Host "and lifts Windows' one-point-per-24-hours limit (Undo all puts that back)."
if (Ask 'Make the restore point now?') {
    Field-Check '2-restore-point' @('--create-restore-point', '--yes')
} else {
    Write-Host 'Skipped.'
}

# 3. Frames from a game
Write-Host ""
Write-Host '== 3. Frames from a game' -ForegroundColor Cyan
$games = 'FortniteClient-Win64-Shipping', 'VALORANT-Win64-Shipping', 'cs2', 'RobloxPlayerBeta', 'r5apex', 'cod',
    'Marvel-Win64-Shipping', 'PioneerGame', 'RocketLeague', 'javaw', 'Minecraft.Windows'
Write-Host 'Start a game and get into a match or a practice area, then come back here.'
Write-Host 'Leave this blank and press Enter to skip.'
$null = Read-Host 'Press Enter when the game is running'
$running = Get-Process -Name $games -ErrorAction SilentlyContinue | Select-Object -First 1
if ($running) {
    $exe = "$($running.ProcessName).exe"
    Write-Host "Found $exe. Switch back to the game within 10 seconds and play for 30 seconds;"
    Write-Host 'this window says when it is done.'
    Start-Sleep -Seconds 10
    Field-Check '3-game-capture' @('--capture', $exe, '--seconds', '30', '--nvml-seconds', '30')
    [console]::beep(880, 300)
} else {
    $name = Read-Host 'No known game is running. Type its exe name (for example game.exe), or press Enter to skip'
    if ($name.Trim()) {
        Write-Host 'Switch to the game within 10 seconds and play for 30 seconds.'
        Start-Sleep -Seconds 10
        Field-Check '3-game-capture' @('--capture', $name.Trim(), '--seconds', '30', '--nvml-seconds', '30')
        [console]::beep(880, 300)
    } else {
        Write-Host 'Skipped.'
    }
}

# 4. In the app
Write-Host ""
Write-Host '== 4. In the app' -ForegroundColor Cyan
Start-Process -FilePath $tester
Write-Host 'PeakTweaks is opening. Please try these and note anything that looks wrong:'
Write-Host '  a. Gaming Mode: the pill at the top of every page. Switch it on, then off.'
Write-Host '  b. Clean memory: Tools > Quick tools. Click it once; check the figures change.'
Write-Host '  c. Games: pick your main game. Its card comes first. Open its tools.'
Write-Host '  d. Record while I play: on that card. Switch it on, play a few minutes, then'
Write-Host '     open Proof and check a sample appears.'
Write-Host '  e. Home > What the scan found: "Other programs" should now list programs.'
$notes = Read-Host 'Type anything that looked wrong (or press Enter), then Enter when done'
Set-Content -Path (Join-Path $out 'notes.txt') -Value $notes -Encoding utf8

# 5. Second snapshot
Stop-App
Field-Check '5-after' @()

# 6. Undo all
Write-Host ""
Write-Host '== 6. Undo' -ForegroundColor Cyan
Write-Host 'Undo all removes every change PeakTweaks has applied on this PC, including ones'
Write-Host 'from before today. It does not delete the restore point.'
if (Ask 'Undo everything PeakTweaks applied?') {
    Field-Check '6-undo-all' @('--revert-all', '--yes')
} else {
    Write-Host 'Left as it is.'
}

Stop-Transcript | Out-Null
$zip = "$out.zip"
Compress-Archive -Path (Join-Path $out '*') -DestinationPath $zip -Force
Write-Host ""
Write-Host "Done. Everything is in $zip" -ForegroundColor Green
Write-Host 'It names your hardware and Windows account, so send it to whoever works on PeakTweaks only.'
Read-Host 'Press Enter to close'
