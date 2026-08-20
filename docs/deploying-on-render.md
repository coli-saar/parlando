# Deploy a Parlando Game on Render

A Parlando game can run on Render as one Docker web service with one persistent
disk. The web service runs the game server and serves the compiled participant
client from the same origin. The disk stores the SQLite catalogue, experiment
configuration, administrator credential, and session data across deploys.

The Space Game provides a complete example:

- [`space-game/Dockerfile`](../space-game/Dockerfile) builds the Rust server and
  browser client into one production image; and
- [`space-game/render.yaml`](../space-game/render.yaml) defines the Render web
  service, health check, persistent disk, and non-secret environment variables.

Use these files together. The Dockerfile describes what runs, whereas the
Blueprint describes the Render resources that keep it available and persistent.

## Before you deploy

You need a Git repository that Render can access and a game that builds from the
repository root. The Space Game Docker build uses files from `rust-server`,
`js-client`, and `space-game`, so its Docker context must be the repository root.

Use one Render instance. Parlando currently owns live rooms, credentials, and
audio tickets in process memory. Multiple instances require a shared state layer
or routing that keeps every HTTP request and WebSocket for a room on the same
instance; the example Blueprint does not provide either mechanism.

SQLite also requires persistent storage. A deployment without the disk may
appear to work, but it loses its administrator account, experiments, and
collected data when Render replaces the instance.

The client build stage runs `npm ci`, which fails without a committed
`package-lock.json`. This repository's root `.gitignore` blanket-ignores
`**/package-lock.json` and un-ignores it per game (see the
`!space-game/client/package-lock.json` line). A new game's client needs the
same exception added and its lockfile committed, or the Docker build fails
during `npm ci` with a plain usage dump rather than a message about a missing
lockfile.

