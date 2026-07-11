# Remote Agent gRPC Design

## Context

Parlando must support agents implemented outside the Rust server binary, especially Python agents. Speed matters because future experiments may include reinforcement learning loops with many repeated agent decisions. The Rust server should remain authoritative for sessions, game state, validation, persistence, conversation, and completion.

## Decision

Use gRPC as the first remote-agent transport. Keep the Rust-side runtime transport-agnostic by adapting remote agents to the same `AgentFactory<A>` and `GameAgent<A>` traits used by in-process Rust agents.

The intended backends are:

- Local Rust factories for built-in/simple agents.
- `RemoteGrpcAgentFactory` for Python or other language agents running in a separate process.

## Protocol Shape

Define `parlando-agent-protocol.proto` with messages for:

- agent initialization, including role, room id, participant session id, seed, protocol version, agent name/version, and config.
- act requests, including observation, available actions, conversation context, invalid action count, last error, and completion flag.
- act results: none, message, action, or action with message.
- structured errors and optional cleanup/shutdown.

Game-specific observations and actions cross the process boundary as protobuf `Struct` or equivalent JSON-compatible values. Inside the linked Rust binary they remain typed and are parsed/validated by the game adapter.

## Python API Goal

Python agent authors should not write gRPC plumbing. A Python SDK should expose a small API like:

```python
class MyAgent(GameAgent):
    async def act(self, observation, available_actions, context):
        return AgentResult.action(available_actions[0])
```

The SDK owns protobuf conversion, request correlation, server startup, error mapping, and cleanup hooks.

## Tradeoffs

gRPC is more structured than WebSocket and requires protobuf definitions/codegen, but it gives stronger cross-language contracts, lower serialization overhead than JSON WebSocket, built-in deadlines/status codes, and mature Rust/Python support.

WebSocket remains a possible future lightweight transport, but it should not be the first remote-agent protocol because platform neutrality and RL-scale performance are higher priorities.

## Follow-Up

- Add gRPC/protobuf dependencies only when implementing Step 10.
- Store remote agent identities as `participant_kind = agent`, `identity_provider = remote_grpc`, and `external_id = <agent_name>@<agent_version>`.
- Store protocol version and config hash in participant metadata; do not store auth tokens or secrets.
- Test `RemoteGrpcAgentFactory` with a mocked gRPC service and ensure returned actions still go through normal Rust validation and persistence.
