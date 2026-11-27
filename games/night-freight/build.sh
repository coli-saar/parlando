#!/usr/bin/env bash
# Build the game archive into dist/. Set PARLANDO_BIN to use an installed CLI.
set -euo pipefail
project="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
exec python3 "$project/../support/packages.py" "$project" build "$@"
