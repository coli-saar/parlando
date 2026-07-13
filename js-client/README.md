# @coli-saar/parlando-client

Reusable browser SDK for Parlando game clients. This package owns shared experiment-client infrastructure and must not depend on a concrete game. Game clients depend on this package; this package does not import game state engines, assets, routes, copy, or UI screens.

## What belongs here

- Protocol and response types for the Rust server HTTP and WebSocket APIs.
- `ExperimentApiClient`, URL helpers, checked JSON helpers, and socket helpers.
- Voice-session orchestration for microphone setup and sink lifecycle.
- Browser-side LiveKit and Speechmatics sink implementations.
- Reusable platform widgets and hooks under `@coli-saar/parlando-client/react`, such as microphone level, voice status, and transcription status components.

## What does not belong here

- Game-specific state, actions, observations, validation, rendering, assets, maps, levels, or scoring.
- Demo app layout or Space Game-specific CSS.
- Committed local checkout assumptions such as `file:../js-client` dependencies in consumers.
- Private LiveKit, Speechmatics, or TTS service configuration. The server is the source of truth for speech configuration and sends only public capability metadata or short-lived room credentials to the browser.

## Package entrypoints

```ts
import { ExperimentApiClient, socketUrl } from "@coli-saar/parlando-client";
import { LiveKitPartnerAudioSink } from "@coli-saar/parlando-client/livekit";
import { SpeechmaticsTranscriptionSink } from "@coli-saar/parlando-client/speechmatics";
import { VoiceStatusChip } from "@coli-saar/parlando-client/react";
```

The root entrypoint contains protocol types, API helpers, WebSocket helpers, audio-session controller classes, microphone helpers, and non-React utility functions. Optional integration-specific code is exported from subpaths so consumers can import only the pieces they need.

For voice-enabled games, create or join a room before rendering the readiness waiting room. `ExperimentApiClient.createRoom(...)` returns a `room_id` for Player A immediately, and `ExperimentApiClient.joinRoom(...)` adds Player B to that existing room. In human-agent mode, the server supplies Player B when the room is created. Once the client has `room_id` and `participant_session_id`, it can open the room WebSocket and request `/api/rooms/{room_id}/audio-session` while the UI still shows Player A, Player B/agent, and STT readiness.

## Speech configuration

The SDK does not configure LiveKit, Speechmatics, or TTS services directly. A game client reads public capability information from `/api/config`, then requests room-specific audio credentials from `/api/rooms/{room_id}/audio-session`. The Rust server owns provider selection, private API keys, token minting, temporary Speechmatics keys, and TTS voice settings.

## Widgets

The SDK may include reusable platform widgets when they describe Parlando runtime state rather than game UI. Good examples are mic-level meters, voice join controls, consent controls, waiting-room indicators, connection status chips, and STT readiness chips.

These widgets should stay generic. They should accept state and callbacks from the game app, avoid direct knowledge of a game's state model, and use CSS classes or explicit props so each game can style them. If default SDK styling is added later, it should be opt-in through a separate CSS export.

## Local development from a game checkout

Normal game clients should depend on the released npm package:

```json
"@coli-saar/parlando-client": "^0.1.3"
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

Committed consumer manifests should normally depend on a version, for example `"@coli-saar/parlando-client": "^0.1.3"`, not on `file:` paths.

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
