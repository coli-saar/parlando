# Step-By-Step Implementation Plan: Parlando Rust Server

## Summary

Implement Parlando as a fresh Rust workspace with two crates: `parlando-server` for reusable server/runtime behavior and `parlando-space-game` for the concrete Space Game adapter, state engine, binary, and agent factories. Build in vertical slices so each phase compiles, has tests, and preserves the existing TypeScript client contract.

## Implementation Steps

### 1. Workspace Skeleton

- Create root `Cargo.toml` workspace with `crates/parlando-server` and `crates/parlando-space-game`.
- Add baseline dependencies: `tokio`, `axum`, `serde`, `serde_json`, `serde_yaml`, `clap`, `thiserror`, `anyhow`, `async-trait`, `uuid`, `chrono` or `time`, `sqlx`, `reqwest`, `tokio-tungstenite`, `tower-http`.
- Add minimal library exports and a `parlando-space-game` binary that parses `--config`, `--host`, and `--port`.
- Acceptance: `cargo check` passes for empty server and binary.

### 2. Protocol And Config Foundation

- Port protocol structs from Python/TypeScript with exact snake_case JSON names.
- Port YAML loading: `extends`, `include/includes`, optional includes, `${ENV_VAR}`, relative path resolution, and `EXPERIMENT_AGENTS_MODE`.
- Accept legacy Python-only config fields as ignored transitional fields with warnings.
- Add validation for LiveKit, Speechmatics, and TTS credentials only when needed by enabled features.
- Acceptance: config unit tests pass against copied/adapted current YAML fixtures.

### 3. Core Traits And App State

- Define `GameAdapter`, `GameAgent`, `AgentFactory`, `AgentInitContext`, `AgentActContext`, and `AgentResult`.
- Define shared `AppState<A>` with adapter, config, storage, connection hubs, transcript hubs, and optional agent factory.
- Keep game state generic inside the server and convert to JSON only at protocol/storage boundaries.
- Acceptance: a tiny test adapter can instantiate the reusable server state.

### 4. Evaluation Storage Interface And SQLite v1

- Define a general async storage trait for experiments, durable participants, sessions, session participants, consent declarations, and ordered session events.
- Implement SQLite first using `database.url = sqlite:///...`; fail clearly for unsupported schemes.
- Add in-memory storage only for focused unit tests.
- Persist timestamps for every stored record.
- Acceptance: DB-backed tests prove evaluation entities, unusual participant/session shapes, consent declarations, and session events are inserted and exported.

### 5. Participants, Rooms, Consent, And Matchmaking

- Implement `/health`, `/api/config`, participant creation, direct start/enter/wait, consent, room create/join, matchmaking join/status.
- Preserve roles `A` and `B`; reconnecting participants keep their existing role and additional player joins are rejected.
- In human-vs-agent mode, create a human room with role `A` and an agent participant with role `B`.
- Acceptance: HTTP integration tests cover human-vs-human and human-vs-agent room creation.

### 6. Space Game Port

- Port Space Game level data, state engine, adapter, observations, available actions, events, and completion summary into `parlando-space-game`.
- Keep state JSON shape client-compatible.
- Implement `factory_from_config(&ExperimentConfig)` for known Space Game agent selectors, including the current back-and-forth agent.
- Acceptance: Rust tests match the existing Python Space Game behavior for launch prerequisites, oxygen fan trip, observation filtering, available actions, and event generation.

### 7. WebSocket Runtime

- Implement `/ws/game/{room_id}?participantSessionId=...`.
- Support `ready`, `heartbeat`, `submitAction`, `consentUpdated`, and `sendChatMessage`.
- Emit `roleAssigned`, `presenceChanged`, `voiceStatusChanged`, `stateChanged`, `conversationMessageAdded`, `completed`, and `error`.
- Do not store generic "move count" in the reusable server; persist action/event/state snapshots and let the game state define any counters.
- Acceptance: WebSocket tests cover connect, presence, rejected early actions, accepted actions, chat, completion, and reconnect.

### 8. Conversation, Transcripts, Diagnostics, And Export

- Implement conversation endpoints, transcript POST/GET/SSE, transcription context, voice diagnostics, and admin export.
- Transcript posts must create both transcript records and `origin: "voice_transcript"` conversation messages.
- Typed chat uses `origin: "typed"`; agent messages use `origin: "agent"`.
- Export reads through the storage trait and returns the full persisted timeline.
- Acceptance: transcript and conversation tests pass with SQLite persistence and SSE event delivery.

