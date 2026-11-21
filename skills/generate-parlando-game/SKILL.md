---
name: generate-parlando-game
description: Generate Parlando dialogue games using the Parlando Game SDK and @coli-saar/parlando-client React participant application. Use when creating, porting, or iterating on a Parlando game.
---

# Generate Parlando Game

Use the Parlando CLI from this checkout to generate a Wasm game package. Start with `parlando new <name> --lang rust`. Keep the generated manifest, SDK dependency and scenario format together. Read `docs/component-games.md` in the checkout for the build and deployment workflow and `references/browser-client.md` for the participant UI. For a project pinned to registry packages, use the documentation for its installed release.

## Clarify only domain choices

Ask only when missing information changes mechanics: game goal and terminal outcomes, private information for A and B, actions and validation, observations, game-owned configuration, analysis/completion fields, agent behavior, communication/voice needs, or deployment target. Do not ask game authors to choose pairing, session limits, participant-page configuration, provider types, or server internals; the dashboard owns them.

## Non-negotiable boundaries

- Exactly two roles exist: `PlayerRole::A` and `PlayerRole::B`. Either can be human or an agent.
- The Wasm game instance owns authoritative session state.
- Human and automated players receive the same accepted actions and shared `Completion`, together with role-specific `Observation`.
- Actions alone change state; messages only communicate between players.
- Games and agents use structured domain data and make no human-presentation assumptions.
- The Rust server is frontend-neutral. The JS client and React `ParticipantApp` are optional conveniences.
- Generated code must not use `test_support`, internal modules, provider configuration, storage, routers, or protocol DTOs.
- Never generate compatibility aliases for `GameAdapter`, events, state delivery, or old agent callbacks.

## Output

Generate a game package, participant application, deterministic mechanics tests, executable Wasm scenarios and any requested agents.

## Rust

Implement the session-owned `parlando_game_sdk::Game` trait and call `parlando_game_sdk::export!(YourGame)`. Derive `GameValue` for exchanged domain types. Implement seeded `init`, actor-aware `step`, role-specific `view_a` and `view_b`, and shared `completion`. Supply optional analysis `metadata` and paired checkpoint/restore methods when needed. Reject an illegal action before changing state. Include actionable choices in the relevant observation if the task discloses them.

## Agents

Read `docs/agents.md` for the current policy interface. Persist the role observation delivered in `start` and `observe_transition`. Use it in `respond` to choose an action or message.

Return a non-empty `Response::action`, `Response::message`, or `Response::action_and_message`. A message does not change game state. Release per-session resources in `shutdown`. The server includes the remote gRPC factory by default; configure its endpoint and optional YAML settings in the dashboard, and use the Python `Agent`, `Context`, `Response`, and `serve` API for remote implementations. Keep agent credentials in the remote process. Python combines outputs with `Response.action_with_message`.

Do not solve model readiness with game messages. The current construction-ready rule is completion of factory creation; explicit readiness and blocking-load isolation are deferred runtime work.

## Participant application

Use `ParticipantApp` and `GameSnapshot<Observation, Action>` from `@coli-saar/parlando-client/react`. Render current state from `snapshot.observation`; use the nullable `snapshot.transition` only when the game surface needs the most recent accepted actor and action. Send actions with `snapshot.act` and disable game controls unless `snapshot.interactionEnabled` is true. `ParticipantApp` owns conversation, microphone controls, connection status, leaving, replay controls, credential recovery, partner-disconnect pausing, recipient-specific terminal outcomes, and recruitment handoff. The game styles these widgets through the SDK’s rendered classes and CSS custom properties rather than implementing their behavior.

Do not read authoritative state, generic events, complete experiment configuration, provider credentials, or peer controller type. Do not build custom audio transport, transcription, TTS, startup lifecycle, or WebSocket message handling when the SDK supplies it.

Do not implement transport-disconnect or recruitment end screens in game code. `ParticipantApp` renders the standard terminal surface and prevents normal controls after every terminal outcome. Completion is determined by Rust `Game::completion`, and its shared payload must be safe for both roles. Put private terminal facts in each role's final observation.

## Configuration and deployment

The dashboard owns experiment creation, participant information and consent copy, pairing, capacity, lifecycle limits, privacy, conversation, agents, voice, transcription, and TTS. Process bootstrap owns listener address, database URL, optional participant asset path, public URL, and provider secrets. Game code owns only `Game::Config`; an agent factory owns only its settings. Do not create a configurable participant title; the standard application uses functional screen headings and the game frontend owns game presentation.

Keep provider secrets in server or remote-agent environment variables. Never place them in frontend code or checked-in experiment data.

## Validate

Run formatting, Rust checks/tests, JS build/tests, and Python tests when used. Test deterministic initialization, every legal/illegal action, A/B observation privacy, success and failure completion, transition metadata shape, messages not changing state, agent lifecycle, disabled controls while `interactionEnabled` is false, and compatibility with the SDK terminal/reconnect UI. Inspect generated code for old names listed in `docs/migrating-0.3.0-to-0.4.0.md`; none may remain.

End with changed files, commands and results, exact run command, dashboard URL, agent commands, deployment notes, and assumptions.
