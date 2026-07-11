# Steps 1-5 Foundation Decisions

## Context

The first Parlando implementation checkpoint covers the Rust workspace skeleton, protocol/config foundation, core server traits/app state, the first storage backend, and participant/session room creation flows. The server and game crate are linked into one binary, so game internals should stay typed and only serialize at HTTP, WebSocket, and persistence boundaries.

## Chosen Approach

- Keep two crates: `parlando-server` for reusable runtime behavior and `parlando-space-game` for concrete game logic.
- Use associated types on `GameAdapter` for `State`, `Action`, `Observation`, `Event`, and `Summary`.
- Parse client JSON into a typed game action once at the server boundary.
- Keep YAML compatibility with the Python-era config files, including `extends`, `include/includes`, optional includes, environment substitution, and relative path resolution.
- Use a general `ExperimentStore` trait for durable evaluation data, with SQLite as the first implementation.
- Model experiments, durable participant identities, sessions, session-local participants, consent declarations, and ordered session events relationally.
- Keep `MemoryState` only as an active-game runtime cache for WebSockets, broadcasts, and in-progress game execution.
- Preserve client-facing `room_id` and `participant_session_id` while storing evaluation-facing `experiment_id`, integer `session_id`, and integer `participant_id`.
- Cover human-vs-human and human-vs-agent room creation through router-level HTTP tests rather than only unit-level helpers.

## Tradeoffs

- A typed adapter interface is more verbose than `serde_json::Value`, but it prevents accidental JSON-shaped game logic inside the linked binary.
- The evaluation schema is more specific than a generic event log, but it gives researchers stable keys for experiment/session exports.
- A single `session_events` table stores session activity once. It avoids redundant category/audit tables while still supporting ordered reconstruction and event-type filtering.
- The config loader accepts legacy Python fields instead of rejecting them so existing YAML files remain usable during migration.
- Step-5 tests use the Axum router directly, which keeps them fast while still exercising real routes and JSON shapes.

## Risks And Follow-Up

- Recovery of full active runtime state from the evaluation database is still future work.
- The runtime cache must not become a hidden persistence layer; later steps should keep moving semantic writes into `ExperimentStore`.
- Endpoint-level tests should expand beyond storage-level tests as participant, session, WebSocket, transcript, and agent flows mature.
- Config validation should remain compatible with existing YAML files as real deployments are tested.
- Public rustdoc and function comments need to continue expanding as each later step is implemented.

# Step 6 Space Game Boundary Decisions

## Context

The Space Game crate is linked into the same Rust binary as the reusable server. The boundary should therefore be a typed Rust boundary, not an internal JSON boundary, while still preserving the existing TypeScript client protocol at transport edges.

## Chosen Approach

- Keep `GameAdapter` as the only reusable-server interface for game semantics.
- Let `GameAdapter::parse_action` default to serde deserialization so concrete games do not repeat JSON parsing boilerplate.
- Treat `Seat` as exactly `A` or `B`; spectator-era optional role conversion was removed from the server path.
- Keep agent factory selection in `parlando-space-game`, because selectors such as `space_game.back_and_forth` are game-specific.
- Test client JSON compatibility only at protocol-shape assertions; all state transition tests operate on typed Rust structs.

## Tradeoffs

- The Space Game adapter still implements the semantic methods explicitly, which makes the boundary clear at the cost of a small delegating layer.
- Default JSON parsing in the reusable trait reduces boilerplate, but games can still override parsing if they need custom compatibility behavior.
- Removing optional player roles simplifies the runtime and agent loop, but any future observer role must be introduced as a separate server concept rather than silently reusing player seats.

## Risks And Follow-Up

- Remote gRPC agents will need an explicit serialization boundary because they are out of process; that should be isolated to the remote-agent adapter rather than leaking JSON handling into in-process game logic.
- As more games are added, we should revisit whether helper traits for action ownership or observation construction reduce repeated code without hiding game semantics.

# Server-Owned Role Assignment

## Context

