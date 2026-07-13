# Agents

Parlando supports agents as first-class participants. An agent observes role-specific state, accepted actions, and conversation messages as they happen. After each observation, the runtime asks whether the agent wants to respond. Any returned action goes through the normal server validation path.

Use agents when you want to compare human partners with automated partners, run controlled partner policies, prototype human-AI interaction, or keep a scripted participant available during development. You do not need an agent for a human-vs-human study.

Agents can run in two ways:

- in process, written in Rust and linked into the game binary.
- out of process, written in Python or another language and connected over gRPC.

## Rust Agents

In-process Rust agents implement `GameAgent<A>` and are created by an `AgentFactory<A>`.

```rust
#[async_trait::async_trait]
impl GameAgent<MyAdapter> for MyAgent {
    async fn observe_action(
        &mut self,
        actor: PlayerRole,
        action: MyAction,
        resulting_observation: MyObservation,
    ) -> anyhow::Result<()> {
        self.last_observation = Some(resulting_observation);
        self.last_actor = Some(actor);
        self.last_action = Some(action);
        Ok(())
    }

    async fn observe_message(
        &mut self,
        speaker: PlayerRole,
        kind: AgentUtteranceKind,
        text: String,
    ) -> anyhow::Result<()> {
        self.messages.push((speaker, kind, text));
        Ok(())
    }

    async fn maybe_act(
        &mut self,
        available_actions: Option<Vec<MyAction>>,
    ) -> anyhow::Result<Option<AgentResponse<MyAction>>> {
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

The server creates one mutable agent instance per agent participant. If an agent needs memory, store it in that agent instance. The server intentionally does not pass room ids, participant-session ids, conversation history, invalid-action counts, or completion flags into the agent callbacks.

The demo game's factory selector lives in `space-game/server/src/agents.rs`. It currently supports:

- `space_game.back_and_forth`: deterministic in-process demo agent.
- `remote_grpc` or `parlando.remote_grpc`: remote gRPC bridge for Python or another language.

## Python Agents Over gRPC

Remote agents let researchers write policies in Python while keeping the Rust server authoritative.

Install the SDK dependencies in your Python environment, generate protobuf modules from a checkout, and run an agent service:

```sh
cd rust-server/python/parlando-agent-sdk
python -m pip install -e .
python -m parlando_agent_sdk.generate_protos
python my_agent.py
```

Minimal `my_agent.py`:

```python
from parlando_agent_sdk import AgentResponse, GameAgent, serve_agent

class FirstActionAgent(GameAgent):
    async def maybe_act(self, available_actions):
        if available_actions:
            return AgentResponse.action(available_actions[0])
        return None

serve_agent(FirstActionAgent, host="127.0.0.1", port=50051)
```

Configure the game server to use it:

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

The gRPC create request contains:

- the role controlled by the agent.
- the seed and agent config from YAML.

The observation RPCs contain:

- role-specific state snapshots.
- accepted actions with actor role and resulting observation.
- conversation messages with speaker role, modality, and text.

Decision RPCs contain optional role-specific available actions. The response contains optional `message` and optional `action`; at least one must be present when a response is returned.

Returned actions must be JSON-compatible dictionaries matching the game action schema. In the demo game, a movement action looks like:

```python
{"type": "moveStep", "player": "B", "direction": "up"}
```

## Deployment Notes

For local experiments, the Rust server can call `http://127.0.0.1:50051` if the Python agent runs on the same machine.

For hosted deployments, the gRPC endpoint must be reachable from the Rust service. Common options are:

- run the Python agent as a second service on the same private network.
- package the agent into the same container and supervise both processes.
- deploy the agent on a separate host with a private or protected endpoint.

Record the agent name and version in config. Parlando persists remote-agent participant metadata as `identity_provider = remote_grpc` and `external_id = <agent_name>@<agent_version>`, which helps later analysis distinguish policies.

If the agent uses an LLM or another hosted model, keep provider credentials in the agent process or server-side configuration. Do not put model credentials in browser code or public config.

## Reference Files

- `rust-server/src/agents.rs`: local agent traits.
- `rust-server/src/remote_agent.rs`: Rust gRPC bridge.
- `rust-server/proto/parlando_agent_v1.proto`: remote-agent protocol.
- `space-game/server/src/agents.rs`: demo local and remote agent selection.
- `rust-server/python/parlando-agent-sdk`: Python agent SDK.
