#!/usr/bin/env bash
# Upload the packages from the last build to a running Parlando server.
set -euo pipefail
project="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
exec python3 "$project/../../games/support/packages.py" "$project" deploy "$@"
