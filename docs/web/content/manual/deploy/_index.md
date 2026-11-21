---
title: Deploy to a shared host
---

# Deploy to a shared host

A shared Parlando host runs experiments from several WASM game packages. Upload a package to make its revision available, create an experiment to bind a revision to study settings, and open intake when the condition is ready. You can repeat those steps without restarting the service.

## Deploy from the dashboard

Ask your host operator for the administrator site URL and your own credentials. Build your game locally and sign in to the dashboard. All experimenter accounts on the host can access its games and experiments.

1. In **Games and builds**, choose **Upload a game build** and select the `dist/parlando/` directory. Use the game ID and paths from its built `game.yaml`.
2. Choose **Create experiment** beside the uploaded revision. Enter a host-wide experiment ID such as `beacon-pilot`. Use `{}` for the initial direct-entry condition or supply the [study's settings](../run/configuration/).
3. Choose **Start testing** and play with two separate browser profiles. Inspect **Sessions**, then choose **Stop testing**. Choose **Open intake** when you are ready to admit research participants.

Select a game name to open its dashboard. Uploading a revision leaves existing experiments and participant links attached to their original package. To run a changed build, create a new experiment ID. You can reuse an earlier revision in a new experiment to return to a known build.

## Deploy with the CLI

Ask for a game-scoped upload token and the service URL. On macOS and Linux, read the token from your secret manager and pipe it to `parlando login --server "$PARLANDO_SERVER" --game beacon`. The CLI stores it in Keychain or Linux Secret Service. Linux desktop installs require `secret-tool`. In CI, inject `PARLANDO_DEPLOY_TOKEN` and set `PARLANDO_DEPLOY_ORIGIN` to the service origin.

Set `PARLANDO_SERVER` to your host's HTTPS origin and run these commands from the project.

```sh
parlando build
parlando deploy --server "$PARLANDO_SERVER" --package dist/parlando --json
```

Copy `result.revision` from the response. Select it in the dashboard, or use the operator CLI if you operate the host. The scoped upload token allows package upload for its game. The operator CLI uses `PARLANDO_HOST_ADMIN_TOKEN` and checks `PARLANDO_HOST_ADMIN_ORIGIN` against the receiving service. Set `REVISION` to the returned digest before running these commands.

```sh
parlando select --server "$PARLANDO_SERVER" --experiment beacon-pilot \
  --game beacon --revision "$REVISION"
parlando intake --server "$PARLANDO_SERVER" --experiment beacon-pilot testing
```

Pass `--config experiment.yaml` to `select` for a custom condition. The upload token can be supplied to an automation tool through its execution environment. Keep all credentials out of game files and prompts.

## Upload through REST

Use `parlando deploy` for terminal and CI uploads. A custom client can send
`PUT /api/hosting/packages/{game_id}` with the game-scoped token in the bearer
authorization header. Send JSON with one `files` object mapping each relative
package path to its standard base64-encoded bytes. Include `game.yaml`, the
component, and any declared browser assets. The request body limit is 64 MiB.
The response includes the package `revision`, which the experiment selects.
Retrying with the same file names and bytes returns the same revision.

For installation, accounts, networking, and backups, see [operate a shared host](host/).
