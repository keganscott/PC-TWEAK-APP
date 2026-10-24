<#
Puts a PeakTweaks shortcut, with the PeakTweaks icon, on this PC's desktop.

Run from the repository folder, in a normal PowerShell:
  powershell -ExecutionPolicy Bypass -File scripts\make-desktop-shortcut.ps1
Optionally point it at a specific exe:
  powershell -ExecutionPolicy Bypass -File scripts\make-desktop-shortcut.ps1 -Exe C:\path\to\peaktweaks-tester.exe
#>
param([string]$Exe)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot

if (-not $Exe) {
    # The newest build wins: the tester build first, then a release build.
    $candidates = @(
        (Join-Path $root 'tester-build\peaktweaks-tester.exe'),
        (Join-Path $root 'target\release\peaktweaks.exe')
    ) | Where-Object { Test-Path $_ }
    if (-not $candidates) { throw 'No PeakTweaks exe found. Build it first (scripts\build-tester.ps1), or pass -Exe <path>.' }
    $Exe = $candidates | Sort-Object { (Get-Item $_).LastWriteTime } -Descending | Select-Object -First 1
}
$Exe = (Resolve-Path $Exe).Path

# Keep the icon somewhere stable, so the shortcut keeps it if the repo moves.
$iconDir = Join-Path $env:LOCALAPPDATA 'PeakTweaks'
New-Item -ItemType Directory -Force $iconDir | Out-Null
$icon = Join-Path $iconDir 'PeakTweaks.ico'
Copy-Item (Join-Path $root 'assets\brand\PeakTweaks.ico') $icon -Force

# GetFolderPath follows a Desktop that OneDrive has moved.
$desktop = [Environment]::GetFolderPath('Desktop')
$link = Join-Path $desktop 'PeakTweaks.lnk'
$shell = New-Object -ComObject WScript.Shell
$s = $shell.CreateShortcut($link)
$s.TargetPath = $Exe
$s.WorkingDirectory = Split-Path -Parent $Exe
$s.IconLocation = "$icon,0"
$s.Description = 'PeakTweaks'
$s.Save()

Write-Host "Shortcut created: $link"
Write-Host "  opens: $Exe"
Write-Host "  icon:  $icon"
