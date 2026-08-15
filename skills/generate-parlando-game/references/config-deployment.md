# Parlando Config And Deployment Reference

## Dashboard-owned experiment configuration

Generated games use one process for one compiled game and any number of that game's
experiments. Researchers create, clone, and edit experiments in `/admin/experiments`.
Each save creates an immutable database revision; YAML is not the ongoing source of truth.

The process must receive only bootstrap values needed before the dashboard exists:
`--host`, required `--port`, `--database-url`, `--client-dist`, and provider secret
environment variables. `--config` may remain as an optional one-time migration input.
Main experiment sections shown in the dashboard are:

- `experiment`: durable experiment id.
- `study`: study title and waiting/reconnect timing.
- `direct`: room-code and consent settings.
- `agents`: human-vs-human or human-vs-agent mode and agent selection.
- `voice`: server audio-relay format and buffering settings.
- `speechmatics`: server-side STT credentials and realtime options.
- `transcription`: provider-neutral transcription settings.
- `tts`: agent text-to-speech settings.
- `conversation`: conversation-history settings.

The institution is game-level configuration shared by every experiment. For local non-voice studies, keep `voice.enabled`, `transcription.enabled`, and `tts.enabled` false. For voice studies, enable the Parlando relay and the requested Speechmatics/ElevenLabs features in the dashboard, while supplying credentials through `SPEECHMATICS_API_KEY` and `ELEVENLABS_API_KEY`.

Voice enablement is a server/config decision. The generated game adapter and game-specific browser UI must not decide whether voice is enabled; they should consume the capability metadata and controls exposed by `parlando-server` and `@coli-saar/parlando-client`.

Protocol version 1 is fixed at 24 kHz mono PCM16 in 20 ms frames. Keep `sample_rate_hz: 24000` and `frame_duration_ms: 20`; use `jitter_buffer_ms: 100` unless deployment measurements justify another value. Configure TTS as `pcm_24000`. Do not offer codec, channel, or transport choices that the runtime does not implement.

TTS enablement is also a server/config decision. When `tts.enabled` is true, generated agents should return participant-facing utterances in `AgentResponse.message`; `parlando-server` handles synthesis, diagnostics, and audio publishing. Browser clients should display the conversation message as usual and must not call TTS providers directly.

## Legacy migration template

```yaml
experiment:
  id: local-<game-slug>

study:
  name: "<Game Name>"

direct:
  enabled: true
  consents: []

agents:
  mode: human_vs_human

voice:
  enabled: false

transcription:
  enabled: false

tts:
  enabled: false

```

## Consent Config

Consent items live in the selected experiment's database-backed dashboard configuration.

Use:

```yaml
direct:
  enabled: true
  consents:
    - id: study
      title: Study Consent
      body: "I agree to participate in this study."
      required: true
    - id: recording
      title: Voice Recording Consent
      body: "I agree that my voice may be recorded and transcribed for research."
      required: true
```

Generated games should include reasonable placeholder consent text when the user asks for a study flow, especially for voice studies. The final answer must tell the user to replace placeholder consent text with their approved IRB/ethics-board language before collecting data.

The presence of one or more `direct.consents` items enables the consent screen and server-side enforcement. An empty list skips consent; there is no separate enablement flag. `participant_information_version` and `participant_information_url` are optional evidence metadata and do not enable or disable consent.

## Voice Infrastructure Config

Only configure Parlando's voice services; do not reimplement the relay, Speechmatics, or ElevenLabs integration. A generated game should expose the standard dashboard settings, keep credentials in server environment variables, and let `parlando-server` and `@coli-saar/parlando-client` handle runtime behavior.

If the server/session exposes voice capability, the game must allow that capability through the SDK-provided session controls/status. If voice capability is absent, the game should omit or disable voice UI from session state rather than from game-specific policy.

When the user asks to enable conversational voice, use `rust-server/config/experiment.voice.full.example.yaml` as a field-value reference, then expose the non-secret settings through the normal dashboard editor.

When voice or transcription is enabled, generated clients should delegate browser startup behavior to `ParlandoStartupGate` in `@coli-saar/parlando-client/react`.

