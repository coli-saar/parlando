# Parlando Space Game

Space Game is a complete example of a custom Parlando game. Its browser
application presents an interactive space station with role-specific information
and controls. Its Rust crate defines movement, station systems, repair actions,
observations, completion, and example agents. Both parts use Parlando's reusable
experiment infrastructure.

## Project role

The Parlando checkout has three parts:

- `rust-server` provides the reusable Rust server crate.
- `js-client` builds the reusable SDK package `@coli-saar/parlando-client`.
- `space-game` owns the demo game's Rust server crate, browser UI, assets, state interpretation, controls, configs, and tests.

The browser dependency direction is one-way. Published builds consume the released SDK and Rust crate; `make run-local` temporarily overrides both with the sibling checkout without changing either manifest.

The station interface is intentionally game-specific. It illustrates how a
project can use Parlando's startup, connection, chat, and voice facilities while
giving participants a visual and interactive experience of its own.

![Space Game participant interface with role-specific information, action
controls, station state, and communication](../docs/images/space-game-interface.jpg)

## Start the game

Run the compiled Space Game host from this directory:

```bash
cd /Users/koller/Documents/workspace/parlando/space-game
make run-local
```

The command installs or refreshes the sibling Rust and JavaScript Parlando artifacts before starting the server. It can take a while after source changes because it may run `cargo install`, build `@coli-saar/parlando-client`, reinstall client dependencies, and rebuild the browser app. Use `make run` to instead test released dependencies.

Open `http://127.0.0.1:8000/admin`, configure the inactive starter experiment or create another one, and activate it. Participant pages live at `http://127.0.0.1:8000/e/{experiment_id}/`. Stop the process with `Ctrl-C` in the terminal running `make`.

### Administrator login

Open `http://localhost:8000/admin` after starting the game. On the first visit for the game database, choose an administrator username and a password of at least 12 characters. Parlando stores only an Argon2id password hash in SQLite and shows the ordinary login form afterward. The credential survives server restarts and binary reinstalls. See [Running and Deployment](../docs/running-and-deployment.md#establish-administrator-access) for persistence, first-visitor behavior, dashboard IP ranges, and recovery.

## Local install

From this directory, install local Space Game dependencies first:

```bash
cd /Users/koller/Documents/workspace/parlando/space-game
make install-local
```

That command is no longer necessary: `make run-local` prepares the local overrides itself.

Then build or run the demo:

```bash
make build
make run-local
```

## Game-local Makefile

This directory has the Space Game Makefile. `make run` uses the published dependencies declared in its manifests. `make run-local` applies ephemeral local overrides for development:

- the default `.local/bin/parlando-space-game`, or `SERVER_BIN=/path/to/parlando-space-game`.
- the local `js-client` checkout already built with `npm run build`.

Run from this directory:

```bash
make run
make run-local
```

`make run-local` patches Cargo's `parlando` crate to the sibling `rust-server` directory and temporarily installs the sibling `js-client` package into `client/node_modules`. The manifests retain their published versions, so `make run` continues to test the crates.io and npm releases.

## Solo voice test

Start the ordinary server with `make run`, then create or clone an inactive
experiment in the dashboard and configure:

Create or clone an inactive experiment in the dashboard and configure:

- `agents.mode = human_vs_agent`
- `agents.human_vs_agent.factory = space_game.back_and_forth`
- Parlando server-relayed browser audio
- server-side Speechmatics transcription
- ElevenLabs agent TTS publishing

The deterministic agent also answers typed or spoken questions about its visible world, including player locations, launch readiness, battery, fuses, breakers, valves, relay state, and its private hints. This makes the configured experiment an end-to-end microphone → transcription → agent → TTS smoke test without requiring an LLM provider.

Add Speechmatics and ElevenLabs credentials explicitly under Game settings, then configure both exact provider endpoints and the ElevenLabs voice in the experiment form. The browser only receives public capability metadata and a short-lived credential for Parlando's audio relay.

The shared SDK owns PCM framing, output-rate interpolation, jitter buffering, and underrun recovery; the demo game contains no media transport code. See [`docs/audio-transport.md`](../docs/audio-transport.md) for the protocol and operational model.

## Using SDK widgets

This app can use SDK widgets for Parlando platform state, such as microphone readiness and transcription progress:

```tsx
import {
  MicrophoneLevelMeter,
  MicrophoneMuteButton,
  TranscriptionProgress,
} from "@coli-saar/parlando-client/react";
```

Game-specific UI should stay here. For example, the station map, inventory/action controls, game status panels, and Space Game copy belong in this project, not in the SDK.

## Runtime expectation

The client expects a Parlando-compatible server that exposes experiment-scoped `/e/{experiment_id}/api/*` and WebSocket routes. In local runs, the Makefile passes this app's built `client/dist` directory to the Rust process.

## Deploy on Render

The Space Game includes a production [`Dockerfile`](Dockerfile) and a complete
[`render.yaml`](render.yaml) Blueprint. Together they build the Rust server and
browser client, attach persistent storage for SQLite, and configure Render's
database-aware health check.

Follow [Deploy a Parlando Game on
Render](../docs/deploying-on-render.md#deploy-the-space-game-blueprint) to deploy
this example or adapt the same two files to another game.
