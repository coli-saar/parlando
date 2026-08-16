# Running and Deployment

A Parlando process runs one compiled game and the experiments stored for that
game. Starting another experiment of the same game is a dashboard operation;
starting a different game requires another binary, database, and port.

Give each compiled game its own SQLite database. This keeps the catalogue,
configuration revisions, and recorded game version within one clearly identified
installation.

## Start the Space Game locally

Run the complete Rust test suite first:

```sh
cargo test --manifest-path rust-server/Cargo.toml
```

For the normal development path, build the SDK, browser, and server and then start
the example game:

```sh
make -C space-game run
```

To run an already built client and server directly from the repository root, use:

```sh
cargo run --manifest-path space-game/server/Cargo.toml -- \
  --host 127.0.0.1 \
  --port 8000 \
  --client-dist space-game/client/dist
```

The port is required. This makes simultaneous local game processes explicit:
for example, one game can use port 8000 and another port 8001.

Open `http://127.0.0.1:8000/admin`. On a new database, create the first
administrator and then create an experiment with its permanent experiment ID.
Starting a game process never creates an experiment. Activate the experiment only when participant intake should begin. Its participant
page is `http://127.0.0.1:8000/e/{experiment_id}/`.

![Space Game experiment dashboard after first-time administrator
setup](images/parlando-dashboard.jpg)

Useful routes are:

- `/health`: process health;
- `/admin/experiments`: experiment configuration and monitoring;
- `/admin/privacy`: configuration-derived privacy status;
- `/e/{experiment_id}/`: participant entry point; and
- `/api/admin/runtime/{experiment_id}/export`: authenticated experiment export.

Every process start resets every experiment to `inactive`, including restarts
against an existing database. This provides a deliberate checkpoint before
recruitment reopens. Deactivation closes new participant and room creation while
allowing sessions already in progress to finish.

Each experiment also has explicit room-lifecycle bounds. Waiting rooms expire
after `session.waiting_room_timeout_seconds`; disconnected rooms after
`session.reconnect_grace_seconds`; active but idle sessions after
`session.session_idle_timeout_seconds`; and every unfinished session after
`session.session_max_lifetime_seconds`. Expiration closes live transports and stores
a `session_expired` event with a stable reason.

The bundled client sends an in-memory game heartbeat once per second. The server
closes a transport after 90 seconds without any game-channel message; the client
then obtains a fresh one-use ticket and retries during the configured reconnect
grace. Heartbeats neither write SQLite nor extend the meaningful idle deadline.

The authenticated dashboard's Load page visualizes the corresponding per-role
health bands, the earliest session lifecycle deadline, current capacity, recent
throughput, audio/ASR pressure, and SQLite plus WAL growth. Its five-second
samples are held in memory for one hour and reset on restart; use host-level
monitoring as well when alerting must survive a process failure.

## Choose bootstrap settings

Only settings needed before the dashboard opens belong to process bootstrap:

| Setting | Purpose | Normal source |
| --- | --- | --- |
| `--host` | Listener interface | Command line |
| `--port` or `PORT` | Listener port | Command line locally; platform environment when required |
| `--database-url` or `PARLANDO_DATABASE_URL` | SQLite experiment catalogue and session data | Command line or server environment |
| `--client-dist` or `PARLANDO_CLIENT_DIST` | Compiled browser client | Command line or server environment |
| `SPEECHMATICS_API_KEY` | Hosted transcription credential | Secret server environment |
| `ELEVENLABS_API_KEY` | Text-to-speech credential | Secret server environment |

The default Space Game database is `sqlite:///./parlando-space-game.sqlite`, and
the default client path is `./client/dist`. Override both when the current working
directory is not the game directory or when data must live on a persistent disk.

There is no external-URL setting. Participant, API, and WebSocket paths are
relative to the current process origin and include the experiment id. If a reverse
proxy changes the public host, preserve its normal `Host` header behavior.

## Configure experiments

Use `/admin/experiments` for normal configuration. The dashboard separates:

- **game settings**, currently the institution shared by all experiments;
- **catalogue metadata**, including pinning and obsolete status; and
- **experiment configuration**, including session lifecycle, consent, agents, capacity,
  voice, transcription, TTS, conversation, and privacy settings.

Capacity is admitted per research session rather than independently per endpoint.
Configure active and waiting sessions, unattached participant credentials, and
reserved transcription streams in the dashboard. Agent TTS is trusted experiment
behavior and has no application quota. File-backed SQLite pauses only new
sessions at the dashboard-configured disk reserve (256 MiB by default); already
admitted sessions may continue.

An experiment must be inactive before its effective configuration can be edited.
Each successful save creates an immutable numbered revision and uses optimistic
concurrency: if another administrator has already saved a newer revision, the
stale save is rejected.

An experiment also stores the exact semantic game version under which it was
created. A version-mismatched experiment remains available for inspection and
export, but it cannot be edited or activated. Clone it to obtain a new inactive
experiment for the currently compiled version.

