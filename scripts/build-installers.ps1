<#
Builds the PeakTweaks installers (docs/RELEASE.md) and checks each one:
  PeakTweaks-Setup-<version>.exe         the release build
  PeakTweaks-Tester-Setup-<version>.exe  the tester build (every plan unlocked, labelled)

Signing: when AZURE_TENANT_ID, AZURE_CLIENT_ID, AZURE_CLIENT_SECRET,
SIGNING_ENDPOINT, SIGNING_ACCOUNT and SIGNING_PROFILE are all set, Tauri signs
the app exe and the installer with Azure Artifact Signing (trusted-signing-cli).
Otherwise the installers are built unsigned and the script says so.

Each installer is then installed silently, checked (program files, admin
manifest, desktop shortcut, signature) and uninstalled again.
#>
param([Parameter(Mandatory = $true)][string]$Out)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root
New-Item -ItemType Directory -Force $Out | Out-Null

$version = (Get-Content src-tauri\tauri.conf.json -Raw | ConvertFrom-Json).version
$names = 'AZURE_TENANT_ID', 'AZURE_CLIENT_ID', 'AZURE_CLIENT_SECRET', 'SIGNING_ENDPOINT', 'SIGNING_ACCOUNT', 'SIGNING_PROFILE'
$signed = -not ($names | Where-Object { -not [Environment]::GetEnvironmentVariable($_) })

$configArgs = @()
if ($signed) {
    Write-Host '== Signing is configured: installing trusted-signing-cli'
    cargo install trusted-signing-cli --locked
    if ($LASTEXITCODE -ne 0) { throw 'cargo install trusted-signing-cli failed' }
    $cmd = "trusted-signing-cli -e $env:SIGNING_ENDPOINT -a $env:SIGNING_ACCOUNT -c $env:SIGNING_PROFILE -d PeakTweaks %1"
    $cfg = Join-Path $Out 'sign.conf.json'
    @{ bundle = @{ windows = @{ signCommand = $cmd } } } | ConvertTo-Json -Depth 5 | Set-Content $cfg -Encoding utf8
    $configArgs = @('--config', $cfg)
} else {
    Write-Host '== Signing is NOT configured: building unsigned installers (docs/RELEASE.md says how to add signing)'
}

function Build-Installer([string[]]$extra, [string]$name) {
    $bundleDir = Join-Path $root 'target\release\bundle\nsis'
    if (Test-Path $bundleDir) { Remove-Item $bundleDir -Recurse -Force }
    Write-Host "== tauri build $($extra -join ' ')"
    npx tauri build @extra @configArgs
    if ($LASTEXITCODE -ne 0) { throw "tauri build failed for $name" }
    $setup = Get-ChildItem $bundleDir -Filter '*-setup.exe' | Select-Object -First 1
    if (-not $setup) { throw "no NSIS installer was produced for $name" }
    $dest = Join-Path $Out $name
    Copy-Item $setup.FullName $dest -Force
    return $dest
}

function Wait-Until([scriptblock]$cond, [int]$seconds) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while (-not (& $cond) -and $sw.Elapsed.TotalSeconds -lt $seconds) { Start-Sleep -Milliseconds 500 }
    return (& $cond)
}

function Test-Installer([string]$setup) {
    Write-Host "== Checking $(Split-Path -Leaf $setup) ($([math]::Round((Get-Item $setup).Length / 1MB, 1)) MB)"
    $sig = Get-AuthenticodeSignature $setup
    Write-Host "   installer signature: $($sig.Status) $($sig.SignerCertificate.Subject)"
    if ($signed -and $sig.Status -ne 'Valid') { throw "the installer is not validly signed: $($sig.Status)" }

    $dir = Join-Path $env:ProgramFiles 'PeakTweaks'
    $exe = Join-Path $dir 'peaktweaks.exe'
    $p = Start-Process -FilePath $setup -ArgumentList '/S' -PassThru -Wait
    if ($p.ExitCode -ne 0) { throw "the silent install exited with $($p.ExitCode)" }
    if (-not (Wait-Until { Test-Path $exe } 60)) { throw "the install did not create $exe" }
    Write-Host "   installed: $exe"

    $text = [Text.Encoding]::ASCII.GetString([IO.File]::ReadAllBytes($exe))
    if (-not $text.Contains('requireAdministrator')) { throw 'the installed exe does not ask for admin' }
    $exeSig = Get-AuthenticodeSignature $exe
    Write-Host "   app signature: $($exeSig.Status)"
    if ($signed -and $exeSig.Status -ne 'Valid') { throw "the installed exe is not validly signed: $($exeSig.Status)" }

    $links = @(
        (Join-Path ([Environment]::GetFolderPath('CommonDesktopDirectory')) 'PeakTweaks.lnk'),
        (Join-Path ([Environment]::GetFolderPath('Desktop')) 'PeakTweaks.lnk')
    ) | Where-Object { Test-Path $_ }
    if ($links) { Write-Host "   desktop shortcut: $($links -join ', ')" } else { Write-Host '   desktop shortcut: none created by the silent install' }
    $start = Get-ChildItem (Join-Path $env:ProgramData 'Microsoft\Windows\Start Menu\Programs') -Recurse -Filter 'PeakTweaks.lnk' -ErrorAction SilentlyContinue
    Write-Host "   start menu: $(if ($start) { $start.FullName -join ', ' } else { 'none' })"

    $uninstaller = Join-Path $dir 'uninstall.exe'
    if (-not (Test-Path $uninstaller)) { throw "no uninstaller at $uninstaller" }
    Start-Process -FilePath $uninstaller -ArgumentList '/S' -Wait | Out-Null
    if (-not (Wait-Until { -not (Test-Path $exe) } 90)) { throw "uninstall left $exe behind" }
    Write-Host '   uninstalled cleanly'
}

$release = Build-Installer @() "PeakTweaks-Setup-$version.exe"
$tester = Build-Installer @('--features', 'tester') "PeakTweaks-Tester-Setup-$version.exe"
Test-Installer $release
Test-Installer $tester
Remove-Item (Join-Path $Out 'sign.conf.json') -ErrorAction SilentlyContinue
Write-Host "== Installers ready in $Out (signed: $signed)"
