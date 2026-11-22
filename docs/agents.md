# Agents

An agent controls `PlayerRole::A` or `PlayerRole::B` under exactly the same game rules and information boundary as a human. It receives role-specific observations, accepted actions, messages from the other player, and the same shared completion delivered to human players. It never receives authoritative state or frontend events.

## Run a local WASM agent

A local agent controls one seat in the host process. Its own project
manifest, `agent.yaml`, names the target game and the policy to build. The
Last Screening partner shares Rust types with its game and uses a Cargo
feature to select its agent export:

```yaml
game: last-screening
id: last_screening.operator
name: Screening partner
version: 1.0.0
cargo_manifest: server/Cargo.toml
features: [agent-component]
```

For an already compiled policy, use `component: path/to/agent.wasm` in
place of the Cargo manifest and features. Paths resolve relative to
`agent.yaml`. The agent ZIP contains a built `agent.yaml` and `agent.wasm`.
Build and check it from the agent project:

```sh
parlando build --manifest agent.yaml
parlando check --package dist/agent-last_screening.operator-1.0.0.zip
```

Start a shared host with `parlando serve` and deploy the game first. Sign
in with your dashboard account to upload both ZIPs from the CLI. Set
`HOST` to the host URL or its configured alias. For The Last Screening:

```sh
parlando login --server "$HOST"
parlando deploy --host "$HOST" --package dist/last-screening-0.1.0.zip
parlando deploy --host "$HOST" --package dist/agent-last_screening.operator-1.0.0.zip
```

The host binds the policy to the current game package revision. Activate
the game and open its dashboard from **Games**. Create an experiment or
open an inactive one. In **Configuration**, set **Participant pairing** to
**Human vs agent**, choose `last_screening.operator` as **Agent factory**,
and save the configuration. Start a **Local preview** and play one seat.

Stop the game's experiments and let active sessions finish before installing
or updating a policy. After updating the game, upload its agent ZIP again
for the new game revision. Record both package revisions with your study.
See [deploy to a shared host](web/content/manual/deploy/_index.md) for the
browser upload and CLI authorization steps.

To use both archives in headless play, name the agent factory in a separate
[agent-experiment specification](agent-agent-and-rl-experiments.md).

```sh
parlando run evaluation.yaml --package dist/last-screening-0.1.0.zip \
  --agent dist/agent-last_screening.operator-1.0.0.zip
```

The game and its local agents share one execution setting. Fuel metering is
the default. For a headless run, set `execution.wasm_metering: none` in the
experiment YAML. For a shared host, start it with
`parlando serve --wasm-metering none`. Unmetered guests have no fuel
instruction budget per callback. Seat isolation and the game's rules for
accepting actions still apply.

## Write a typed Rust agent

A Rust policy uses `parlando-game-sdk` for the game's wire types and the
policy callbacks. This example plays either seat in Beacon Relay, the Rust
starter created by `parlando new beacon --lang rust`. It adds one tick
whenever its role has the turn.

Create a separate Cargo library for the policy. Its `Cargo.toml` declares a
`cdylib` so the compiler can emit WebAssembly. Use the same SDK version as
the game. If you build Parlando from a checkout, use the matching local SDK
path for both crates.

```toml
[package]
name = "beacon-agent"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
parlando-game-sdk = "0.7.0"
serde = { version = "1", features = ["derive"] }
```

Save this as `src/lib.rs`. The three wire types match the starter. The
policy stores only the current seat's permission to act.

