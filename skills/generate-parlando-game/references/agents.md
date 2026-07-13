# Parlando Agents Reference

Use this reference only when the generated project needs server-side agent support, a demo agent, or remote gRPC agents.

Agents are first-class participants. They observe role-specific state, accepted actions, and conversation messages. After each observation, the runtime asks whether the agent wants to respond. Returned actions go through the normal server parsing, validation, persistence, and broadcast path.

Do not make game semantics or browser UI depend on whether a participant is human or agent-controlled. The game contract is role, observation, action, event, and summary. A browser instance offers controls for its one human participant and receives the other participant's accepted actions/events through Parlando, regardless of whether that peer is human or agent-controlled.

## In-Process Rust Agents

Implement `GameAgent<A>` and create instances with `AgentFactory<A>`.

```rust
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use parlando_server::{
    AgentFactory, AgentInitContext, AgentParticipantIdentity, AgentResponse, ExperimentConfig,
    GameAgent, PlayerRole,
};

pub fn factory_from_config(
    config: &ExperimentConfig,
) -> Result<Option<Arc<dyn AgentFactory<MyGameAdapter>>>> {
    if config.agents.mode != parlando_server::config::AgentsMode::HumanVsAgent {
        return Ok(None);
    }
    Ok(Some(Arc::new(DemoAgentFactory)))
}

pub struct DemoAgentFactory;

impl AgentFactory<MyGameAdapter> for DemoAgentFactory {
    fn create(&self, context: AgentInitContext) -> Result<Box<dyn GameAgent<MyGameAdapter> + Send>> {
        Ok(Box::new(DemoAgent {
            role: context.role,
            last_actor: None,
        }))
    }

    fn participant_identity(&self) -> AgentParticipantIdentity {
        AgentParticipantIdentity {
            identity_provider: "<game-slug>".to_string(),
            external_id: Some("demo-agent@0.1.0".to_string()),
            metadata: serde_json::json!({ "agent_type": "demo" }),
        }
    }
}

pub struct DemoAgent {
    role: String,
    last_actor: Option<PlayerRole>,
}

#[async_trait]
impl GameAgent<MyGameAdapter> for DemoAgent {
    async fn observe_action(
        &mut self,
        actor: PlayerRole,
        _action: GameAction,
        _resulting_observation: GameObservation,
    ) -> Result<()> {
        self.last_actor = Some(actor);
        Ok(())
    }

    async fn maybe_act(
        &mut self,
        available_actions: Option<Vec<GameAction>>,
    ) -> Result<Option<AgentResponse<GameAction>>> {
        if let Some(actions) = available_actions {
            if let Some(action) = actions.into_iter().next() {
                return Ok(Some(AgentResponse {
                    message: None,
                    action: Some(action),
                }));
            }
        }
        Ok(None)
    }
}
```

Create one mutable agent instance per agent participant. If the policy needs memory, store it in the agent struct. Do not assume the server passes room ids, participant-session ids, full conversation history, invalid-action counts, or completion flags into callbacks.

If participant messages are relevant to the game, the agent must implement `observe_message` and use that memory in `maybe_act`. Parlando calls `observe_message` for typed messages and voice transcripts with a `speaker`, an `AgentUtteranceKind`, and the text. Store the latest relevant utterance or a bounded conversation summary in the agent instance, then return an `AgentResponse` when the agent should answer, clarify, negotiate, or take an action because of that message.

Do not generate a human-vs-agent game where the participant is expected to talk to the agent but the agent ignores `observe_message`. A purely deterministic demo agent may be enough for smoke tests, but if natural-language understanding is central to the game, ask the user whether they can provide LLM provider credentials. Put those credentials only in server-side config, a private overlay, or the remote agent process environment; never put them in browser code or checked-in YAML.

`AgentResponse` has optional fields:

- `message: Option<String>`
- `action: Option<Action>`

