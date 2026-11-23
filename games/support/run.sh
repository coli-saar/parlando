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

source "$repo_dir/games/support/packages.sh"
build_packages "$game_dir" "$bundled_agents"
if [[ "$serve" == false ]]; then
  exit 0
fi

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

# The deployment host stores game data independently of the embedded agent host.
serve_args=(serve --bind "$host:$port" --store "${PARLANDO_STORE:-$game_dir/.local/host}")
printf 'Built %s. Deploy and activate the game, then configure experiment intake.\n' "$package" >&2
if [[ -n ${PARLANDO_PUBLIC_URL:-} ]]; then
  serve_args+=(--public-url "$PARLANDO_PUBLIC_URL")
fi
exec "${run_cli[@]}" "${serve_args[@]}"
