#!/usr/bin/env bash
# Package the Python guest and verify its component import surface.
set -euo pipefail
cd "$(dirname "$0")"
mkdir -p build
componentize-py -d ../../../rust-server/wit -w game componentize \
  --stub-wasi last_token -o build/last-token-python.wasm
wasm-tools validate build/last-token-python.wasm
wasm-tools component wit build/last-token-python.wasm > build/last-token-python.wit
if grep -q '^ *import ' build/last-token-python.wit; then
  cat build/last-token-python.wit >&2
  exit 1
fi

exec python3 "../../../games/support/packages.py" "$PWD" build "$@"
