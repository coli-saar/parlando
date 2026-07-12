# Architecture

Parlando separates reusable experiment infrastructure from the game-specific mechanics of a particular study. A game author writes a typed Rust game adapter and a browser experience; the reusable server handles the surrounding work needed to run the experiment. The planned Parlando LLM skill is intended to generate and revise much of this adapter/client code from a game description, so the architecture is explicit and regular rather than hidden behind a large framework.

## Components

The reusable Rust server owns:

- participant creation, consent records, room creation, matchmaking, joins, reconnects, and readiness.
- WebSocket delivery and participant-specific observations.
- action validation flow, action persistence, state-change persistence, completion records, transcripts, conversation messages, agent events, and diagnostics.
- optional LiveKit, Speechmatics, and ElevenLabs integration.
- local and remote agent execution.
- admin monitoring and export.

The game crate owns:

- the authoritative game state type.
- the action type accepted from humans or agents.
- the role-specific observation type sent to each player.
- validation and state transitions.
- available-action hints, if the game can cheaply provide them.
- game-specific events and completion summaries.
- game-specific agent factory selection.

The browser client owns the participant experience: screens, controls, rendering, audio setup, and WebSocket interaction. The server remains authoritative even when the client computes controls locally.

## Runtime Boundaries

Inside the linked Rust binary, game logic stays typed. JSON appears at boundaries:

- browser HTTP requests and WebSocket messages.
- persisted event payloads.
- admin and export APIs.
- remote gRPC agents, where game-specific observations and actions are represented as protobuf `Struct` values.

This boundary keeps the reusable server generic without forcing game logic into untyped `serde_json::Value` code.

## Session Flow

A typical human-vs-human session follows this path:

1. A participant is created and, when required, records consent.
2. The participant enters direct-room or matchmaking flow.
3. The server creates or joins a room and assigns roles `A` and `B`.
4. Browser clients connect to `/ws/game/{room_id}` with their participant-session ids.
5. The server waits until required participants and audio setup are ready.
6. The server sends role-specific start payloads and observations.
7. Players submit actions; the game adapter validates and applies them.
8. The server persists actions and state changes, broadcasts targeted updates, and records completion when the game ends.

Human-vs-agent sessions use the same room and action path. The agent participant is created by the server and acts through the same validation and persistence pipeline as a human.

## Example Implementation

Use these files as the concrete reference:

- `space-game/server/src/game/state_engine.rs`: typed demo state, actions, observations, events, summary, and transition helpers.
- `space-game/server/src/game/adapter.rs`: the `GameAdapter` implementation.
- `space-game/server/src/agents.rs`: in-process and remote-agent selection.
- `space-game/server/src/main.rs`: binary entry point that loads config and starts the reusable server.
