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

# Remote gRPC Agent Bridge

## Context

Parlando needs agents that can be written in Python or other languages while the Rust server remains authoritative for rooms, state transitions, validation, persistence, and completion. This is especially important for future reinforcement-learning loops where the transport should be structured and efficient.

## Chosen Approach

The reusable server now includes a tonic/protobuf gRPC bridge:

- `rust-server/proto/parlando_agent_v1.proto` defines the language-neutral service.
- `RemoteGrpcAgentFactory<A>` implements the same `AgentFactory<A>` trait used by in-process agents.
- `RemoteGrpcAgent` sends `CreateAgent` once per room agent and `Act` calls afterward.
- Observations, available actions, and returned actions cross the process boundary as protobuf `Struct` values.
- Returned actions are deserialized into the game-specific Rust action type and validated through the normal runtime before changing state.
- Protobuf `Struct` represents numbers as doubles. The Rust bridge converts finite integral doubles back to JSON integers when deserializing returned actions so typed integer action fields keep working in normal cases.

## Tradeoffs

This preserves a clean Rust runtime boundary and gives Python agents a stable target, but it still uses JSON-compatible protobuf `Struct` for game-specific data. That is acceptable at the network boundary; it should not leak into in-process game state or action handling.

## Follow-Up

- Add a real Python-process integration test after installing SDK runtime dependencies.
- Add a remote-agent config hash to participant metadata if needed for experiment reproducibility.

## Update

The Python SDK wrapper now lives under `python/parlando-agent-sdk`, and remote gRPC participant exports now record `identity_provider = remote_grpc` plus `<agent_name>@<agent_version>`. A config hash is still not recorded.

# Static Serving And Deployment Packaging

## Context

The Rust server should be able to serve the existing TypeScript client as a drop-in backend/frontend bundle, but this Rust workspace does not currently contain the frontend source tree.

## Chosen Approach

Static serving remains runtime-configured through `server.client_dist_path`. When the path contains `index.html`, the server serves `/assets/*`, `/`, and SPA fallback paths while preserving backend `/api/*` and `/ws/*` routes. Deployment packaging builds the Rust binary and can serve a frontend build copied into `/app/client-dist`.

## Tradeoffs

The Dockerfile is honest about this workspace boundary: it builds the Rust binary and includes a Render-safe config, but it does not build a frontend that is not present in the Docker context. A production image that bundles the UI should either include the frontend source in the context or copy a prebuilt client dist into `/app/client-dist`.

## Follow-Up

- Decide where the TypeScript client source lives relative to the production Docker context.
- Add a deploy smoke test that starts the container with a real `client_dist_path` and verifies browser navigation.

# Agent Audio Publishing

## Context

Agent TTS should become audible in the LiveKit room instead of only producing diagnostics. The publisher needs to be separate from TTS so tests can mock it and so a sidecar remains possible if native RTC publishing is not reliable.

## Chosen Approach

The server now uses `AgentAudioPublisher` as the narrow boundary. `LiveKitAgentAudioPublisher` connects as a short-lived `agent-voice` participant, publishes a native LiveKit audio track, waits for LiveKit to confirm that at least one subscriber attached to the local track, submits raw PCM chunks via `NativeAudioSource`, and records publish diagnostics through the normal TTS diagnostic event stream.

## Tradeoffs

This keeps Rust authoritative for state, messages, TTS, diagnostics, and tokens. Waiting for subscriber attachment avoids losing short finite TTS clips during LiveKit subscription negotiation, at the cost of failing the publish operation if no subscriber appears within the timeout. Agent audio publishing also depends on the native LiveKit Rust SDK. Local macOS validation exposed an Objective-C category retention issue in the final binary: the WebRTC static archive contained `NSString(AbslStringView)`, but the test executable initially omitted it and aborted during video encoder initialization. Passing `-ObjC` from the final-linking Parlando crates retains those categories and allows the RTC conversation test to pass on macOS.

## Follow-Up

- Run the ignored RTC PCM conversation test on Linux or Render-like infrastructure too, so the macOS fix is not the only native-platform validation.
- Manually verify that browsers hear the `agent-voice` track.
- Keep the sidecar option available if native Rust RTC remains unstable in developer environments.

# Release Dependency Hygiene

## Context

Live-service tests use real LiveKit, Speechmatics, and ElevenLabs clients, but the release binary should not gain extra dependencies merely because tests need helpers. Cargo feature unification can also make a normal dependency heavier than intended if one crate enables broad defaults.

