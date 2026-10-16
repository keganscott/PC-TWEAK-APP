<#
Builds the PeakTweaks tester build (docs/TEST-ON-YOUR-PC.md) on this PC, for
when GitHub Actions cannot build it. Writes:
  tester-build\peaktweaks-tester.exe        the app, labelled "Tester build"
  tester-build\peaktweaks-field-check.exe   the evidence tool (docs/FIELD_CHECK.md)

Needs Rust (rustup), Node.js 22 or newer, and Visual Studio Build Tools with
"Desktop development with C++". The script checks for each and says where to
get what is missing; it installs nothing itself.

Run from the repository folder, in a normal (not administrator) PowerShell:
  powershell -ExecutionPolicy Bypass -File scripts\build-tester.ps1
#>
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$script:missing = $false
function Need([bool]$ok, [string]$what, [string]$how) {
    if ($ok) {
        Write-Host "ok       $what"
    } else {
        Write-Host "MISSING  $what" -ForegroundColor Red
        Write-Host "         $how"
        $script:missing = $true
    }
}

Write-Host '== Checking what the build needs'
Need ([bool](Get-Command cargo -ErrorAction SilentlyContinue)) 'Rust (cargo)' `
    'Install it from https://rustup.rs with the default options, then open a new PowerShell window.'

$nodeOk = $false
if (Get-Command node -ErrorAction SilentlyContinue) {
    $v = (& node --version) -replace '^v', ''
    $nodeOk = ([version]$v).Major -ge 22
}
Need $nodeOk 'Node.js 22 or newer' `
    'Install the LTS version from https://nodejs.org, then open a new PowerShell window.'

$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
$vc = $null
if (Test-Path $vswhere) {
    $vc = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
}
Need ([bool]$vc) 'Visual Studio C++ build tools' `
    'Install "Build Tools for Visual Studio" from https://visualstudio.microsoft.com/downloads/ and tick "Desktop development with C++".'

if ($script:missing) { throw 'Install what is missing above, then run this script again.' }

# Every native command is checked: PowerShell does not stop on a failed exe.
function Run([string]$what, [scriptblock]$cmd) {
    Write-Host "== $what"
    & $cmd
    if ($LASTEXITCODE -ne 0) { throw "$what failed (exit code $LASTEXITCODE)" }
}

# The pinned, Intel-signed PresentMon that the app embeds; refused unless its
# hash, size and signature match vendor\presentmon\PINNED.json.
Write-Host '== PresentMon (pinned; hash and signature checked)'
& (Join-Path $PSScriptRoot 'fetch-presentmon.ps1')

# rust-toolchain.toml pins the compiler; cargo installs it on first use.
Run 'Rust toolchain' { cargo --version }
Run 'npm ci' { npm ci }
Run 'App, tester build' { npx tauri build --no-bundle --features tester }
Run 'Field-check tool' { cargo build --release -p peaktweaks-field-check }

$out = Join-Path $root 'tester-build'
New-Item -ItemType Directory -Force $out | Out-Null
Copy-Item (Join-Path $root 'target\release\peaktweaks.exe') (Join-Path $out 'peaktweaks-tester.exe') -Force
Copy-Item (Join-Path $root 'target\release\peaktweaks-field-check.exe') $out -Force

Write-Host ''
Write-Host 'Built:' -ForegroundColor Green
Get-ChildItem $out -Filter *.exe | ForEach-Object {
    Write-Host ('  {0}  ({1:N1} MB, SHA-256 {2})' -f $_.FullName, ($_.Length / 1MB), (Get-FileHash $_.FullName).Hash)
}
Write-Host 'Next: follow docs\TEST-ON-YOUR-PC.md'
