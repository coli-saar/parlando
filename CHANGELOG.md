# Changelog

All notable changes to Parlando releases are tracked in this file.

The format is based on Keep a Changelog, and this project uses semantic versioning for the published Rust server crate and JavaScript client package.

## Unreleased

### Added

- Added a game-scoped experiment catalogue to each compiled game server, with dashboard creation and cloning, pin/obsolete metadata, immutable configuration revisions, shared institution settings, and isolated participant runtimes at `/e/{experiment_id}`.
- Added exact semantic game-version binding for experiments and sessions. Historical experiments stay visible, while cloning creates a new experiment for the currently compiled game version.
- Added Privacy Contract version 1 with four persistence switches, versioned participant-information evidence, fixed `research`, `corpus`, and `full` exports, an authenticated installation-wide privacy status, and counted manual participant deletion in the admin dashboard.
- Added random three-word identifiers for human participants and dialogues. Human participant identifiers are scoped to one experiment and remain consistent across that experiment's sessions and repeated exports; dialogue identifiers remain consistent across repeated exports. Agent participants instead use descriptive identifiers containing their agent type, implementation name when available, and version.
- Added application-owned administrator authentication, roles, CSRF protection, participant bearer credentials, one-use game/audio upgrade tickets, exact origin enforcement, bounded resources, transactional action persistence, secret-redaction tooling, hardened container packaging, and authenticated TLS-capable remote agents.

### Changed

- Made each server process own one compiled game implementation and any number of experiments for that exact game version. Startup resets every experiment to `inactive`; the authenticated dashboard explicitly activates or deactivates each intake, and deactivation leaves existing sessions connected.
- Moved experiment configuration into database-backed dashboard editing. Listener port, database location, client bundle, and provider credentials remain process bootstrap settings rather than experiment revisions.
- Simplified the JavaScript startup gate to show only the configured study title and current platform tasks; removed arbitrary startup-label, eyebrow, setup-copy, and game-hint customization.
- Added an optional game-level operating institution shown above the game title, and derived consent display and enforcement solely from configured consent items instead of a separate `require_consent` flag.
- Removed participant-entered display names from the browser flow, protocol, runtime, database, admin interface, and exports.
- Changed the `corpus` export description to a publication-oriented `corpus_candidate`: it removes internal identifiers and absolute timestamps but requires removal of recruitment mappings and content review before it can be treated as anonymous.
- Scoped external recruitment identities to one experiment, so the same external identifier receives an independently generated participant identifier in every experiment.
- Made configuration strict and fail-closed: unknown fields and missing environment placeholders now fail startup, runtime limits and provider URLs are validated, consent copy is plain text, and obsolete no-op switches are no longer accepted.
- Made participant intake capacity-safe with per-peer and global creation limits, pre-insert capacity checks, and cleanup tied to credential expiry.
- Made all room assignment server-owned and made JavaScript participant authentication implicit in each client instance rather than passing session identifiers through authenticated method signatures.
- Versioned SQLite compatibility migrations and narrowed the published Rust and JavaScript package contents and public surface.
- Removed conditional test authentication and the final participant-ID WebSocket compatibility path; unit tests now use real administrator sessions, bearer credentials, CSRF tokens, and one-use game tickets.
- Split router construction, lifecycle policy, dashboard markup, application tests, and storage tests into focused source files.

### Removed

- Removed the `agents.available` configuration and the `/admin/games`, `/api/admin/games`, and `/api/direct/start` compatibility aliases.
- Removed participant-session identifiers as authentication material and removed credentials, recruitment identifiers, consent evidence, and full configuration from normal research/corpus exports.
- Removed public room-code joining, caller-selected room modes, the test-only memory persistence backend, duplicate voice UI implementations, unused agent selector aliases, and the Space Game's unexposed direct-move and reset actions.

## [0.2.0] - 2026-08-13

### Changed

- Added an authenticated Parlando PCM audio relay and server-side transcription provider boundary.
- Routed final speech utterances through the normal conversation and agent-observation path, with duplicate-final protection.
- Routed agent TTS through the same room relay and kept provider credentials out of browser code.
- Stabilized synthesized speech with absolute server frame deadlines, jitter prebuffering, linear browser resampling, short underrun recovery, and underrun diagnostics.
- Made playback worklets self-contained for consumer bundlers and prevented waiting-room transport failures from reacquiring an already prepared microphone.
- Added credential-free provider-contract, token, reconnect, and multi-room tests plus a standalone ten-minute Ratatui stress dashboard with CLI-selectable realistic, saturation, and impaired relay workloads.

## [0.1.3]

### Added

- Added generated-game guidance for explicit terminal outcomes, agent message observation, SDK voice-status widgets, and reliable microphone meter styling.

### Changed

- Improved the default human-human startup flow by keeping room pairing server-owned instead of exposing generic room selection in the shared startup gate.
- Treat browser tab/window teardown like the in-game leave action so room presence updates when participants close the game.
- Made agent action/message responses apply game actions before speech, and made follow-up decisions wait for the next delivered observation.
- Preserved complete playback of longer synthesized agent messages.
- Updated researcher-facing documentation around workflows, build requirements, route names, analysis summaries, and deployment boundaries.

## [0.1.2]

### Changed

- Reworked the agent API toward an event-driven observation and decision model.
- Updated the remote agent protocol and Python SDK surfaces to match the new agent API.

## [0.1.1]

### Added

- Added `ParlandoStartupGate` to `@coli-saar/parlando-client/react` so generated games can reuse one setup, consent, voice-preflight, room-backed waiting-room, WebSocket, and audio-session startup flow.
- Added startup-flow tests for the JavaScript client package.
- Added public config metadata that tells browser clients whether the study is human-vs-human or human-vs-agent.
- Added room presence snapshots to room responses so clients can render readiness for Player A, Player B or an agent, and voice/transcription services.

### Changed

- Moved the game setup sequence from generated/demo game clients into the shared JavaScript client SDK.
- Simplified the Space Game browser client so it renders the active game through `ParlandoStartupGate` instead of carrying its own lobby, consent, matchmaking, WebSocket, and voice startup code.
- Changed human-vs-agent room creation so the server supplies the agent as role B immediately, then keeps the room in the waiting phase until readiness gates pass.
- Updated game-generation guidance to require the SDK startup gate and a room-backed waiting room instead of custom no-room matchmaking flows.
- Bumped `parlando-server` and `@coli-saar/parlando-client` from `0.1.0` to `0.1.1`.

### Removed

- Removed the public no-room matchmaking endpoints and client helpers from the main startup flow.
- Removed generated-client guidance that reimplemented room startup, waiting-room polling, and voice setup locally in each game.

## [0.1.0]

### Added

- First packaged Parlando baseline with `parlando-server` as the reusable Rust experiment server runtime.
- Published the reusable browser SDK as `@coli-saar/parlando-client`.
- Included typed game adapter boundaries, room creation and join flows, WebSocket game updates, conversation events, consent handling, and durable experiment/session export support.
- Included browser audio-session support, transcription, server-side TTS, and agent audio publishing foundations.
- Included local development and generation support through the Space Game demo, documentation, Makefiles, and the `generate-parlando-game` skill.
