<#
Runs the core checks of the CI "windows" job on this PC, so they cost no
GitHub Actions minutes. Same order and flags as .github/workflows/ci.yml.
Needs what scripts\build-tester.ps1 needs (Rust, Node, VS Build Tools).

Run from the repository folder, in a normal PowerShell:
  powershell -ExecutionPolicy Bypass -File scripts\check-windows.ps1
Add -Quick to skip the release build.
#>
param([switch]$Quick)
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path -Parent $PSScriptRoot)
$env:RUSTFLAGS = '-D warnings'

function Step([string]$name, [scriptblock]$run) {
    Write-Host "== $name" -ForegroundColor Cyan
    & $run
    if ($LASTEXITCODE -ne 0) { throw "FAILED: $name" }
}

Step 'cargo fmt --check' { cargo fmt --all -- --check }
Step 'cargo clippy'      { cargo clippy --workspace --all-targets -- -D warnings }
Step 'cargo test'        { cargo test --workspace }
Step 'Generated TypeScript is up to date' {
    $changes = git status --porcelain -- src/generated
    if ($changes) { Write-Host $changes; throw "src/generated is stale: commit the regenerated files" }
}
Step 'npm ci'            { npm ci }
Step 'typecheck'         { npm run typecheck }
Step 'UI copy lint'      { npm run test:scripts; if ($LASTEXITCODE -eq 0) { npm run lint:copy } }
Step 'frontend tests'    { npm test }
if (-not $Quick) {
    Step 'tauri build (no bundle)' { npx tauri build --no-bundle }
}
Write-Host 'All checks passed.' -ForegroundColor Green