YAML is not a process bootstrap API. Migrate an older configuration by creating
or cloning an inactive experiment in the dashboard and transferring its values,
using the `study` to `session` transformation in
[`migrating-to-clean-api.md`](migrating-to-clean-api.md). Do not add a `--config`
argument to a game binary. Provider credentials come from process secrets or the
authenticated game-settings surface and are redacted from configuration and
exports.

## Establish administrator access

Each database has one persistent administrator credential. On the first visit to
`/admin`, Parlando asks for a username and a password of at least 12 characters.
The server stores an Argon2id password hash, not the cleartext password.

First-visitor setup is intentionally open until the singleton administrator row
exists. Complete it before exposing a new service to participants or the public
internet. Concurrent setup attempts are resolved by an atomic database insert, so
only the first successful request creates the account.

Administrator credentials and roles are database-backed and are not configured
through environment variables. If the database password is lost, recover the
database from backup or deliberately replace its credential row while the service
is stopped; reinstalling the binary does not reset authentication.

After signing in, configure **Administrator IP ranges** under Game settings in
the dashboard. Enter one IPv4 or IPv6 CIDR per line. The setting is stored in
SQLite and shared by every experiment in the game process; it is not an
environment variable. An empty list permits every direct network peer. Parlando
does not trust forwarding headers for this decision, so a deployment behind a
reverse proxy that hides client addresses must enforce the equivalent
institutional or VPN range at that trusted proxy.

## Serve the participant client

Pass a compiled browser directory with `--client-dist` or
`PARLANDO_CLIENT_DIST`. The server serves the same compiled client beneath each
experiment root while preserving experiment-scoped API and WebSocket routes.

The bundled client uses relative assets, APIs, and WebSocket paths. A separately
hosted client must either proxy the complete `/e/{experiment_id}/` route tree to
Parlando or use an explicitly allowed browser origin. Do not construct participant
WebSocket URLs from a configured public hostname.

## Deploy with Docker and Render

The example production image in `space-game/Dockerfile` builds the Rust binary and
browser client, runs as uid 10001, and reads these defaults:

```text
PORT=8000
PARLANDO_DATABASE_URL=sqlite:////data/parlando.sqlite
PARLANDO_CLIENT_DIST=/app/client-dist
```

For Render:

1. Create a Docker web service using `space-game/Dockerfile` or
   `space-game/render.yaml`.
2. Mount a persistent disk at `/data`.
3. Set `PARLANDO_DATABASE_URL=sqlite:////data/parlando.sqlite`.
4. Let the platform supply `PORT`, or retain `8000` where fixed ports are
   supported.
5. Add `SPEECHMATICS_API_KEY` and `ELEVENLABS_API_KEY` only when the corresponding
   experiment features are enabled.
6. Deploy, complete administrator setup, configure an experiment, and activate it
   before distributing its participant URL.
7. Configure the administrator network range in the dashboard or at the trusted
   ingress proxy.
8. Set a disk-usage alert and arrange an encrypted off-service SQLite backup.
9. Deploy a reviewed clean Git commit and record the resulting image digest with
   the study materials.

Provider secrets remain in the process environment. The browser receives only
public capability metadata, its participant credential, and distinct one-use
game and audio tickets.

The Render blueprint checks `/health`, which acquires a SQLite write transaction
and performs a read before reporting success. A live-process response therefore
does not hide a missing, locked, or read-only experiment database.

Parlando emits HSTS on every application response. Browsers honor it only when
the response arrives over HTTPS, so local HTTP development is unaffected while a
TLS-terminating production proxy can pass the policy through unchanged.

## Back up collected data

The attached SQLite disk is the live copy, not a backup. Before intake, create an
off-service backup using SQLite's online backup mechanism or a provider snapshot,
then restore it into a separate test installation and verify that the dashboard,
session list, and an export open successfully. Repeat backups during collection
at an interval appropriate to the recruitment rate. Keep backup access at least
as restricted as administrator export access, and include backup deletion in the
study retention plan.

## Operate voice deployments

Audio rooms and one-use tickets are process-local. A voice deployment must use one
Parlando replica or sticky routing that sends every HTTP request and WebSocket for
a room to the same replica. Ordinary stateless load balancing can split room state
and break the session.

Before enabling voice, read [Audio Transport](audio-transport.md) and run the
credential-free checks in [Audio Testing](audio-testing.md). Parlando does not
persist raw microphone audio. When hosted Speechmatics transcription is enabled,
it streams that audio to Speechmatics for recognition; the installation privacy
page makes this active boundary visible.

## Migrate databases that may contain secrets

Older Parlando versions could persist provider fields in experiment configuration.
Audit such a database before reuse:

```sh
cargo run --manifest-path rust-server/Cargo.toml --bin redact_experiment_secrets -- \
  <sqlite-url>
```

The command is a dry run unless `--apply` is supplied. Back up the database first.
Rotate a credential when the audit shows that it crossed the intended boundary,
survives in a shared artifact, or institutional policy requires rotation; internal
testing alone does not establish compromise.

## Reference files

- `space-game/Dockerfile`: production image.
- `space-game/render.yaml`: Render service definition.
- `space-game/server/src/main.rs`: game descriptor and bootstrap implementation.
- `space-game/config/*.yaml`: legacy migration examples, not ongoing
  configuration.
