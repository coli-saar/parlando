# Remote Agent gRPC Design

## Context

Parlando must support agents implemented outside the Rust server binary, especially Python agents. Speed matters because future experiments may include reinforcement learning loops with many repeated agent decisions. The Rust server should remain authoritative for sessions, game state, validation, persistence, conversation, and completion.

## Decision

Use gRPC as the first remote-agent transport. Keep the Rust-side runtime transport-agnostic by adapting remote agents to the same `agent::Factory<A>` and `agent::Agent<A>` traits used by in-process Rust agents.

The intended backends are:

- Local Rust factories for built-in/simple agents.
- `agent::grpc::RemoteAgent` for Python or other language agents running in a separate process.

## Protocol Shape

The protobuf contract lives at `rust-server/proto/parlando_agent_v3.proto` and defines the `parlando.agent.v3` package for:

- agent initialization, including role, seed, protocol version, agent name/version, and config.
- observation requests for role-specific state snapshots, accepted actions, and messages.
- decision requests with the same optional role-specific available actions that a human UI would receive.
- a terminal callback carrying the same game-specific completion value delivered to human clients.
- non-empty decision responses containing an action, a player message, or both.
- structured errors and optional cleanup/shutdown.

Game-specific observations and actions cross the process boundary as protobuf `Struct` or equivalent JSON-compatible values. Inside the linked Rust binary they remain typed and are parsed/validated by the game.

Do not expose room ids, participant-session ids, conversation history, invalid-action counts, last errors, or internal runtime status through the agent API. The game-specific `Completion` value is domain information and is deliberately delivered to agents through `Finish`, just as it is delivered to human clients through `completed`. A remote agent process that needs memory should keep it in its own per-agent instance or stream/session state.

## Python API Goal

Python agent authors should not write gRPC plumbing. A Python SDK should expose a small API like:

```python
class MyAgent(Agent):
    async def respond(self, available_actions):
        if not available_actions:
            return None
        return Response.action(available_actions[0])
```

The SDK owns protobuf conversion, request correlation, server startup, error mapping, and cleanup hooks.

## Tradeoffs

gRPC is more structured than WebSocket and requires protobuf definitions/codegen, but it gives stronger cross-language contracts, lower serialization overhead than JSON WebSocket, built-in deadlines/status codes, and mature Rust/Python support.

WebSocket remains a possible future lightweight transport, but it should not be the first remote-agent protocol because platform neutrality and RL-scale performance are higher priorities.

## Follow-Up

- Add a real Python-process integration test after installing the SDK dependencies in a test environment.
- Store a config hash in remote-agent metadata if we need stronger reproducibility than the current name/version/protocol metadata.

## Current Implementation

- `parlando` exposes the non-generic `agent::grpc::RemoteAgent`; its transport settings remain private.
- The factory implements the same `agent::Factory<A>` trait as in-process Rust agents.
- `RemoteGrpcAgent` lazily connects to the configured gRPC endpoint, sends one `CreateAgent` request, forwards observations through `Start`, `ObserveTransition`, and `ObserveMessage`, delivers the shared terminal result through `Finish`, and asks for responses through `Respond`.
- Returned actions are deserialized into the game-specific Rust action type and still pass through normal server validation before changing game state.
- Remote gRPC factories require a non-empty `agent_version` and export durable participant metadata as `participant_kind = agent`, `identity_provider = remote_grpc`, and `external_id = <agent_name>@<agent_version>`. Administration and research exports use a descriptive identifier such as `agent:remote_grpc:<agent_name>@<agent_version>` rather than a human-style random name. Historical stored participants without version metadata remain readable as `unversioned`.
- The Python SDK wrapper lives in `python/parlando-agent-sdk`. It provides `Agent`, `Response`, `serve`, a protobuf generation command, and a bundled copy of the protocol.
- The mock-client integration suite starts an in-process tonic gRPC service and verifies remote-agent messages, actions, terminal delivery, and persisted session events through the real HTTP/WebSocket server.

## Remaining Risks

- The Python SDK has unit coverage for callback delegation and protobuf conversion, but a separate Python-process integration test remains useful for packaging and process-boundary failures.
- The current gRPC boundary uses protobuf `Struct`, which keeps the contract language-neutral but still imposes JSON-compatible shapes for game-specific observations/actions outside the Rust binary.
- Remote-agent metadata currently records protocol/name/version but not a config hash.
