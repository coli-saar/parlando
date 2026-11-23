#!/usr/bin/env bash
# Build the Cue Choice game ZIP and the independently uploadable dealer agent.
set -euo pipefail
game_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_dir="$(cd "$game_dir/../.." && pwd)"
source "$repo_dir/games/support/packages.sh"
build_packages "$game_dir" true
