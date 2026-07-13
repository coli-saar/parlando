# Parlando Config And Deployment Reference

## Experiment YAML

Generated games should include `config/experiment.local.yaml` and, when deployment is requested, `config/experiment.render.example.yaml`.

When voice, transcription, or TTS is requested, tell the user exactly where the generated YAML expects those values:

- Local runs read `config/experiment.local.yaml` by default through `make run` or `--config config/experiment.local.yaml`.
- If secrets are needed locally, generate or document an uncommitted overlay such as `config/experiment.voice.private.yaml`, include it from `config/experiment.local.yaml`, and add it to `.gitignore`.
- Render/Docker deployments should read the checked-in base file, usually `config/experiment.render.example.yaml`, plus a secret overlay mounted at deployment time. State the exact mount path used by the generated Docker/Render config.
- Put non-secret defaults in checked-in YAML. Put `LIVEKIT_API_SECRET`, `SPEECHMATICS_API_KEY`, `ELEVENLABS_API_KEY`, and other private values in the local private overlay or deployment secret file.

`ExperimentConfig::from_yaml` supports includes, environment-variable substitution, and relative path resolution. Main sections:

- `experiment`: durable experiment id.
- `study`: study name and waiting/reconnect timing.
- `server`: public base URL, CORS origins, optional `client_dist_path`.
- `database`: SQLite URL.
- `direct`: room-code and consent settings.
- `agents`: human-vs-human or human-vs-agent mode and agent selection.
- `livekit`: realtime audio room settings.
- `speechmatics`: browser STT credentials and realtime options.
- `transcription`: transcription mode exposed to the browser.
- `tts`: agent text-to-speech settings.
- `conversation`: conversation-history settings.

For local non-voice studies, keep `livekit.enabled`, `speechmatics.enabled`, `transcription.enabled`, and `tts.enabled` false. For voice studies, enable the full stack: LiveKit, Speechmatics transcription, and ElevenLabs TTS. Keep service credentials in a private YAML include or secret file, not in checked-in config or frontend build variables.

Voice enablement is a server/config decision. The generated game adapter and game-specific browser UI must not decide whether voice is enabled; they should consume the capability metadata and controls exposed by `parlando-server` and `@coli-saar/parlando-client`.

TTS enablement is also a server/config decision. When `tts.enabled` is true, generated agents should return participant-facing utterances as `AgentResult::Message` or `AgentResult::ActionWithMessage`; `parlando-server` handles synthesis, diagnostics, and audio publishing. Browser clients should display the conversation message as usual and must not call TTS providers directly.

## Local Config Template

```yaml
experiment:
  id: local-<game-slug>

study:
  name: "<Game Name>"

server:
  public_base_url: http://localhost:8000
  cors_origins:
    - http://localhost:5173
  client_dist_path: client/dist

database:
  url: sqlite:///.local/parlando.sqlite

direct:
  enabled: true
  allow_room_codes: true
  require_consent: false
  consents: []

agents:
  mode: human_vs_human

livekit:
  enabled: false

speechmatics:
  enabled: false

transcription:
  enabled: false

tts:
  enabled: false

conversation:
  enabled: true
```

## Consent Config

Consent items live in the same config file the server reads at startup, normally `config/experiment.local.yaml` for local runs and `config/experiment.render.example.yaml` plus deployment secret overlay for Render.

Use:

```yaml
direct:
  enabled: true
  allow_room_codes: true
  require_consent: true
  consents:
    - id: study
      title: Study Consent
      body_html: "I agree to participate in this study."
      required: true
    - id: recording
      title: Voice Recording Consent
      body_html: "I agree that my voice may be recorded and transcribed for research."
      required: true
```

Generated games should include reasonable placeholder consent text when the user asks for a study flow, especially for voice studies. The final answer must tell the user to replace placeholder consent text with their approved IRB/ethics-board language before collecting data.

## Voice Infrastructure Config

Only configure Parlando's voice services; do not explain or reimplement how Parlando uses LiveKit, Speechmatics, or ElevenLabs internally. A generated game should set YAML keys, keep credentials private, and let `parlando-server` and `@coli-saar/parlando-client` handle voice runtime behavior.

If the server/session exposes voice capability, the game must allow that capability through the SDK-provided session controls/status. If voice capability is absent, the game should omit or disable voice UI from session state rather than from game-specific policy.

When the user asks to enable voice, generate the full voice package: LiveKit, Speechmatics, and ElevenLabs. Do not create partial voice configs unless the user explicitly asks for a reduced setup. Use `rust-server/config/experiment.voice.full.example.yaml` as the canonical source template when working in this repository.

When voice or transcription is enabled, generated clients should delegate browser startup behavior to `ParlandoStartupGate` in `@coli-saar/parlando-client/react`.

