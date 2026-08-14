# Parlando Space Game

Demo browser client and Rust game server for the Parlando Space Game. The browser app consumes the reusable `@coli-saar/parlando-client` SDK; the Rust crate in `server/` links the reusable `parlando-server` crate to Space Game-specific state, adapter, and agent code.

## Project role

The local Parlando checkout has three parts:

- `rust-server` provides the reusable Rust server crate.
- `js-client` builds the reusable SDK package `@coli-saar/parlando-client`.
- `space-game` owns the demo game's Rust server crate, browser UI, assets, state interpretation, controls, configs, and tests.

The browser dependency direction is one-way: this app consumes the sibling SDK through its local `file:` dependency. The Rust game server is source-local in `server/` and depends on the sibling `rust-server` crate through a Cargo path dependency.

## Start the game

For the normal two-human local game, run from this directory:

```bash
cd /Users/koller/Documents/workspace/parlando/space-game
make run
```

For the single-browser back-and-forth voice test, run:

```bash
cd /Users/koller/Documents/workspace/parlando/space-game
make run-solo-voice
```

`make run` intentionally uses `config/experiment.local-rust.yaml`, where voice is disabled, so that mode does not show microphone preparation. Use `make run-solo-voice` when testing microphone capture, Speechmatics transcription, or agent speech. In either mode, the browser UI follows the server's public `voice.enabled` capability instead of enabling or hiding microphone access in game-specific code.

Both commands install or refresh shared local artifacts before starting the server. They are convenient after source changes, but they can take a while because they may run `cargo install`, build `@coli-saar/parlando-client`, reinstall client dependencies, and rebuild the browser app.

Open the game at `http://127.0.0.1:8000/`. Stop it with `Ctrl-C` in the terminal running `make`.

### Administrator login

Open `http://localhost:8000/admin` after starting the game. On the first visit for that configuration's database, choose an administrator username and a password of at least 12 characters. Parlando stores only an Argon2id password hash in SQLite and shows the ordinary login form afterward.

`make run` and `make run-solo-voice` use different SQLite files, so each needs this one-time setup. The credential survives server restarts and binary reinstalls. See [Running And Deployment](../docs/running-and-deployment.md#first-administrator-setup) for persistence, first-visitor behavior, and recovery overrides.

## Local install

From this directory, install local Space Game dependencies first:

```bash
cd /Users/koller/Documents/workspace/parlando/space-game
make install-local
```

That command installs the Rust server binary into `.local/bin` and builds the local `@coli-saar/parlando-client` checkout.

Then build or run the demo:

```bash
make build
make run
```

## Game-local Makefile

This directory has the Space Game Makefile. It can prepare its own local dependencies, and it also accepts externally prepared artifacts:

- the default `.local/bin/parlando-space-game`, or `SERVER_BIN=/path/to/parlando-space-game`.
- the local `js-client` checkout already built with `npm run build`.

Run from this directory:

```bash
make build
make run
```

The Rust and browser manifests point directly at the sibling `rust-server` and `js-client` directories. This makes `make run` and `make run-solo-voice` development workflows: they always compile the current Parlando checkout rather than the last packages published to crates.io or npm.

## Solo voice test

For a single-browser test against the deterministic back-and-forth agent, run from this directory:

```bash
cd /Users/koller/Documents/workspace/parlando/space-game
make run-solo-voice
```

Or, after local dependencies are installed:

```bash
make run-solo-voice
```

The solo voice config enables:

- `agents.mode = human_vs_agent`
- `space_game.back_and_forth`
- Parlando server-relayed browser audio
- server-side Speechmatics transcription
- ElevenLabs agent TTS publishing

The deterministic agent also answers typed or spoken questions about its visible world, including player locations, launch readiness, battery, fuses, breakers, valves, relay state, and its private hints. This makes `make run-solo-voice` an end-to-end microphone → transcription → agent → TTS smoke test without requiring an LLM provider.

It asks the Rust server to include the local private service configuration from `../rust-server/config/experiment.voice.private.yaml`. That file supplies Speechmatics and ElevenLabs settings for local voice testing. The browser only receives public capability metadata and a short-lived credential for Parlando's audio relay.

The shared SDK owns PCM framing, output-rate interpolation, jitter buffering, and underrun recovery; the demo game contains no media transport code. See [`docs/audio-transport.md`](../docs/audio-transport.md) for the protocol and operational model.

## Using SDK widgets

This app can use SDK widgets for Parlando platform state, such as microphone readiness and transcription progress:

```tsx
import { VoiceStatusChip, TranscriptionStatusChip } from "@coli-saar/parlando-client/react";
```

Game-specific UI should stay here. For example, the station map, inventory/action controls, game status panels, and Space Game copy belong in this project, not in the SDK.

## Runtime expectation

The client expects a Parlando-compatible server that exposes the documented `/api/*` and `/ws/game/*` routes. In local runs, the Rust server serves this app's built `client/dist` directory through `server.client_dist_path`.
