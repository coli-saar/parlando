# Parlando design principles

**Status:** Authoritative. New APIs, protocols, games, agents, dashboard features, and participant applications must follow these principles. If older documentation conflicts, this document wins until the conflict is corrected.

## Domain model

Parlando models three levels:

- A **game** is compiled mechanics plus its domain types.
- An **experiment** is one dashboard-managed, versioned configuration and data-collection context for a game.
- A **session** is one play-through within an experiment.

A research **study** may contain several experiments or work outside Parlando. It is not a Parlando runtime entity and must not be used as a synonym for experiment or session in APIs.

A session has exactly two player roles: `PlayerRole::A` and `PlayerRole::B`. A role does not say whether its controller is human or automated. Do not add a public pairing-mode taxonomy to game code.

## One authoritative state, role-safe observations

The Rust runtime exclusively owns authoritative `Game::State`. It is stored and logged according to runtime privacy policy and is never sent to a participant application or agent.

`Game::Observation` is the complete domain information available to one role at a moment in the game. Human and automated players receive the same kind of role-specific observation. Frontends must not fall back to state, and agents must not receive privileged state merely because they run on the server.

Games must make information boundaries explicit in `observation(state, role)`. Tests should treat absence of private information as a security property.

Human players and agents receive the same domain information. After an accepted action, both receive the actor, the accepted `Action`, and their own role-specific resulting `Observation`. At normal game completion, both receive the same shared `Completion`. A frontend may ignore the action and render only the observation; delivery does not dictate presentation. An accepted `Action` is therefore observable to both roles. A game that needs to conceal how a state change was produced must make the shared action value non-revealing, for example with a `SecretAction` variant, and expose permitted consequences through each role's observation.

The shared `Game::Completion` sent at termination carries the game-specific public result, such as win/loss, winner, and scores. Parlando does not impose a universal result schema. Role-private terminal facts belong only in the final observation.

## Actions change state; messages communicate

A player action is a typed proposal to change game state. Every action, whether submitted by a human or agent, passes through `Game::apply_action(state, action, actor)`. That operation combines authorization, rule validation, and transition so they cannot drift.

A message is text sent from one player to the other. It does not invoke game mechanics and cannot change state by itself. An agent may use a received message when choosing a later action, just as a human may, but the state changes only when that action is accepted.

Expected rule failures use stable `ActionRejection` codes. They are distinct from runtime failures. Codes are machine-readable domain identifiers, not localized presentation strings or diagnostic channels.

## Games and agents are presentation-agnostic

Game states, actions, observations, completions, rejection codes, and transition metadata are structured domain data. They must not assume:

- that either player is human;
- that a browser, React, text terminal, voice interface, or any other presentation exists;
- how a human-readable sentence, animation, color, sound, or control is rendered; or
- what language or accessibility mode a participant uses.

Agents consume the same role-safe domain observations as humans. Agent callbacks describe domain occurrences: initial observation, accepted transition, other-player message, and opportunity to respond. They do not describe UI events.

Human presentation belongs in the participant application. It derives labels, prose, animation, and accessibility behavior from structured observations and transitions.

## The server is frontend-neutral

The Rust server must not depend on React or on the implementation of a participant frontend. Its external participant boundary is a documented, versioned HTTP/JSON/WebSocket protocol with explicit compatibility failure.

`@coli-saar/parlando-client` is a convenient implementation of that protocol for custom browser applications. Its React entry point provides `ParticipantApp` as a simple way to build React frontends. Neither package defines server semantics, and using them is optional.

Serving compiled participant assets from `Server::participant_app` is a deployment convenience, not architectural coupling. The protocol must remain implementable by a separately hosted or non-React client without changing the game semantics or server protocol.

The current `Server` builder supports same-origin assets and deployments that put a frontend and
server behind one reverse-proxy origin. It does not expose an allowed-origin policy for a browser
frontend hosted on a different origin. Cross-origin browser deployment is therefore unsupported.
A future public contract would need a narrow origin allowlist; exposing the complete internal server
configuration is not an acceptable substitute.

The protocol carries accepted actions with role-specific resulting observations, messages, presence, shared completion, and narrow capabilities. It does not carry authoritative state, generic presentation events, server credentials, provider configuration, or storage records.

## Agent lifecycle

