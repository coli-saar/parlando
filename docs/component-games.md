# Create and run a Parlando game

Implement `Game` for the type that owns one session’s state. The game defines
its rules and each player’s view. Parlando runs it in live sessions and
headless evaluations, and owns participant connections, scheduling and records.

## Move a native Rust game to the Wasm host

Move the state and rules into a game library and implement the guest SDK’s `Game` trait. The instance owns the mutable state formerly passed to each rule call. Put role-specific output in `view_a` and `view_b`, then export the game with `parlando_game_sdk::export!`. A standard host loads the compiled component. Games with compiled Rust policies can use a thin launcher that passes a `LoadedGame` to `Server::new` and registers its agent factories.

Start a new database or hosted experiment for the port. Keep the original executable and database for replaying historical native sessions. Wasm replay requires the matching component digest, recorded initialization and accepted actions or a compatible checkpoint. A historical native state snapshot alone cannot initialize the Wasm guest.

## Create and play a Rust game

Install the CLI from the repository root. Building Rust games also requires
the Wasm target and `wasm-tools`. The generated browser project uses Node and npm.

```sh
cargo install --locked --path rust-server --bin parlando
rustup target add wasm32-unknown-unknown
cargo install --locked wasm-tools
parlando new beacon --lang rust
cd beacon
parlando test
parlando build
parlando serve
```

Open the local dashboard at `http://127.0.0.1:8000/admin`, create an experiment,
open intake and visit the participant URL from two browser profiles. The
starter game, Beacon Relay, asks the two players to take turns adding charge.
Edit `src/lib.rs` for the rules and `client/main.jsx` for the participant view.

When the CLI finds its bundled SDK or the source checkout, `new` copies the
SDK into `vendor/parlando-game-sdk` and records a relative path. The
project can move or build on another machine together with its vendor directory.

The **Parlando Game SDK** crate is `parlando-game-sdk`. Derive `GameValue`
on game-specific types exchanged with the host. This supplies the public schema
and encoder, plus a decoder when the fields support it. A role view can borrow
a slice from session state.

The Rust game owns one session's state. Implement
[`parlando_game_sdk::Game`](../parlando-game-sdk/src/game.rs) and
call `parlando_game_sdk::export!(YourGame)`. The SDK derives the six-part
schema and exports `describe`, `init`, `step` and `restore`. Each role's view
can borrow data from the game during encoding. A successful `step` returns
`Ok(())`. The adapter then reads both views, optional completion and transition
metadata in the same Wasm call. Reject an action before mutating state or
advancing the RNG. An encoding failure after mutation traps and the host can
recover the last committed state. Implement `checkpoint` and `restore` when
you want snapshots in addition to deterministic replay.

`parlando test` runs the native game tests and checks the expectations in
`scenarios.yaml` against the compiled Wasm. The starter scenario checks both
players' views, a rejection, and recovery. Use `parlando check` for a build
and loader check. The deployable output of `parlando build` lives in
`dist/parlando/`. Pass the built manifest to `check` to check those exact bytes.

```sh
parlando check --manifest dist/parlando/game.yaml --json
```

Project commands default to `game.yaml` in the working directory and accept
`--manifest`. Add `--json` to read the result on stdout. Exit status 0 means
success, 1 means an operation failed, and 2 indicates invalid arguments.
See [Host and deploy component games](component-hosting.md) to upload a built
package and select its revision for a live experiment.

## Create and play a Python game

Use the same `parlando` CLI with Python 3.10 or later and Node.js. Start with
the Python version of Beacon Relay. Both players take turns adding one or two
ticks of charge and activate the beacon by reaching its target exactly.

```sh
parlando new beacon --lang python
cd beacon
python3 -m venv .venv
.venv/bin/python -m pip install componentize-py==0.25.1
parlando test
parlando build
parlando serve
```

Open `http://127.0.0.1:8000/admin`, create an experiment, and open intake.
Use its participant link from two browser profiles. Edit `game.py` for the
rules, `client/main.jsx` for the participant view, and `scenarios.yaml` for
the expected behavior of the compiled game. The project includes the Python
SDK under `vendor/parlando-python-sdk` when you scaffold it with a bundled CLI
or a CLI built from the source checkout. The project can move with that directory.

