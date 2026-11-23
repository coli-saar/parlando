# Build helpers shared by browser launchers and headless experiments.
repo_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
export npm_config_cache="${NPM_CACHE:-$repo_dir/.local/npm-cache}"
if [[ -z ${PARLANDO_BIN:-} ]]; then
  run_cli=(cargo run --quiet --manifest-path "$repo_dir/rust-server/Cargo.toml" --bin parlando --)
else
  run_cli=("$PARLANDO_BIN")
fi

# Set package to the built game ZIP and optionally build its agents against that exact archive.
build_packages() {
  local game_dir=$1 include_agents=$2 manifest
  package=$("${run_cli[@]}" build --manifest "$game_dir/game.yaml" --json | python3 -c 'import json, sys; print(json.load(sys.stdin)["result"]["package"])')
  printf 'Built game archive: %s\n' "$package"
  if [[ "$include_agents" == true ]]; then
    for manifest in "$game_dir/agent.yaml" "$game_dir"/agents/*/agent.yaml; do
      [[ -f "$manifest" ]] || continue
      "${run_cli[@]}" agent set-game "$package" --manifest "$manifest"
      "${run_cli[@]}" build --manifest "$manifest"
    done
  fi
}
