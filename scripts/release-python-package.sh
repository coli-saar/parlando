#!/usr/bin/env bash
# Build and validate a fresh sdist and wheel, then upload only when requested.
set -euo pipefail
python=${1:?expected Python executable with build and twine installed}
source_dir=${2:?expected Python project directory}
mode=${3:?expected check or upload}
case "$mode" in
    check|upload) ;;
    *) echo "Expected check or upload" >&2; exit 2 ;;
esac
output=$(mktemp -d)
trap 'rm -rf "$output"' EXIT
"$python" -m build --outdir "$output" "$source_dir"
"$python" -m twine check --strict "$output"/*.tar.gz "$output"/*.whl
if [[ $mode == upload ]]; then
    "$python" -m twine upload --repository pypi "$output"/*.tar.gz "$output"/*.whl
fi
