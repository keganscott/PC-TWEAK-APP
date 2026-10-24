<#
Downloads the PresentMon release named in vendor/presentmon/PINNED.json and
checks it. Fails (and deletes the file) unless ALL of these hold:
  * the SHA-256 equals the pinned value,
  * the size equals the pinned value,
  * the Authenticode signature is valid and the signer contains the pinned name.
#>
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$dir = Join-Path $root 'vendor\presentmon'
$pin = Get-Content (Join-Path $dir 'PINNED.json') -Raw | ConvertFrom-Json
$dest = Join-Path $dir $pin.file

function Test-Pinned($path) {
    if (-not (Test-Path $path)) { return "missing" }
    if ((Get-Item $path).Length -ne $pin.size) { return "size $((Get-Item $path).Length) is not $($pin.size)" }
    $h = (Get-FileHash $path -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($h -ne $pin.sha256.ToLowerInvariant()) { return "sha256 $h is not $($pin.sha256)" }
    $sig = Get-AuthenticodeSignature $path
    if ($sig.Status -ne 'Valid') { return "Authenticode status is $($sig.Status)" }
    if ($sig.SignerCertificate.Subject -notlike "*$($pin.signer)*") { return "signer is $($sig.SignerCertificate.Subject)" }
    return $null
}

if (Test-Path $dest) {
    $why = Test-Pinned $dest
    if (-not $why) { Write-Host "PresentMon $($pin.version) already present and verified."; return }
    Write-Host "Existing file rejected ($why); downloading again."
    Remove-Item $dest -Force
}

Write-Host "Downloading $($pin.url)"
Invoke-WebRequest -Uri $pin.url -OutFile $dest -UseBasicParsing
$why = Test-Pinned $dest
if ($why) {
    Remove-Item $dest -Force -ErrorAction SilentlyContinue
    throw "PresentMon download rejected: $why"
}
$sig = Get-AuthenticodeSignature $dest
Write-Host "PresentMon $($pin.version) verified: sha256 $($pin.sha256), signed by $($sig.SignerCertificate.Subject)"
