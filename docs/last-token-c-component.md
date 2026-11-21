# Run Last Token, a Parlando game written in C

Last Token is a two-player subtraction game. The session seed selects a pile
of 12 to 15 tokens, and player A moves first. Players alternate taking one to
three tokens. The player who empties the pile wins. The C guest owns the rules
and session state. The stock Parlando host loads the guest and serves the
separately compiled React participant client.

## Build and play

Use the Parlando checkout that defines `parlando:game@4.0.0`. The crates.io
`parlando` 0.6.0 package lacks the component host feature. The browser app uses
the published `@coli-saar/parlando-client` 0.6.0 package.

From the repository root, with Rust, Zig, `wasm-tools` and Node/npm installed:

```sh
cargo install --locked --no-default-features --features c wit-bindgen-cli --version 0.57.1
experiments/c-last-token/build.sh
npm --prefix experiments/c-last-token/client ci
npm --prefix experiments/c-last-token/client run build
cargo build --release --locked --manifest-path rust-server/Cargo.toml \
  --bin parlando
rust-server/target/release/parlando \
  serve --manifest experiments/c-last-token/game.yaml \
  --database-url sqlite:///./experiments/c-last-token/build/last-token.sqlite \
  --client-dist experiments/c-last-token/client/dist
```

Open [the local Parlando dashboard](http://127.0.0.1:8000/admin). Create an
administrator, create an experiment with the default game configuration
(`{}`), and start it for local testing. Open its participant link in two
independent browser contexts and enter the waiting room from both.

## Check the game and reproduce the benchmark

```sh
cargo run --locked --release --target-dir rust-server/target \
  --manifest-path experiments/c-last-token/probe/Cargo.toml -- check
cargo run --locked --release --target-dir rust-server/target \
  --manifest-path experiments/c-last-token/probe/Cargo.toml -- bench
```

The probe loads the actual component with `LoadedGame`. It checks
seeded initialization, both role views, each move size, wrong-turn and
overdraw rejections, unknown action rejection, both winners, terminal
rejection, independent sessions, and replay from a historical state.
The benchmark runs these checks before timing. A separate live smoke test
paired desktop and phone-width Chromium sessions, checked turn gating and
matching pile updates, and verified that both browser sessions displayed
the same winner.

| Operation | Median | Min–max |
| --- | ---: | ---: |
| Load and compile the component in a fresh engine | 3.035 ms | 2.750–3.609 ms |
| Create and initialize a fresh session | 7.822 µs | 7.389–8.799 µs |
| Accept a turn on the current session state | 0.371 µs | 0.366–0.378 µs |

The measurements used an Apple M5 running macOS 27.2, the Rust 1.96.0 release
profile, Wasmtime 36.0.23, Zig 0.16.0, `wit-bindgen` 0.57.1 and
`wasm-tools` 1.253.0. Startup is the median of 21 loads within one process.
The session and turn figures are medians of 21 batch means, after one warmup
batch. Each batch plays 1,000 games from a fixed seed with 12 single-token
moves per game. Timing a full move sequence amortizes the clock overhead.
Turn time includes converting a prepared named action to compact MessagePack,
calling the guest, lifting both views, and retaining recovery history. The
range describes batch means. Individual turn latency can vary within each
batch.

The packaged component occupies **7,388 bytes** and exports the mechanics
interface with zero imports. Compilation is a one-time cost for a running host.
The steady-state measurements describe tiny payloads and trivial rules in one
process. They exclude named observation decoding, network transport, SQLite,
browser rendering, and historical replay. A native baseline or a larger game
would be needed to estimate either the cost of WASM itself or the throughput
of a live experiment.

## How the C guest fits the component interface

`wit-bindgen c` generates C declarations and canonical ABI code from the
host's WIT. Zig supplies a WebAssembly C compiler and libc. The build wraps the
resulting core module with `wasm-tools`, validates the component, and checks
its import list. The generated wrappers handle the ABI and release return
buffers after lifting them into host values. [Bytecode Alliance C component tooling](https://component-model.bytecodealliance.org/language-support/building-a-simple-component/c.html)

The guest declares an action enum with the public tags `take_one`,
`take_two`, and `take_three`. Parlando maps those tags to compact integers
`0`, `1`, and `2`. Each view is a compact array of the remaining count and
the recipient's turn flag. The bounded schema lets the C code encode these
values directly. When changing the schema, update the encoder and check the
named payloads through the host probe. [Parlando component format](component-games.md)

The pile and whose turn it is can be reconstructed from the seed and the
accepted moves. The guest therefore emits no checkpoints. Recovery replays at
most 15 accepted moves in this game. Longer games would incur a replay cost
that grows with the action history.
