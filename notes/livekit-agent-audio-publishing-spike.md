# LiveKit Agent Audio Publishing Spike

## Context

Step 12 requires publishing synthesized agent audio into the LiveKit room as a mono `agent-voice` track, or documenting and smoke-testing a narrow sidecar fallback while keeping Rust authoritative for game state, storage, tokens, agents, conversation, and diagnostics.

## Current Status

Rust now owns the upstream pieces needed before publishing:

- Agent messages are persisted as `conversation_message` events.
- The agent runtime calls TTS directly for text returned by agents.
- Message IDs are marked seen before synthesis starts.
- ElevenLabs WebSocket streaming is implemented and tested with a mock WebSocket server.
- TTS diagnostics are persisted as `tts_diagnostic` events for start, first audio, chunks, completion, and failure.

What is not implemented:

- No Rust LiveKit RTC publisher is wired into the server.
- No `agent-voice` track is created.
- No browser/manual test has verified that clients hear synthesized agent audio.
- No sidecar process has been implemented or smoke-tested in this Rust workspace.

## Design Assessment

The clean next step is to introduce a narrow audio publishing abstraction, for example:

- `AgentAudioPublisher::publish(room_id, message_id, chunks) -> Result<AudioPublishSummary>`
- A Rust LiveKit implementation if the selected crate can publish PCM frames reliably.
- A sidecar implementation only if Rust RTC publishing is not viable.

The existing direct TTS path should depend on that abstraction instead of directly depending on LiveKit. That keeps TTS provider streaming, diagnostics, and LiveKit publishing separable.

## Risks

- LiveKit Rust RTC support may not expose the exact high-level publishing API needed for browser-compatible PCM track publishing.
- Audio format conversion may be required if LiveKit expects a frame format different from the ElevenLabs PCM output.
- The manual acceptance test needs real LiveKit credentials and a browser client in the room.

## Required Verification

Before step 12 can be marked complete, one of these must be true:

- A Rust publisher is implemented and a manual ignored test proves browsers hear the `agent-voice` track.
- A sidecar fallback is implemented, documented, and smoke-tested while Rust remains authoritative for state, storage, tokens, agents, conversation, and diagnostics.
