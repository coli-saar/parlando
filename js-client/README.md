# @parlando/client

Reusable browser SDK for Parlando game clients. This package owns shared experiment-client infrastructure and must not depend on a concrete game. Game clients depend on this package; this package does not import game state engines, assets, routes, copy, or UI screens.

## What belongs here

- Protocol and response types for the Rust server HTTP and WebSocket APIs.
- `ExperimentApiClient`, URL helpers, checked JSON helpers, and socket helpers.
- Voice-session orchestration for microphone setup and sink lifecycle.
- Browser-side LiveKit and Speechmatics sink implementations.
- Reusable platform widgets and hooks under `@parlando/client/react`, such as microphone level, voice status, and transcription status components.

## What does not belong here

- Game-specific state, actions, observations, validation, rendering, assets, maps, levels, or scoring.
- Demo app layout or Space Game-specific CSS.
- Local checkout assumptions such as `file:../js-client` dependencies in consumers.
- Private LiveKit, Speechmatics, or TTS service configuration. The server is the source of truth for speech configuration and sends only public capability metadata or short-lived room credentials to the browser.

## Package entrypoints

```ts
import { ExperimentApiClient, socketUrl } from "@parlando/client";
import { LiveKitPartnerAudioSink } from "@parlando/client/livekit";
import { SpeechmaticsTranscriptionSink } from "@parlando/client/speechmatics";
import { VoiceStatusChip } from "@parlando/client/react";
```

The root entrypoint contains protocol types, API helpers, WebSocket helpers, audio-session controller classes, microphone helpers, and non-React utility functions. Optional integration-specific code is exported from subpaths so consumers can import only the pieces they need.

## Speech configuration

The SDK does not configure LiveKit, Speechmatics, or TTS services directly. A game client reads public capability information from `/api/config`, then requests room-specific audio credentials from `/api/rooms/{room_id}/audio-session`. The Rust server owns provider selection, private API keys, token minting, temporary Speechmatics keys, and TTS voice settings.

## Widgets

The SDK may include reusable platform widgets when they describe Parlando runtime state rather than game UI. Good examples are mic-level meters, voice join controls, consent controls, waiting-room indicators, connection status chips, and STT readiness chips.

These widgets should stay generic. They should accept state and callbacks from the game app, avoid direct knowledge of a game's state model, and use CSS classes or explicit props so each game can style them. If default SDK styling is added later, it should be opt-in through a separate CSS export.

## Local development with yalc

Use `yalc` when testing unpublished SDK changes from a separate game client checkout. This gives the consumer a package-shaped dependency without publishing every debug build to GitHub Packages.

From this directory:

```bash
npm install
npm run yalc
```

From a game client checkout:

```bash
yalc add @parlando/client
npm install
```

After SDK edits:

```bash
npm run build
yalc push
```

A consumer can remove the local yalc override with:

```bash
yalc remove @parlando/client
npm install
```

Committed consumer manifests should normally depend on a version, for example `"@parlando/client": "^0.1.0"`, not on `file:` paths.

## Local publishing

The JavaScript SDK is published locally with yalc rather than to an online npm registry:

```bash
npm install
npm test
npm run yalc
```

The package is marked `private` to avoid accidental npm or GitHub Packages publishing.
