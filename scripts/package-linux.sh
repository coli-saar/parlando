#!/usr/bin/env bash

set -euo pipefail

DEFAULT_TARGET="x86_64-unknown-linux-musl"
target="$DEFAULT_TARGET"
run_tests=true
local_dependencies=false

# Prints the command-line contract for packaging one game from its root directory.
usage() {
  cat <<'EOF'
Usage: path/to/package-linux.sh [--local] [--target TARGET] [--skip-tests]

Build the Parlando game in the current directory and create one rsync-ready
Linux directory beneath .local/package.

Supported targets:
  x86_64-unknown-linux-musl   (default)
  aarch64-unknown-linux-musl

The current directory must contain server/Cargo.toml and client/package.json.
By default the game uses its published Cargo and npm dependencies. Pass --local
to package the sibling rust-server and js-client checkouts instead.
EOF
}

# Reports a packaging error without hiding the command that needs attention.
fail() {
  printf 'package-linux: %s\n' "$*" >&2
  exit 1
}

# Confirms that a required build program is available before changing build output.
require_command() {
  local command_name="$1"
  command -v "$command_name" >/dev/null 2>&1 || fail "missing required command: $command_name"
}

# Removes only the private temporary directory created for this invocation.
cleanup() {
  if [[ -n "${work_dir:-}" && -d "$work_dir" ]]; then
    rm -rf -- "$work_dir"
  fi
}

# Maps a supported Rust target to the stable package suffix and expected file description.
configure_target() {
  case "$target" in
    x86_64-unknown-linux-musl)
      package_platform="x86_64-linux"
      expected_file_architecture="x86-64"
      ;;
    aarch64-unknown-linux-musl)
      package_platform="aarch64-linux"
      expected_file_architecture="ARM aarch64"
      ;;
    *)
      fail "unsupported target '$target'; expected x86_64-unknown-linux-musl or aarch64-unknown-linux-musl"
      ;;
  esac
}

# Reads package and binary facts through Cargo's stable metadata interface.
read_cargo_metadata() {
  local metadata
  metadata=$(
    cd "$cargo_execution_dir"
    cargo metadata \
      --locked \
      --no-deps \
      --format-version 1 \
      --manifest-path "$server_manifest" \
      ${cargo_dependency_args[@]+"${cargo_dependency_args[@]}"}
  )

  local cargo_facts
  cargo_facts=$(
    node -e '
const fs = require("node:fs");
const path = require("node:path");
const manifest = path.resolve(process.argv[1]);
const metadata = JSON.parse(fs.readFileSync(0, "utf8"));
const pkg = metadata.packages.find((candidate) => path.resolve(candidate.manifest_path) === manifest);
if (!pkg) {
  process.stderr.write(`package-linux: cargo metadata did not contain ${manifest}\n`);
  process.exit(1);
}
const binaries = pkg.targets.filter((candidate) => candidate.kind.includes("bin"));
const primary = binaries.find((candidate) => candidate.name === pkg.default_run)
  ?? binaries.find((candidate) => candidate.name === pkg.name)
  ?? (binaries.length === 1 ? binaries[0] : null);
if (!primary) {
  process.stderr.write(
    "package-linux: cannot infer the primary server binary; set package.default-run or name a binary after the package\n",
  );
  process.exit(1);
}
process.stdout.write([pkg.name, pkg.version, primary.name, metadata.target_directory].join("\t"));
    ' "$server_manifest" <<<"$metadata"
  )
  IFS=$'\t' read -r cargo_package_name cargo_package_version server_binary_name cargo_target_directory \
    <<<"$cargo_facts"
  [[ -n "$cargo_package_name" && -n "$cargo_package_version" && -n "$server_binary_name" && -n "$cargo_target_directory" ]] \
    || fail "could not read package facts from cargo metadata"
}

# Builds the sibling browser SDK and returns a package tarball for an exact local override.
build_browser_sdk() {
  printf 'Building local browser SDK...\n'
  npm --cache "$npm_cache" --prefix "$browser_sdk_dir" ci --ignore-scripts
  if [[ "$run_tests" == true ]]; then
    npm --cache "$npm_cache" --prefix "$browser_sdk_dir" test
  fi
  npm --cache "$npm_cache" --prefix "$browser_sdk_dir" run build

  local pack_output
  pack_output=$(
    cd "$browser_sdk_dir"
    npm --cache "$npm_cache" pack \
        --ignore-scripts \
        --json \
        --pack-destination "$work_dir"
  )
  browser_sdk_tarball=$(
    node -e '
      const entries = JSON.parse(process.argv[1]);
      if (!Array.isArray(entries) || entries.length !== 1 || !entries[0].filename) process.exit(1);
      process.stdout.write(entries[0].filename);
    ' "$pack_output"
  ) || fail "npm did not report the browser SDK tarball"
  browser_sdk_tarball="$work_dir/$browser_sdk_tarball"
  [[ -f "$browser_sdk_tarball" ]] || fail "browser SDK package was not created"
}

