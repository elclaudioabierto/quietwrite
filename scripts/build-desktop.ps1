$ErrorActionPreference = "Stop"
$Target = "x86_64-pc-windows-msvc"
# Windows tools cannot safely place Cargo lock files on a WSL UNC filesystem.
$env:CARGO_INCREMENTAL = "0"
$env:CARGO_TARGET_DIR = Join-Path $env:TEMP "quietwrite-target"

rustup target add $Target
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
cargo test
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
cargo build --release --target $Target
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
New-Item -ItemType Directory -Force dist | Out-Null
Copy-Item "$env:CARGO_TARGET_DIR/$Target/release/quietwrite.exe" "dist/quietwrite-0.8.0-windows-x64.exe"
Write-Host "Built dist/quietwrite-0.8.0-windows-x64.exe"
