#!/usr/bin/env bash
# Build the hotel against a coordinated Parlando source and host its participant client.
set -euo pipefail
game_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_dir="$(cd "$game_dir/../.." && pwd)"
client_dir="$game_dir/client/web"
mode=local
serve=true
rebuild=false
for arg in "$@"; do
  case "$arg" in
    --local) mode=local ;;
    --published) mode=published ;;
    --build-only) serve=false ;;
    --rebuild) rebuild=true ;;
    *) echo "Usage: $0 [--local|--published] [--build-only] [--rebuild]" >&2; exit 2 ;;
  esac
done

export npm_config_cache="${NPM_CACHE:-$repo_dir/.local/npm-cache}"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$game_dir/server/target}"
mkdir -p "$game_dir/.local"
stamp="$game_dir/.local/client-build"

# Hash source and built assets so edits, removed files and a source-mode switch invalidate the bundle.
client_stamp() {
  (
    cd "$client_dir"
    echo "$mode"
    node --version
    npm --version
    paths=(src index.html tsconfig.json vite.config.ts package.json package-lock.json "$game_dir/run.sh")
    if [[ -d public ]]; then paths+=(public); fi
    if [[ -d dist ]]; then paths+=(dist); fi
    if [[ "$mode" == local ]]; then
      paths+=("$repo_dir/js-client/src" "$repo_dir/js-client/tsconfig.json" "$repo_dir/js-client/package.json" "$repo_dir/js-client/package-lock.json")
    fi
    find "${paths[@]}" -type f -exec cksum {} + | LC_ALL=C sort
  ) | shasum -a 256
}

if [[ "$rebuild" == true || ! -s "$client_dir/dist/index.html" || ! -f "$stamp" || "$(client_stamp)" != "$(cat "$stamp")" ]]; then
  echo "Building the hotel browser client with $mode Parlando"
  (cd "$client_dir" && npm ci --no-audit --no-fund)
  if [[ "$mode" == local ]]; then
    (cd "$repo_dir/js-client" && npm ci --no-audit --no-fund && npm run build)
    mv "$client_dir/node_modules/@coli-saar/parlando-client" "$client_dir/node_modules/.registry-parlando-client"
    ln -s "$repo_dir/js-client" "$client_dir/node_modules/@coli-saar/parlando-client"
  fi
  (cd "$client_dir" && npm run build)
  client_stamp > "$stamp"
else
  echo "Using the built hotel browser client ($mode)"
fi

# Cargo reads config from the invocation directory. Keep the repository patch out of published builds.
cargo_options=(--manifest-path "$game_dir/server/Cargo.toml")
if [[ "$mode" == local ]]; then
  cargo_options+=(--features local-replay --config "patch.crates-io.parlando.path=\"$repo_dir/rust-server\"")
fi
(cd / && cargo build "${cargo_options[@]}")

if [[ "$serve" == true ]]; then
  port="${PORT:-8081}"
  cd "$game_dir"
  printf '\nPlay solo: http://127.0.0.1:%s/e/porter/\n' "$port"
  printf 'Counterweight Hotel dashboard: http://127.0.0.1:%s/admin/experiments\n' "$port"
  exec "$CARGO_TARGET_DIR/debug/parlando-counterweight-hotel" \
    --port "$port" --client-dist "$client_dir/dist" \
    --database-url "${PARLANDO_DATABASE_URL:-sqlite:///$game_dir/.local/hotel.sqlite}"
fi
