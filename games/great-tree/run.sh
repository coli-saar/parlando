#!/usr/bin/env bash
# Builds the client and runs the great-tree game server.
#
# Usage:
#   ./run.sh              # build + run on port 8080
#   ./run.sh --local      # use sibling Rust and JavaScript Parlando packages
#   PORT=3000 ./run.sh    # build + run on a different port
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")"
GREAT_TREE_DIR="$PWD"
PARLANDO_DIR="$(cd ../.. && pwd)"
NPM_CACHE="${NPM_CACHE:-$PARLANDO_DIR/.local/npm-cache}"
LOCAL=false

if [[ "${1:-}" == "--local" ]]; then
  LOCAL=true
  shift
fi
if [[ "$#" -ne 0 ]]; then
  echo "Usage: ./run.sh [--local]" >&2
  exit 2
fi

if [[ "$LOCAL" == true ]]; then
  echo "==> Building local Parlando JavaScript client"
  (cd "$PARLANDO_DIR/js-client" && npm --cache "$NPM_CACHE" install && npm --cache "$NPM_CACHE" run build)

  echo "==> Installing local Parlando JavaScript client into Great Tree"
  (cd client && npm --cache "$NPM_CACHE" install && npm --cache "$NPM_CACHE" install --no-save --package-lock=false "$PARLANDO_DIR/js-client")
else
  echo "==> Installing published client dependencies"
  (cd client && npm --cache "$NPM_CACHE" install)
fi

echo "==> Building client"
(cd client && npm --cache "$NPM_CACHE" run build)

echo "==> Building and starting server"
cd server
PORT="${PORT:-8080}"
if [[ "$LOCAL" == true ]]; then
  cargo install --path "$GREAT_TREE_DIR/server" \
    --root "$PARLANDO_DIR/.local" \
    --force \
    --config "patch.crates-io.parlando.path=\"$PARLANDO_DIR/rust-server\""
  exec "$PARLANDO_DIR/.local/bin/parlando-great-tree" \
    --port "$PORT" --client-dist "$GREAT_TREE_DIR/client/dist"
fi
exec cargo run -- --port "$PORT" --client-dist "$GREAT_TREE_DIR/client/dist"