### 9. LiveKit And Speechmatics

- Implement LiveKit identity formatting/parsing and HS256 token signing.
- Implement participant and worker token endpoints.
- Implement audio-session responses matching the current client:
  - disabled when LiveKit is disabled
  - `livekit-combined` when not using Speechmatics
  - split `livekit-partner` plus `speechmatics-transcription` when Speechmatics is enabled
- Implement Speechmatics temporary key minting with mocked tests.
- Acceptance: token claims and audio-session JSON match Python/client expectations.

### 10. Agent Runtime

- In `parlando-server`, create fresh agents from `Option<Arc<dyn AgentFactory<A>>>` per agent participant.
- Add a transport-agnostic remote agent backend using gRPC as the first network protocol, so agents can be implemented in Python or other languages without changing the Rust-side agent runtime.
- Define a `parlando-agent-protocol.proto` contract for agent initialization, action requests, action results, errors, and optional shutdown/cleanup. Use protobuf `Struct`/JSON-compatible fields at the process boundary for game-specific observations, available actions, and returned actions.
- Implement a `RemoteGrpcAgentFactory` that satisfies the same Rust-side `AgentFactory<A>`/`GameAgent<A>` interface as in-process agents. The server runtime should not know whether an agent is local Rust or remote gRPC.
- Provide a Python agent SDK/server wrapper that hides gRPC details behind a clean Python API where authors implement an async `act(observation, available_actions, context)` method.
- Run the agent loop with readiness waiting, act timeout, invalid action limit, last error tracking, completion awareness, conversation history, and normal room action submission.
- Persist agent lifecycle, actions, messages, and errors.
- Persist remote-agent identity as `participant_kind = agent`, `identity_provider = remote_grpc`, and `external_id = <agent_name>@<agent_version>`; store protocol version and config hash in metadata, but never secrets.
- Acceptance: tests prove one fresh local or remote agent per room, gRPC request/response behavior with a mocked service, messages become conversation entries, actions go through Rust validation, timeouts/errors are persisted, and invalid agents stop cleanly.

### 11. ElevenLabs TTS

- Implement ElevenLabs streaming WebSocket client and output-format-to-sample-rate mapping.
- Implement per-room TTS task that polls new `origin: "agent"` messages, marks message IDs seen before speaking, streams audio chunks, and records diagnostics.
- Acceptance: mocked WebSocket tests cover streaming frames, final frames, failure continuation, and diagnostic sequence.

### 12. LiveKit Agent Audio Publishing Spike

- Spike Rust LiveKit RTC publishing for a mono `agent-voice` track using decoded PCM chunks.
- If viable, integrate it into the TTS task.
- If not viable, implement a narrow audio-publishing sidecar while keeping Rust authoritative for state, storage, tokens, agents, conversation, and diagnostics.
- Acceptance: ignored/manual live test proves browsers hear the agent voice, or the sidecar fallback is documented and smoke-tested.

### 13. Static Serving And Deployment

- Implement static client serving from `server.client_dist_path`, including `/assets/*`, `/`, frontend fallback, backend prefix exclusion, and path traversal prevention.
- Implement CLI/env port precedence: `--port`, then `PORT`, then local default.
- Add Docker/Render packaging for building the existing frontend and Rust binary.
- Acceptance: local server can serve the existing client without client protocol changes.

### 14. End-To-End Validation

- Run full automated test suite with SQLite-backed integration tests.
- Run browser smoke tests against the existing TypeScript client:
  - two participants join
  - WebSocket gameplay works
  - typed chat works
  - transcript POST appears in conversation
  - LiveKit token/audio-session shape is accepted
  - Speechmatics temporary key flow works with credentials
  - human-vs-agent produces agent messages and TTS diagnostics
- Acceptance: Rust server is a drop-in replacement for the Python server from the client perspective.

## Assumptions

- The saved high-level plan lives at `plans/parlando-rust-server-v1.md` and remains the product-level source of truth.
- The first production storage backend is SQLite, but all server logic depends on a general storage trait.
- Existing YAML files are reused and adapted, with dynamic Python import fields treated as transitional compatibility fields.
- `parlando-space-game` owns all Space Game-specific adapter and agent construction.
- The reusable server never interprets game-specific move counters; it stores actions, state snapshots, and timestamps.
