# Agents

An agent controls `PlayerRole::A` or `PlayerRole::B` under exactly the same game rules and information boundary as a human. It receives role-specific observations, accepted actions, messages from the other player, and the same shared completion delivered to human players. It never receives authoritative state or frontend events.

## Rust agents

Implement `agent::Agent<G>` and create session-local instances with `agent::Factory<G>`:

```rust
use anyhow::Result;
use async_trait::async_trait;
use parlando::{
    agent::{Agent, Context, Definition, Factory, Identity, Response},
    PlayerRole,
};

struct MyAgent {
    observation: Option<MyObservation>,
    completion: Option<MyCompletion>,
}

#[async_trait]
impl Agent<MyGame> for MyAgent {
    async fn start(&mut self, initial_observation: MyObservation) -> Result<()> {
        self.observation = Some(initial_observation);
        Ok(())
    }

    async fn observe_transition(
        &mut self,
        _actor: PlayerRole,
        _action: MyAction,
        observation: MyObservation,
    ) -> Result<()> {
        self.observation = Some(observation);
        Ok(())
    }

    async fn observe_message(&mut self, sender: PlayerRole, text: String) -> Result<()> {
        remember_message(sender, text);
        Ok(())
    }

    async fn finish(&mut self, completion: MyCompletion) -> Result<()> {
        self.completion = Some(completion);
        Ok(())
    }

    async fn respond(
        &mut self,
        available_actions: Option<Vec<MyAction>>,
    ) -> Result<Option<Response<MyAction>>> {
        Ok(available_actions
            .and_then(|actions| actions.into_iter().next())
            .map(Response::action))
    }
}
```

A `Response` is always non-empty: action, message, or action and message. `None` means the agent declines to respond now. Messages communicate with the other player and do not change state. Every action is validated by the game.

Once completion is known, the runtime requests no further responses. It delivers already-queued observations in order through the terminal `observe_transition`, then calls `finish` with the shared completion and finally calls `shutdown` for resource cleanup.

`Factory::create(Context { role, seed, settings, factory_secrets, agent_instance_secrets }).await` constructs the agent before game state is delivered. Settings are server-validated, normalized non-secret JSON. The two secret sets contain only explicitly referenced values and redact their debug output. Factory secrets are for local construction or transport authentication; agent-instance secrets may cross the remote protocol in its separate field. After construction returns, the runtime creates the initial game state and calls `start(initial_observation)`.

A factory's `Definition` uses semantic `ConfigValue` types: strings (optionally URI-formatted), booleans, bounded integers and numbers, choices, nested objects, and purpose-tagged secret references. The server validates definitions at registration and settings at save and runtime; the dashboard derives controls from these semantics. Secret fields store a `game.<key>` reference, never its value. Every factory must implement `identity(settings)` and supply a non-empty semantic name and implementation version for the automated participant. Configuration choices such as model and prompt belong in normalized settings and their fingerprint, not in the implementation-version field. Register the factory with `Server::agent`.

Each automated participant also records `sha256:<hex>` over a versioned canonical document containing its factory ID and normalized settings. Reference names are included but resolved values are not, so credential rotation does not change the fingerprint. This identifies configuration, not remote model weights or provider behavior.

## Remote Python agents

The supported Python authoring API mirrors Rust:

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

    async def respond(self, available_actions):
        if available_actions:
            return Response.action(available_actions[0])
        return None

serve(FirstActionAgent, host="127.0.0.1", port=50051)
```

Register `parlando::agent::grpc::Factory::<MyGame>::new()` on the Rust server and configure its endpoint and identity through the dashboard.

Remote protocol v4 delivers `agent_instance_secrets` separately from `config`. Selecting such a reference authorizes delivery to the configured endpoint. Non-loopback endpoints still require HTTPS, an allowed host, and a factory-purpose bearer credential.

## Readiness follow-up

Today the runtime considers an agent constructed when async factory creation, or remote `CreateAgent`, returns. `start` then delivers the initial observation. A separate readiness signal, initialization-specific timeout and cancellation, and isolation for synchronous model loading are not yet specified. They are the first planned lifecycle follow-up; agents should not emulate readiness with player messages or game actions.
