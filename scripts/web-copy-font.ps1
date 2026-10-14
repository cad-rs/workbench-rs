# Copy a local CJK font as the web demos' fallback font (not committed: font
# files are machine-specific and license-restricted; the frontends' bundled
# fonts have no CJK glyphs, so without this Chinese text shows as boxes on web).
# Usage: powershell -ExecutionPolicy Bypass -File scripts\web-copy-font.ps1 [fontPath]
param([string]$fontPath)

if (-not $fontPath) {
    $candidates = @(
        "C:\Windows\Fonts\msyh.ttc",
        "C:\Windows\Fonts\msyh.ttf",
        "C:\Windows\Fonts\simhei.ttf",
        "C:\Windows\Fonts\simsun.ttc"
    )
    $fontPath = $candidates | Where-Object { Test-Path $_ } | Select-Object -First 1
}

if (-not $fontPath -or -not (Test-Path $fontPath)) {
    Write-Error "No CJK font found. Pass one explicitly: web-copy-font.ps1 C:\path\to\font.ttf"
    exit 1
}

$products = @("product-web")
foreach ($product in $products) {
    $destDir = Join-Path $PSScriptRoot "..\products\$product\assets\fonts"
    New-Item -ItemType Directory -Force -Path $destDir | Out-Null
    Copy-Item $fontPath (Join-Path $destDir "cjk.ttf") -Force
    Write-Host "Copied $fontPath -> products/$product/assets/fonts/cjk.ttf"
}
Write-Host "The font is served by trunk (copy-dir) and fetched by the web entries at startup."