# Builds an isolated game-client copy against the sibling SDK without changing source manifests.
build_game_client() {
  printf 'Building game client...\n'
  client_build_dir="$work_dir/client-build"
  mkdir -p "$client_build_dir"
  rsync -a \
    --exclude node_modules \
    --exclude dist \
    "$client_dir/" \
    "$client_build_dir/"

  if [[ "$local_dependencies" == true ]]; then
    node - "$client_build_dir/package.json" "$browser_sdk_tarball" <<'NODE'
const fs = require("node:fs");
const packagePath = process.argv[2];
const sdkTarball = process.argv[3];
const manifest = JSON.parse(fs.readFileSync(packagePath, "utf8"));
manifest.dependencies ??= {};
manifest.dependencies["@coli-saar/parlando-client"] = `file:${sdkTarball}`;
fs.writeFileSync(packagePath, `${JSON.stringify(manifest, null, 2)}\n`);
NODE
  fi

  npm --cache "$npm_cache" --prefix "$client_build_dir" install --ignore-scripts
  if [[ "$local_dependencies" == false ]]; then
    local published_sdk_requirement
    published_sdk_requirement=$(node -e '
      const manifest = require(process.argv[1]);
      const requirement = manifest.dependencies?.["@coli-saar/parlando-client"];
      if (!requirement || requirement.startsWith("file:")) process.exit(1);
      process.stdout.write(`@coli-saar/parlando-client@${requirement}`);
    ' "$client_build_dir/package.json") \
      || fail "game client does not declare a published @coli-saar/parlando-client dependency"
    npm --cache "$npm_cache" --prefix "$client_build_dir" install \
      --ignore-scripts \
      --no-save \
      --package-lock=false \
      "$published_sdk_requirement"
  fi
  if [[ "$run_tests" == true ]]; then
    npm --cache "$npm_cache" --prefix "$client_build_dir" test
  fi
  npm --cache "$npm_cache" --prefix "$client_build_dir" run build
  client_dist="$client_build_dir/dist"
  [[ -f "$client_dist/index.html" ]] || fail "client build did not create client/dist/index.html"
}

# Tests the host build and cross-compiles the selected server binary for Linux.
build_server() {
  if [[ "$run_tests" == true ]]; then
    printf 'Testing game server on the host...\n'
    (
      cd "$cargo_execution_dir"
      cargo test \
        --locked \
        --manifest-path "$server_manifest" \
        ${cargo_dependency_args[@]+"${cargo_dependency_args[@]}"}
    )
  fi

  printf 'Cross-compiling %s for %s...\n' "$server_binary_name" "$target"
  (
    cd "$cargo_execution_dir"
    CARGO_ZIGBUILD_CACHE_DIR="$zigbuild_cache" \
      ZIG_GLOBAL_CACHE_DIR="$zig_global_cache" \
      ZIG_LOCAL_CACHE_DIR="$zig_local_cache" \
      cargo zigbuild \
      --release \
      --locked \
      --manifest-path "$server_manifest" \
      --target "$target" \
      --bin "$server_binary_name" \
      ${cargo_dependency_args[@]+"${cargo_dependency_args[@]}"}
  )

  cross_binary="$cargo_target_directory/$target/release/$server_binary_name"
  [[ -x "$cross_binary" ]] || fail "cross-compiled binary is missing: $cross_binary"
}

# Verifies that Cargo produced a static Linux executable for the requested architecture.
verify_cross_binary() {
  binary_description=$(file "$cross_binary")
  [[ "$binary_description" == *"ELF 64-bit"* ]] || fail "cross binary is not a 64-bit ELF executable: $binary_description"
  [[ "$binary_description" == *"$expected_file_architecture"* ]] || fail "cross binary has the wrong architecture: $binary_description"
  [[ "$binary_description" == *"static"* ]] || fail "cross binary is not statically linked: $binary_description"
}

# Writes a relocatable launcher that keeps mutable data outside the packaged release.
write_launcher() {
  cat >"$stage_dir/run" <<EOF
#!/bin/sh
set -eu

release_dir=\$(CDPATH= cd -- "\$(dirname -- "\$0")" && pwd)
exec "\$release_dir/bin/$server_binary_name" \\
  --client-dist "\$release_dir/client-dist" \\
  "\$@"
EOF
  chmod 0755 "$stage_dir/run"
}

# Records enough build context to audit an unpacked deployment directory without source access.
write_build_info() {
  local browser_sdk_version
  browser_sdk_version=$(node -e '
    process.stdout.write(require(process.argv[1]).version);
  ' "$client_build_dir/node_modules/@coli-saar/parlando-client/package.json")
  cat >"$stage_dir/BUILD-INFO" <<EOF
package_name=$cargo_package_name
package_version=$cargo_package_version
binary_name=$server_binary_name
rust_target=$target
package_platform=$package_platform
browser_sdk_version=$browser_sdk_version
dependency_source=$(if [[ "$local_dependencies" == true ]]; then printf local; else printf published; fi)
built_at=$(date -u +%Y-%m-%dT%H:%M:%SZ)
rustc=$(rustc --version)
zig=$(zig version)
node=$(node --version)
npm=$(npm --version)
binary_file=$binary_description
EOF
}

# Produces deterministic checksum ordering for every deployed file except the checksum file itself.
write_checksums() {
  node - "$stage_dir" <<'NODE' >"$stage_dir/SHA256SUMS"
const crypto = require("node:crypto");
const fs = require("node:fs");
const path = require("node:path");
const root = process.argv[2];

function filesBelow(directory) {
  return fs.readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const absolute = path.join(directory, entry.name);
    if (entry.isDirectory()) return filesBelow(absolute);
    return absolute;
  });
}

for (const absolute of filesBelow(root).sort()) {
  const relative = path.relative(root, absolute);
  if (relative === "SHA256SUMS") continue;
  const digest = crypto.createHash("sha256").update(fs.readFileSync(absolute)).digest("hex");
  process.stdout.write(`${digest}  ${relative}\n`);
}
NODE
}

