# Running And Deployment

This page covers local development, experiment configuration, frontend serving, Docker, Render, and hosted remote agents.

The same server shape applies to direct links, matched participant sessions, human-vs-agent studies, and Prolific-oriented flows: create or recover participant sessions, route participants into the waiting-room flow, and persist the same evaluation records.

## Local Development

Run Rust tests:

```sh
cd rust-server
cargo test
cd ..
```

Run the demo server with defaults:

```sh
cargo run --manifest-path space-game/server/Cargo.toml -- --host 127.0.0.1 --port 8000
```

Run with a config:

```sh
cargo run --manifest-path space-game/server/Cargo.toml -- \
  --host 127.0.0.1 \
  --port 8000 \
  --config space-game/config/experiment.render.example.yaml
```

Useful routes:

- `GET /health`
- `GET /api/config`
- `GET /admin/experiments`
- `GET /api/admin/export`

`/admin/games` remains available as a compatibility alias for older deployments.

## Configuration

Experiment configuration is YAML loaded by `ExperimentConfig::from_yaml`. It supports includes, environment-variable substitution, and relative path resolution.

Keep one checked-in base config for public, repeatable settings. Put deployment-specific secrets, service credentials, and private URLs in an ignored include file or a platform secret file.

The main sections are:

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

For local development without paid audio services, keep `livekit.enabled`, `speechmatics.enabled`, `transcription.enabled`, and `tts.enabled` false. For voice studies, keep service credentials in a private YAML include rather than in checked-in config or frontend build variables.

## Serving A Frontend

When a frontend build is available, set:

```yaml
server:
  client_dist_path: /path/to/client/dist
```

The server serves `/`, `/assets/*`, and SPA fallback paths from that directory while preserving `/api/*` and `/ws/*`.

The Space Game frontend lives under `space-game/client`. Local configs point the server at `client/dist`; the production Dockerfile builds that client and copies it to `/app/client-dist`.

If you serve the frontend separately, keep `server.public_base_url` and CORS origins aligned with the deployed browser origin, and leave API/WebSocket routes on the Parlando server.

## Render Deployment

The repository includes Docker and Render examples for the demo server under `space-game/`.

1. Create a Render web service from the repository.
2. Use `space-game/render.yaml` or configure a Docker service manually with `space-game/Dockerfile`.
3. Attach a persistent disk mounted at `/data`.
4. Add a Render Secret File named `parlando-render.yaml`. Render mounts it at `/etc/secrets/parlando-render.yaml` for Docker services.
5. Paste deployment-specific YAML into that secret file. Start from `space-game/config/experiment.render.secret.example.yaml`, set `experiment.id` and `server.public_base_url`, and add LiveKit, Speechmatics, and ElevenLabs credentials only when voice features are enabled.
6. Deploy.

The committed Render config at `space-game/config/experiment.render.example.yaml` includes `/etc/secrets/parlando-render.yaml` as an optional overlay. The secret file deep-merges over the checked-in base config, so it can override public deployment settings, enable `agents.mode`, or provide private service credentials without expanding the Render environment-variable list.

The browser client should not receive LiveKit, Speechmatics, or TTS settings from build-time environment variables. It receives public capability metadata from `/api/config` and room-scoped audio credentials from `/api/rooms/{room_id}/audio-session`.

The included `space-game/Dockerfile` builds `parlando-space-game` and starts:

```sh
parlando-space-game --host 0.0.0.0 --config /app/config/experiment.yaml
```

The included Render config stores SQLite at:

```yaml
database:
  url: sqlite:////data/parlando.sqlite
```

The production image serves the browser app from `/app/client-dist`. If you deploy the frontend separately, remove or adjust `server.client_dist_path`.

## Reference Files

- `space-game/config/experiment.render.example.yaml`: deployment config template.
- `space-game/config/experiment.render.secret.example.yaml`: example contents for the Render secret file overlay.
- `space-game/render.yaml`: Render service template.
- `space-game/Dockerfile`: production image for the demo server.
- `space-game/server/src/main.rs`: server entry point.
