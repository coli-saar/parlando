# Parlando Space Game Client

Demo browser client for the Parlando Space Game. This project is a consumer of the reusable `@parlando/client` SDK and an installed `parlando-space-game` server binary. It should not import SDK or server source by relative path.

## Project role

The local Parlando checkout has three parts:

- `rust-server` builds and installs the `parlando-space-game` server binary.
- `js-client` builds and publishes the reusable SDK package as `@parlando/client` into the local yalc store.
- `space-game` is this demo game client, with Space Game-specific UI, assets, state interpretation, controls, and tests.

The dependency direction is one-way: this app consumes installed artifacts. It does not assume the SDK or server source directories exist.

## Start the game

For the normal two-human local game, run from the Parlando top-level directory:

```bash
cd /Users/koller/Documents/workspace/parlando
make run-space-game
```

For the single-browser back-and-forth voice test, run:

```bash
cd /Users/koller/Documents/workspace/parlando
make run-space-game-solo-voice
```

Both commands install or refresh shared local artifacts before starting the server. They are convenient after source changes, but they can take a while because they may run `cargo install`, publish `@parlando/client` to yalc, reinstall client dependencies, and rebuild the browser app.

After dependencies are already installed and you only want to restart the game, use the game-local command instead:

```bash
cd /Users/koller/Documents/workspace/parlando/space-game
PATH="/Users/koller/Documents/workspace/parlando/.local/bin:$PATH" make run-solo-voice
```

For the non-voice local config, use:

```bash
cd /Users/koller/Documents/workspace/parlando/space-game
PATH="/Users/koller/Documents/workspace/parlando/.local/bin:$PATH" make run
```

Open the game at `http://127.0.0.1:8000/`. Stop it with `Ctrl-C` in the terminal running `make`.

## Top-level local install

From the Parlando top-level directory, install local dependencies first:

```bash
make install-local
```

That command installs the Rust server binary into `.local/bin` and publishes `@parlando/client` into the local yalc store.

Then build or run the demo:

```bash
make build-space-game
make run-space-game
```

## Game-local Makefile

This directory also has a source-agnostic Makefile. It expects:

- `parlando-space-game` on `PATH`, or `SERVER_BIN=/path/to/parlando-space-game`.
- `@parlando/client@0.1.0` already published in the local yalc store.

Run from this directory:

```bash
make build
make run
```

The Makefile installs the SDK package from yalc with `npm install --no-save`, so the committed dependency stays versioned as `"@parlando/client": "^0.1.0"`.

## Solo voice test

For a single-browser test against the deterministic back-and-forth agent, run from the top-level directory:

```bash
make run-space-game-solo-voice
```

Or from this directory, after local dependencies are installed:

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

The client expects a Parlando-compatible server that exposes the documented `/api/*` and `/ws/game/*` routes. In production-like deployments, the Rust server can serve this app's built `dist` directory through `server.client_dist_path`.
