# workbench-rs one-shot verification script
# Usage: powershell -ExecutionPolicy Bypass -File scripts\verify.ps1 [-SkipSmoke]
param([switch]$SkipSmoke)
$ErrorActionPreference = "Stop"

function Step($name) { Write-Host "`n=== $name ===" -ForegroundColor Cyan }
function Run([string]$cmdline) {
    cmd /c "$cmdline 2>&1"
    if ($LASTEXITCODE -ne 0) { throw "$cmdline failed (exit $LASTEXITCODE)" }
}

Step "cargo check"
Run "cargo check --workspace -q"
Write-Host "OK"

Step "cargo test --workspace"
cmd /c "cargo test --workspace 2>&1" | Select-String -Pattern "test result"
if ($LASTEXITCODE -ne 0) { throw "cargo test failed" }

Step "product-demo selfcheck (headless)"
Run "cargo run -q -p product-demo -- --selfcheck"

if (-not $SkipSmoke) {
    Step "product-demo smoke"
    Run "cargo run -q -p product-demo -- --smoke"
}

Write-Host ""
Write-Host "=== ALL PASSED ===" -ForegroundColor Green
