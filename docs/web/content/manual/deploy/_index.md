---
title: Deploy to a shared host
---

# Deploy to a shared host

A shared Parlando host serves one current WASM package per game. Activate a game to make participant access available. Use its game dashboard to create experiments and start or stop their individual runs. You can deploy and activate games without restarting the host.

## Deploy from the dashboard

Ask your host administrator for the administrator site URL and an individual account. Build your game locally and sign in. Experimenter accounts can access the games and experiments on this host.

1. In **Games**, expand **Deploy or update a game**, select `dist/beacon-0.1.0.zip`, and choose **Deploy game**. Parlando reads the game ID from the archive. The IDs `admin`, `api`, and `health` are reserved for host routes.
2. Choose **Activate** in the `beacon` row. Open the **Game ID** link to go to the Beacon Relay dashboard at `/beacon` on the administrator origin.
3. Choose **New experiment**, enter an ID such as `beacon-pilot`, and confirm with **Create experiment**. Configure the [study condition](../run/configuration/) in that dashboard.
4. Choose **Start experiment** and start **Local preview** to pilot. Open the participant URL in two independent browser profiles. Inspect the resulting sessions, then choose **Stop**. Use **Official intake** when the study is ready for participants.

## Update a deployed game

Stop testing, pause open intake, and let existing sessions finish. Use **Update** in the game's row to upload a ZIP with the same game ID. Parlando replaces the package and keeps the game's research database. It rejects replacement or deactivation while an experiment is running or has sessions still in progress. **Deactivate** disables participant access for the entire game.

After updating a game, select the existing experiment and start a local preview to check its configuration with the new build. Compatible settings can be used immediately with the same experiment ID and participant URL. If the updated game rejects a setting, edit the configuration while the experiment is inactive and save the corrected values. Each session records the game version it ran.

Keep the exact ZIP and its revision alongside the study records when you need to reproduce earlier results. The host serves the current package after an update.

## Define host aliases

Create `~/.parlando/servers.yaml`. Give each server a name and set `default` to the alias you usually use.

```yaml
local: http://127.0.0.1:8000
lab: https://parlando.example.org
default: local
```

With this configuration, `parlando login` and `parlando deploy` target the local server. To deploy to the lab, run `parlando login --server lab` once and use `parlando deploy --host lab`. An explicit URL or alias overrides the default for that command. The same default applies to `activate` and `deactivate`.

## Deploy with the CLI

Sign in once per server with your dashboard account. For a local server, run:

```sh
parlando login --server http://127.0.0.1:8000
```

The CLI opens a link for this login request. Sign in to the dashboard and choose **Authorize CLI**. The link expires after five minutes. The terminal saves a persistent credential in macOS Keychain or Linux Secret Service. Linux requires `secret-tool` and an unlocked Secret Service. Use `--no-browser` to open the printed link yourself.

Both dashboard roles can deploy any game on the server, including its first version. Run these commands from a game project, using the same server URL for login and deployment.

```sh
parlando build
parlando deploy --host http://127.0.0.1:8000 --package dist/beacon-0.1.0.zip
```

Record the package revision printed by the command. For scripts, add `--json` and read `result.revision`. Changing the account password or disabling the account revokes its saved CLI credentials. Activation and deactivation require the host administrator token. Set `PARLANDO_HOST_ADMIN_TOKEN` and set `PARLANDO_HOST_ADMIN_ORIGIN` to the service origin before using the CLI for those operations.

```sh
parlando activate --host http://127.0.0.1:8000 --game beacon
```

To withdraw the game after its experiments and sessions finish, run `parlando deactivate --host http://127.0.0.1:8000 --game beacon`. Use the game dashboard for experiment creation and study intake. Keep all credentials out of game files and prompts.

## Upload through REST

Use `parlando deploy` for terminal and CI uploads. A custom client can send
`PUT /api/hosting/packages/{game_id}` with an account deployment credential in the bearer
authorization header. Set `Content-Type: application/zip` and send the ZIP bytes as the request body.
Place `game.yaml` at the archive root and include the component and declared
browser assets at their manifest paths. The host limits both the upload and the
combined uncompressed entries to 64 MiB.
The response includes the package `revision`. The same file names and bytes yield the same revision.

For installation, accounts, networking, and backups, see [operate a shared host](host/).
