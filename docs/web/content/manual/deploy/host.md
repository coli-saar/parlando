---
title: Operate a shared host
---

# Operate a shared host

Install the CLI on a machine that can stay running. A single host can serve many games and experiments. It needs a persistent directory, a public participant address, and a separate administrator hostname for the browser dashboard.

```sh
cargo install --locked parlando --bin parlando
```

The host loads compiled packages. Build tools belong on game authors' machines or in CI.

## Start and expose the service

For a local pilot, run `parlando serve` in the host directory and open the administrator link printed in the terminal. The same `/admin` page lets you create the first operator account in a new store and sign in to an existing store.

To test an explicitly configured pair of origins, set `PARLANDO_HOST_ADMIN_TOKEN` in the host process environment to a long random value. Start the service from a writable directory.

```sh
parlando serve --bind 127.0.0.1:8000 --store ./parlando-host \
  --public-url http://127.0.0.1:8000 --admin-url http://localhost:8000
```

The two names reach the same listener. They count as different browser origins, keeping uploaded participant JavaScript away from the dashboard's authority.

For a public host, use HTTPS on both hostnames, route them to the service, and preserve the requested Host header and WebSocket upgrades. Set `--public-url` to the participant origin and `--admin-url` to the administrator origin. Use a single host process for the persistent store. Check `/health` on the running service, then open `/admin` on the administrator origin.

## Create the first operator on a configured host

In the operator shell, set `PARLANDO_ADMIN_URL` to the configured administrator origin and provide the bootstrap token in `PARLANDO_HOST_ADMIN_TOKEN`. Run this from an interactive terminal with Python 3 for the password prompt. It creates a named operator called `operator`.

```sh
python3 - <<'PY'
import getpass
import json
import os
import urllib.request

class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *args, **kwargs):
        return None

origin = os.environ["PARLANDO_ADMIN_URL"].rstrip("/")
request = urllib.request.Request(
    origin + "/api/hosting/accounts",
    headers={
        "Authorization": "Bearer " + os.environ["PARLANDO_HOST_ADMIN_TOKEN"],
        "Content-Type": "application/json",
    },
    data=json.dumps({
        "username": "operator",
        "password": getpass.getpass("Operator password: "),
        "role": "operator",
    }).encode(),
    method="POST",
)
with urllib.request.build_opener(NoRedirect).open(request) as response:
    print("Created", json.load(response)["username"])
PY
```

Use a password of at least 12 bytes. Sign in at `/admin` and create an experimenter account for each colleague. Experimenters share access to every game and experiment. Operators can also create accounts, reset passwords, disable accounts, and review audit events. A reset or disable revokes that account's current dashboard sessions.

## Grant machine uploads

The dashboard supports browser uploads with an experimenter account. To enable CLI or automated uploads, pass `--grants GRANTS_FILE` to `parlando serve`. Use a protected JSON file mapping game IDs to distinct random tokens. Keep those tokens separate from the host administrator token. Provision one token for each game a client needs to upload. Restart the host after changing the file.

`parlando serve` can also run without the browser dashboard. Specify the public origin with `--public-url` and omit the administrator origin. Supply the grants file or set `PARLANDO_DEPLOY_GAME` and `PARLANDO_DEPLOY_TOKEN` for a single game. Use the operator token to select packages and change intake through the CLI. See [deploy to a shared host](../).

## Retain the store

Back up the entire store while the service is stopped, including package files, experiment pins, research databases, and the dashboard account and audit database. Store backups and provider credentials in access-controlled locations. Restarting restores the selected packages and closes intake. Verify participant entry and explicitly reopen the experiments you intend to run.

When upgrading a schema-16 installation to the server with `communication.mode`, plan the change offline. Map the old `voice.enabled` value in each experiment and its configuration revisions to `speech` for true or `typed` for false. Preserve recorded events, including typed messages from older voice-enabled sessions.

On a shared host, each database filename contains the hash of its immutable selection. Converting a selection changes that hash. Keep its converted database paired with the new filename before restarting, and verify the experiment's session counts against the backup. A selection whose database is missing can open an empty store. Rebuild the participant assets against the matching client library and use a fresh game revision and experiment pin when replacing a deployed browser build.

Use a shared participant origin for game publishers you trust within the same browser origin. Run a different host on a different origin when publishers need independent browser trust boundaries. Use a separate participant link for each updated study run and retain earlier packages with the original data.