When transcription/STT is enabled, active game screens should display SDK voice status from the `ActiveParlandoSession`, including a microphone level meter and ASR status pill using `MicLevelMeter` and `TranscriptionStatusChip` from `@coli-saar/parlando-client/react`.

The initial stored defaults should keep services disabled:

```yaml
voice:
  enabled: false

transcription:
  enabled: false

tts:
  enabled: false
```

Researchers enable these non-secret experiment fields in the dashboard:

```yaml
voice:
  enabled: true
  sample_rate_hz: 24000
  frame_duration_ms: 20
  jitter_buffer_ms: 100

transcription:
  enabled: true
  provider: speechmatics
  language: en-US
  model: enhanced

speechmatics:
  realtime_url: wss://eu.rt.speechmatics.com/v2
  max_delay: 2.0
  enable_partials: true
  end_of_utterance_silence_trigger: 1.2

tts:
  enabled: true
  provider: elevenlabs
  model: eleven_flash_v2_5
  voice_id: ${ELEVENLABS_VOICE_ID}
  voice_name: Agent Voice
  output_format: pcm_24000
```

Validation requirements:

- `voice.sample_rate_hz` must be `24000` and `voice.frame_duration_ms` must be `20` when voice is enabled.
- `SPEECHMATICS_API_KEY` must be present in the server environment when Speechmatics transcription is enabled.
- `tts.voice_id` and `ELEVENLABS_API_KEY` are required when TTS is enabled.

Never put Speechmatics or ElevenLabs secrets in experiment revisions, frontend code, or checked-in files.

For human-vs-agent, set:

```yaml
agents:
  mode: human_vs_agent
  human_vs_agent:
    factory: <game-slug>.demo_agent
    seed: 0
    act_timeout_seconds: 5
    invalid_action_limit: 3
    config: {}
```

This setting controls how the server fills participant slots. It must not change the game contract or browser UI model: each browser instance still represents one human participant, and the other participant's actions arrive through the same server event/action path whether they came from a human or an agent.

## Local Build And Run

Generated Makefiles should install and run from the generated project, not from a Parlando source checkout.

Useful targets:

- `install-client-deps`: run `npm install` in `client`.
- `build-client`: run `npm run build` in `client`.
- `install-server`: run `cargo install --path server --root .local`.
- `build`: install dependencies, build client, and install server.
- `test`: run Rust tests and client tests.
- `run`: run `.local/bin/<game-binary> --host 127.0.0.1 --port 8000 --database-url sqlite:///.local/parlando.sqlite --client-dist client/dist`.
- `clean`: remove local build artifacts where appropriate.

Use one run target for every local mode:

```sh
make run
```

Do not generate separate `make run-voice`, `make run-no-voice`, or similar targets. Voice behavior is controlled by each experiment's dashboard configuration.

Useful routes after start:

- `GET /health`
- `GET /e/{experiment_id}/api/config`
- `GET /admin/experiments`
- `GET /api/admin/runtime/{experiment_id}/export`

## Frontend Serving

When the browser build exists, pass `--client-dist client/dist`.

The server serves the SPA beneath `/e/{experiment_id}/` while preserving scoped API and WebSocket paths.

For production images, build the client, copy `client/dist` to `/app/client-dist`, and set `PARLANDO_CLIENT_DIST=/app/client-dist`.

## Docker

Generated Dockerfiles should:

1. Install Rust and Node build dependencies.
2. Copy the generated project source.
3. Build the client with `npm install` and `npm run build`.
4. Build/install the Rust game server binary.
5. Copy the client dist to `/app/client-dist`.
6. Start the binary with `--host 0.0.0.0 --port 8000`.

## Render

For Render deployment:

- Use a Docker web service.
- Mount a persistent disk at `/data`.
- Store SQLite at `sqlite:////data/parlando.sqlite`.
- Set `PARLANDO_DATABASE_URL=sqlite:////data/parlando.sqlite`.
- Keep Speechmatics and TTS credentials in secret environment variables.
- Run one Parlando process or configure sticky room routing. Audio rooms and one-use audio credentials are process-local, so ordinary stateless balancing across instances will break voice sessions.

The browser client should receive public capability metadata and room-scoped audio credentials through `@coli-saar/parlando-client`; do not bake voice service credentials into frontend environment variables.