When TTS is enabled in config, participant-facing agent speech must flow through `AgentResponse.message`. The server persists the returned text as an agent-origin conversation message, then synthesizes and publishes it through the configured TTS provider and audio publisher. Do not implement separate browser TTS, Web Speech API calls, or custom game-level audio publication for agent messages.

## Agent Config

Human-vs-agent server YAML:

```yaml
agents:
  mode: human_vs_agent
  human_vs_agent:
    factory: <game-slug>.demo_agent
    seed: 0
    act_timeout_seconds: 5
    invalid_action_limit: 3
    config: {}
```

The generated `factory_from_config` should:

- return `Ok(None)` unless `agents.mode` is `human_vs_agent`.
- validate that `agents.human_vs_agent` is present when needed.
- support a game-specific demo selector.
- support `remote_grpc` if remote agents are requested.

This config selects how the server supplies an agent participant. It should not introduce separate action schemas, observations, UI branches, or game rules for agent opponents.

## Remote gRPC Agents

Remote agents let researchers write policies in Python or another language while the Rust server stays authoritative.

Rust selector pattern:

```rust
use std::{sync::Arc, time::Duration};

use anyhow::{bail, Result};
use parlando_server::{
    AgentFactory, ExperimentConfig, RemoteGrpcAgentConfig, RemoteGrpcAgentFactory,
};
use serde::Deserialize;
use serde_json::Value;

pub fn factory_from_config(
    config: &ExperimentConfig,
) -> Result<Option<Arc<dyn AgentFactory<MyGameAdapter>>>> {
    if config.agents.mode != parlando_server::config::AgentsMode::HumanVsAgent {
        return Ok(None);
    }
    let human_vs_agent = config.agents.human_vs_agent.as_ref().ok_or_else(|| {
        anyhow::anyhow!("agents.human_vs_agent is required when agents.mode is human_vs_agent")
    })?;
    match human_vs_agent.factory.as_deref().unwrap_or("<game-slug>.demo_agent") {
        "<game-slug>.demo_agent" => Ok(Some(Arc::new(DemoAgentFactory))),
        "remote_grpc" | "parlando.remote_grpc" => {
            let config = RemoteAgentSelectorConfig::from_value(
                human_vs_agent.config.clone(),
                human_vs_agent.act_timeout_seconds,
            )?;
            Ok(Some(Arc::new(RemoteGrpcAgentFactory::<MyGameAdapter>::new(config))))
        }
        other => bail!("unknown agent factory selector: {other}"),
    }
}

#[derive(Debug, Deserialize)]
struct RemoteAgentSelectorConfig {
    endpoint: String,
    #[serde(default = "default_remote_agent_name")]
    agent_name: String,
    agent_version: Option<String>,
    #[serde(default = "default_remote_agent_protocol")]
    protocol_version: String,
}

impl RemoteAgentSelectorConfig {
    fn from_value(value: Value, act_timeout_seconds: f64) -> Result<RemoteGrpcAgentConfig> {
        let selector: Self = serde_json::from_value(value)?;
        let mut config = RemoteGrpcAgentConfig::new(selector.endpoint, selector.agent_name);
        config.agent_version = selector.agent_version;
        config.protocol_version = selector.protocol_version;
        config.request_timeout = Duration::from_secs_f64(act_timeout_seconds.max(0.1));
        Ok(config)
    }
}

fn default_remote_agent_name() -> String {
    "remote-agent".to_string()
}

fn default_remote_agent_protocol() -> String {
    "parlando-agent-v2".to_string()
}
```

Remote-agent YAML:

```yaml
agents:
  mode: human_vs_agent
  human_vs_agent:
    factory: remote_grpc
    act_timeout_seconds: 5
    invalid_action_limit: 3
    config:
      endpoint: http://127.0.0.1:50051
      agent_name: first-action-agent
      agent_version: dev
      protocol_version: parlando-agent-v2
```

The gRPC create request contains the controlled role and seed/config. Observation RPCs carry state snapshots, accepted actions, or messages. Decision RPCs carry optional role-specific available actions. Decision responses contain optional `message` and optional JSON `action`; at least one must be present when a response is returned.

