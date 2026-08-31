# Cross-compile and package a game for Linux

Parlando's Linux packaging script turns a game checkout into one directory that
can be copied to a server. The directory contains a statically linked Linux
server, the compiled participant client, and a launcher that connects the two.
The target server therefore does not need Docker, Node.js, Rust, Zig, or a
separately installed SQLite library.

Run the script on macOS from the game root. The game root is the directory that
contains `server/Cargo.toml` and `client/package.json`.

## Package a game from the current checkout

Install the build tools and the default Rust target once:

```sh
brew install zig
cargo install cargo-zigbuild
rustup target add x86_64-unknown-linux-musl
```

Then package Great Tree against the sibling `rust-server` and `js-client`
checkouts:

```sh
cd games/great-tree
../../scripts/package-linux.sh --local
```

The script prints the completed package path. For Great Tree, the path is:

```text
games/great-tree/.local/package/parlando-great-tree-x86_64-linux/
```

The directory name is stable: rebuilding replaces that directory only after the
new server and client have built successfully. This makes it suitable as the
source of an `rsync --delete` operation.

## Choose the dependency source

Dependency mode determines whether the package represents the current repository
checkout or only released packages.

Use `--local` while developing Parlando and a game together. In this mode, the
script patches the Rust dependency to the repository's `rust-server` directory,
builds and packs the sibling `js-client`, and installs that package into a
temporary copy of the game client. It does not rewrite the game's manifests or
lockfiles.

Omit `--local` when producing a package entirely from the published dependencies
declared by the game:

```sh
../../scripts/package-linux.sh
```

Published mode is appropriate only when the game's source and declared package
versions agree. Great Tree and Space Game declare the coordinated Parlando 0.4
packages. Published mode for those checkouts therefore requires both 0.4 packages
to be available from their registries; use `--local` while testing an unpublished
Parlando checkout.

## Understand what the script builds

The script uses conventions instead of game-specific configuration. It requires
this layout:

```text
game-root/
├── server/
│   └── Cargo.toml
└── client/
    └── package.json
```

Cargo metadata supplies the package name, version, target directory, and primary
server binary. If the Rust package contains several binaries, the script selects
the first applicable rule:

1. the binary named by `package.default-run`;
2. the binary whose name matches the Cargo package; or
3. the package's only binary.

The build stops with an error if these rules do not identify one binary. Set
`package.default-run` in `server/Cargo.toml` when a game contains several
non-package-named binaries.

Unless `--skip-tests` is present, the script tests the game client and the Rust
server on the Mac before cross-compiling the release server. Local mode also
tests the sibling browser SDK. Host tests check the source logic, while the final
`file` check confirms that the packaged artifact is a static 64-bit Linux ELF for
the requested architecture.

## Inspect the package

A completed package has this structure:

```text
parlando-great-tree-x86_64-linux/
├── bin/
│   └── parlando-great-tree
├── client-dist/
├── run
├── BUILD-INFO
└── SHA256SUMS
```

`run` is an executable, extensionless shell launcher. It locates the package
relative to itself, starts the binary with `--client-dist` pointing at the
packaged browser assets, and forwards every additional argument. For example:

```sh
./run --host 0.0.0.0 --port 8000
```

`BUILD-INFO` records the package and binary names, dependency mode, target,
browser SDK version, build time, tool versions, and detected binary format.
`SHA256SUMS` covers every deployed file except the checksum file itself. Verify a
copied package on Linux with:

```sh
cd /opt/parlando/parlando-great-tree-x86_64-linux
sha256sum --check SHA256SUMS
```

## Copy and run the package

Stop the remote service before updating its directory. Although local publication
replaces the package only after a successful build, `rsync` does not replace the
remote directory atomically. A running process could otherwise serve a mixture
of old and new browser assets.

From the Great Tree game root, copy the package with:

```sh
rsync -a --delete \
  .local/package/parlando-great-tree-x86_64-linux/ \
  server:/opt/parlando/parlando-great-tree-x86_64-linux/
```

Keep mutable data and secrets outside the copied directory. In particular, do
not place the SQLite database or provider credentials under the package path,
because the next `rsync --delete` can remove files that are not part of the local
package. A remote invocation can instead use a persistent data directory:

```sh
PARLANDO_DATABASE_URL=sqlite:////var/lib/parlando/great-tree.sqlite \
  /opt/parlando/parlando-great-tree-x86_64-linux/run \
  --host 0.0.0.0 \
  --port 8000
```

The Linux host needs its normal CA-certificate bundle when the game makes
outbound TLS connections. The package does not contain a service manager,
reverse-proxy configuration, database, or credentials.

## Select another Linux architecture

The default target is `x86_64-unknown-linux-musl`, which produces an
`x86_64-linux` package. ARM64 Linux is also supported:

```sh
rustup target add aarch64-unknown-linux-musl
../../scripts/package-linux.sh \
  --local \
  --target aarch64-unknown-linux-musl
```

This produces a directory whose name ends in `aarch64-linux`. Other Rust targets
are rejected because the script has no corresponding architecture verification
or package naming rule.

## Command reference

```text
path/to/package-linux.sh [--local] [--target TARGET] [--skip-tests]
```

- `--local` uses the sibling `rust-server` and `js-client` checkouts.
- `--target TARGET` selects a supported MUSL target. The default is
  `x86_64-unknown-linux-musl`.
- `--skip-tests` omits all host-side test commands. Use it only when the same
  source and dependency mode have already passed their tests.
- `-h` or `--help` prints the command summary.

The script exits on the first failed prerequisite, dependency installation, test,
build, or binary verification. An npm deprecation warning by itself is not a
failure. In particular, the local browser SDK's test tooling currently reaches
`glob@10.5.0` through the coverage dependency chain; this development dependency
is not copied into `client-dist`. An `npm ERR!` message or a nonzero script exit
indicates an actual packaging failure.
