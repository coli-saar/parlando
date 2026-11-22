---
title: Deploy to a shared host
---

# Deploy to a shared host

A shared Parlando host runs experiments from several WASM game packages. Upload a package to make its revision available, create an experiment to bind a revision to study settings, and open intake when the condition is ready. You can repeat those steps without restarting the service.

## Deploy from the dashboard

Ask your host operator for the administrator site URL and your own credentials. Build your game locally and sign in to the dashboard. All experimenter accounts on the host can access its games and experiments.

1. In **Games and builds**, choose **Upload a game build** and select `dist/beacon-0.1.0.zip`. Parlando reads the game ID from the manifest inside the archive.
2. Choose **Create experiment** beside the uploaded revision. Enter a host-wide experiment ID such as `beacon-pilot`. Use `{}` for the initial direct-entry condition or supply the [study's settings](../run/configuration/).
3. Choose **Start testing** and play with two separate browser profiles. Inspect **Sessions**, then choose **Stop testing**. Choose **Open intake** when you are ready to admit research participants.

Select a game name to open its dashboard. Uploading a revision leaves existing experiments and participant links attached to their original package. To run a changed build, create a new experiment ID. You can reuse an earlier revision in a new experiment to return to a known build.

## Deploy with the CLI

Ask for a game-scoped upload token and the service URL. On macOS and Linux, read the token from your secret manager and pipe it to `parlando login --server "$PARLANDO_SERVER" --game beacon`. The CLI stores it in Keychain or Linux Secret Service. Linux desktop installs require `secret-tool`. In CI, inject `PARLANDO_DEPLOY_TOKEN` and set `PARLANDO_DEPLOY_ORIGIN` to the service origin.

Set `PARLANDO_SERVER` to your host's HTTPS origin and run these commands from the project.

```sh
parlando build
parlando deploy --host "$PARLANDO_SERVER" --package dist/beacon-0.1.0.zip
```

Copy the printed revision. For scripts, add `--json` and read `result.revision`. Select it in the dashboard, or use the operator CLI if you operate the host. The scoped upload token allows package upload for its game. The operator CLI uses `PARLANDO_HOST_ADMIN_TOKEN` and checks `PARLANDO_HOST_ADMIN_ORIGIN` against the receiving service. Set `REVISION` to the returned digest before running these commands.

```sh
parlando select --host "$PARLANDO_SERVER" --experiment beacon-pilot \
  --game beacon --revision "$REVISION"
parlando intake --host "$PARLANDO_SERVER" --experiment beacon-pilot testing
```

Pass `--config experiment.yaml` to `select` for a custom condition. The upload token can be supplied to an automation tool through its execution environment. Keep all credentials out of game files and prompts.


## Upload a local agent

Build the [agent ZIP](../build/packages/#build-an-agent-zip) and upload it
after the game ZIP. Use the same game's upload credential.

```sh
parlando deploy --host "$PARLANDO_SERVER" \
  --package dist/agent-counterweight_hotel.porter-11.0.0.zip
```

Upload the agent before selecting a new experiment when you want that
experiment to use it. The experiment pins the compatible agent revisions
during selection.

## Upload through REST

Use `parlando deploy` for terminal and CI uploads. A custom client can send
`PUT /api/hosting/packages/{game_id}` with the game-scoped token in the bearer
authorization header. Set `Content-Type: application/zip` and send the ZIP bytes as the request body.
A game upload contains `game.yaml`, the game component, and its declared
browser assets. An agent upload contains `agent.yaml` and the policy component
for the same game ID. The host limits both the upload and the combined
uncompressed entries to 64 MiB. The response includes the package `revision`
and, for an agent upload, the agent's factory ID. Retrying with the same file
names and bytes returns the same revision.

For installation, accounts, networking, and backups, see [operate a shared host](host/).
