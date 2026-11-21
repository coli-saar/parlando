#!/usr/bin/env bash
# Build a portable game and pair it with a static Linux copy of the generic host.
set -euo pipefail

# Prints the command-line options for the game project in the current directory.
usage() {
  printf '%s\n' 'Usage: package-linux.sh [--target x86_64-unknown-linux-musl|aarch64-unknown-linux-musl] [--skip-tests]'
}

target=x86_64-unknown-linux-musl
test_game=true
while [[ $# -gt 0 ]]; do
  case "$1" in
    --target)
      [[ $# -gt 1 ]] || { usage >&2; exit 2; }
      target=$2
      shift 2
      ;;
    --skip-tests) test_game=false; shift ;;
    -h|--help) usage; exit 0 ;;
    *) usage >&2; exit 2 ;;
  esac
done

case "$target" in
  x86_64-unknown-linux-musl) platform=x86_64-linux; architecture=x86-64 ;;
  aarch64-unknown-linux-musl) platform=aarch64-linux; architecture="ARM aarch64" ;;
  *) usage >&2; exit 2 ;;
esac

repo_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
game_dir="$PWD"
host_manifest="$repo_dir/rust-server/Cargo.toml"
target_dir="${CARGO_TARGET_DIR:-$repo_dir/rust-server/target}"
export CARGO_TARGET_DIR="$(mkdir -p "$target_dir" && cd "$target_dir" && pwd)"
[[ -f game.yaml ]] || { printf 'Run from a game project containing game.yaml.\n' >&2; exit 2; }

if [[ -n ${PARLANDO_BIN:-} ]]; then
  host_cli=("$PARLANDO_BIN")
else
  host_cli=(cargo run --quiet --locked --manifest-path "$host_manifest" --bin parlando --)
fi
if [[ "$test_game" == true ]]; then
  "${host_cli[@]}" test --manifest "$game_dir/game.yaml"
fi
"${host_cli[@]}" build --manifest "$game_dir/game.yaml"

CARGO_ZIGBUILD_CACHE_DIR="${CARGO_ZIGBUILD_CACHE_DIR:-$CARGO_TARGET_DIR/zigbuild-cache}" \
  ZIG_GLOBAL_CACHE_DIR="${ZIG_GLOBAL_CACHE_DIR:-$CARGO_TARGET_DIR/zig-global-cache}" \
  ZIG_LOCAL_CACHE_DIR="${ZIG_LOCAL_CACHE_DIR:-$CARGO_TARGET_DIR/zig-local-cache}" \
  cargo zigbuild --release --locked --manifest-path "$host_manifest" --bin parlando --target "$target"
host_binary="$CARGO_TARGET_DIR/$target/release/parlando"
description="$(file "$host_binary")"
if [[ "$description" != *"ELF 64-bit"* || "$description" != *"$architecture"* || "$description" != *static* ]]; then
  printf 'Expected a static Linux executable for %s, got %s\n' "$target" "$description" >&2
  exit 1
fi

output="$game_dir/.local/package"
mkdir -p "$output"
stage="$(mktemp -d "$output/.build.XXXXXX")"
# Removes the staging directory if a build or copy fails.
cleanup() { rm -rf -- "$stage"; }
trap cleanup EXIT

mkdir "$stage/bin"
cp "$host_binary" "$stage/bin/parlando"
cp -R "$game_dir/dist/parlando" "$stage/game"
cat >"$stage/run" <<'RUN'
#!/bin/sh
# Starts the bundled game using the generic Linux host.
set -eu
release_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
exec "$release_dir/bin/parlando" serve --manifest "$release_dir/game/game.yaml" "$@"
RUN
chmod 0755 "$stage/run"
(
  cd "$stage"
  find bin game run -type f -exec shasum -a 256 {} + | LC_ALL=C sort > SHA256SUMS
)

# Publish a complete release under its checksum so staging never changes a deployed copy.
fingerprint="$(shasum -a 256 "$stage/SHA256SUMS" | cut -d ' ' -f 1)"
destination="$output/$platform-$fingerprint-${stage##*.}"
mv "$stage" "$destination"
printf '%s\n' "$destination"