```rust
use parlando_game_sdk::{
    agent::{async_trait, Agent, Definition, InitContext, Observation, RespondReason, Response, Result},
    export_agent, GameValue,
};
use serde::Deserialize;

#[derive(Deserialize)]
struct Settings {}

#[derive(GameValue)]
struct Pulse {
    ticks: u8,
}

#[derive(GameValue)]
struct View {
    charge: u8,
    target: u8,
    your_turn: bool,
}

#[derive(GameValue)]
struct Outcome {
    activated_at: u8,
}

struct BeaconAgent {
    your_turn: bool,
}

#[async_trait]
impl Agent for BeaconAgent {
    type Config = Settings;
    type Action = Pulse;
    type View = View;
    type Completion = Outcome;

    fn definition() -> Definition {
        Definition {
            id: "beacon.one_tick",
            name: "One-tick Beacon partner",
            version: "0.1.0",
            description: "Adds one tick when it has the turn.",
            config_fields: "[]",
        }
    }

    async fn init(_: InitContext<Settings>) -> Result<Self> {
        Ok(Self { your_turn: false })
    }

    async fn start(&mut self, view: View) -> Result<()> {
        self.your_turn = view.your_turn;
        Ok(())
    }

    async fn observe(&mut self, event: Observation<Pulse, View>) -> Result<()> {
        if let Some(transition) = event.transition {
            self.your_turn = transition.view.your_turn;
        }
        Ok(())
    }

    async fn respond(&mut self, _: RespondReason) -> Result<Response<Pulse>> {
        Ok(if self.your_turn {
            Response::action(Pulse { ticks: 1 })
        } else {
            Response::default()
        })
    }
}

export_agent!(BeaconAgent);
```

Save an `agent.yaml` beside that crate's `Cargo.toml`. Its game ID
must match the game ZIP and its agent ID and name must match the policy's
`definition`.

```yaml
game: beacon
id: beacon.one_tick
name: One-tick Beacon partner
version: 0.1.0
cargo_manifest: Cargo.toml
```