In `game.py`, subclass `parlando_game_sdk.Game` with the configuration,
action, two role views, completion, and metadata types. Declare structured
payloads with dataclasses and bind the game with `Mechanics = mechanics(Beacon)`
at module scope. The SDK derives the wire schema from these types, decodes
the configuration and actions, and encodes the returned views and outcome.
Raise `Rejection` with a stable code before mutating the game. Checkpoints are
optional when the seed and accepted moves suffice for replay.

Dataclass fields travel in declaration order. Use `int` for signed 64-bit
values or `U8` and `U64` for unsigned values with checked bounds. The SDK
also supports finite floats, strings, booleans, options, lists, tuples,
string-keyed dictionaries, and unions of dataclasses marked with `@variant`.
Keep variant order stable because each alternative receives its compact tag
from its position in the union.

`parlando test` runs Python's `unittest` suite in `tests/` and then uses the
same Wasm scenario runner as for Rust games. `parlando build` stages the
component, manifest, and participant assets in `dist/parlando/`. The build
uses `componentize-py --stub-wasi` and the host checks the result with an
empty linker. Keep the game rules deterministic and use the seed provided by
`init` to derive randomness. Import guest dependencies at module scope so
componentize-py can resolve them during packaging. At runtime, WASI calls
through the generated stubs trap.

## Package a component from another language

Compile your game as a WebAssembly **component** that exports the `game` world in
[`parlando:game@4.0.0`](../rust-server/wit/game.wit). The component must have no
host imports. For a component built against an earlier version of the world, regenerate
the guest bindings, implement `describe`, and encode values according to the description before deploying to this host.
Keep the matching host and component together when replaying older sessions.
Place the component binary beside a game manifest such as `game.yaml`.

```yaml
id: my-game
name: My Game
version: 1.0.0
component: my-game.wasm
```

The path in `component` resolves relative to the manifest. The game ID is the
stable key used by experiment definitions, and the semantic version identifies
the rules and data contract. Parlando computes a SHA-256 digest of the loaded
component bytes and exposes it as build provenance. Give a changed component a
new release version before using it for new live experiments.

Select a component package from a source you trust. The host compiles and checks
it at startup, before opening the listener or planning sessions. A missing
component, unresolved import, or incompatible export prevents startup. In local `serve` mode the host loads one package for the life of the process.

Build the host from the repository root.

```sh
cargo build --release --manifest-path rust-server/Cargo.toml \
  --bin parlando
```

## Run a live session

```sh
rust-server/target/release/parlando serve --manifest game.yaml \
  --database-url sqlite:///./my-game.sqlite \
  --client-dist ./client/dist
```

Open `http://127.0.0.1:8000/admin`, create an administrator on a new database,
and create an experiment. Use a separate SQLite database per game. The listener
defaults to localhost and port 8000. Use `--bind` to select another address.
Complete the first administrator setup locally before exposing the service.
Supply the compiled participant browser application through `--client-dist`
when the server should serve those assets. The dashboard also exposes the
`remote_grpc` agent for human–agent sessions.
[Running and deployment](running-and-deployment.md)

## Run a headless evaluation

Start a Parlando gRPC agent service that understands your game's action and
observation objects. Both seats can select the same service. Parlando still
creates an independent agent instance for each seat.

```yaml
schema: parlando-agent-experiment/v1
game:
  id: my-game
agents:
  policy:
    factory: remote_grpc
    settings:
      endpoint: http://127.0.0.1:50051
sessions:
  scenarios:
    - name: baseline
      config: {}
      seeds: [7]
  seats:
    player_a: policy
    player_b: policy
schedule:
  kind: alternate_every_response
  first: player_a
output:
  directory: results/my-game-baseline
  trace: decisions
```

Set `config` to the game configuration your guest expects. Save the experiment as
`evaluation.yaml` and run it against the same package.

```sh
rust-server/target/release/parlando run evaluation.yaml --manifest game.yaml
```

The command prints the runner's summary as JSON when one is available. A runner
error or a summary reporting any newly failed sessions produces a nonzero exit
status. The output directory contains `run.json` and
one JSON result per finalized session. Run the same command to resume. The
runner checks the loaded component's SHA-256 digest against `run.json` before
reusing any results. Resuming skips all finalized sessions, including failures.
Inspect their result files for the original status. For a fresh sample or a
changed component, select a new output directory.

