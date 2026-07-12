# Running And Deployment

This page covers local development, experiment configuration, frontend serving, Docker, Render, and hosted remote agents.

Parlando is also expected to include infrastructure for Prolific-driven experiments. The local and Render setup below still applies: Prolific entry points should create or recover participant sessions, route participants into the same room/matchmaking flow, and persist the same evaluation records.

## Local Development

Run Rust tests:

```sh
cargo test
```

Run the demo server with defaults:

```sh
cargo run -p parlando-space-game -- --host 127.0.0.1 --port 8000
```

Run with a config:

```sh
cargo run -p parlando-space-game -- \
  --host 127.0.0.1 \
  --port 8000 \
  --config config/experiment.render.example.yaml
```

Useful routes:

- `GET /health`
- `GET /api/config`
- `GET /admin/games`
- `GET /api/admin/export`

## Configuration

Experiment configuration is YAML loaded by `ExperimentConfig::from_yaml`. It supports includes, environment-variable substitution, and relative path resolution.

The main sections are:

- `experiment`: durable experiment id.
- `study`: study name and waiting/reconnect timing.
- `server`: public base URL, CORS origins, optional `client_dist_path`.
- `database`: SQLite URL.
- `direct`: direct room-code and matchmaking settings, including consent text.
- `agents`: human-vs-human or human-vs-agent mode and agent selection.
- `livekit`: realtime audio room settings.
- `speechmatics`: browser STT credentials and realtime options.
- `transcription`: transcription mode exposed to the browser.
- `tts`: agent text-to-speech settings.
- `conversation`: conversation-history settings.

For local development without paid audio services, keep `livekit.enabled`, `speechmatics.enabled`, `transcription.enabled`, and `tts.enabled` false. For voice studies, use environment variables for all secrets.

## Serving A Frontend

When a frontend build is available, set:

```yaml
server:
  client_dist_path: /path/to/client/dist
```

The server serves `/`, `/assets/*`, and SPA fallback paths from that directory while preserving `/api/*` and `/ws/*`.

The current Rust workspace does not contain the TypeScript frontend source. The server can serve a built client directory, but the provided Dockerfile does not build a frontend that is outside this Docker context.

## Render Deployment

The repository includes Docker and Render examples for the demo server.

1. Create a Render web service from the repository.
2. Use `render.yaml` or configure a Docker service manually.
3. Attach a persistent disk mounted at `/data`.
4. Set `PARLANDO_EXPERIMENT_ID` and `PARLANDO_PUBLIC_BASE_URL`.
5. Set LiveKit, Speechmatics, and ElevenLabs secrets if voice features are enabled.
6. Deploy.

The included `Dockerfile` builds `parlando-space-game` and starts:

```sh
parlando-space-game --host 0.0.0.0 --config /app/config/experiment.yaml
```

The included Render config stores SQLite at:

```yaml
database:
  url: sqlite:////data/parlando.sqlite
```

If you want the Rust service to serve the browser app, include a built client at `/app/client-dist` in the production image or adjust `server.client_dist_path` to the deployed path.

## Reference Files

- `config/experiment.render.example.yaml`: deployment config template.
- `render.yaml`: Render service template.
- `Dockerfile`: production image for the demo server.
- `crates/parlando-space-game/src/main.rs`: server entry point.
