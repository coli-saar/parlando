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
PARLANDO_RUN_LIVE_TESTS=1 cargo test -p parlando-server --test live_services -- --ignored
```

Use a specific private config:

```bash
PARLANDO_RUN_LIVE_TESTS=1 PARLANDO_LIVE_CONFIG=config/experiment.livekit.private.yaml cargo test -p parlando-server --test live_services -- --ignored
```

Run one expensive test by name:

```bash
PARLANDO_RUN_LIVE_TESTS=1 cargo test -p parlando-server --test live_services speechmatics_transcribes_elevenlabs_generated_audio -- --ignored
```

Check ElevenLabs can generate PCM audio, writing only to a temporary file:

```bash
PARLANDO_RUN_LIVE_TESTS=1 cargo test -p parlando-server --test live_services elevenlabs_generates_temp_audio_file -- --ignored
```

Run Speechmatics STT against the saved resource without calling ElevenLabs:

```bash
PARLANDO_RUN_LIVE_TESTS=1 cargo test -p parlando-server --test live_services speechmatics_transcribes_saved_test_resource -- --ignored
```

The default audio resource path is:

```text
crates/parlando-server/tests/resources/elevenlabs-speechmatics-test.pcm
```

Override it with `PARLANDO_LIVE_AUDIO_PCM=/path/to/audio.pcm`. The ElevenLabs test does not update this resource.

## Test Coverage

- `livekit_credentials_mint_join_token_from_private_config`: loads private LiveKit credentials and mints a short-lived room token.
- `speechmatics_temporary_key_mints_with_live_credentials`: calls the real Speechmatics management API for a temporary realtime key.
- `elevenlabs_generates_temp_audio_file`: generates short PCM audio with ElevenLabs and writes it to a temporary file.
- `speechmatics_transcribes_saved_test_resource`: streams the saved PCM resource to Speechmatics realtime STT.
- `speechmatics_transcribes_elevenlabs_generated_audio`: generates fresh PCM audio with ElevenLabs, then checks Speechmatics realtime STT.

Last verified locally on 2026-07-11 with all five ignored live tests passing.

## Limitations

- The LiveKit test does not yet join a real RTC room.
- The Speechmatics tests spend paid API credits.
- Regenerating the PCM test resource spends ElevenLabs credits.
- These tests are intended for explicit pre-release validation, not routine development.