The stock CLI registers the remote agent factory for policy evaluation.
Register game-specific rewards and learners in a Rust launcher if you need
reinforcement learning.
[Agent–agent and reinforcement-learning experiments](agent-agent-and-rl-experiments.md)

## Implement deterministic turns

The guest receives one MessagePack configuration and a recorded seed in
`init`. Each `step` receives the acting role and one MessagePack action.
The guest returns either a frame or a rule-rejection code. A rejection
leaves the entire session state, including the random generator, unchanged.
Each frame carries two complete role observations, optional completion and
transition metadata, and an optional opaque recovery checkpoint.

### Declare the interface for your game

Implement `describe() -> string` on the component. The returned UTF-8 JSON
document describes six payloads: the configuration, the action, each role's
view, the completion value, and the transition metadata. The host calls
`describe` once on a disposable instance while loading the component. It
validates the declaration before accepting sessions. The declaration applies
to all configurations, seeds and sessions of that build. The component digest
identifies the matching declaration for replay.

A Rust game can derive `GameValue` alongside Serde. This generates both
the declaration and the writer from the same Rust type. For example, the
following helpers declare a score view for both roles and an action with a
string tag in its public representation.

```rust
use parlando_game_sdk::{
    encode_compact, ComponentValue, GameValue, GameSchema, Schema,
};
use serde::{Deserialize, Serialize};

#[derive(GameValue, Deserialize, Serialize)]
struct View {
    score: u32,
}

#[derive(GameValue, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Action {
    Play { card: String },
    Pass,
}

// Return this string from the guest's describe export.
fn description() -> Result<String, String> {
    GameSchema::new(
        Schema::Dynamic, Action::schema(), View::schema(), View::schema(),
        Schema::U64, Schema::Dynamic,
    ).to_description()
}

// Put the returned bytes in the appropriate role's frame field.
fn score_view(score: u32) -> Result<Vec<u8>, String> {
    encode_compact(&View { score })
}

// A local agent can read by public field name.
fn read_score(observation: &ComponentValue) -> anyhow::Result<u32> {
    Ok(observation.decode::<View>()?.score)
}
```

In this example a score of seven travels as the two bytes `91 07`, a
MessagePack array containing the integer. A browser, remote agent, or
persisted record sees `{"score":7}`. A local policy can deserialize a subset
of the named fields and borrow text from the host's buffer. To send an action,
construct a `ComponentValue::from_typed(&action)`. The host puts the fields
in the declared order. For a Rust action with `GameValue`, the guest calls
`parlando_game_sdk::decode_declared::<Action>(&bytes)`. Its generated
reader constructs the action directly from field slots and the numeric variant
tag. Derive it on the action and its nested declared Rust types. Fields can
borrow strings for the lifetime of the input. Use `decode_with_schema` when
the type needs its Serde deserializer or the declaration is available only
at runtime.
A local policy that shares the game's action type can instead pass
`encode_compact(&action)` bytes and `factory.schema().action.clone()` to
`ComponentValue::from_schema_bytes`. The host can borrow those bytes
directly when submitting the action. The gRPC protocol expects objects at
the top level of actions and observations.

### Read selected fields in a local policy

A Rust policy that reads the same fields on every turn can resolve those
fields once against the appropriate seat's declaration. For the score view
above, initialize a projection while loading the game.

```rust
use parlando::component::{ProjectedValue, Projection, Selection};

let scores = Projection::resolve(
    factory.schema().view_a.clone(),
    &Selection::Fields(&[("score", Selection::U64(()))]),
)?;
```

For each role-A observation returned by the component, apply the saved
projection. The callback receives values in wire order. It can borrow string
contents from the observation and enum labels from the projection.

```rust
let mut score = None;
scores.read(&observation, |(), field| {
    if let ProjectedValue::U64(value) = field {
        score = Some(value);
    }
    Ok(())
})?;
```

Use distinct keys when selecting multiple values. `Selection::List` applies
the same selection to each element and `Selection::EnumLabel` reads the
selected variant's public name. The policy still receives every observation.
Resolve a separate projection where the other seat has a different schema.
The reader accepts schema-associated compact values. For a named value loaded
from a public record, use the ordinary named decoder. Accumulate a policy
update locally if it should take effect only after the entire read succeeds.

