# Architecture

Parlando separates reusable experiment infrastructure from the game-specific mechanics of a particular study. A game author writes a typed Rust game adapter and a browser experience; the reusable server handles the surrounding work needed to run the experiment.

The design goal is a clear research boundary: your study defines the world, roles, legal actions, private information, UI, agents, and analysis summary. Parlando provides the room lifecycle, communication channels, storage, monitoring, export, and deployment shape around that study.

The included `generate-parlando-game` skill generates and revises adapter and client code from a game description, so the architecture is explicit and regular rather than hidden behind a large framework.

## Terminology

- **Game**: the runnable binary plus its linked game adapter, browser client, agents, and runtime configuration template. The game has build-time provenance: crate/package versions, Git revision, build time, and local dependency warnings.
- **Experiment**: one data-collection campaign using a game. It has a durable id, display name, lifecycle status, consent text, condition labels, selected agent identities, and a captured game/server/client version manifest.
- **Session**: one play-through within an experiment. A session has a room id, one or two participants or agents, role assignments, events, transcripts, actions, and completion data.

Configuration files bootstrap a game run, and Parlando stores the effective non-secret experiment metadata and version manifest with the experiment record when data collection starts.

## Components

The reusable Rust server owns:

- participant creation, consent records, room creation, joins, reconnects, and readiness.
- WebSocket delivery and participant-specific observations.
- action validation flow, action persistence, state-change persistence, completion records, transcripts, conversation messages, agent events, and diagnostics.
- optional server-owned audio relay, server-side Speechmatics, and ElevenLabs integration.
- local and remote agent execution.
- authenticated admin monitoring, privacy status, fixed exports, and manual participant deletion.

The game crate owns:

- the authoritative game state type.
- the action type accepted from humans or agents.
- the role-specific observation type sent to each player.
- validation and state transitions.
- available-action hints, if the game can cheaply provide them.
- game-specific events and completion summaries.
- game-specific agent factory selection.

The browser client owns the participant experience: screens, controls, rendering, audio setup, and WebSocket interaction. The server remains authoritative even when the client computes controls locally.

The data store owns the durable record for later analysis. Browser-local state and in-memory rooms are useful during play, but exported study data should come from persisted experiment, session, participant, action, conversation, transcript, agent, and diagnostic records.

## Runtime Boundaries

Inside the linked Rust binary, game logic stays typed. JSON appears at boundaries:

- browser HTTP requests and WebSocket messages.
- persisted event payloads.
- admin and export APIs.
- remote gRPC agents, where game-specific observations and actions are represented as protobuf `Struct` values.

This boundary keeps the reusable server generic without forcing game logic into untyped `serde_json::Value` code.

## Session Flow

A typical human-vs-human session follows this path:

1. A participant is created with a non-secret session handle, a separate opaque credential, and an experiment-specific random participant identifier; when required, the server durably records the configured declarations.
2. The participant creates or joins a waiting room.
3. The server creates or joins a room and assigns roles `A` and `B`.
4. The authenticated browser requests a one-use game ticket and connects to `/ws/game/{room_id}?token=...`; a participant-session id is never accepted as a credential.
5. The server waits until required participants and audio setup are ready.
6. The server sends role-specific start payloads and observations.
7. Players submit actions; the game adapter validates and applies them.
8. The server persists actions and state changes, broadcasts targeted updates, and records completion when the game ends.

Human-vs-agent sessions use the same room and action path. The agent participant is created by the server and acts through the same validation and persistence pipeline as a human.

When voice is enabled, each browser opens one authenticated `/ws/audio/{room_id}` connection. Fixed 24 kHz mono PCM16 frames fan out independently to the partner browser and to a server-side transcription session. Final provider utterances enter the same conversation and agent-observation path as typed messages. Agent TTS returns through the room relay and is not transcribed again. Browser playback starts behind a small jitter buffer; finite TTS streams are scheduled against absolute frame deadlines so timer overhead cannot accumulate into audible gaps. See [Audio Transport](audio-transport.md) for the complete boundary.

## Example Implementation

Use these files as concrete references:

- `space-game/server/src/game/state_engine.rs`: typed demo state, actions, observations, events, summary, and transition helpers.
- `space-game/server/src/game/adapter.rs`: the `GameAdapter` implementation.
- `space-game/server/src/agents.rs`: in-process and remote-agent selection.
- `space-game/server/src/main.rs`: binary entry point that loads config and starts the reusable server.
- `space-game/client`: demo participant UI using the reusable JavaScript client.
