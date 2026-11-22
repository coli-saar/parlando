# Agent integration

An agent controls one seat and receives the same role-private view and shared
completion as a participant. Use the game's accepted actions and the other
player's messages as policy inputs. Keep private policy state per session and
use the recorded seed for reproducible choices. The game validates every
proposed action.

## Write a typed Rust policy

Use the same `parlando-game-sdk` dependency as the game. Share the game's
Rust action, view and completion definitions, with `GameValue` derived for
the compact payloads. Against a non-Rust game, declare equivalent Rust types
with the same compact layouts. Import `Role` and the derive from
`parlando_game_sdk`, and import the policy API from
`parlando_game_sdk::agent`.

Implement `Agent` with `#[parlando_game_sdk::agent::async_trait]` and
declare `Config`, `Action`, `View` and `Completion`. Declare the policy ID,
name, implementation version and configuration fields in `definition`.
The ID and name must match the agent package manifest. The implementation
version identifies the policy. The manifest version identifies the archive
release.

- Construct one fresh policy in `init(InitContext)` using the typed settings,
  assigned role and recorded seed. Use `start` to receive the initial view.
- In `observe`, handle the optional accepted transition before the optional
  partner message. A transition carries the actor, action and this seat's
  resulting view. Use `LazyView<T>` only when the policy can ignore complete
  views on some transitions, and call `decode()` on the branches that use them.
- Implement `respond(RespondReason)` using `Response::action`,
  `Response::message`, `Response::action_and_message` or
  `Response::default()` to yield. When a response includes a move and text,
  the runtime sends the message only after it accepts the move. The live
  runtime can deliver chat text or finalized speech transcripts and can
  schedule a wakeup requested with `wake_after`. For headless experiments,
  let the experiment schedule supply decisions and return no wakeup request.
- Use `finish` for terminal completion and `shutdown` to release resources.

In the policy's Rust `cdylib` crate, invoke
`parlando_game_sdk::agent::export_agent!(Policy)` at crate root. When the
policy and mechanics share a crate, use a project feature to select one export
per build. A separate agent crate needs no SDK feature. The import-free WASM
callback must complete in one poll. Use the remote gRPC option for network
inference or other work that needs an asynchronous service.

## Build and exercise separate archives

Create `agent.yaml` next to the project. Use the target game's ID and the
policy's own stable ID, display name and semantic archive version:

```yaml
game: last-screening
id: last_screening.operator
name: Screening partner
version: 1.0.0
cargo_manifest: server/Cargo.toml
features: [agent-component]
```

The example uses a feature because the game and policy share this crate.
Omit `features` for an independent policy crate without a feature gate. For
an existing WASM component, use `component: path/to/agent.wasm` instead of
the Cargo fields. Paths resolve relative to the manifest.

Run native policy tests and build the independent agent ZIP. The exact
native test features are defined by the project:

```sh
cargo test --manifest-path server/Cargo.toml --features agent-component --lib
parlando build --manifest agent.yaml
parlando check --package dist/agent-last_screening.operator-1.0.0.zip
```

The archive check validates the agent component interface and its declared
identity. Exercise compatibility and behavior by configuring both seats with
the intended factory IDs and running a seeded headless experiment:

```sh
parlando run experiment.yaml --package dist/last-screening-0.1.0.zip \
  --agent dist/agent-last_screening.operator-1.0.0.zip
```

Check completed sessions, accepted and rejected actions, message order and
terminal outcomes. Use a full trace when verifying parity across native and
WASM execution. For a live pilot, test the relevant role, dialogue and
presentation timing in the browser. Upload the game ZIP first and the agent
ZIP separately, then pin the intended revisions through experiment selection.
See [deployment](config-deployment.md) for the package and authority workflow.

## Use a remote gRPC service

For a policy running in an independent process, select `remote_grpc` and
configure its endpoint and settings in the experiment. The Python service can
use the public `parlando_agent_sdk` package. It receives transitions and
partner messages through `observe_transition` and `observe_message`.
Use `Response.action_with_message` for a move with text, and return `None`
to yield. Keep model-provider credentials in the agent process. Follow the
[remote-agent guide](https://coli-saar.github.io/parlando/manual/build/agents-and-voice/)
for endpoint authentication and service setup.
