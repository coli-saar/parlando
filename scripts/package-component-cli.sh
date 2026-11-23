#!/usr/bin/env bash
# Package an already-built native CLI with the matching portable component SDKs.
set -euo pipefail
binary=${1:?usage: package-component-cli.sh BINARY PLATFORM OUTPUT_DIRECTORY [CROSS_BUILD_VERSION]}
platform=${2:?expected platform name}
output_dir=${3:?expected output directory}
repo=$(cd "$(dirname "$0")/.." && pwd)
# A cross-build caller supplies the crate version because the binary runs on another OS.
if [[ $# -eq 4 ]]; then
    version=$4
else
    version=$("$binary" --version)
    version=${version#parlando }
fi
case "$version" in *[!a-zA-Z0-9.+-]*|'') echo "Invalid CLI version" >&2; exit 1;; esac
case "$platform" in *[!a-zA-Z0-9_-]*|'') echo "Invalid platform" >&2; exit 1;; esac
mkdir -p "$output_dir"
output_dir=$(cd "$output_dir" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
sdk="$repo/parlando-game-sdk"
mkdir -p "$work/bundle/share/parlando-game-sdk/derive"
cp "$binary" "$work/bundle/parlando"
cp -R "$sdk"/{Cargo.toml,Cargo.lock,examples,src,wit} "$work/bundle/share/parlando-game-sdk/"
cp -R "$sdk"/derive/{Cargo.toml,src} "$work/bundle/share/parlando-game-sdk/derive/"
python_package=src/parlando_game_sdk
mkdir -p "$work/bundle/share/parlando-python-sdk/$python_package"
cp "$repo/parlando-python-sdk/$python_package/"{*.py,py.typed} \
    "$work/bundle/share/parlando-python-sdk/$python_package/"
archive="parlando-$version-$platform.tar.gz"
tar -czf "$work/$archive" -C "$work/bundle" parlando share
(cd "$work" && shasum -a 256 "$archive" > "$archive.sha256")
mv "$work/$archive" "$work/$archive.sha256" "$output_dir/"
