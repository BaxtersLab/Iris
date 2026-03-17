param()

set -e

Write-Output "Building release binaries..."
Push-Location "$(Resolve-Path ..)\Iris"

cargo build -p iris-ui --release

Pop-Location

$outdir = "ci-artifacts/phase-6"
if (-not (Test-Path $outdir)) { New-Item -ItemType Directory -Path $outdir | Out-Null }

$exe = "target\release\iris-ui.exe"
if (-not (Test-Path $exe)) { throw "Built executable not found: $exe" }

$zip = Join-Path $outdir "iris-ui-windows.zip"
if (Test-Path $zip) { Remove-Item $zip }

Write-Output "Packaging $exe -> $zip"
Compress-Archive -Path $exe -DestinationPath $zip

Write-Output "Package created: $zip"
