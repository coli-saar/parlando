---
title: Install Parlando and play a game
---

# Install Parlando and play a game

You will create Beacon Relay, a cooperative task in which two players take turns adding one or two ticks of charge to a beacon. The game ends when they reach the target exactly. You will run both seats on a server that listens only on your computer.

## Install the tools

Install Rust with [rustup](https://rustup.rs/) and install [Node.js](https://nodejs.org/) with npm. Use Node.js 22 or later for the starter's browser build. Run these commands in a terminal.

```sh
cargo install --locked parlando --bin parlando
rustup target add wasm32-unknown-unknown
cargo install --locked wasm-tools
parlando --version
```

If you already have a built game package, install only the `parlando` CLI.
Skip to [Start a local server](#start-a-local-server) and use your ZIP file
and game ID in the commands. The extra tools compile the Rust starter and its
browser application. The [Python
starter](../build/create-game/#start-with-python) uses a Python build tool for mechanics.

## Build Beacon Relay

Use a directory where you can create a new `beacon` folder.

```sh
parlando new beacon --lang rust
cd beacon
parlando test
parlando build
```

`test` checks the game rules and a scenario that runs through the compiled component. `build` writes the deployable package to `dist/beacon-0.1.0.zip`. The filename uses the game ID and game version from `game.yaml`. The last line of the build output gives its absolute path.

## Start a local server

The local host includes an administrator dashboard. For a browser-only pilot, create a host directory, run `parlando serve` there, and Cmd-click the administrator link printed in the terminal. On your first visit to `/admin`, choose an operator username and password. Use the same URL for later visits. From the dashboard you can upload your ZIP, select a revision, and start testing.

The following walkthrough uses the CLI for those same operations. From the game project, create a sibling directory for the host’s data. In Bash or Zsh, generate one token for uploading Beacon and another for administering experiments. Start the server in the new directory.

```sh
mkdir -p ../parlando-host
export PARLANDO_DEPLOY_GAME=beacon
export PARLANDO_DEPLOY_TOKEN="$(openssl rand -hex 32)"
export PARLANDO_HOST_ADMIN_TOKEN="$(openssl rand -hex 32)"
export PARLANDO_DEPLOY_ORIGIN=http://127.0.0.1:8000
export PARLANDO_HOST_ADMIN_ORIGIN="$PARLANDO_DEPLOY_ORIGIN"
(cd ../parlando-host && exec parlando serve) &
```

The server uses its working directory as the store for uploaded revisions, experiment selections, and session databases. The parentheses keep your terminal in the game project, where the ZIP is available under `dist/`. To reopen the store later, start the server from the same host directory or specify its path with `--store`.

When the server is ready, it prints the administrator link. On later starts, use your operator account to sign in at `http://localhost:8000/admin`.

## Deploy the game and start a pilot

```sh
parlando deploy --host "$PARLANDO_DEPLOY_ORIGIN" --package dist/beacon-0.1.0.zip
```

Copy the digest printed after **Revision**. Paste it when the last line of
this block waits for input, then press Enter.

```sh
printf 'Revision: '
read -r REVISION
```

Select that exact revision for an experiment, then put the experiment into
testing mode. The default configuration pairs two humans with typed chat.

```sh
parlando select --host "$PARLANDO_HOST_ADMIN_ORIGIN" --experiment beacon-pilot \
  --game "$PARLANDO_DEPLOY_GAME" --revision "$REVISION"
parlando intake --host "$PARLANDO_HOST_ADMIN_ORIGIN" --experiment beacon-pilot testing
```

## Play both seats

1. Open the participant URL printed by `parlando select` in two separate browser profiles. Two ordinary tabs can share one participant identity.
2. Complete the participant entry screens in both profiles. When they pair, alternate **Add one tick** and **Add two ticks** as appropriate to reach the displayed target exactly.
3. Close intake when you finish.

```sh
parlando intake --host "$PARLANDO_HOST_ADMIN_ORIGIN" --experiment beacon-pilot inactive
fg
```

Press Ctrl+C to stop the server after bringing it to the foreground with
`fg`. Start it with the same store to reuse the package, experiment, and
session records. Reopen intake when you are ready to play again. Create a new
experiment ID to test a changed package revision.

Testing sessions are excluded from research corpus exports. To inspect them,
open the administrator dashboard and choose **Sessions**. See [configure and
run a study](../run/configuration/) when you are ready to collect research
sessions.

## Find the next command

| Task | Command |
| --- | --- |
| Create a project | `parlando new NAME --lang rust` or `--lang python` |
| Test rules and compiled scenarios | `parlando test` |
| Produce a package | `parlando build` |
| Validate a built package | `parlando check --package dist/beacon-0.1.0.zip` |
| Start a server | `parlando serve` |
| Upload a package | `parlando deploy --host ORIGIN --package dist/beacon-0.1.0.zip` |
| Pin a game revision to an experiment | `parlando select --host ORIGIN --experiment ID --game GAME --revision REVISION` |
| Open testing intake | `parlando intake --host ORIGIN --experiment ID testing` |

Use `parlando COMMAND --help` for options. Build commands read `game.yaml`
in the working directory unless you pass `--manifest`. Commands print readable
results by default. In scripts, add `--json` and read fields such as
`result.revision` from the response.
