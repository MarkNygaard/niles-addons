# Build the component and copy it to integration.wasm, the name a release
# carries and the file `niles plugin dev .` and `niles plugin check .` run.
# Needs: rustup target add wasm32-wasip2
$ErrorActionPreference = "Stop"
Push-Location $PSScriptRoot
try {
    cargo build --release --target wasm32-wasip2
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    Copy-Item target/wasm32-wasip2/release/niles_bambulab.wasm integration.wasm
    Write-Output "wrote $(Resolve-Path integration.wasm)"
} finally {
    Pop-Location
}
