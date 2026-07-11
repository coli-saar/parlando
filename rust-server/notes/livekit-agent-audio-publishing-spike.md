# LiveKit Agent Audio Publishing Spike

## Context

Step 12 requires publishing synthesized agent audio into the LiveKit room as a mono `agent-voice` track, or documenting and smoke-testing a narrow sidecar fallback while keeping Rust authoritative for game state, storage, tokens, agents, conversation, and diagnostics.

## Current Status

Rust now owns the upstream pieces needed before publishing and has a first Rust publishing implementation:

- Agent messages are persisted as `conversation_message` events.
- The agent runtime calls TTS directly for text returned by agents.
- Message IDs are marked seen before synthesis starts.
- ElevenLabs WebSocket streaming is implemented and tested with a mock WebSocket server.
- TTS diagnostics are persisted as `tts_diagnostic` events for start, first audio, chunks, publish start, publish completion, completion, and failure.
- `AgentAudioPublisher` is implemented as the narrow publishing abstraction.
- `LiveKitAgentAudioPublisher` connects a short-lived `agent-voice` participant, publishes a native audio track, and submits PCM chunks through LiveKit's Rust SDK.
- The Space Game binary wires `LiveKitAgentAudioPublisher` when LiveKit and TTS are both enabled.
- Unit/integration tests verify PCM conversion and the mocked TTS-to-publisher diagnostic path.

What remains not fully proven:

- No browser/manual test has verified that clients hear synthesized agent audio.
- No sidecar process has been implemented or smoke-tested in this Rust workspace.

## Design Assessment

The current Rust path follows the intended narrow abstraction:

- `AgentAudioPublisher::publish(room_id, message_id, chunks) -> Result<AudioPublishSummary>`
- A Rust LiveKit implementation using `NativeAudioSource` and `LocalAudioTrack`.
- A sidecar implementation only if Rust RTC publishing is not viable.

The existing direct TTS path should depend on that abstraction instead of directly depending on LiveKit. That keeps TTS provider streaming, diagnostics, and LiveKit publishing separable.

## Risks

- LiveKit Rust RTC support exposes the required audio publishing API. Local macOS validation initially aborted during WebRTC video encoder initialization because the final test binary dropped an Objective-C category from the static WebRTC archive; adding final-link `-ObjC` flags fixed that path.
- Browser SDK behavior may still differ from the Rust subscriber used by the live tests.
- The remaining manual acceptance test needs real LiveKit credentials and a browser client in the room.

## Required Verification

Before step 12 can be marked complete with browser-level production confidence, one of these must be true:

- The Rust publisher is run successfully on Linux or the deployment platform and a manual/ignored test proves browsers hear the `agent-voice` track.
- A sidecar fallback is implemented, documented, and smoke-tested while Rust remains authoritative for state, storage, tokens, agents, conversation, and diagnostics.

## Live Test

The ignored test `livekit_streams_saved_pcm_between_participants_and_speechmatics_transcribes_it` connects two LiveKit participants, publishes the bundled PCM resource as an audio track, receives decoded PCM from the other participant, and submits the received audio to Speechmatics. The ignored test `livekit_audio_session_and_agent_voice_work_through_parlando_game_room` starts a real Parlando server with a test-only game, creates and joins a room through HTTP, requests `/audio-session`, connects with the returned LiveKit credentials, publishes through the production `LiveKitAgentAudioPublisher`, receives the `agent-voice` track, and verifies it through Speechmatics. Both passed locally on macOS on 2026-07-11 with `PARLANDO_ALLOW_MACOS_LIVEKIT_RTC=1`.
