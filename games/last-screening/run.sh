#!/usr/bin/env bash
# Run this game package and its optional bundled agents locally.
set -euo pipefail
game_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_dir="$(cd "$game_dir/../.." && pwd)"
exec bash "$repo_dir/games/support/run.sh" "$game_dir" "$repo_dir" 8080 "$@"