## Chosen Approach

Test-only direct dependencies remain under `dev-dependencies`. Production HTTP uses `reqwest` with `default-features = false` and `rustls-tls-webpki-roots`, matching the rest of the Rust TLS stack and avoiding accidental `native-tls` linkage.

## Tradeoffs

This makes TLS behavior more explicit and keeps the release graph cleaner. The release binary still includes LiveKit, tonic/prost, and tokio-tungstenite because those are production features: RTC agent audio publishing, remote gRPC agents, server WebSockets, Speechmatics, and ElevenLabs streaming.

## Follow-Up

- If we later want a smaller binary without RTC publishing or remote agents, add explicit Cargo features for those production capabilities rather than hiding them as test dependencies.

# Waiting Room And STT Readiness

## Context

The server needs predictable start dynamics across human-human and human-agent rooms. Human-human games should not accept actions until both human players have connected. Human-agent games should create the agent participant at room creation time, but the agent should not start acting merely because the room exists. When Speechmatics transcription is enabled, game progression should wait until the human participant's audio session has finished initializing STT.

## Chosen Approach

Room-local runtime participants now track `connected` and `audio_ready`. Agents are marked audio-ready immediately. Human participants are marked audio-ready immediately unless Speechmatics transcription is enabled; in that case `/api/rooms/{room_id}/audio-session` marks the participant ready only after the Speechmatics temporary key has been minted. Game action submission and agent startup both use the same room-readiness predicate: both roles must be connected, and any human role that requires Speechmatics must be audio-ready.

The client-facing sequence is deliberately explicit:

1. Setup screens collect consent, participant name/identity, and microphone/audio setup.
2. Waiting-room responses and waiting-room WebSocket messages may expose presence and voice readiness, but they do not include game observations or available actions.
3. `roleAssigned` is the game-start payload. It is sent only after the room-readiness predicate is satisfied, and it carries the participant-specific observation and optional available actions. Reconnecting participants in an already-started room receive the same targeted payload.

For human-human matchmaking, the first participant receives `status: "waiting"` even though the server may already have created an internal room/session handle. The second participant creates the visible match, and the first participant can poll status to obtain the client-facing `room_id` and role once the room is actually paired.

## Tradeoffs

`audio_ready` is active runtime coordination state, not a durable identity or evaluation field. The durable event stream records the timing with `voice_diagnostic` events such as `stt_initialized`, while active memory lets the WebSocket/game loop avoid repeated DB reads. Treating `roleAssigned` as the start/reconnect payload reuses an existing client concept instead of adding a new message type, but it means clients should not interpret it as a mere room-entry acknowledgement.

## Follow-Up

- Verify the actual TypeScript client calls `/audio-session` early enough in both human-human and human-agent flows.
- If browser-side STT initialization can fail after a temporary key is minted, add an explicit client-to-server `sttReady` signal instead of treating successful `/audio-session` as readiness.

# Audio Session Provider Boundary

## Context

The Python server built `/audio-session` responses inline, while the TypeScript client had the cleaner abstraction: an audio-session plan containing sink descriptions, with browser-side sink implementations for LiveKit and Speechmatics. Rust should keep the stable client contract but avoid treating LiveKit and Speechmatics as interchangeable STT services. LiveKit is realtime room transport; Speechmatics is a browser-side STT provider; the old `transcription.provider = livekit` mode is a server/worker STT architecture layered on LiveKit audio.

## Chosen Approach

The Rust server now has `audio_session` as a reusable module with typed planning boundaries:

- `DefaultAudioSessionPlanner` composes partner audio and transcription pieces into the existing `AudioSessionPlanResponse`.
- `PartnerAudioProvider` owns LiveKit partner-audio sink construction and token minting.
- `TranscriptionProvider` owns provider-specific transcription sinks and readiness semantics.
- `SpeechmaticsBrowserTranscriptionProvider` returns the `speechmatics-transcription` WebSocket sink and marks readiness as satisfied once the temporary key has been minted.
- `LiveKitWorkerTranscriptionProvider` keeps transcription attached to the LiveKit sink and marks readiness as requiring an external worker/client signal.

`app.rs` now handles room authorization and readiness side effects, but not provider-specific credential construction.

## Tradeoffs

This is not a generic "STT service" trait because that would blur transport and recognition responsibilities. The planner still emits JSON-compatible sink credentials because that is the public browser protocol boundary, but the server-side choice of providers is typed and isolated. The current `NoopTranscriptionProvider` preserves the previous `livekit-combined` response shape for LiveKit-enabled studies without explicit Speechmatics.

