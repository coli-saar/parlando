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

From the game project, create a sibling directory for the host's data. Start the server there.

```sh
mkdir -p ../parlando-host
(cd ../parlando-host && exec parlando serve)
```

The server stores packages, experiment data, and dashboard accounts in its working directory. On later starts, use the same directory or pass its path with `--store`.

Open the administrator URL printed in the terminal. On the default local listener it is `http://localhost:8000/`. Create your administrator account on the first visit and sign in.

## Deploy the game and start a pilot

1. In **Games**, expand **Deploy or update a game** and select the ZIP printed by your build. Choose **Deploy game**.
2. Click the `beacon` game ID to open the game dashboard at `/beacon` on the administrator origin.
3. In the experiment catalogue, choose **New experiment**, enter `beacon-pilot`, and select **Create experiment**. The default condition pairs two humans with typed chat.
4. In **Start experiment**, choose **Start** under **Local preview**. Copy the participant URL. On the default host it is `http://127.0.0.1:8000/e/beacon~beacon-pilot/`.

## Play both seats

1. Open the participant URL in two separate browser profiles. Two ordinary tabs can share one participant identity.
2. In the first profile, choose **Enter waiting room**. The countdown shows how long the seat will stay open. Your seat reads **Connected** and your partner’s reads **Waiting**.
3. Enter the waiting room in the second profile. The game starts automatically in both windows. Alternate **Add one tick** and **Add two ticks** to reach the displayed target exactly.
4. In the game dashboard, choose **Stop** to close testing. Inspect the testing session in the session list. Testing sessions stay out of research corpus exports.

{{< figure src="manual/beacon-waiting-room.png" alt="Beacon Relay waiting room with ten minutes remaining, one connected seat, and one waiting seat." >}}

To customize this screen, edit `client/beacon.css` in your game project. The starter styles the countdown with `.parlando-waiting-countdown` and the participant cards with `.seat-grid`. Parlando adds `.seat-ready` to each connected seat.

Press Ctrl+C in the server terminal when you finish. Restart with the same store to reuse the game and experiment records. See [configure and run a study](../run/configuration/) when you are ready to collect research sessions.

## Find the next command

| Task | Command |
| --- | --- |
| Create a project | `parlando new NAME --lang rust` or `--lang python` |
| Test rules and compiled scenarios | `parlando test` |
| Produce a package | `parlando build` |
| Validate a built package | `parlando check --package dist/beacon-0.1.0.zip` |
| Start a server | `parlando serve` |
| Sign in for CLI uploads | `parlando login --server SERVER_URL` |
| Upload a package | `parlando deploy --host SERVER_URL --package dist/beacon-0.1.0.zip` |

Use `parlando COMMAND --help` for options. Build commands read `game.yaml`
in the working directory unless you pass `--manifest`. Commands print readable
results by default. In scripts, add `--json` and read fields such as
`result.revision` from the response.
