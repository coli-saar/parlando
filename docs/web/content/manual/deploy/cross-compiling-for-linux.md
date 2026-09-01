---
title: Cross-compile and package a game for Linux
---

# Cross-compile and package a game for Linux

The Linux packaging script turns a game checkout into one relocatable directory: a statically linked
server, compiled participant assets, a launcher, build metadata, and checksums. The target host does
not need Rust, Node.js, Zig, Docker, or a separately installed SQLite library.

Use this route when you control a Linux host and want an auditable directory rather than a container
image. The package deliberately excludes mutable state, credentials, reverse-proxy configuration,
and service management.

## Satisfy the package convention

Run the script on macOS from a game root with this structure:

```text
game-root/
├── server/
│   └── Cargo.toml
└── client/
    └── package.json
```

Install the build tools and default target once:

```sh
brew install zig
cargo install cargo-zigbuild
rustup target add x86_64-unknown-linux-musl
```

Then run the repository script from the game root:

```sh
path/to/scripts/package-linux.sh
```

The default target is 64-bit x86 Linux. The script tests the browser and server on the Mac, builds
the release package, and verifies that the result is suitable for the target host.

## Choose the dependency source

Published mode uses the Rust and JavaScript versions declared by the game's manifests and lockfiles.
Choose it for a release whose source agrees with published package versions.

`--local` temporarily substitutes the sibling `rust-server` and `js-client` checkouts. Choose it
while developing Parlando and a game together or before coordinated packages have been published.
It does not rewrite the game's manifests or lockfiles.

```sh
path/to/scripts/package-linux.sh --local
```

This choice is part of build provenance. `BUILD-INFO` records the dependency mode, versions, target,
tool versions, and build time.

## Select the server program when the crate contains several

Most games need no extra setting. If the server crate contains several programs, set
`package.default-run` in `Cargo.toml` to the participant-facing server. The package command stops
and tells you to do this when it cannot choose safely.

## Treat the directory as an immutable release

A completed package has this form:

```text
parlando-my-game-x86_64-linux/
├── bin/
│   └── parlando-my-game
├── client-dist/
├── run
├── BUILD-INFO
└── SHA256SUMS
```

The `run` launcher locates the package relative to itself, points the server at `client-dist`, and
forwards additional arguments. `SHA256SUMS` covers every deployed file except the checksum file.
After copying, verify the directory on Linux:

```sh
cd /opt/parlando/parlando-my-game-x86_64-linux
sha256sum --check SHA256SUMS
```

Stop the service before replacing the directory. `rsync --delete` updates files individually rather
than atomically, so a running process could otherwise serve a mixture of old and new assets.

Keep the database and credentials outside the package path. A future `rsync --delete` may remove
anything that is not part of the local package.

```sh
PARLANDO_DATABASE_URL=sqlite:////var/lib/parlando/my-game.sqlite \
  /opt/parlando/parlando-my-game-x86_64-linux/run \
  --host 0.0.0.0 \
  --port 8000
```

The target host still needs its ordinary CA-certificate bundle for outbound TLS.

{{< callout type="example" title="Package the included game" >}}
From the repository root, package the current Great Tree checkout and local Parlando packages:

```sh
cd games/great-tree
../../scripts/package-linux.sh --local
```

The result is
`games/great-tree/.local/package/parlando-great-tree-x86_64-linux/`. This concrete name
illustrates the general package shape above.
{{< /callout >}}

## Select another architecture

ARM64 Linux is also supported:

```sh
rustup target add aarch64-unknown-linux-musl
path/to/scripts/package-linux.sh --target aarch64-unknown-linux-musl
```

Other targets are rejected because the script has no corresponding architecture verification or
package naming rule. `--skip-tests` omits host-side tests; use it only when the same source and
dependency mode have already passed those tests.

After transferring the package, return to the deployment chapter's public-origin, database,
administrator, pilot, export, and restore checks. The next chapter shows the same operating model
on a managed container host: [Deploy on Render](render/).