# Assembles and atomically publishes the stable rsync source directory.
publish_package() {
  mkdir -p "$stage_dir/bin" "$stage_dir/client-dist"
  cp "$cross_binary" "$stage_dir/bin/$server_binary_name"
  chmod 0755 "$stage_dir/bin/$server_binary_name"
  cp -R "$client_dist/." "$stage_dir/client-dist/"
  write_launcher
  write_build_info
  write_checksums

  local previous_dir="$work_dir/previous-package"
  if [[ -e "$package_dir" ]]; then
    mv "$package_dir" "$previous_dir"
  fi
  if ! mv "$stage_dir" "$package_dir"; then
    if [[ -e "$previous_dir" ]]; then
      mv "$previous_dir" "$package_dir"
    fi
    fail "could not publish package directory"
  fi
  rm -rf -- "$previous_dir"
}

while [[ "$#" -gt 0 ]]; do
  case "$1" in
    --target)
      [[ "$#" -ge 2 ]] || fail "--target requires a value"
      target="$2"
      shift 2
      ;;
    --skip-tests)
      run_tests=false
      shift
      ;;
    --local)
      local_dependencies=true
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      fail "unknown argument: $1"
      ;;
  esac
done

configure_target

require_command cargo
require_command cargo-zigbuild
require_command file
require_command node
require_command npm
require_command rustc
require_command rustup
require_command rsync
require_command zig

game_dir=$(pwd -P)
server_manifest="$game_dir/server/Cargo.toml"
client_dir="$game_dir/client"

[[ -f "$server_manifest" ]] || fail "run this script from a game root containing server/Cargo.toml"
[[ -f "$client_dir/package.json" ]] || fail "run this script from a game root containing client/package.json"
rustup target list --installed | grep -Fx "$target" >/dev/null \
  || fail "Rust target '$target' is not installed; run: rustup target add $target"

script_dir=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
repository_dir=$(CDPATH= cd -- "$script_dir/.." && pwd)
browser_sdk_dir="$repository_dir/js-client"
[[ -f "$browser_sdk_dir/package.json" ]] || fail "shared browser SDK is missing: $browser_sdk_dir/package.json"
npm_cache="$repository_dir/.local/npm-cache"
mkdir -p "$npm_cache"
zigbuild_cache="$repository_dir/.local/cargo-zigbuild-cache"
mkdir -p "$zigbuild_cache"
zig_global_cache="$repository_dir/.local/zig-global-cache"
zig_local_cache="$repository_dir/.local/zig-local-cache"
mkdir -p "$zig_global_cache" "$zig_local_cache"

# Cargo discovers configuration from its working directory, not from --manifest-path. Running
# outside the checkout prevents the repository's development patch from affecting published mode.
cargo_execution_dir="${TMPDIR:-/tmp}"
cargo_dependency_args=()
if [[ "$local_dependencies" == true ]]; then
  cargo_dependency_args=(--config "patch.crates-io.parlando.path=\"$repository_dir/rust-server\"")
fi

read_cargo_metadata

package_parent="$game_dir/.local/package"
package_name="$server_binary_name-$package_platform"
package_dir="$package_parent/$package_name"
mkdir -p "$package_parent"
work_dir=$(mktemp -d "$package_parent/.package-linux.XXXXXX")
stage_dir="$work_dir/$package_name"
trap cleanup EXIT

if [[ "$local_dependencies" == true ]]; then
  build_browser_sdk
fi
build_game_client
build_server
verify_cross_binary
publish_package

printf '%s\n' "$package_dir"