For hosted deployment, make sure the Rust service can reach the gRPC endpoint. Common options are a second service on the same private network, an agent process in the same container, or a protected separate host.

If the remote agent uses an LLM, generate explicit server/agent-side environment variables for the provider credentials and model name, document them in the private config or deployment secret instructions, and keep prompts free of game secrets that participants should not see. The Rust game server should still validate every returned action.

## Python gRPC SDK Agent

When the user asks for a Python agent, generate a small Python package or script that uses `parlando_agent_sdk`. The game-specific code implements observation callbacks only when it needs memory, implements `GameAgent.maybe_act(available_actions)`, and starts `serve_agent(...)`.

For chat-relevant agents, implement `observe_message(self, speaker, kind, text)` in the generated Python agent and store the utterance before `maybe_act` decides. Return `AgentResponse.say(...)` for message-only replies and `AgentResponse.action_with_message(...)` when the reply should accompany a game action.

Install/setup notes for the generated README:

```sh
cd agent
python -m venv .venv
. .venv/bin/activate
python -m pip install parlando-agent-sdk
python -m parlando_agent_sdk.generate_protos
python my_agent.py
```

If `parlando-agent-sdk` is not published in the target environment, say so and point the user at the local SDK packaging/install step requested by their project. Do not make the generated Rust game depend on a Python source checkout.

Minimal Python agent:

```python
from typing import Any

from parlando_agent_sdk import AgentResponse, GameAgent, serve_agent


class FirstAvailableActionAgent(GameAgent):
    async def maybe_act(
        self, available_actions: list[dict[str, Any]] | None
    ) -> AgentResponse | None:
        if available_actions:
            return AgentResponse.action(available_actions[0])
        return None


if __name__ == "__main__":
    serve_agent(FirstAvailableActionAgent, host="127.0.0.1", port=50051)
```

For a stateful Python policy, use a factory function so each Parlando agent participant gets its own Python object:

```python
from typing import Any

from parlando_agent_sdk import AgentResponse, GameAgent, serve_agent


class ScriptedAgent(GameAgent):
    def __init__(self, role: str, seed: int | None, config: dict[str, Any]) -> None:
        self.role = role
        self.seed = seed
        self.config = config
        self.turn = 0

    async def observe_message(self, speaker: str, kind: str, text: str) -> None:
        self.last_message = (speaker, kind, text)

    async def maybe_act(
        self, available_actions: list[dict[str, Any]] | None
    ) -> AgentResponse | None:
        self.turn += 1
        if available_actions:
            action = available_actions[0]
            return AgentResponse.action_with_message(action, "I will take the first available action.")
        return AgentResponse.say("I am waiting.")

    async def close(self) -> None:
        return None


def create_agent(context: dict[str, Any]) -> ScriptedAgent:
    return ScriptedAgent(
        role=context["role"],
        seed=context.get("seed"),
        config=context.get("config") or {},
    )


if __name__ == "__main__":
    serve_agent(create_agent, host="127.0.0.1", port=50051)
```

Python response helpers:

- return `None` from `maybe_act` to skip responding.
- `AgentResponse.say("text")`
- `AgentResponse.action({...})`
- `AgentResponse.action_with_message({...}, "text")`

Returned actions must exactly match the game action JSON schema defined in the generated TypeScript/Rust contract. If the game uses server-provided `available_actions`, prefer choosing from that list for simple agents.

Connect the Python service from the game config:

```yaml
agents:
  mode: human_vs_agent
  human_vs_agent:
    factory: remote_grpc
    act_timeout_seconds: 5
    invalid_action_limit: 3
    config:
      endpoint: http://127.0.0.1:50051
      agent_name: first-available-action-agent
      agent_version: dev
      protocol_version: parlando-agent-v2
```

Tell the user which processes to run:

1. Start the Python agent service, for example `python agent/my_agent.py`.
2. Start the Rust game server with the config file that sets `factory: remote_grpc`.
