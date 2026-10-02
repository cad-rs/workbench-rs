# workbench-rs 一键验证脚本（阶段 5：自动化）
# 用法：powershell -ExecutionPolicy Bypass -File scripts\verify.ps1 [-SkipSmoke]
# 前提：PYO3_PYTHON 已在 .cargo/config.toml 配置；Python 解释器目录需在 PATH（pyo3 运行时加载 DLL）。
param([switch]$SkipSmoke)
$ErrorActionPreference = "Stop"

# 将常见 Python 安装目录加入 PATH（本机 miniforge3；按机器调整）
$candidate = "C:\ProgramData\miniforge3"
if (Test-Path $candidate) { $env:PATH = "$candidate;$env:PATH" }

function Step($name) { Write-Host "`n=== $name ===" -ForegroundColor Cyan }
function Run([string]$cmdline) {
    cmd /c "$cmdline 2>&1"
    if ($LASTEXITCODE -ne 0) { throw "$cmdline 失败（exit $LASTEXITCODE）" }
}

Step "cargo check"
Run "cargo check --workspace -q"
Write-Host "OK"

Step "cargo test --workspace"
cmd /c "cargo test --workspace 2>&1" | Select-String -Pattern "test result"
if ($LASTEXITCODE -ne 0) { throw "cargo test 失败" }

Step "egui selfcheck（无头）"
Run "cargo run -q -p product-egui-demo -- --selfcheck"

Step "gpui selfcheck（无头）"
Run "cargo run -q -p product-gpui-demo -- --selfcheck"

if (-not $SkipSmoke) {
    Step "egui smoke"
    Run "cargo run -q -p product-egui-demo -- --smoke"
    Step "gpui smoke"
    Run "cargo run -q -p product-gpui-demo -- --smoke"
}

Write-Host ""
Write-Host "=== 全部通过 ===" -ForegroundColor Green