When transcription/STT is enabled, active game screens should display SDK voice status from the `ActiveParlandoSession`, including a microphone level meter and ASR status pill using `MicLevelMeter` and `TranscriptionStatusChip` from `@coli-saar/parlando-client/react`.

Checked-in local config should include the private overlay as optional and keep services disabled by default if the overlay is absent:

```yaml
include:
  - path: config/experiment.voice.private.yaml
    optional: true

livekit:
  enabled: false

transcription:
  enabled: false

speechmatics:
  enabled: false

tts:
  enabled: false
```

The generated `config/experiment.voice.private.yaml` or deployment secret file should contain fields for all three providers:

```yaml
livekit:
  enabled: true
  url: ${LIVEKIT_URL}
  api_key: ${LIVEKIT_API_KEY}
  api_secret: ${LIVEKIT_API_SECRET}

transcription:
  enabled: true
  provider: speechmatics
  language: en-US
  model: enhanced
  store_audio: false

speechmatics:
  enabled: true
  api_key: ${SPEECHMATICS_API_KEY}
  realtime_url: wss://eu.rt.speechmatics.com/v2
  management_url: https://mp.speechmatics.com/v1/api_keys
  temporary_key_ttl_seconds: 60
  max_delay: 2.0
  enable_partials: true
  end_of_utterance_silence_trigger: 1.2

tts:
  enabled: true
  provider: elevenlabs
  model: eleven_flash_v2_5
  voice_id: ${ELEVENLABS_VOICE_ID}
  voice_name: Agent Voice
  api_key: ${ELEVENLABS_API_KEY}
  output_format: pcm_24000
```

Validation requirements:

- `livekit.url`, `livekit.api_key`, and `livekit.api_secret` are required when `livekit.enabled` is true.
- `speechmatics.enabled` and `speechmatics.api_key` are required when `transcription.enabled` is true and `transcription.provider` is `speechmatics`.
- `tts.voice_id` and `tts.api_key` are required when `tts.enabled` is true.

Use environment-variable placeholders or an uncommitted private include for credentials. Never put LiveKit, Speechmatics, or ElevenLabs secrets in frontend code.

Example local private overlay:

```yaml
# config/experiment.voice.private.yaml
livekit:
  url: wss://your-livekit-host
  api_key: your-livekit-api-key
  api_secret: your-livekit-api-secret

speechmatics:
  api_key: your-speechmatics-api-key

tts:
  voice_id: your-elevenlabs-voice-id
  api_key: your-elevenlabs-api-key
```

If you generate this file, ensure `.gitignore` excludes it. If you instead use environment variables in YAML, the user must set those variables before starting the server.

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
- `run`: run `.local/bin/<game-binary> --host 127.0.0.1 --port 8000 --config config/experiment.local.yaml`.
- `clean`: remove local build artifacts where appropriate.

Use one run target for every local mode:

```sh
make run
```

Do not generate separate `make run-voice`, `make run-no-voice`, or similar targets. Voice behavior is controlled by `config/experiment.local.yaml` and its optional private overlay.

Useful routes after start:

- `GET /health`
- `GET /api/config`
- `GET /admin/games`
- `GET /api/admin/export`

## Frontend Serving

When the browser build exists, configure:

```yaml
server:
  client_dist_path: client/dist
```

The server serves `/`, `/assets/*`, and SPA fallback paths from that directory while preserving `/api/*` and `/ws/*`.

For production images, build the client and copy `client/dist` to `/app/client-dist`, then configure:

```yaml
server:
  client_dist_path: /app/client-dist
```

## Docker

Generated Dockerfiles should:

1. Install Rust and Node build dependencies.
2. Copy the generated project source.
3. Build the client with `npm install` and `npm run build`.
4. Build/install the Rust game server binary.
5. Copy the client dist to `/app/client-dist`.
6. Start the binary with `--host 0.0.0.0 --config /app/config/experiment.yaml`.

## Render

For Render deployment:

- Use a Docker web service.
- Mount a persistent disk at `/data`.
- Store SQLite at `sqlite:////data/parlando.sqlite`.
- Use a checked-in base config plus a Render Secret File for private overrides. If the generated app follows the Parlando convention, mount the secret file at `/etc/secrets/parlando-render.yaml` and include or merge it from the production config.
- Keep LiveKit, Speechmatics, and TTS credentials in secrets.
- Set `server.public_base_url` to the deployed URL.

Example production database config:

```yaml
database:
  url: sqlite:////data/parlando.sqlite
```

The browser client should receive public capability metadata and room-scoped audio credentials through `@coli-saar/parlando-client`; do not bake voice service credentials into frontend environment variables.
