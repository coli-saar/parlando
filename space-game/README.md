# Parlando Space Game

Demo browser client and Rust game server for the Parlando Space Game. The browser app consumes the reusable `@parlando/client` SDK; the Rust crate in `server/` links the reusable `parlando-server` crate to Space Game-specific state, adapter, and agent code.

## Project role

The local Parlando checkout has three parts:

- `rust-server` provides the reusable Rust server crate.
- `js-client` builds and publishes the reusable SDK package as `@parlando/client` into the local yalc store.
- `space-game` owns the demo game's Rust server crate, browser UI, assets, state interpretation, controls, configs, and tests.

The browser dependency direction is one-way: this app consumes the SDK as an installed package. The Rust game server is source-local in `server/` and depends on the reusable `rust-server` crate.

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

Both commands install or refresh shared local artifacts before starting the server. They are convenient after source changes, but they can take a while because they may run `cargo install`, package/link `@parlando/client` with yalc, reinstall client dependencies, and rebuild the browser app.

Open the game at `http://127.0.0.1:8000/`. Stop it with `Ctrl-C` in the terminal running `make`.

## Local install

From this directory, install local Space Game dependencies first:

```bash
cd /Users/koller/Documents/workspace/parlando/space-game
make install-local
```

That command installs the Rust server binary into `.local/bin` and prepares `@parlando/client` in the local yalc store.

Then build or run the demo:

```bash
make build
make run
```

## Game-local Makefile

This directory has the Space Game Makefile. It can prepare its own local dependencies, and it also accepts externally prepared artifacts:

- the default `.local/bin/parlando-space-game`, or `SERVER_BIN=/path/to/parlando-space-game`.
- `@parlando/client@0.1.0` already available in the local yalc store.

Run from this directory:

```bash
make build
make run
```

The Makefile installs the SDK package from yalc with `npm install --no-save`, so the committed dependency stays versioned as `"@parlando/client": "^0.1.0"`.

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
- LiveKit browser room audio
- Speechmatics browser transcription
- ElevenLabs agent TTS publishing

It asks the Rust server to include the local private service configuration from `../rust-server/config/experiment.livekit.private.yaml`. That file supplies the LiveKit, Speechmatics, and ElevenLabs settings for local voice testing. The browser app does not own those settings; it only receives public capability metadata from `/api/config` and room-specific credentials from `/api/rooms/{room_id}/audio-session`.

## Using SDK widgets

This app can use SDK widgets for Parlando platform state, such as microphone readiness and transcription progress:

```tsx
import { VoiceStatusChip, TranscriptionStatusChip } from "@parlando/client/react";
```

Game-specific UI should stay here. For example, the station map, inventory/action controls, game status panels, and Space Game copy belong in this project, not in the SDK.

## Runtime expectation

The client expects a Parlando-compatible server that exposes the documented `/api/*` and `/ws/game/*` routes. In local runs, the Rust server serves this app's built `client/dist` directory through `server.client_dist_path`.
