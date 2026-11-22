#!/usr/bin/env bash
# Build the portable game package, then start its bundled-agent or deployment host.
set -euo pipefail

game_dir=$1
repo_dir=$2
port_default=$3
shift 3
cd "$game_dir"

serve=true
bundled_agents=true
for option in "$@"; do
  case "$option" in
    --build-only) serve=false ;;
    --without-bundled-agents) bundled_agents=false ;;
    *) echo "Usage: $0 [--build-only] [--without-bundled-agents]" >&2; exit 2 ;;
  esac
done

export npm_config_cache="${NPM_CACHE:-$repo_dir/.local/npm-cache}"
if [[ -z ${PARLANDO_BIN:-} ]]; then
  run_cli=(cargo run --quiet --manifest-path "$repo_dir/rust-server/Cargo.toml" --bin parlando --)
else
  run_cli=("$PARLANDO_BIN")
fi
if [[ "$serve" == false ]]; then
  exec "${run_cli[@]}" build --manifest "$game_dir/game.yaml"
fi
package=$("${run_cli[@]}" build --manifest "$game_dir/game.yaml" --json | python3 -c 'import json, sys; print(json.load(sys.stdin)["result"]["package"])')
printf 'Built game archive: %s\n' "$package"


mkdir -p .local
host=${HOST:-127.0.0.1}
port=${PORT:-$port_default}
database=${PARLANDO_DATABASE_URL:-sqlite:///$game_dir/.local/component.sqlite}
if [[ "$bundled_agents" == true ]]; then
  # The embedded agent host serves unpacked browser files for its lifetime.
  assets=$(mktemp -d "$game_dir/.local/launch.XXXXXX")
  trap 'rm -rf -- "$assets"' EXIT
  unzip -q "$package" -d "$assets"
  cargo run --quiet --manifest-path "$game_dir/server/Cargo.toml" \
    --features bundled-agents --bin "${game_dir##*/}-with-agents" -- \
    --component "$assets/game.wasm" --client-dist "$assets/client" \
    --database-url "$database" --host "$host" --port "$port"
  exit
fi

# The deployment host stores each pinned experiment in its own SQLite database.
export PARLANDO_DEPLOY_GAME="${game_dir##*/}"
serve_args=(serve --bind "$host:$port" --store "${PARLANDO_STORE:-$game_dir/.local/host}")
printf 'Built %s. Deploy and select it on this host, then open intake.\n' "$package" >&2
if [[ -n ${PARLANDO_PUBLIC_URL:-} ]]; then
  serve_args+=(--public-url "$PARLANDO_PUBLIC_URL")
fi
exec "${run_cli[@]}" "${serve_args[@]}"
