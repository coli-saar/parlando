#!/usr/bin/env bash
# Build the C guest against the host's WIT and reject components with imports.
set -euo pipefail
cd "$(dirname "$0")"
mkdir -p build
export ZIG_LOCAL_CACHE_DIR="$PWD/build/zig-local"
export ZIG_GLOBAL_CACHE_DIR="$PWD/build/zig-global"
wit-bindgen c --world game --out-dir build ../../rust-server/wit
zig cc -target wasm32-wasi -mexec-model=reactor -O2 -g0 -std=c11 \
  -Wall -Wextra -Werror -Ibuild -c last-token.c -o build/last-token.o
zig cc -target wasm32-wasi -mexec-model=reactor -O2 -g0 -Wl,--strip-debug \
  build/last-token.o build/game.c build/game_component_type.o -o build/last-token.core.wasm
wasm-tools component new build/last-token.core.wasm -o build/last-token.wasm
wasm-tools validate build/last-token.wasm
wasm-tools component wit build/last-token.wasm > build/last-token.wit
if grep -q '^ *import ' build/last-token.wit; then
  cat build/last-token.wit >&2
  exit 1
fi

source "../../games/support/packages.sh"
build_packages "$PWD" false