## Follow-Up

- Decide whether `transcription.enabled = false` should continue labeling `livekit-combined` with the `transcription` purpose for legacy client compatibility, or whether new clients should receive partner-audio only.
- Add an explicit readiness signal for LiveKit worker transcription before reintroducing worker-based STT in production.

# JavaScript Client Package Split

## Context

Game clients will often be developed on machines without local checkouts of the Rust server. The reusable browser runtime therefore needs to be a real package boundary rather than a workspace-only folder or a `file:` dependency from one demo game.

## Chosen Approach

- Move the reusable browser runtime into a sibling `../js-client` project named `@parlando/client`.
- Keep `../js-client` game-agnostic: HTTP API helpers, WebSocket helpers, protocol types, audio-session orchestration, LiveKit/Speechmatics sink implementations, and optional React helpers live there.
- Move the Space Game browser app into sibling `../space-game` as a demo client that depends on `@parlando/client` by package name.
- Use GitHub Packages as the planned release channel for versioned SDK builds.
- Use `yalc` for unpublished local debug loops so demo clients can test a package-shaped artifact without publishing every intermediate build.
- Keep the Rust Space Game server crate source-local while the repo layout settles.

## Tradeoffs

This creates three top-level project areas instead of keeping everything under the Rust workspace, but it matches the intended dependency direction: games consume the SDK, and the SDK does not consume games. `yalc` adds one local-development tool, but it avoids committing a `file:` dependency that would break on machines without the sibling SDK checkout.

## Follow-Up

- Add package repository metadata once the GitHub owner/repo path is finalized.
- Add CI to build, test, and publish `@parlando/client` to GitHub Packages on version tags.
- Update Render/Docker packaging once the final production build context includes both the Rust binary and the Space Game client build.

# Local Install Boundary For Demo Games

## Context

The Space Game demo should model how third-party games consume Parlando. A game developer may have only installed artifacts available, not local source checkouts of the Rust server or JavaScript SDK.

## Chosen Approach

- Add a top-level Parlando Makefile that knows local source layout and installs shared artifacts.
- Install the Rust Space Game server binary into a local prefix with `cargo install`.
- Publish `@parlando/client` into the local yalc store from the top-level install step.
- Keep the Space Game Makefile source-agnostic: it expects `parlando-space-game` on `PATH` and `@parlando/client` in the yalc store.
- Add a Space Game solo voice config that uses `agents.mode = human_vs_agent`, the `space_game.back_and_forth` agent, and includes the local private LiveKit, Speechmatics, and ElevenLabs config from the Rust-server checkout.

## Tradeoffs

The top-level Makefile is now the local development convenience layer for this checkout. Individual games remain closer to external-consumer behavior, but the top-level `run` target rebuilds/reinstalls shared artifacts before launching, which is simple and conservative rather than maximally fast.

## Follow-Up

- Add a faster top-level run target that skips reinstalling shared artifacts when the developer knows they are current.
- Decide whether local private service config should live under the top-level Parlando directory once the Git root moves up.

# Database-Backed Game Monitor

## Context

Operators need a simple way to inspect recent games over HTTP: recent sessions, session metadata, participant roles, player actions, and transcription results. This should work from durable records and should not depend on active in-memory rooms, the game WebSocket, or a game-specific client bundle.

## Chosen Approach

- Add DB-oriented storage queries for recent session summaries and session participants.
- Add HTTP admin endpoints under `/api/admin/games` that read session metadata and ordered events from the experiment store.
- Serve a self-contained `/admin/games` page from the Rust server. The page polls the DB-backed event endpoint with `after=<event_index>` so it stays current without attaching to the live game socket.
- Summarize important event rows into a stable, game-agnostic timeline focused on actions, conversation messages, and transcript segments.
- Keep timeline rows succinct by default and reveal full event JSON only when the operator expands a row.
- Use compact A/B/SYS badges in the monitor instead of repeating role labels inside event titles and summaries.

## Tradeoffs

Polling is simpler and keeps the monitor decoupled from runtime room buses, but it is less immediate than a pushed event stream. The event summaries intentionally omit full game state snapshots to keep the UI readable; raw export remains available from `/api/admin/export` when full forensic detail is needed.

## Follow-Up