Render's free plan does not support attached disks; a Blueprint with a `disk:`
block on `plan: free` is rejected outright at review time. Decide up front
whether this deployment needs to survive restarts — if not, see
[Deploy without a persistent disk](#deploy-without-a-persistent-disk) below
instead of the Blueprint steps that follow.

## Deploy the Space Game Blueprint

The shortest complete path is to deploy the checked-in Blueprint.

1. Fork or push this repository to a Git provider connected to Render.
2. In Render, create a **Blueprint** and select the repository.
3. Set the Blueprint path to `space-game/render.yaml`. Render otherwise looks
   for `render.yaml` at the repository root.
4. Review the proposed `parlando-space-game` web service and its 10 GB disk,
   then apply the Blueprint.
5. Wait until the Docker build completes and the `/health` check passes.
6. Open `https://<service-name>.onrender.com/admin` immediately. Create the
   first administrator with a password of at least 12 characters.
7. Create an experiment, configure it while it is inactive, and activate it.
8. Open the participant URL shown by the dashboard. Its form is
   `https://<service-name>.onrender.com/e/<experiment_id>/`.

The first administrator setup is open until one administrator exists. Complete
it before sharing the service URL. Parlando stores the password hash in SQLite,
so the attached disk preserves the account across restarts and deploys.

## Understand the example configuration

The relevant Blueprint fields are:

```yaml
services:
  - type: web
    name: parlando-space-game
    runtime: docker
    plan: starter
    dockerfilePath: ./space-game/Dockerfile
    dockerContext: .
    healthCheckPath: /health
    envVars:
      - key: PARLANDO_DATABASE_URL
        value: sqlite:////data/parlando.sqlite
      - key: PARLANDO_CLIENT_DIST
        value: /app/client-dist
    disk:
      name: parlando-data
      mountPath: /data
      sizeGB: 10
```

`dockerContext: .` is necessary because the Dockerfile copies the shared
Parlando crates and packages as well as the Space Game. `mountPath: /data` and
the database URL refer to the same directory; four slashes after `sqlite:`
denote the absolute path `/data/parlando.sqlite`. The client path names the
compiled browser assets copied into the runtime image.

Render supplies `PORT`; the Docker image defaults to port 8000 when the variable
is absent. The server binds to `0.0.0.0`, which makes it reachable through
Render's proxy. No public-hostname setting is required because participant HTTP
and WebSocket routes are relative to the service origin.

The `/health` endpoint checks more than process liveness: it opens a SQLite write
transaction and performs a read. A successful Render health check therefore
also establishes that the mounted database is present and writable.

## Adapt the example to another game

Copy the two Space Game files into the new game's directory, then change only
the game-specific build inputs:

1. In the Dockerfile, replace `space-game/server`, `space-game/client`, and the
   final binary name with the corresponding paths and binary for the new game.
2. In the Blueprint, change the service name and `dockerfilePath`.
3. Keep the repository-root Docker context if the game consumes this monorepo's
   shared Rust crate or JavaScript client.
4. Keep the `/data` disk and absolute SQLite URL unless you deliberately choose
   another persistent mount.
5. Keep one service instance unless the deployment supplies sticky routing and
   shared authentication and room state.

Do not add an experiment YAML file or a `--config` argument. Process bootstrap
contains only listener, database, client-path, logging, and provider-secret
settings. Create and revise experiments through `/admin`; each successful edit
becomes an immutable configuration revision in SQLite.

## Deploy without a persistent disk

Render's free plan cannot mount a disk. Use this path only for a demo or
testing deployment that does not need to retain its administrator account,
experiments, or collected data across restarts — every restart and every
inactivity spin-down (free instances sleep after idle periods) wipes the
database.

1. In the Blueprint, set `plan: free` and remove the `disk:` block entirely.
2. Point `PARLANDO_DATABASE_URL` at a path baked into the image instead of
   `/data`, for example the container user's home directory:
   `sqlite:////home/parlando/parlando.sqlite`. The Dockerfile's
   `useradd --create-home` step already makes that directory writable by the
   non-root user the container runs as.
3. Redo the `/admin` first-administrator setup and recreate/reactivate the
   experiment after every restart or spin-down; nothing here persists it for
   you.

Everything else in this guide — the Blueprint fields, adapting to another
game, voice secrets — applies the same way; only the disk and database path
change.

## Add voice-provider secrets only when needed

Typed games need no provider credentials. If an experiment uses hosted speech
recognition or text-to-speech, add the corresponding secret in the Render
dashboard:

```text
SPEECHMATICS_API_KEY=<secret>
ELEVENLABS_API_KEY=<secret>
```

Do not commit either value to the Blueprint. If you want the Blueprint creation
flow to prompt for them, declare each key with `sync: false`; omit them entirely
for deployments that do not use those providers. Provider secrets remain on the
server and are excluded from experiment exports.

## Operate the deployment

Every process start resets all experiments to inactive. After a deploy or
restart, sign in to `/admin`, confirm the intended configuration, and reactivate
participant intake. Existing in-memory sessions do not survive a process
replacement.

The persistent disk is the live database, not a backup. Before recruitment,
arrange an encrypted off-service SQLite backup and test restoring it into a
separate installation. Also configure disk-usage alerts: SQLite and its WAL file
share the mounted capacity.

A persistent disk prevents Render's ordinary zero-downtime replacement because
only one instance can mount the disk. Schedule deploys around participant
sessions and inspect the dashboard after each deploy.

For production, also:

- restrict administrator access through the dashboard's administrator CIDR
  setting or a trusted ingress control;
- record the deployed Git commit and image digest with the study materials;
- test the participant URL, typed messaging, and any enabled audio path; and
- read [Running and Deployment](running-and-deployment.md) for lifecycle,
  privacy, backup, and migration details.

## Diagnose common failures

**The Docker build cannot find `rust-server` or `js-client`.** The build context
is probably the game subdirectory. Set `dockerContext: .` and keep
`dockerfilePath` relative to the repository root.

**The health check never passes.** Confirm that the disk is mounted at `/data`,
that `PARLANDO_DATABASE_URL` is exactly
`sqlite:////data/parlando.sqlite`, and that the process log shows a listener on
Render's `PORT`.

**Experiments or the administrator disappear after a deploy.** The database was
written to ephemeral storage. Confirm both the disk mount and the absolute
database URL; changing only one of them is insufficient. If this is expected
because the deployment intentionally has no disk, see
[Deploy without a persistent disk](#deploy-without-a-persistent-disk).

**The Blueprint review rejects the `disk:` block.** The plan is `free`. Render
does not support disks on the free tier; either switch to `plan: starter` or
follow [Deploy without a persistent disk](#deploy-without-a-persistent-disk).

**The Docker build fails during `npm ci` for the new game's client, printing
npm's usage help instead of an error about the lockfile.** The game's
`package-lock.json` is not committed — check that the root `.gitignore` has a
`!<game>/client/package-lock.json` exception and that the file is tracked.

**A deployed experiment is inactive.** This is expected after every process
start. Reactivate it deliberately from the dashboard.

**Voice or reconnects fail after scaling.** Return to one instance. The example
does not implement the shared state and sticky routing required for horizontal
scaling.

Render's current field definitions and disk behavior are documented in its
[Blueprint specification](https://render.com/docs/blueprint-spec),
[Docker guide](https://render.com/docs/docker), and
[persistent-disk guide](https://render.com/docs/disks).