The early HTTP request structs exposed a caller-controlled role override for tests and debugging. Production room assignment does not need callers to request a role, and exposing such a field creates edge cases around duplicate roles and invalid session layouts.

## Chosen Approach

- Remove caller-controlled role overrides from the public room create/join protocol.
- Assign role `A` to the room creator and role `B` to the next distinct participant.
- Preserve an existing participant's role on rejoin.
- Reject caller-controlled role fields in room create/join JSON by denying unknown fields on those request bodies.
- Record `participant_joined` only for a new session appearance; rejoining a room returns the current role without appending another join event.

## Tradeoffs

- Tests can no longer construct unusual role layouts through public HTTP APIs, but that is consistent with the production client contract.
- Server-owned assignment is less flexible for manual debugging, but it makes evaluation exports cleaner and prevents impossible duplicate-role sessions.

## Risks And Follow-Up

- If future experiment designs need asymmetric assignment controls, introduce an explicit server-side matchmaking policy rather than a caller-controlled role override.

# Targeted WebSocket Delivery

## Context

Room WebSockets use a shared broadcast channel for room-wide messages, but observations and available actions are participant-specific. Sending all personalized messages to the shared channel without delivery filtering can leak one participant's role assignment or state view to the other participant.

## Chosen Approach

- Keep one room broadcast channel for simple fanout.
- Treat `ServerMessage.participant_session_id` as an optional delivery target.
- Have each socket sender forward untargeted messages to everyone and targeted messages only to the matching participant session.
- Mark `roleAssigned` and `stateChanged` as targeted messages.
- Persist both `game_action_accepted` and `state_changed` events for accepted actions so evaluation exports can query actions and resulting states directly.

## Tradeoffs

- Filtering at the socket sender keeps the room bus simple, but it relies on every personalized message setting a target.
- A future typed envelope could make targeted-vs-broadcast delivery impossible to forget, but the current protocol struct remains compatible with the existing client.

## Risks And Follow-Up

- WebSocket error messages are still generally broadcast unless a call site targets them explicitly. Later runtime work should decide which error classes are participant-local.

# Transcript And Conversation Persistence

## Context

Speech transcription posts arrive from browser paths and include participant-facing metadata. For clean evaluations, the server should persist transcript segments and derived conversation messages exactly once in the session event stream, with trustworthy actor identity. The browser can keep its own live chat log, so the Rust server does not need public conversation or transcript replay endpoints.

## Chosen Approach

- Validate that transcript and voice-diagnostic posts reference a participant that belongs to the room.
- Derive transcript sender role from `session_participants` instead of trusting the client-provided player field.
- Store transcript segments as `transcript_segment` events.
- Store typed chat, agent speech, and voice transcript conversation entries as `conversation_message` events.
- Broadcast live conversation messages over the room WebSocket only.
- Keep `POST /api/rooms/{room_id}/transcripts` for the browser speech pipeline, but remove public transcript history, transcript SSE, transcription-context, and conversation history endpoints.
- Reconstruct conversation internally from `session_events` only where server logic needs it, such as agent context.

## Tradeoffs

- The transcript request still accepts the existing player field for client compatibility, but the server treats it as non-authoritative.
- Reconnected browsers do not receive a server-replayed chat log. This is intentional: the live client owns display history, while the database owns evaluation history.
- External legacy Python transcription or TTS workers that depended on replay endpoints must move to the current browser/Rust paths or to a purpose-built worker protocol.

## Risks And Follow-Up

- If we later need post-crash browser replay, add it deliberately as a client-facing feature backed by `session_events`, not as incidental runtime memory.

# LiveKit And Speechmatics Audio Plans

## Context

The browser client expects the server to decide whether audio is disabled, handled entirely through LiveKit, or split between LiveKit partner audio and Speechmatics realtime transcription.

## Chosen Approach

