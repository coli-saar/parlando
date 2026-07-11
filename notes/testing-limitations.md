# Testing Limitations To Revisit

## Context

The Rust server has broad automated coverage for the reusable server and Space Game crates, but several production-facing behaviors are still verified only with local integration tests, mocks, or protocol-shape assertions. These areas need explicit attention before treating the Rust server as a drop-in replacement in a live experiment.

Steps 5-11 of the implementation plan are complete at the Rust server-library level. That means the server behavior, persistence, WebSocket runtime, game adapter boundary, in-process agents, LiveKit/Speechmatics protocol surfaces, and ElevenLabs TTS path are covered by automated Rust tests. The remaining gap is not basic implementation coverage; it is production confidence against the actual browser client, real RTC audio, deployment packaging, and future remote-agent support.

## Current Automated Coverage

- `cargo test` passes for the full workspace.
- Server tests cover participant creation, consent, rooms, matchmaking, SQLite persistence, WebSocket gameplay, transcripts, diagnostics, export, LiveKit token shape, Speechmatics temporary-key shape, in-process agents, and TTS diagnostics.
- Server integration tests include a mock browser client over HTTP/WebSocket with a test-only dummy game adapter. This covers two-human gameplay, human-vs-agent gameplay, typed chat, transcript POST, audio-session shape, agent messages, TTS diagnostics, and export without depending on `parlando-space-game`.
- Ignored live-service tests have been run successfully against local private credentials for LiveKit token minting, Speechmatics temporary-key minting, Speechmatics realtime transcription from a bundled PCM resource, ElevenLabs PCM generation to a temporary file, and the combined ElevenLabs-to-Speechmatics path.
- Space Game tests cover typed state transitions, observations, available actions, events, serialization shape, completion summaries, and bundled agent factories.

## Known Limitations

- Browser smoke tests against the actual existing TypeScript client have not been run after the latest protocol cleanups.
- Mock-browser tests exercise HTTP and WebSocket flows, but they do not execute the real React/TypeScript client code, browser media APIs, bundling, routing, or UI state management.
- LiveKit token and audio-session tests verify JSON and claim shape, plus local token minting from real credentials. They do not yet prove browser audio in a live LiveKit room.
- Agent TTS currently records diagnostics but does not publish audible audio into LiveKit.
- Remote Python/gRPC agents are designed but not implemented or tested.
- Static serving is implemented, but production packaging and deploy smoke tests are not complete.
- End-to-end browser flows in the actual TypeScript UI have not yet verified two-human gameplay, human-vs-agent gameplay, typed chat, transcript POST, LiveKit audio-session acceptance, Speechmatics transcription, agent messages, or TTS diagnostics together.

## Related Test Backlog

Concrete tests to add next are tracked in `notes/test-todos.md`.
