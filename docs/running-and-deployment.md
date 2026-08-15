# Running And Deployment

This page covers local development, experiment configuration, frontend serving, Docker, Render, and hosted remote agents.

The same server shape applies to direct links, matched participant sessions, and human-vs-agent studies: create participant sessions, route them into the waiting-room flow, and persist the same evaluation records. Recruitment-provider identity must enter through a server-controlled integration; the public participant endpoint accepts direct participants only.

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
- `GET /admin` or `GET /admin/login`
- `GET /admin/experiments`
- `GET /admin/privacy`
- `GET /api/admin/export`

Each server process owns exactly one configured experiment. Every process start resets
that experiment to **inactive**, including restarts against an existing database. After
signing in, open `/admin/experiments` and select **Activate experiment** before admitting
participants. **Deactivate intake** stops new participant creation and room entry without
disconnecting sessions that are already running.

### First administrator setup

Administrator routes require Parlando's built-in login even when a reverse proxy also authenticates users. Start the server, then open `http://localhost:8000/admin`. If the configured database has no administrator credential, the page asks you to choose a username and password. The first successful submission creates the administrator and signs it in; later visits show the normal login form.

Parlando never stores the cleartext password. The server hashes it with Argon2id and stores the username, password hash, role, and creation time in the SQLite `administrator_credential` table. This row persists across server restarts and binary reinstalls. Each database has its own administrator: the normal Space Game and solo-voice configurations use separate SQLite files under `space-game/.local/`, so each one requires setup once.

First-visitor setup is intentionally open until that database has an administrator. Complete setup before exposing a new deployment to participants or the public internet. An atomic singleton insert ensures that only the first successful setup request is accepted.

For credential recovery or automated deployment, environment variables can override the database credential without changing it:

```sh
export PARLANDO_ADMIN_USERNAME="admin"
export PARLANDO_ADMIN_PASSWORD_HASH='<Argon2id PHC hash>'
export PARLANDO_ADMIN_ROLE="administrator" # or "operator"
```

Remove the override and restart to return to the database credential. If the password is lost and no override is available, restore or deliberately replace the SQLite database; reinstalling only the Parlando binary does not reset the administrator.

## Configuration

Experiment configuration is YAML loaded by `ExperimentConfig::from_yaml`. It supports includes, environment-variable substitution, and relative path resolution.

Keep one checked-in base config for public, repeatable settings. Put deployment-specific secrets, service credentials, and private URLs in an ignored include file or a platform secret file.

The main sections are:

- `experiment`: durable experiment id.
- `study`: study name and waiting/reconnect timing.
- `server`: public base URL, CORS origins, optional `client_dist_path`.
- `database`: SQLite URL.
- `direct`: participant intake and consent settings.
- `agents`: human-vs-human or human-vs-agent mode and agent selection.
- `voice`: Parlando audio-relay format and buffering settings.
- `speechmatics`: server-side STT credentials and realtime options.
- `transcription`: provider-neutral transcription settings.
- `tts`: agent text-to-speech settings.
- `privacy`: Privacy Contract version plus the four persistence switches for full game state, typed messages, final transcripts, and minimized voice diagnostics.

For local development without paid audio services, keep `voice.enabled`, `transcription.enabled`, and `tts.enabled` false. For voice studies, keep Speechmatics and ElevenLabs credentials in a private YAML include rather than in checked-in config or frontend build variables.

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
5. Paste deployment-specific YAML into that secret file. Start from `space-game/config/experiment.render.secret.example.yaml`, set `experiment.id` and `server.public_base_url`, and add Speechmatics and ElevenLabs credentials only when their features are enabled.
6. Deploy, immediately open `/admin`, and create the first administrator before sharing the service URL. For automated provisioning, use the environment-variable override documented above instead.

The committed Render config at `space-game/config/experiment.render.example.yaml` includes `/etc/secrets/parlando-render.yaml` as an optional overlay. The secret file deep-merges over the checked-in base config, so it can override public deployment settings, enable `agents.mode`, or provide private service credentials without expanding the Render environment-variable list.

The browser client never receives Speechmatics or TTS secrets. Provider keys are removed from the configuration object before it is persisted, and export applies a second redaction boundary. The browser receives public capability metadata, one in-memory participant credential, and distinct one-use game/audio upgrade tickets.

Before reusing a database created by a version predating the secret-storage boundary, audit it with `cargo run --manifest-path rust-server/Cargo.toml --bin redact_experiment_secrets -- <sqlite-url>`. It is a dry run unless `--apply` is supplied. Back up the database first. Internal testing alone is not evidence that Speechmatics or ElevenLabs credentials were compromised; rotate only when the exposure review finds a credential crossed the controlled boundary, survives in a shared artifact, or policy requires rotation.

Audio rooms and their one-use credentials are process-local. A voice deployment must run one Parlando process or use sticky routing that keeps every request and WebSocket for a room on the same instance. Ordinary stateless load balancing is not sufficient. See [Audio Transport](audio-transport.md) for bandwidth, queue, privacy, and monitoring details.

The included `space-game/Dockerfile` builds `parlando-space-game` and starts:

```sh
parlando-space-game --host 0.0.0.0 --config /app/config/experiment.yaml
```

The included Render config stores SQLite at:

```yaml
database:
  url: sqlite:////data/parlando.sqlite
```

The production image runs as uid 10001, uses the repository `.dockerignore` to exclude private configs and local data, and serves the browser app from `/app/client-dist`. If you deploy the frontend separately, remove or adjust `server.client_dist_path`.

## Reference Files

- `space-game/config/experiment.render.example.yaml`: deployment config template.
- `space-game/config/experiment.render.secret.example.yaml`: example contents for the Render secret file overlay.
- `space-game/render.yaml`: Render service template.
- `space-game/Dockerfile`: production image for the demo server.
- `space-game/server/src/main.rs`: server entry point.
