#!/usr/bin/env bash
# Build the portable game package, then run it on the shared host.
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
"${run_cli[@]}" build --manifest "$game_dir/game.yaml"
if [[ "$serve" == false ]]; then exit 0; fi

mkdir -p .local
host=${HOST:-127.0.0.1}
port=${PORT:-$port_default}
database=${PARLANDO_DATABASE_URL:-sqlite:///$game_dir/.local/component.sqlite}
package=$game_dir/dist/parlando
if [[ "$bundled_agents" == true ]]; then
  exec cargo run --quiet --manifest-path "$game_dir/server/Cargo.toml" \
    --features bundled-agents --bin "${game_dir##*/}-with-agents" -- \
    --component "$package/game.wasm" --client-dist "$package/client" \
    --database-url "$database" --host "$host" --port "$port"
fi

serve_args=(serve --manifest "$package/game.yaml" --bind "$host:$port" --database-url "$database")
if [[ -n ${PARLANDO_PUBLIC_URL:-} ]]; then
  serve_args+=(--public-url "$PARLANDO_PUBLIC_URL")
fi
exec "${run_cli[@]}" "${serve_args[@]}"
