#!/usr/bin/env bash
# Builds the client and runs the great-tree game server.
#
# Usage:
#   ./run.sh              # build + run on port 8080
#   PORT=3000 ./run.sh    # build + run on a different port
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")"

echo "==> Building client"
(cd client && npm install && npm run build)

echo "==> Building and starting server"
cd server
PORT="${PORT:-8080}"
exec cargo run -- --port "$PORT" --client-dist ../client/dist
