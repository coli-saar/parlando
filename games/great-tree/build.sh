#!/usr/bin/env bash
# Build the game and its agent archives.
set -euo pipefail
project="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
exec python3 "$project/../support/packages.py" "$project" build "$@"
