# Parlando Space Game

Space Game puts two participants aboard a station with a failing evacuation
beacon. Each player can inspect and operate different station equipment. The
WebAssembly component owns the rules and the observations for each seat. The
browser displays those observations and sends actions through Parlando.

![Space Game participant interface with role-specific information, action
controls, station state, and communication](../docs/images/space-game-interface.jpg)

## Start the game

Install Rust's `wasm32-unknown-unknown` target, `wasm-tools`, and Node.js.
Then run from this directory.

```bash
make run
```

Open the dashboard at `http://127.0.0.1:8000/admin`. On a fresh installation,
create the administrator, then create and activate an experiment. Share its
participant URL with players. The launcher includes the **Back and forth**
agent. Set `PORT` to change the port or `PARLANDO_DATABASE_URL` to select a
database. The default database is `.local/component.sqlite`.

## Package the game

`make build` writes `dist/parlando/game.yaml`, the component, and the
browser bundle. Use this manifest with `parlando serve` to run the package
with the generic host. For the authored agent, use `make run`.

## Solo voice test

Start the game with `make run`. In an inactive experiment, configure the following settings.

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

The client expects a Parlando-compatible server that exposes experiment-scoped `/e/{experiment_id}/api/*` and WebSocket routes. In local runs, the launcher serves the browser files bundled under `dist/parlando/client`.

## Deploy on Render

The Space Game includes a production [`Dockerfile`](Dockerfile) and a complete
[`render.yaml`](render.yaml) Blueprint. Together they build the WebAssembly component, the host with the authored agent, and the
browser client, attach persistent storage for SQLite, and configure Render's
database-aware health check.

Follow [Deploy a Parlando Game on
Render](../docs/deploying-on-render.md#deploy-the-space-game-blueprint) to deploy
this example or adapt the same two files to another game.
