# Agents

Parlando supports agents as first-class participants. An agent receives the same role-specific observation and optional available-action list that a human UI receives, and any returned action goes through the normal server validation path.

Agents can run in two ways:

- in process, written in Rust and linked into the game binary.
- out of process, written in Python or another language and connected over gRPC.

## Rust Agents

In-process Rust agents implement `GameAgent<A>` and are created by an `AgentFactory<A>`.

```rust
#[async_trait::async_trait]
impl GameAgent<MyAdapter> for MyAgent {
    async fn act(
        &mut self,
        observation: MyObservation,
        available_actions: Option<Vec<MyAction>>,
    ) -> anyhow::Result<AgentResult<MyAction>> {
        if let Some(actions) = available_actions {
            if let Some(action) = actions.into_iter().next() {
                return Ok(AgentResult::Action(action));
            }
        }
        Ok(AgentResult::None)
    }
}
```

The server creates one mutable agent instance per agent participant. If an agent needs memory, store it in that agent instance. The server intentionally does not pass room ids, participant-session ids, conversation history, invalid-action counts, or completion flags into `act`.

The demo game's factory selector lives in `crates/parlando-space-game/src/agents.rs`. It currently supports:

- `space_game.back_and_forth`: deterministic in-process demo agent.
- `remote_grpc` or `parlando.remote_grpc`: remote gRPC bridge for Python or another language.

## Python Agents Over gRPC

Remote agents let researchers write policies in Python while keeping the Rust server authoritative.

Install the SDK dependencies in your Python environment, generate protobuf modules from a checkout, and run an agent service:

```sh
cd python/parlando-agent-sdk
python -m pip install -e .
python -m parlando_agent_sdk.generate_protos
python my_agent.py
```

Minimal `my_agent.py`:

```python
from parlando_agent_sdk import AgentResult, GameAgent, serve_agent

class FirstActionAgent(GameAgent):
    async def act(self, observation, available_actions):
        if available_actions:
            return AgentResult.action(available_actions[0])
        return AgentResult.none()

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
      protocol_version: parlando-agent-v1
```

The gRPC request contains:

- the role controlled by the agent.
- the seed and agent config from YAML.
- the role-specific observation.
- optional role-specific available actions.

The gRPC response may be:

- `none`
- `message`
- `action`
- `action_with_message`

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

## Reference Files

- `crates/parlando-server/src/agents.rs`: local agent traits.
- `crates/parlando-server/src/remote_agent.rs`: Rust gRPC bridge.
- `crates/parlando-server/proto/parlando_agent_v1.proto`: remote-agent protocol.
- `crates/parlando-space-game/src/agents.rs`: demo local and remote agent selection.
- `python/parlando-agent-sdk`: Python agent SDK.
