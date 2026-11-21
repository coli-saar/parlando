---
title: Agents API
---

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


A `Response` contains an action, a message, or both. `None` means that the agent has nothing to do
until another event arrives. Messages communicate with the other player; actions are always checked
against the game rules.

In a live human–agent session, the server queues up to 64 observations while an agent callback is running. If the queue fills and another input needs delivery, the server ends that session with a technical failure before accepting the input. The agent can drain the observations already queued. Other sessions continue independently.

The factory creates one agent for each automated seat. Its definition supplies the dashboard fields,
a stable implementation name and version, and any references to installation secrets. Put model,
prompt, and decoding choices in the configurable settings so that the saved experiment identifies
the policy actually used. Register the factory with `Server::agent`.

## Remote Python agents

Install the SDK from a Parlando checkout with:

```sh
python3 -m venv .venv-agent
.venv-agent/bin/python -m pip install --editable parlando-agent-sdk
```

The manual's [Great Tree Python agent](../examples/great_tree_python_agent.py) is a runnable example
that attaches to both a live human–agent experiment and the headless runner.

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

Every `Server` includes the remote Python option in the dashboard. Configure its endpoint and
optional YAML settings there; they arrive in Python as `Context.settings`. Game binaries use
`Server::agent` only for additional in-process factories.

Keep model credentials and other agent secrets in the Python service's own configuration or
environment. For a service on another machine, use HTTPS, list the host in
`PARLANDO_REMOTE_AGENT_ALLOWED_HOSTS`, and set the same `PARLANDO_REMOTE_AGENT_TOKEN` in the
Parlando server and agent service. The token is never stored in experiment configuration or shown
in the dashboard.

## Start within the configured timeout

The agent must finish construction within the experiment's action timeout. Load expensive shared
models when the Python service starts when possible, then keep per-session construction short. If
participants time out before play begins, inspect the agent service and increase the limit only
after measuring ordinary initialization time.
