#!/usr/bin/env bash
# Build and validate a fresh sdist and wheel for the registry dry run.
set -euo pipefail
python=${1:?expected Python executable with build and twine installed}
source_dir=${2:?expected Python project directory}
output=$(mktemp -d)
trap 'rm -rf "$output"' EXIT
"$python" -m build --outdir "$output" "$source_dir"
"$python" -m twine check --strict "$output"/*.tar.gz "$output"/*.whl