Agent construction precedes game delivery. `Factory::create(context).await` receives role, deterministic seed, and agent-owned settings, allowing an agent to initialize resources without an observation. Only after construction succeeds does the runtime create the session's initial game state and call `Agent::start(initial_observation)`.

This mirrors a human participant entering before receiving game information and avoids requiring game initialization to wait before agent initialization can begin. Subsequent accepted actions call `observe_transition`; other-player messages call `observe_message`; normal game termination calls `finish` with the shared completion before `shutdown`; and `respond` may produce one non-empty action, message, or combined response. `finish` communicates domain information, while `shutdown` releases resources.

Completion of `create` defines construction readiness, and the runtime applies the selected agent's
action timeout to that await. The public lifecycle has no separate readiness signal,
initialization-specific timeout, or isolation for synchronous model loading. Implementations must
complete asynchronous construction within the action-timeout budget. The runtime preserves callback
ordering and runs inbox removal and `shutdown` on terminal exit paths.

## Determinism and replay

Given the same typed game configuration, recorded seed, initial state, and accepted action sequence, a game should produce the same states, observations, transition metadata, and completion. Randomness must be derived from the supplied seed and represented sufficiently in state for replay and diagnosis.

Game mechanics should be pure and must not perform I/O in `apply_action`. External model calls and services belong to agents or runtime facilities, not game transitions.

## Dashboard and durable logs are legitimate extension points

Minimizing the public API does not mean preventing a game or compiled agent from contributing useful administration and analysis data. Supported extension points are narrow:

- `GameMetadata` identifies the compiled game and build.
- `Game::Config` and `validate_config` let the dashboard edit and validate game-owned settings.
- `agent::Definition` and `agent::ConfigField` describe compiled agent choices and their settings.
- `Game::transition_metadata` contributes optional role-neutral structured data to a runtime-owned accepted-transition record.
- `Game::Completion` contributes the structured terminal result.

Parlando owns event ordering, timestamps, actor/action recording, persistence, privacy enforcement, dashboard transport, and export. Games must not receive storage handles or dashboard internals to bypass those policies.

## Configuration ownership

Game code knows only `Game::Config`. Agent factories know only their own `Context::settings`. The dashboard and runtime own participant information and consent copy, access, pairing, capacity, session lifecycle limits, privacy, voice, transcription, TTS, and deployment policy.

There is no configurable participant title. The standard participant application uses functional headings for lifecycle screens, while each game frontend owns its game-specific title and presentation. Do not reintroduce a participant page, page configuration, or title object merely to carry display text.

In particular, there is no public `StudyConfig`, participant-page configuration object, session-limits object, provider configuration, or pairing-mode type in the game-author API. The server builder exposes only process composition needed before the dashboard opens: game metadata, storage location, optional participant assets, registered agent factories, public origin, and listening address.

## Preserve durable data and historical dashboard access

Database schema changes use a backup-first, in-place upgrade procedure. Before changing an existing database, create a restorable backup and verify the target database and schema version. Then apply the smallest explicit transformation directly to that database. Do not build a general migration framework, retain an indefinite ladder of historical migrations, or require export into a replacement database for an ordinary schema change. The backup is the rollback boundary.

Code and schema changes must preserve dashboard access to every stored experiment. An older experiment may become invalid under current configuration rules, but the dashboard must still show the experiment, all of its sessions, and its configuration. The configuration must remain editable so an experimenter can bring it forward. Validation may mark the experiment invalid and must prevent new sessions until the configuration is corrected; validation must never make historical data or the configuration editor unavailable.

Dashboard read paths therefore tolerate configurations that are structurally readable but invalid under current rules. Runtime construction, activation, and configuration saves remain strict. Any schema change must include compatibility tests that exercise an older stored experiment through session listing and configuration display/editing, not only tests that open a newly created database.

## Public API discipline

A symbol is public only when downstream game, agent, or participant-application authors need a supported contract. Use by Parlando's dashboard, internal binaries, storage, or tests does not justify public exposure.

Prefer domain operations over bags of implementation options. Prefer one operation that enforces an invariant over two operations callers must order correctly. Prefer adding a focused extension when a real integration needs it over publishing speculative provider or storage abstractions.

Do not preserve obsolete public aliases merely to carry two vocabularies indefinitely. A breaking
removal requires explicit versioned migration guidance, and supported contracts should otherwise
remain stable within their compatibility window.
