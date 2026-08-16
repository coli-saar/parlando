# @coli-saar/parlando-client

Reusable browser building blocks for Parlando games. The package handles the
parts that games share—participant setup, scoped API access, WebSockets, and
audio—while each game remains free to define its own screens, visual world,
controls, assets, language, and interaction model.

## Shared capabilities

- Protocol and response types for the Rust server HTTP and WebSocket APIs.
- `ExperimentApiClient`, URL helpers, checked JSON helpers, and socket helpers.
- Voice-session orchestration for microphone setup and sink lifecycle.
- A provider-neutral browser audio sink for Parlando's server relay.
- Reusable platform widgets and hooks under `@coli-saar/parlando-client/react`, such as microphone mute, local level, voice status, and transcription status components.

## Game-owned capabilities

- Game-specific state, actions, observations, rendering, assets, maps, levels,
  scoring, and participant-facing language belong in the game client.
- Client-side interaction logic can be as rich as the study needs. The Rust game
  mechanics retain the matching final validation used for browsers and agents.
- Demo layouts and Space Game-specific CSS stay in `space-game/client` so the SDK
  does not impose a visual design on other games.
- Private Speechmatics or TTS configuration stays in the server process. The SDK
  receives public capability metadata and a short-lived room-audio credential.

## Package entrypoints

```ts
import { ExperimentApiClient, socketUrl } from "@coli-saar/parlando-client";
import { ParlandoAudioSink } from "@coli-saar/parlando-client";
import { VoiceStatusChip } from "@coli-saar/parlando-client/react";
```

The root entrypoint contains protocol types, API helpers, WebSocket helpers, the audio-session controller and sink, microphone helpers, and non-React utility functions.

For voice-enabled games, call `ExperimentApiClient.createRoom(...)` before rendering the readiness waiting room. The server pairs a human with an existing compatible waiting room or creates one; in human-agent mode it supplies Player B immediately. The client derives `/e/{experiment_id}` from the participant page and scopes room, game-session, and audio-session requests to it.

## Speech configuration

The SDK does not configure Speechmatics or TTS services directly. A game client reads public capability information from the experiment-scoped configuration endpoint, then requests a room-specific Parlando audio credential through `ExperimentApiClient`. The Rust server owns provider selection, private API keys, transcription sessions, and TTS voice settings.

`ParlandoAudioSink` captures and sends fixed 24 kHz PCM frames and plays partner or agent audio through an AudioWorklet. Playback uses the server-provided jitter target, linear device-rate interpolation, bounded stale-audio trimming, short underrun recovery, and diagnostic reporting. Game clients should normally let `ParlandoStartupGate` construct this sink rather than creating audio nodes or WebSockets themselves.

## Widgets

The SDK may include reusable platform widgets when they describe Parlando runtime state rather than game UI. Good examples are mic-level meters, microphone mute controls, consent controls, waiting-room indicators, connection status chips, and STT readiness chips.

These widgets stay generic: they accept state and callbacks from the game app,
avoid knowledge of a particular state model, and expose CSS classes or props so
each game can style them. This allows a game to use as much or as little of the
shared presentation as fits its participant experience.

## Local development from a game checkout

Normal game clients should depend on the released npm package:

```json
"@coli-saar/parlando-client": "^0.2.0"
```

When debugging SDK changes from a separate local Parlando checkout, temporarily install that checkout by absolute path:

From this directory:

```bash
npm install
npm run build
```

From a game client checkout:

```bash
npm install --no-save file:/absolute/path/to/parlando/js-client
```

After SDK edits, rebuild the SDK and reinstall or rebuild the game client as needed:

```bash
npm run build
```

Because `package.json` cannot contain comments, keep the normal registry dependency committed and use the `file:` dependency only as a temporary local edit or install command. Before committing a game release, restore the registry dependency and refresh `package-lock.json`.

Committed consumer manifests should normally depend on a version, for example `"@coli-saar/parlando-client": "^0.2.0"`, not on `file:` paths.

## Publishing

Before publishing, test and build the package:

```bash
npm install
npm test
npm run build
```

For an online package release, run the repository-level publishing dry run and publish target:

```bash
make publish-js-client-dry-run
make publish-js-client
```