- Add authentication or deployment-level access controls before exposing the admin monitor outside trusted environments.
- Consider replacing polling with server-sent events if the monitor needs lower latency or many simultaneous viewers.

# Topic-Based Experiment Documentation

## Context

Parlando now needs documentation for researchers who want to use the platform to build dialogue game experiments. The documentation has two audiences that overlap but have different reading modes: a quick evaluator who wants to understand what Parlando can do, and an experiment builder who needs architecture, game-modeling, agent, local workflow, deployment, and data-export details.

## Chosen Approach

- Add a root `README.md` that explains Parlando's capabilities, shows the core adapter idea, includes a small Python remote-agent example, and points to the docs index and demo implementation.
- Put GitHub-rendered technical docs under `docs/`.
- Split the implementation documentation by topic: architecture, building games, agents, running/deployment, and data/monitoring.
- Keep the README mostly product-facing and keep longer implementation details out of the root page.
- Avoid screenshot references for now because this Rust workspace does not contain verified browser screenshots. Add screenshots later only when they are checked into a stable path.

## Tradeoffs

This creates several small docs pages instead of one long guide. The split makes individual topics easier to link from GitHub and keeps the README from becoming too dense for first-time readers. The deployment page deliberately documents the current workspace boundary: the Rust server can serve a built client directory, but this Docker context does not build a frontend that lives outside the Rust workspace.

## Follow-Up

- Add verified screenshots or short demo clips once the Space Game client build assets have a durable location.
- Update the docs after the JavaScript client package and production Docker build context are finalized.

# README Positioning And Client Protocol Documentation

## Context

The root README should present Parlando primarily as a dialogue experiment platform. Agents are important, but they should not dominate the first impression. Experiment builders also need enough implementation detail to connect Rust game types to the JavaScript client; saying that Rust structs serialize to JSON is not sufficient because the browser has to consume exact field names, enum tags, WebSocket messages, and action payloads.

## Chosen Approach

- Reframe the README around dialogue experiments, role-specific observations, browser protocol, voice/conversation data, persistence, and deployment.
- Keep agents in the README as an important optional capability rather than the central product claim.
- Add a measured comparison to adjacent platforms: Slurk, Empirica, oTree, Dallinger, jsPsych, and lab.js.
- Describe Prolific-oriented automation as part of Parlando's scope rather than as a limitation compared with recruitment-heavy platforms.
- Position Slurk as typed-chat-first with optional JavaScript task components, while Parlando is JavaScript-game-first with voice designed to stay out of the way.
- Mention the planned Parlando LLM skill so readers understand that deep Rust or JavaScript knowledge should not be required for normal game development.
- Add `docs/client-protocol.md` with concrete Rust-to-JSON naming rules, HTTP room payloads, WebSocket messages, action submission, chat messages, and TypeScript mirror-type guidance.

## Tradeoffs

The README comparison is necessarily selective. It points to established systems and explains fit rather than claiming universal superiority. The protocol document repeats some type information from the JavaScript package, but that duplication is useful for experiment authors who are reading GitHub-rendered docs rather than browsing source files.

## Follow-Up

- Keep the platform comparison current as Parlando's deployment and frontend packaging story stabilizes.
- Update `docs/client-protocol.md` when the `@parlando/client` package API changes.

# Space Game Server Ownership

## Context

The Space Game Rust crate originally lived inside the reusable Rust server workspace. That made the `rust-server` directory carry both reusable platform code and one concrete demo game, while the sibling `space-game` directory held only the browser app and game configs.

## Chosen Approach

- Make `rust-server` the `parlando-server` crate directly, with `src/`, `proto/`, `tests/`, and `Cargo.toml` at the directory root.
- Move the Space Game Rust crate to `space-game/server` so the game owns its browser client, server adapter, agents, configs, Dockerfile, and Render blueprint together.
- Keep the installed binary name `parlando-space-game` for local scripts and deployment commands.
- Use a local path dependency from `space-game/server` to `../../rust-server` inside this monorepo; published external games should depend on the released `parlando-server` crate instead.

## Tradeoffs

The Space Game server now has a direct relative path to the reusable server crate during monorepo development. That is clearer for this checkout and mirrors how new game projects should be organized, but it means the demo game is no longer part of a single Cargo workspace command from `rust-server`.

## Follow-Up

- If more first-party games are added, keep each game self-contained under its own top-level directory with a `server/` crate and browser app.
- Consider adding a root-level convenience script if contributors need one command to test every Rust crate in the repository.
