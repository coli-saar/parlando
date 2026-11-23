---
title: Operate a shared host
---

# Operate a shared host

Install the CLI on a machine that can stay running. A single host can serve many games and experiments. It needs a persistent directory and a public address. Trusted games and the dashboard can share that address.

```sh
cargo install --locked parlando --bin parlando
```

The host loads compiled packages. Build tools belong on game authors' machines or in CI.

## Start and expose the service

For a local pilot, run `parlando serve` in the host directory and open the administrator link printed in the terminal. The same `/admin` page lets you create the first administrator account in a new store and sign in to an existing store.

To test an explicitly configured pair of origins, start the service from a writable directory.

```sh
parlando serve --bind 127.0.0.1:8000 --store ./parlando-host \
  --public-url http://127.0.0.1:8000 --admin-url http://localhost:8000
```

The two names reach the same listener. They count as different browser origins, keeping uploaded participant JavaScript away from the dashboard's authority.

For a public host, use HTTPS and preserve the requested Host header and WebSocket upgrades. To serve trusted games and the dashboard together, pass the same origin to `--public-url` and `--admin-url`. To isolate uploaded game JavaScript from the dashboard, assign the dashboard its own hostname. Use a single host process for the persistent store. Open `/admin` on the dashboard origin after startup.

## Create the first administrator on a configured host

Open `/admin` on the administrator hostname immediately after first startup. Choose your username and a password of at least 12 bytes. The first visitor to complete the form claims the host. Once an account exists, setup closes and stays closed across restarts using the same store.

Sign in at `/admin` and open **Users** to create an experimenter account for each colleague. Experimenters share access to every game and experiment.

Administrators use **Users** to create accounts, reset passwords, and disable accounts. A reset or disable revokes that account's current dashboard sessions. Expand **Audit history** on the same page to review recorded operations.

## Authorize CLI deployments

Enable the dashboard and create an account for each person who deploys games. Admins and experimenters can sign in from the CLI and upload any game to this host. Account password resets and account disabling also revoke that account’s CLI credentials. See [deploy with the CLI](../#deploy-with-the-cli) for the browser sign-in steps.

## Retain the store

Back up the entire store while the service is stopped. Include the current packages, game research databases, and the dashboard account and audit database. Store backups and provider credentials in access-controlled locations. Game activation persists across restarts. Reopen the experiments you intend to run after the host restarts.

When upgrading a schema-16 installation to the server with `communication.mode`, plan the change offline. Map the old `voice.enabled` value in each experiment and its configuration revisions to `speech` for true or `typed` for false. Preserve recorded events, including typed messages from older voice-enabled sessions. See the [0.7.0 migration guide](https://github.com/coli-saar/parlando/blob/main/docs/migrating-0.6.0-to-0.7.0.md).

Use a shared participant origin for game publishers you trust within the same browser origin. Run a different host on a different origin when publishers need independent browser trust boundaries. Keep earlier game ZIPs with the corresponding study data when you need to reproduce a run.
