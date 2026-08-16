# Agents

An agent controls `PlayerRole::A` or `PlayerRole::B` under exactly the same game rules and information boundary as a human. It receives role-specific observations, accepted transitions, and messages from the other player. It never receives authoritative state or frontend events.

## Rust agents

Implement `agent::Agent<G>` and create session-local instances with `agent::Factory<G>`:

```rust
use anyhow::Result;
use async_trait::async_trait;
use parlando_server::{
    agent::{Agent, Context, Definition, Factory, Identity, Response},
    PlayerRole,
};

struct MyAgent {
    observation: Option<MyObservation>,
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

`Factory::create(Context { role, seed, settings }).await` constructs the agent before game state is delivered. After construction returns, the runtime creates the initial game state and calls `start(initial_observation)`. This ordering allows model or remote-resource initialization before the agent sees the game.

A factory's `Definition` and `ConfigField` values populate the dashboard's compiled-agent selector. `identity(settings)` supplies the semantic name and optional version recorded for the automated participant. Register the factory with `Server::agent`.

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

    async def respond(self, available_actions):
        if available_actions:
            return Response.action(available_actions[0])
        return None

serve(FirstActionAgent, host="127.0.0.1", port=50051)
```

Register `parlando_server::agent::grpc::Factory::<MyGame>::new()` on the Rust server and configure its endpoint and identity through the dashboard.

## Readiness follow-up

Today the runtime considers an agent constructed when async factory creation, or remote `CreateAgent`, returns. `start` then delivers the initial observation. A separate readiness signal, initialization-specific timeout and cancellation, and isolation for synchronous model loading are not yet specified. They are the first planned lifecycle follow-up; agents should not emulate readiness with player messages or game actions.