- Use HS256 LiveKit tokens with identities formatted as `room_id:role:participant_session_id`.
- Return disabled token/audio-session payloads when LiveKit is disabled.
- Return one `livekit-combined` sink when LiveKit is enabled without Speechmatics transcription.
- Return split `livekit-partner` and `speechmatics-transcription` sinks when transcription provider is Speechmatics.
- Mint Speechmatics realtime temporary keys through the management API at audio-session request time.

## Tradeoffs

- Worker LiveKit identities are generated from the requested worker role without a durable participant row. That keeps worker tokens lightweight, but evaluation identity is not attached unless a later worker emits persisted events.
- Speechmatics temporary-key minting is synchronous in the audio-session request. This is simple and client-compatible, but request latency includes the Speechmatics management API call.

## Risks And Follow-Up

- LiveKit token grants are broad enough for current browser/worker use (`can_publish`, `can_subscribe`, and data publish). We may want narrower grants for specific worker roles later.

# In-Process Agent Runtime

## Context

Human-vs-agent rooms have an agent participant without a browser WebSocket. The reusable server still needs that participant to behave like an active room participant for readiness and action validation.

## Chosen Approach

- Create one fresh mutable agent instance per agent participant through the configured `AgentFactory`.
- Pass only player-facing setup data to agent creation: role, optional seed, and agent-specific config.
- Pass only the typed role-specific observation and the optional role-specific available-action affordance to each `act` call.
- Mark the agent participant connected when its in-process runtime starts.
- Persist `agent_started`, `agent_action`, `conversation_message`, normal game action events, and `agent_error` when invalid actions hit the configured limit.
- Submit agent actions through the same `submit_action` path as human actions so validation, state transitions, completion, and exports remain shared.
- Do not pass conversation history, room ids, participant-session ids, invalid-action counts, last errors, or completion flags into the agent.
- Stop invalid agents cleanly after `invalid_action_limit`.

## Tradeoffs

- In-process agents are modeled as connected without a WebSocket. This is pragmatic for server-owned agents, but remote gRPC agents will need an explicit connection/liveness model.
- `agent_action` records the proposal before validation; rejected proposals are therefore visible even when no `game_action_accepted` event follows.
- Agents that need memory must keep it in their own per-room instance. This keeps the reusable server from imposing a history policy or doing DB reads for every agent turn.
- `GameAdapter::available_actions` returns `Option<Vec<Action>>` and defaults to `None`. Games that cannot cheaply or cleanly enumerate legal actions can omit the affordance.
- `None` means the game does not provide available actions. `Some(vec![])` means the game provides the affordance and this player currently has no listed actions.
- When a game does provide available actions, the same role-specific affordance sent to a human UI is also passed to an agent controlling that role. Validation remains authoritative, so available actions never replace `validate_action`.

## Risks And Follow-Up

- Agent readiness is currently tied to room connection state rather than an explicit ready protocol. A richer ready barrier may be needed for remote agents or reinforcement-learning loops.
- Removing available actions from the browser protocol requires a coordinated client change so controls are computed client-side or rendered through another game-specific mechanism.

# Agent TTS Boundary

## Context

Agent messages need text-to-speech diagnostics and, eventually, audio publishing. TTS is downstream of the agent runtime: it converts text returned by an agent, not arbitrary conversation history.

## Chosen Approach

- Add an optional `StreamingTtsProvider` to `ServeOptions` for tests and embedding.
- Create an ElevenLabs WebSocket streaming provider from config when TTS is enabled and no provider is injected.
- Call TTS directly when the agent runtime persists an `origin: "agent"` conversation message.
- Do not poll conversation memory for messages.
- Persist the conversation message before synthesis so the diagnostic stream references a durable message id.
- Persist `tts_diagnostic` events for start, first audio, audio chunks, completion, and failure.

## Tradeoffs

- The direct TTS path currently records audio chunk diagnostics but does not publish audio to LiveKit. This keeps step 11 testable while leaving RTC publishing to the explicit step 12 spike.
- Durable exports still come from `session_events`.

## Risks And Follow-Up

- Once audio publishing is integrated, diagnostics should include publish latency and LiveKit track/packet metadata.