Install the Rust `wasm32-unknown-unknown` target and `wasm-tools`, then run
`parlando build --manifest agent.yaml`. Parlando invokes Cargo and wraps
the emitted module as a component. The output is
`dist/agent-beacon.one_tick-0.1.0.zip`. Follow the [ZIP deployment steps
above](#run-a-local-wasm-agent) with the Beacon game ZIP and
`beacon.one_tick` as the experiment's agent factory.

### Use the policy callbacks

Declare `Config`, `Action`, `View`, and `Completion` on the policy type.
The action, view, and completion match the game's compact layouts. If your
game and agent are in separate crates, you can share these types in a library.
`InitContext` provides typed settings, the assigned `Role`, and the recorded
seed. The SDK exports `Role` and `GameValue` at the crate root.

`start` receives the first private view. `observe` receives an optional
accepted transition and an optional partner message. A transition carries the
actor, action, and resulting view for your seat. When both fields are present,
process the transition before the message. `respond` proposes an action,
sends a message, requests a wakeup, or yields with an empty response. The game
validates proposed actions. A response with an action and a message sends the
message when the action is accepted.

Most policies can use an eager view such as `type View = MyView`. If a
policy handles frequent transitions from the action alone, use
`type View = LazyView<MyView>`. Call `transition.view.decode()?` on the
branches that need it. Every transition still carries the complete
role-private view, and the WASM boundary still transfers its bytes.

Live decisions can follow session activation, observations, a rejected
action, or a requested wakeup. Use `WakeupRequest::After(duration)` to set
or replace a timer and `WakeupRequest::Cancel` to clear it. An absent
request preserves the pending timer. A partner's input can prompt an earlier
decision. A headless run uses its own decision schedule and fails a session
if a policy requests a wakeup. For Hotel rollouts set `pacing: instant`.

WASM callbacks must finish within one poll. Use the [remote gRPC
adapter](#remote-python-agents) for network inference or other external
services. For larger policies, see the Hotel
[porter](../games/counterweight-hotel/server/src/porter.rs) and the
[Screening partner](../games/last-screening/server/src/operator.rs).

## Register a Rust policy in a custom host

For a typed policy, build a `parlando::agent::native::TypedAgentFactory`
using `TypedAgentFactory::<MyPolicy>::for_game(&loaded_game)` and register it
with `Server::agent` or `ExperimentRunner::agent`. The factory checks the
policy's compact layouts against the game and creates a new instance
for each seat.

For direct integration with the host, implement `parlando::agent::Agent`
and `parlando::agent::Factory`. The host passes actions, views, and completion
as `ComponentValue`. Decode inputs to match the game's schema and use
`ComponentValue::from_typed` to encode an action. Return a
`parlando::agent::Decision` from `decide` to supply an action, a message, or
a wakeup request.

In a live human–agent session, the server queues up to 64 observations while an agent callback is running. If the queue fills and another input needs delivery, the server ends that session with a technical failure before accepting the input. The agent can drain the observations already queued. Other sessions continue independently.

Once completion is known, the runtime requests no further responses. It delivers queued observations through the terminal transition, calls
`finish` with the shared completion, and then calls `shutdown` to release
resources.

`Factory::create(Context { role, seed, settings, factory_secrets, agent_instance_secrets }).await` constructs the agent before game state is delivered. Settings are server-validated, normalized non-secret JSON. The two secret sets contain only explicitly referenced values and redact their debug output. They are available to in-process factories and agents; the standard remote adapter declares no secret references and does not transport either set. After construction returns, the runtime creates the initial game state and calls `start(initial_observation)`.

A factory's `Definition` uses semantic `ConfigValue` types: strings (optionally URI-formatted), booleans, bounded integers and numbers, choices, nested objects, and purpose-tagged secret references. The server validates definitions at registration and settings at save and runtime; the dashboard derives controls from these semantics. Secret fields store a `game.<key>` reference, never its value. Every factory must implement `identity(settings)` and supply a non-empty semantic name and implementation version for the automated participant. Configuration choices such as model and prompt belong in normalized settings and their fingerprint, not in the implementation-version field. Register the factory with `Server::agent`.

Each automated participant also records `sha256:<hex>` over a versioned canonical document containing its factory ID and normalized settings. Reference names are included but resolved values are not, so credential rotation does not change the fingerprint. This identifies configuration, not remote model weights or provider behavior.

## Remote Python agents

The Python SDK delivers the same role-specific values as dictionaries. Its
`respond` callback keeps the optional action argument for protocol compatibility.
Read legal moves from the view if the game publishes them.

```python
from parlando_agent_sdk import Agent, Response, serve

class FirstActionAgent(Agent):
    async def start(self, initial_observation):
        self.observation = initial_observation

    async def observe_transition(self, actor, action, observation):
        self.observation = observation

    async def observe_message(self, sender, text):
        self.last_message = (sender, text)

    async def finish(self, completion):
        self.completion = completion

    async def respond(self, _available_actions):
        legal_actions = self.observation.get("available_actions", [])
        if legal_actions:
            return Response.action(legal_actions[0])
        return None

serve(lambda context: FirstActionAgent(), host="127.0.0.1", port=50051)
```

The remote gRPC factory is available in hosted experiments and custom
servers. Configure its endpoint and optional YAML settings in the dashboard.
For a Rust policy running in the host process, use the [local ZIP
workflow](#run-a-local-wasm-agent). An empty YAML document sends an empty settings mapping. Rust parses other YAML mappings
and delivers them to Python as `Context.settings`.

Keep model credentials and other agent secrets in the remote process's own configuration or
environment. Protocol v5 has no secret-delivery field. Loopback HTTP needs no transport
credential. For a non-loopback endpoint, use HTTPS, list the host in
`PARLANDO_REMOTE_AGENT_ALLOWED_HOSTS`, and set the same `PARLANDO_REMOTE_AGENT_TOKEN` in the
Parlando server and remote-agent processes. Both adapters read that transport token automatically;
it is never stored in experiment configuration or shown in the dashboard.

## Construction readiness

The runtime considers an agent constructed when asynchronous factory creation, or remote
`CreateAgent`, returns. `start` then delivers the initial observation. The protocol has no separate
readiness signal and no distinct timeout for initialization. Synchronous model loading can therefore
block the agent process, and long initialization consumes the configured action-timeout budget.
Agents must not emulate readiness with player messages or game actions.
