#!/bin/sh
# Build the component and copy it to integration.wasm, the name a release
# carries and the file `niles plugin dev .` and `niles plugin check .` run.
# Needs: rustup target add wasm32-wasip2
set -eu
cd "$(dirname "$0")"
cargo build --release --target wasm32-wasip2
cp target/wasm32-wasip2/release/niles_bambulab.wasm integration.wasm
echo "wrote $(pwd)/integration.wasm"
