# Package a Parlando game for Linux

`parlando build` produces a game package containing a manifest, a WebAssembly
component, and any browser assets. The same package runs on any supported
Parlando host. To deploy to a Linux machine without an installed host, bundle the
package with a static Linux build of the `parlando` command.

## Build the deployable directory

On macOS, install Node.js, `wasm-tools`, Zig, and `cargo-zigbuild`. Install
Rust’s component and Linux targets for a 64-bit x86 server.

```sh
rustup target add wasm32-unknown-unknown x86_64-unknown-linux-musl
```

Run the packaging script from a game directory containing `game.yaml`. From the
repository root, this example packages The Great Tree.

```sh
cd games/great-tree
../../scripts/package-linux.sh
```

The script runs the project’s Parlando tests, builds the portable package, and
cross-compiles the generic host for Linux. It prints the finished directory under
`.local/package/`. Use `--target aarch64-unknown-linux-musl` after installing
that Rust target for a 64-bit Arm server. Use `--skip-tests` if you have already
tested the same source and dependencies.

The generic host supports human pairs and remote agents. Game projects that
provide local agent factories can deploy those agents with their documented
agent-enabled launchers or container images.

## Copy and run the deployment

Copy the whole printed directory to a new directory on the Linux machine. It
contains the host in `bin/`, the portable package in `game/`, and the launcher
`run`. On Linux, verify the files and start the game.

```sh
sha256sum --check SHA256SUMS
./run --bind 0.0.0.0:8000 \
  --database-url sqlite:////var/lib/parlando/great-tree-component.sqlite
```

Create the database’s parent directory and give the host user write access
before starting the service. Keep the database outside the copied release so
deploying a new version cannot overwrite research data. Set `--public-url` to
the public HTTPS origin when a reverse proxy terminates HTTPS.

Open `/admin` to configure the game. Use the [deployment guide](running-and-deployment.md)
for administrator setup, backups, and experiment activation.
