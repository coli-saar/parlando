#!/bin/sh
# Rebuild the import-free games embedded in the native network test executables.
set -eu
cd "$(dirname "$0")"
cargo build --locked --release --target wasm32-unknown-unknown --target-dir target --lib --example contract
mkdir -p embedded
wasm-tools component new target/wasm32-unknown-unknown/release/examples/contract.wasm -o embedded/contract.wasm
wasm-tools component new target/wasm32-unknown-unknown/release/parlando_stress_fixture.wasm -o embedded/stress.wasm
