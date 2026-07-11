# Testing Limitations To Revisit

## Context

The Rust server has broad automated coverage for the reusable server and Space Game crates, but several production-facing behaviors are still verified only with local integration tests, mocks, or protocol-shape assertions. These areas need explicit attention before treating the Rust server as a drop-in replacement in a live experiment.

Steps 5-11 of the implementation plan are complete at the Rust server-library level. That means the server behavior, persistence, WebSocket runtime, game adapter boundary, in-process agents, mock remote gRPC agents, LiveKit/Speechmatics protocol surfaces, and ElevenLabs TTS path are covered by automated Rust tests. The remaining gap is not basic implementation coverage; it is production confidence against the actual browser client, fully verified RTC audio, deployment packaging, and a real Python remote-agent process.

## Current Automated Coverage

- `cargo test` passes for the full workspace.
- Server tests cover participant creation, consent, rooms, matchmaking, waiting-room readiness, Speechmatics readiness gating, SQLite persistence, WebSocket gameplay, transcripts, diagnostics, export, LiveKit token shape, Speechmatics temporary-key shape, in-process agents, remote gRPC metadata, and TTS/audio-publisher diagnostics.
- Server integration tests include a mock browser client over HTTP/WebSocket with a test-only dummy game adapter. This covers two-human gameplay, human-vs-agent gameplay, typed chat, transcript POST, audio-session shape, in-process agent messages/actions, mock remote gRPC agent messages/actions, TTS diagnostics, and export without depending on `parlando-space-game`.
- Ignored live-service tests have been run successfully against local private credentials for LiveKit token minting, Speechmatics temporary-key minting, Speechmatics realtime transcription from a bundled PCM resource, ElevenLabs PCM generation to a temporary file, and the combined ElevenLabs-to-Speechmatics path.
- Ignored LiveKit RTC tests have passed locally on macOS with `PARLANDO_ALLOW_MACOS_LIVEKIT_RTC=1`, including a game-room path that creates a Parlando room through HTTP, requests `/audio-session`, publishes `agent-voice` with the production publisher, receives audio over LiveKit, and verifies it through Speechmatics. These still need Linux/deployment validation because native WebRTC behavior can differ by platform.
- Space Game tests cover typed state transitions, observations, available actions, events, serialization shape, completion summaries, and bundled agent factories.

## Known Limitations

- Browser smoke tests against the actual existing TypeScript client have not been run after the latest protocol cleanups.
- Mock-browser tests exercise HTTP and WebSocket flows, but they do not execute the real React/TypeScript client code, browser media APIs, bundling, routing, or UI state management.
- LiveKit token and audio-session tests verify JSON and claim shape, plus local token minting from real credentials. The Rust LiveKit audio publisher is implemented, and ignored live tests prove Rust-to-Rust audio transport plus Speechmatics transcription through both a low-level LiveKit room and the real Parlando game-room `/audio-session` path. Audible browser audio has not been manually verified.
- Agent TTS records diagnostics and calls an audio-publisher abstraction. The LiveKit implementation compiles, and macOS RTC validation passes after retaining WebRTC Objective-C categories with final-link `-ObjC`.
- Remote gRPC agents are implemented and tested against an in-process mock tonic service. The Python SDK/server wrapper exists, but a real Python-process test has not been run because grpcio dependencies are not installed in the ambient interpreter.
- Static serving and basic Docker/Render packaging are implemented, but production deploy smoke tests are not complete.
- End-to-end browser flows in the actual TypeScript UI have not yet verified two-human gameplay, human-vs-agent gameplay, typed chat, transcript POST, LiveKit audio-session acceptance, Speechmatics transcription, agent messages, or TTS diagnostics together.

## Related Test Backlog

Concrete tests to add next are tracked in `notes/test-todos.md`.
