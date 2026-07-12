# Parlando Rust Server v1

Source context: this plan revises `../../space-game/plans/rust-port-speechmatics-elevenlabs-livekit-plan.md` for the new Parlando Rust workspace.

## Summary

Build a fresh Rust workspace in `/Users/koller/Documents/workspace/parlando/rust-server` as a drop-in replacement for the existing Python server from the TypeScript client's perspective. The workspace has two crates: a reusable Parlando server library and a Space Game server crate that links the Space Game adapter and agent implementations at compile time.

Use the existing YAML config files as the starting point and adapt them for Rust by removing dynamic Python import behavior while preserving the rest of the operational config.

## Crates And Architecture

- Create a Rust workspace with:
  - `rust-server`: reusable server library with HTTP/WebSocket protocol, config, storage interface, room runtime, LiveKit, Speechmatics, conversation, agent runtime, TTS, export, and static serving.
  - `space-game/server`: Space Game-specific crate with binary entrypoint, game state engine, adapter, game-specific agent factories, and game-specific agent implementations.
- The Space Game binary constructs `SpaceGameAdapter`, loads `ExperimentConfig`, resolves the configured Space Game agent factory, and passes both to `parlando-server`.
- The reusable server receives `Option<Arc<dyn AgentFactory<SpaceGameAdapter>>>`, not an agent instance. It creates a fresh `GameAgent` per agent participant/room.
- Do not support Python dynamic imports, `game.adapter`, `game.module_paths`, or a generic configured-app entrypoint.
- Preserve existing client protocol shapes exactly: endpoint paths, JSON field names, WebSocket message types, LiveKit identity format, Speechmatics audio-session sink plans, transcript/conversation payloads, and static frontend behavior.

## Agent Model

- Define reusable traits in `parlando-server`:

```rust
#[async_trait::async_trait]
pub trait GameAgent<A: GameAdapter>: Send {
    async fn act(
        &mut self,
        observation: serde_json::Value,
        available_actions: Vec<serde_json::Value>,
        context: AgentActContext,
    ) -> anyhow::Result<AgentResult>;
}

pub trait AgentFactory<A: GameAdapter>: Send + Sync + 'static {
    fn create(&self, context: AgentInitContext) -> anyhow::Result<Box<dyn GameAgent<A> + Send>>;
}
```

- Keep YAML-driven agent selection. `agents.human_vs_agent.factory` may select either an in-process Rust game factory or a remote gRPC agent backend.
- Implement `parlando-space-game::agents::factory_from_config(&ExperimentConfig)`:
  - returns `Ok(None)` unless `agents.mode == human_vs_agent`
  - validates `agents.human_vs_agent`
  - maps known selector strings such as `space_game.back_and_forth` to concrete Rust factories
  - maps remote selectors such as `remote_grpc` to a reusable `RemoteGrpcAgentFactory`
  - fails clearly on unknown selectors
- Define a language-neutral gRPC remote-agent protocol for agent init, act requests, act results, errors, and cleanup. Game-specific observations/actions cross the network as protobuf `Struct`/JSON-compatible values; inside the Rust binary they remain typed.
- Provide a Python SDK/server wrapper so Python agent authors implement a clean async `act(observation, available_actions)` API rather than hand-writing protocol code.
- The reusable server owns agent lifecycle behavior:
  - create an agent participant in role `B`
  - call the factory once per agent participant
  - pass role, room id, participant id, seed, and agent config through `AgentInitContext`
  - wait until the room is ready
  - call `act` with timeout, conversation history, game observation, available actions, invalid action count, last error, and completion flag
  - submit returned actions through normal room action handling
  - record returned messages as `origin: "agent"`
  - persist agent actions, messages, errors, and lifecycle events
  - treat local Rust and remote gRPC agents identically after factory creation

## Config And Runtime Behavior

- Port YAML loading with `extends`, `include/includes`, optional includes, `${ENV_VAR}` substitution, relative path resolution, and `EXPERIMENT_AGENTS_MODE`.
- Continue accepting the current YAML files, but treat Python-only `game.adapter`, `game.module_paths`, and Python import-style agent factory strings as transitional fields. The Space Game crate may map existing known strings to Rust factories where practical.
- Keep runtime config sections for `study`, `direct`, `server`, `database`, `livekit`, `speechmatics`, `transcription`, `tts`, `conversation`, and `agents`.
- Port all v1 endpoints and `/ws/game/{room_id}?participantSessionId=...` as a client-compatible replacement for the Python server.
- Remove reusable-server assumptions about "moves" as a generic concept. The reusable server stores game events and state snapshots, but game-specific counters or summaries come only from the game adapter's serialized state and completion summary.

## Database And Storage

- Define a general async storage trait in `parlando-server`, then implement SQLite first behind that trait.
- The storage interface must persist everything that happens in the game with timestamps:
  - participant sessions
  - rooms
  - room participants and connection/role changes
  - consent decisions
  - submitted game actions/events
  - authoritative game state snapshots after accepted actions
  - WebSocket-relevant presence and completion events
  - transcript segments
  - voice diagnostics
  - conversation messages
  - agent actions/messages/errors
  - TTS diagnostics
  - completion records
- Keep an in-memory implementation only for focused tests; production/server runs should use the configured storage backend.
- Use `database.url` in the existing YAML as the first backend selector. `sqlite:///...` instantiates the SQLite implementation. Other schemes should fail clearly until implemented.
- Admin export should read from the storage trait and include the full persisted session timeline.

## Game, Audio, And TTS

- Port the Space Game engine into `parlando-space-game`, preserving existing state shape, observations, available actions, event payloads, and completion behavior.
- Implement room creation/join for human-vs-human and human-vs-agent studies. In agent mode, the server supplies the selected Rust agent as role B when the room is created.
- Implement LiveKit token signing and audio-session responses with the existing client-compatible shapes.
- Implement Speechmatics temporary-key minting, transcript ingestion, voice diagnostics, and transcript-to-conversation conversion through the session event stream.
- Implement ElevenLabs streaming TTS for `origin: "agent"` messages directly when the agent runtime emits them; do not poll conversation history.
- Spike Rust LiveKit RTC audio publishing for the `agent-voice` track. If Rust RTC publishing is not viable, keep the fallback limited to an audio-publishing sidecar; Rust remains authoritative for config, state, storage, agents, conversation, tokens, and diagnostics.

## Test Plan

- Add unit tests for config loading, protocol serialization, agent factory selection, per-room agent instantiation, LiveKit tokens, Speechmatics key parsing, storage trait behavior, SQLite persistence, Space Game state parity, and removal of reusable move-count assumptions.
- Add integration tests for participants, consents, rooms, joins, WebSockets, action submission, completion, transcript posting, conversation, export, reconnect behavior, and human-vs-agent lifecycle.
- Add DB-backed tests asserting that every accepted user/server/agent/game/audio event is persisted with timestamps and can be exported.
- Add mocked external-service tests for Speechmatics and ElevenLabs.
- Add ignored/manual live tests for real Speechmatics, LiveKit client audio, and LiveKit agent audio publishing.
- Validate against the existing TypeScript client without changing its protocol expectations.
