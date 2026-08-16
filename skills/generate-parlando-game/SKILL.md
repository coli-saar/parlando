---
name: generate-parlando-game
description: Generate Parlando dialogue games using the published parlando runtime and @coli-saar/parlando-client React participant application. Use when creating, porting, or iterating on a Parlando game.
---

# Generate Parlando Game

Generate a complete two-player game using published registry packages. Read `references/server-adapter.md` and `references/browser-client.md`; read `references/config-deployment.md` for deployment or dashboard work and `references/agents.md` when agents are requested. These references are authoritative for generation; do not search for a different Parlando API.

## Discover versions

Query crates.io for `parlando` and npm for `@coli-saar/parlando-client`. If unavailable, use the repository's current package versions and disclose that fallback.

## Clarify only domain choices

Ask only when missing information changes mechanics: game goal and terminal outcomes, private information for A and B, actions and validation, observations, game-owned configuration, analysis/completion fields, agent behavior, communication/voice needs, or deployment target. Do not ask game authors to choose pairing, session limits, participant-page configuration, provider types, or server internals; the dashboard owns them.

## Non-negotiable boundaries

- Exactly two roles exist: `PlayerRole::A` and `PlayerRole::B`. Either can be human or an agent.
- The Rust runtime alone owns authoritative `State`.
- Human and automated players receive the same accepted actions and shared `Completion`, together with role-specific `Observation`.
- Actions alone change state; messages only communicate between players.
- Games and agents use structured domain data and make no human-presentation assumptions.
- The Rust server is frontend-neutral. The JS client and React `ParticipantApp` are optional conveniences.
- Generated code must not use `test_support`, internal modules, provider configuration, storage, routers, or protocol DTOs.
- Never generate compatibility aliases for `GameAdapter`, events, state delivery, or old agent callbacks.

## Output

Generate a Rust server/game crate, a participant application, pure mechanics tests, build/run files, and optional agents. Keep mechanics in pure functions behind a thin `Game` implementation. Use registry dependencies, not local Parlando paths.

## Rust

Implement `Game` with `Config`, `State`, `Action`, `Observation`, and `Completion`; implement `validate_config`, seeded `initial_state`, actor-aware `apply_action`, `observation`, optional `available_actions`, optional role-neutral `transition_metadata`, and `completion`.

`apply_action` performs authorization and validation atomically and returns stable `ActionRejection` codes for expected failures. `observation` is the complete player-visible domain model and must filter private data. `transition_metadata` is analysis data for runtime-owned logs, never participant prose.

Start with `Server::new(game, metadata)`, set database and optional participant assets, register optional factories with `.agent(...)`, and call `.serve(address)`. Do not construct `ExperimentConfig`, `ServeOptions`, providers, or routers.

## Agents

Implement `agent::Agent<G>` and `agent::Factory<G>`. Factory creation receives role, seed, and agent-owned settings before game delivery. Store the first observation in `start`; observe later accepted actions in `observe_transition`; observe other-player text in `observe_message`; receive the shared terminal result in `finish`; and decide in `respond`.

Return a non-empty `Response::action`, `Response::message`, or `Response::action_and_message`. A message does not change game state. For remote agents, register `agent::grpc::Factory::<G>::new()` and use the Python `Agent`, `Response`, and `serve` API.

Do not solve model readiness with game messages. The current construction-ready rule is completion of factory creation; explicit readiness and blocking-load isolation are deferred runtime work.

## Participant application

Use `ParticipantApp` and `GameSession<Observation, Action, Completion>` from `@coli-saar/parlando-client/react`. Render current state from `session.observation`; use the nullable `session.transition` only when the presentation needs the most recent accepted actor and action. Send actions with `sendAction` and player messages with `sendMessage`. Use the shared `completion`, `voiceEnabled`, `voiceStatus`, and the exported microphone/transcription widgets as needed.

Do not read authoritative state, generic events, complete experiment configuration, provider credentials, or peer controller type. Do not build custom audio transport, transcription, TTS, startup lifecycle, or WebSocket message handling when the SDK supplies it.

When complete, render a terminal view and disable normal game controls. Presentation is frontend-owned; completion is determined by Rust `Game::completion` and its shared payload must be safe for both roles. Put private terminal facts in each role's final observation.

## Configuration and deployment

The dashboard owns experiment creation, participant information and consent copy, pairing, capacity, lifecycle limits, privacy, conversation, agents, voice, transcription, and TTS. Process bootstrap owns listener address, database URL, optional participant asset path, public URL, and provider secrets. Game code owns only `Game::Config`; an agent factory owns only its settings. Do not create a configurable participant title; the standard application uses functional screen headings and the game frontend owns game presentation.

Keep provider secrets in server or remote-agent environment variables. Never place them in frontend code or checked-in experiment data.

## Validate

Run formatting, Rust checks/tests, JS build/tests, and Python tests when used. Test deterministic initialization, every legal/illegal action, A/B observation privacy, success and failure completion, transition metadata shape, messages not changing state, agent lifecycle, and terminal UI. Inspect generated code for old names listed in `docs/migrating-to-clean-api.md`; none may remain.

End with changed files, commands and results, exact run command, dashboard URL, agent commands, deployment notes, and assumptions.
