---
title: Deploy with Docker on Render
---

# Deploy with Docker on Render

Render can host one Parlando game as one Docker web service. The image contains the Rust server and
compiled participant application; a persistent disk contains SQLite. Render terminates HTTPS,
supplies the public port, and checks `/health`.

This division follows the general deployment model: replaceable code and assets belong in the image,
while administrator state, experiment revisions, and research records belong on durable storage.

## Build one same-origin image

Start from the checked-in Great Tree Dockerfile and adapt the game-specific paths. Keep the
repository root as `dockerContext`, even though the Dockerfile lives in a game directory, so the
build can see both the game and its Parlando dependencies. Keep the client lockfile in the build
context.

## Declare the service and its state

A research deployment needs a plan that supports a persistent disk:

```yaml
services:
  - type: web
    name: parlando-my-game
    runtime: docker
    plan: starter
    dockerfilePath: ./games/my-game/Dockerfile
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

The mount and database URL must refer to the same directory. Four slashes after `sqlite:` denote the
absolute path `/data/parlando.sqlite`. Changing only the mount or only the URL can leave the live
database on ephemeral storage.

Use one service instance. The Blueprint does not provide shared room state or sticky routing. A
persistent disk may also prevent overlapping replacement instances because only one instance can
mount it; schedule deployments outside active sessions.

## Distinguish a demo from a study

Render's free plan does not support a persistent disk. A free service may be useful for a disposable
demonstration if SQLite is written to the container user's home directory, but every replacement or
spin-down can remove the administrator, experiments, and sessions. Do not collect research data
under that assumption.

The checked-in Great Tree Blueprint is intentionally such a free demonstration. It illustrates the
build and health-check path, not a durable research configuration. For a study, change to a
disk-capable plan and use the persistent configuration above.

## Deploy and establish control

The included game provides concrete Docker and Blueprint files:

- [`games/great-tree/Dockerfile`](https://github.com/coli-saar/parlando/blob/main/games/great-tree/Dockerfile)
- [`games/great-tree/render.yaml`](https://github.com/coli-saar/parlando/blob/main/games/great-tree/render.yaml)

To exercise the example, create a Render Blueprint for the repository and set its path to
`games/great-tree/render.yaml`. Wait for the build and `/health`, then immediately open
`https://<service-name>.onrender.com/admin` and create the administrator.

For a real study, adapt the service name, Dockerfile path, binary, and game client; retain the
repository-root context, one replica, persistent absolute SQLite path, and health check. Create
experiments through `/admin` rather than adding an experiment YAML or `--config` argument to the
container.

Provider credentials belong in authenticated **Game settings** after administrator setup. They are
not Docker build arguments and are excluded from experiment exports.

## Interpret health correctly

Success from `/health` shows that the server can use the configured database. It does not show that
participant intake is open, that remote providers work, or that two participants can complete a
session through the public address.

Every process start returns experiments to inactive. After deployment, inspect the database and
configuration, then reactivate only the intended condition.

Before recruitment, run the deployment checklist from [Design a deployment](../), configure a
disk alert, and restore an encrypted off-service backup elsewhere. Record the image digest and game
version with the study.

## Diagnose the boundary that failed

- **Shared packages are missing during build:** the Docker context is probably the game directory
  rather than the repository root.
- **`npm ci` prints usage and stops:** the game client's lockfile is absent from the context.
- **The health check fails:** inspect the absolute database path, disk permissions, and listener on
  Render's `PORT`.
- **The administrator or experiments disappear:** SQLite was written to ephemeral storage or the
  URL does not point at the mounted disk.
- **Render rejects the disk:** the selected plan does not support persistent disks.
- **The experiment is inactive after deployment:** this is the intended restart behavior.
- **Reconnect or voice fails after scaling:** return to one instance; multiple replicas are not
  supported for a live study.

Render documents its current fields in the
[Blueprint specification](https://render.com/docs/blueprint-spec),
[Docker guide](https://render.com/docs/docker), and
[persistent-disk guide](https://render.com/docs/disks).
