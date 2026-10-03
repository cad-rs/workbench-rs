# workbench-rs one-shot verification script
# Usage: powershell -ExecutionPolicy Bypass -File scripts\verify.ps1 [-SkipSmoke]
# Prerequisites: PYO3_PYTHON is configured in .cargo/config.toml; the Python
# interpreter directory must be on PATH (pyo3 loads its DLL at runtime).
param([switch]$SkipSmoke)
$ErrorActionPreference = "Stop"

# Put common Python installations on PATH (this machine: miniforge3; adjust per machine)
$candidate = "C:\ProgramData\miniforge3"
if (Test-Path $candidate) { $env:PATH = "$candidate;$env:PATH" }

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

Step "egui selfcheck (headless)"
Run "cargo run -q -p product-egui-demo -- --selfcheck"

Step "gpui selfcheck (headless)"
Run "cargo run -q -p product-gpui-demo -- --selfcheck"

if (-not $SkipSmoke) {
    Step "egui smoke"
    Run "cargo run -q -p product-egui-demo -- --smoke"
    Step "gpui smoke"
    Run "cargo run -q -p product-gpui-demo -- --smoke"
}

Write-Host ""
Write-Host "=== ALL PASSED ===" -ForegroundColor Green