### Implement the compact format in another language

Each descriptor contains a `type` and, for compound types, a `value`.
Here is a complete declaration for a game taking a Boolean action, returning
a one-field score view for each seat and an unsigned terminal result.

```json
{
  "config": {"type":"dynamic"},
  "action": {"type":"bool"},
  "view_a": {"type":"struct","value":[{"name":"score","schema":{"type":"u64"}}]},
  "view_b": {"type":"struct","value":[{"name":"score","schema":{"type":"u64"}}]},
  "completion": {"type":"u64"},
  "metadata": {"type":"dynamic"}
}
```

| Descriptor type | Compact MessagePack representation |
|---|---|
| `null`, `bool`, `i64`, `u64`, `f64`, `string` | Corresponding scalar. Strings contain UTF-8 and floats must be finite. |
| `dynamic` | One ordinary JSON-shaped MessagePack value, including string keys for objects. |
| `option`, `list`, `map` | The `value` descriptor specifies the child type. An option uses nil or the child value. A list uses an array. A map retains its UTF-8 keys. |
| `tuple` | An array with the exact length and order of the child descriptors in `value`. |
| `struct` | An array with one slot per declared field in order. An absent optional value occupies a nil slot. A field declared with `"omit_none":true` is omitted in the public object when its value is nil. |
| `enum` | A zero-based variant number when every variant is a unit variant. Otherwise an array starting with that number, followed by every payload field in order. |

An enum declaration specifies its named representation and its variants. For
the Rust action above the enum's `value` is the following object.

```json
{
  "representation": {"type":"internal","value":{"tag":"type"}},
  "variants": [
    {"name":"play","payload":{"type":"struct","value":[{"name":"card","schema":{"type":"string"}}]}},
    {"name":"pass","payload":{"type":"unit"}}
  ]
}
```

The public action `{"type":"play","card":"ace"}` travels as `[0,"ace"]`.
The host uses the declaration to recover the names. External tagging and
adjacent tagging are also supported. Declarations must give fields and
variants unique public names. Choose explicit tagged variants for your game.
Use `dynamic` for data whose structure is determined at runtime.

Keep each payload within 8 MiB, 64 nested containers, and 262,144 nodes
including object keys. Reading or rendering a named value also allows up to
8 MiB of text synthesized from schema field and variant names. Supply each
required field in named input, including fields with a Rust construction
default. A compatible producer enforces these rules while
writing. The host checks returned buffer lengths and can retain or forward
the bytes until a consumer reads them. Opaque recovery checkpoints use the
game's own encoding. They have no associated view schema.

### Handle actions and recovery

Parlando serves the role views from the selected frame. A component has no
separate operation to list available actions. A policy can use affordances
included in its own view. Keep role-private information out of the other role's
view and out of shared metadata. The guest has no filesystem, network, clock,
environment, or credential imports. Treat the game configuration as guest input
and keep provider credentials in host-side secret settings. The host gives
each guest call 50 million Wasmtime fuel units and limits each linear memory
to 64 MiB, with at most four memories and 64 core Wasm instances per guest
store. A guest trap, including fuel exhaustion, is an execution failure.

The host stores a full checkpoint only when the guest returns one. A checkpoint
contains all guest state needed to restore that position in a fresh instance,
including the random number generator. Recovery and historical replay use
recorded checkpoints where available and re-execute actions for positions
without one. A game that relies on nondeterminism must return a checkpoint from
`init` and from every accepted `step` so the recorded result can be recovered.
Historical replay requires the same component bytes recorded at session
creation. Retain deployed component binaries for sessions you plan to replay
after an upgrade.


## Run the component conformance tests

Install the Rust `wasm32-unknown-unknown` target and make `wasm-tools` available
on your PATH. Run the suite from the repository root.

```sh
rustup target add wasm32-unknown-unknown
make test-components
```

The suite builds a Rust guest with fault injection and an independent guest
written in WebAssembly text format. It checks that the host uses the WIT
interface, isolates sessions, preserves committed views after a guest failure,
restores checkpoints, and matches the native fixture in a headless experiment.
The generated test components are written to `target/component-tests/`.
