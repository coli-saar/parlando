# Parlando Space Game

Space Game puts two participants aboard a station with a failing evacuation
beacon. Each player can inspect and operate different station equipment. The
WebAssembly component owns the rules and the observations for each seat. The
browser displays those observations and sends actions through Parlando.

![Space Game participant interface with role-specific information, action
controls, station state, and communication](../docs/images/space-game-interface.jpg)

## Build and deploy

From this directory, build the packages, then upload them to a running Parlando server. The build requires Rust’s `wasm32-unknown-unknown` target, `wasm-tools`, Python 3, and Node.js. Replace `lab` with your server URL or configured alias.

```sh
./build.sh
parlando login --server lab
./deploy.sh --host lab
parlando activate --host lab --game space-game
```

Sign in once per server. The deploy script uploads the game first, followed by its agents. Omit `--host` to use your CLI’s default server. Open the game from **Games** in the server dashboard to configure an experiment and start intake.

The upload includes the **Back and forth** agent. You can also use `make build` and `make deploy HOST=lab`.

## Solo voice test

In an inactive experiment, configure the following settings.

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

The client expects a Parlando-compatible server that exposes experiment-scoped `/e/{experiment_id}/api/*` and WebSocket routes. The host serves the browser files from the ZIP’s `client/` directory.

## Deploy to Railway

Build the game ZIP from this directory and upload it to your shared host. See [Deploy Parlando on Railway](../docs/web/content/manual/deploy/railway.md) for server setup, package uploads, and game activation.
