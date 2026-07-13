# Changelog

All notable changes to Parlando releases are tracked in this file.

The format is based on Keep a Changelog, and this project uses semantic versioning for the published Rust server crate and JavaScript client package.

## [0.1.3]

### Added

- Added generated-game guidance for explicit terminal outcomes, agent message observation, SDK voice-status widgets, and reliable microphone meter styling.
- Added generated-server guidance for retaining WebRTC Objective-C categories in final macOS game binaries.

### Changed

- Improved the default human-human startup flow by keeping room pairing server-owned instead of exposing generic room selection in the shared startup gate.
- Treat browser tab/window teardown like the in-game leave action so room presence updates when participants close the game.
- Made agent action/message responses apply game actions before speech, and made follow-up decisions wait for the next delivered observation.
- Kept LiveKit agent audio tracks alive for the computed PCM playback duration so longer TTS messages are not truncated.
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
- Included browser audio-session support for LiveKit partner audio and Speechmatics transcription, plus server-side TTS and agent audio publishing foundations.
- Included local development and generation support through the Space Game demo, documentation, Makefiles, and the `generate-parlando-game` skill.
