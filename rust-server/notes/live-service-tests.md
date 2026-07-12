# Live Service Tests

## Context

LiveKit, Speechmatics, and ElevenLabs checks can use real credentials and paid external APIs. They must never run as part of normal `cargo test`.

## Credential Files

- Private YAML files live under `config/`.
- `config/*.private.yaml` and `config/*.private.yml` are ignored by git.
- The copied old-server credential file is `config/experiment.livekit.private.yaml`.
- Add Speechmatics and ElevenLabs credentials to a private YAML before running the STT/TTS live tests.

## Running Tests

All live tests are marked `#[ignore]` and also require `PARLANDO_RUN_LIVE_TESTS=1`.

Use the default private config:

```bash
PARLANDO_RUN_LIVE_TESTS=1 cargo test --test live_services -- --ignored
```

Use a specific private config:

```bash
PARLANDO_RUN_LIVE_TESTS=1 PARLANDO_LIVE_CONFIG=config/experiment.livekit.private.yaml cargo test --test live_services -- --ignored
```

Run one expensive test by name:

```bash
PARLANDO_RUN_LIVE_TESTS=1 cargo test --test live_services speechmatics_transcribes_elevenlabs_generated_audio -- --ignored
```

Check ElevenLabs can generate PCM audio, writing only to a temporary file:

```bash
PARLANDO_RUN_LIVE_TESTS=1 cargo test --test live_services elevenlabs_generates_temp_audio_file -- --ignored
```

Run Speechmatics STT against the saved resource without calling ElevenLabs:

```bash
PARLANDO_RUN_LIVE_TESTS=1 cargo test --test live_services speechmatics_transcribes_saved_test_resource -- --ignored
```

The default audio resource path is:

```text
rust-server/tests/resources/elevenlabs-speechmatics-test.pcm
```

Override it with `PARLANDO_LIVE_AUDIO_PCM=/path/to/audio.pcm`. The ElevenLabs test does not update this resource.

Run the LiveKit RTC PCM conversation test:

```bash
PARLANDO_RUN_LIVE_TESTS=1 PARLANDO_ALLOW_MACOS_LIVEKIT_RTC=1 cargo test --test live_services livekit_streams_saved_pcm_between_participants_and_speechmatics_transcribes_it -- --ignored --nocapture
```

This test connects two LiveKit participants, streams the saved PCM resource through LiveKit, receives decoded PCM as the other participant, and submits that received audio to Speechmatics. On macOS it is skipped by default because it exercises native WebRTC and paid external services; set `PARLANDO_ALLOW_MACOS_LIVEKIT_RTC=1` when intentionally validating the macOS path.

Run the LiveKit game-room agent-voice test:

```bash
PARLANDO_RUN_LIVE_TESTS=1 PARLANDO_ALLOW_MACOS_LIVEKIT_RTC=1 cargo test --test live_services livekit_audio_session_and_agent_voice_work_through_parlando_game_room -- --ignored --nocapture
```

This test starts a real Parlando server with a test-only game adapter, creates and joins a room through the public HTTP API, requests the same `/api/rooms/{room_id}/audio-session` plan used by the browser client, connects a LiveKit participant with the returned sink credentials, publishes saved PCM through the production `LiveKitAgentAudioPublisher` as `agent-voice`, receives the audio from LiveKit, and submits the received audio to Speechmatics.

## Test Coverage

- `livekit_credentials_mint_join_token_from_private_config`: loads private LiveKit credentials and mints a short-lived room token.
- `speechmatics_temporary_key_mints_with_live_credentials`: calls the real Speechmatics management API for a temporary realtime key.
- `elevenlabs_generates_temp_audio_file`: generates short PCM audio with ElevenLabs and writes it to a temporary file.
- `speechmatics_transcribes_saved_test_resource`: streams the saved PCM resource to Speechmatics realtime STT.
- `speechmatics_transcribes_elevenlabs_generated_audio`: generates fresh PCM audio with ElevenLabs, then checks Speechmatics realtime STT.
- `livekit_streams_saved_pcm_between_participants_and_speechmatics_transcribes_it`: streams the saved PCM resource through a real LiveKit room, receives it as another participant, and transcribes the received audio with Speechmatics.
- `livekit_audio_session_and_agent_voice_work_through_parlando_game_room`: exercises the actual Parlando game-room LiveKit path through public HTTP room APIs, `/audio-session`, the production `agent-voice` publisher, LiveKit receive, and Speechmatics verification.

Last verified locally on 2026-07-11 with the original five ignored live tests passing. The lower-level RTC test and game-room agent-voice test also passed locally on macOS after adding final-link `-ObjC` flags for the LiveKit/WebRTC Objective-C categories.

## Limitations

- The Speechmatics tests spend paid API credits.
- Regenerating the PCM test resource spends ElevenLabs credits.
- The LiveKit RTC tests spend LiveKit/Speechmatics resources and may be slower or flaky because they depend on realtime media setup.
- These tests are intended for explicit pre-release validation, not routine development.
