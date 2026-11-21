# Agents

An agent controls `PlayerRole::A` or `PlayerRole::B` under exactly the same game rules and information boundary as a human. It receives role-specific observations, accepted actions, messages from the other player, and the same shared completion delivered to human players. It never receives authoritative state or frontend events.

## Rust agents

Implement `agent::Agent` for each session-local policy and `agent::Factory`
for the constructor. The host passes observations, actions, and completion as
`ComponentValue`. Decode an incoming value to match the component’s schema.
Use `ComponentValue::from_typed` to encode outgoing actions.

For a game whose view lists available actions in an `available_actions` field,
this policy selects the first action:

```rust
use anyhow::Result;
use async_trait::async_trait;
use parlando::{agent::{Agent, Response}, component::ComponentValue, PlayerRole};
use serde::Deserialize;

#[derive(Deserialize)]
struct Observation {
    #[serde(default)]
    available_actions: Vec<serde_json::Value>,
}

#[derive(Default)]
struct FirstActionAgent {
    next_action: Option<serde_json::Value>,
}

#[async_trait]
impl Agent for FirstActionAgent {
    async fn start(&mut self, observation: ComponentValue) -> Result<()> {
        self.next_action = observation.decode::<Observation>()?.available_actions.into_iter().next();
        Ok(())
    }

    async fn observe_transition(
        &mut self,
        _actor: PlayerRole,
        _action: ComponentValue,
        observation: ComponentValue,
    ) -> Result<()> {
        self.start(observation).await
    }

    async fn respond(&mut self) -> Result<Option<Response>> {
        self.next_action
            .take()
            .map(|action| ComponentValue::from_typed(&action).map(Response::action))
            .transpose()
    }
}
```


A `Response` is always non-empty: action, message, or action and message. `None` means the agent declines to respond now. Messages communicate with the other player and do not change state. Every action is validated by the game.

In a live human–agent session, the server queues up to 64 observations while an agent callback is running. If the queue fills and another input needs delivery, the server ends that session with a technical failure before accepting the input. The agent can drain the observations already queued. Other sessions continue independently.

Once completion is known, the runtime requests no further responses. It delivers already-queued observations in order through the terminal `observe_transition`, then calls `finish` with the shared completion and finally calls `shutdown` for resource cleanup.

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

Every `Server` includes the remote gRPC agent in the dashboard. Configure its endpoint and
optional YAML settings there; game binaries only need `Server::agent` for additional in-process
factories. An empty YAML document sends an empty settings mapping. Rust parses other YAML mappings
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
