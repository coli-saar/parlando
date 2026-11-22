# WebAssembly component interface

The [experimenter manual](web/content/manual/_index.md) explains how to
install Parlando, create a Rust or Python game, and deploy its finished
package. This reference is for authors who need to implement or inspect
the interface directly.

Parlando loads a binary WebAssembly component that exports the `game`
world of [`parlando:game@4.0.0`](../rust-server/wit/game.wit). The component
has no host imports. Each session gets a separate guest instance. The guest
owns the task state. The host owns connections, scheduling, and the research
record.

Build Rust games with the [Rust Game SDK](https://docs.rs/parlando-game-sdk/).
Its `Game` trait supplies the six payload types and session operations,
`GameValue` derives the schema and codec for those types, and `export!`
provides the WIT exports. For Python, the
[Python Game SDK](https://pypi.org/project/parlando-python-sdk/) supplies
the same interface through a typed `Game` subclass and its `mechanics` adapter.
Run `parlando check --manifest dist/parlando/game.yaml` against the staged
artifact to verify it with the actual host loader.

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
and keep provider credentials in host-side secret settings. By default the host gives
each guest call 50 million Wasmtime fuel units and limits each linear memory
to 64 MiB, with at most four memories and 64 core Wasm instances per guest
store. A guest trap, including fuel exhaustion, is an execution failure.

Use `parlando serve --wasm-metering none` for a local game or
`parlando host --wasm-metering none` for all games on a shared host to remove
the per-call fuel budget. Memory and instance limits still apply. An unmetered
game can loop indefinitely inside a call. For headless evaluations, select the
mode in the [agent–agent experiment YAML](agent-agent-and-rl-experiments.md#choose-the-game-components-execution-budget).

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
