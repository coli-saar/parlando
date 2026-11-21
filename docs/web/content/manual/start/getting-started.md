---
title: Install Parlando and play a game
---

# Install Parlando and play a game

You will create Beacon Relay, a cooperative task in which two players take turns adding one or two ticks of charge to a beacon. The game ends when they reach the target exactly. You will run both seats locally before uploading anything.

## Install the tools

Install Rust with [rustup](https://rustup.rs/) and install [Node.js](https://nodejs.org/) with npm. Use Node.js 22 or later for the starter's browser build. Run these commands in a terminal.

```sh
cargo install --locked parlando --bin parlando
rustup target add wasm32-unknown-unknown
cargo install --locked wasm-tools
parlando --version
```

If you already have a built game package, install only the `parlando` CLI.
Run `parlando serve --manifest path/to/game.yaml` with the package's manifest,
then follow [Play both seats](#play-both-seats). The extra tools compile the
Rust starter and its browser application. The [Python
starter](../build/create-game/#start-with-python) uses a Python build tool for mechanics.

## Build and start Beacon Relay

Use a directory where you can create a new `beacon` folder.

```sh
parlando new beacon --lang rust
cd beacon
parlando test
parlando build
parlando serve --manifest dist/parlando/game.yaml
```

`test` checks the game rules and a scenario that runs through the compiled component. `build` writes the deployable package to `dist/parlando/`. `serve` loads that package and listens on loopback by default. Leave the terminal running while you play.

## Play both seats

1. Open [the local dashboard](http://127.0.0.1:8000/admin) and create its administrator account.
2. Choose **New experiment**, enter an ID such as `beacon-pilot`, and create it. Keep the default direct-entry settings for this walkthrough.
3. Start **Local preview** and open its participant link in two separate browser profiles. Two ordinary tabs can share one participant identity.
4. Complete the participant entry screens in both profiles. When they pair, alternate **Add one tick** and **Add two ticks** as appropriate to reach the displayed target exactly.
5. Return to **Sessions** in the dashboard and inspect the completed play-through. Close intake when you finish.

Local preview records testing sessions. Research corpus exports exclude them. Use the session view to check this play-through. See [configure and run a study](../run/configuration/) when you are ready to collect research sessions.

Press Ctrl+C in the terminal to stop. Parlando keeps the local records in `parlando.sqlite` in the directory where you ran `serve`. Start the same command to reopen the dashboard. Reopen intake explicitly when you are ready.

## Find the next command

| Task | Command |
| --- | --- |
| Create a project | `parlando new NAME --lang rust` or `--lang python` |
| Test rules and compiled scenarios | `parlando test` |
| Produce a package | `parlando build` |
| Validate a built package | `parlando check --manifest dist/parlando/game.yaml` |
| Pilot one game locally | `parlando serve --manifest dist/parlando/game.yaml` |
| Upload a package | `parlando deploy --server ORIGIN --package dist/parlando` |
| Start a shared service | `parlando host` |

Use `parlando COMMAND --help` for options. Build commands read `game.yaml`
in the working directory unless you pass `--manifest`. Use `--json` for
structured results in scripts.
