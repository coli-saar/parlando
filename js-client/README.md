# @coli-saar/parlando-client

Frontend-neutral browser support for Parlando participant applications. The Rust server does not depend on this package; it implements the versioned protocol documented in `docs/client-protocol.md`.

## Entry points

The root exports the managed client and stable participant data types:

```ts
import {
  ParticipantClient,
  type AudioSessionPlan,
  type ExperimentInfo,
  type GameSessionPlan,
  type JoinedRoom,
  type PlayerMessage,
  type PlayerRole,
  type VoicePreflight,
  type VoiceStatus,
} from "@coli-saar/parlando-client";
```

The React entry point exports the participant lifecycle and platform widgets:

```tsx
import {
  ParticipantApp,
  type GameSession,
  MicrophoneMuteButton,
  MicrophoneLevelMeter,
  TranscriptionProgress,
} from "@coli-saar/parlando-client/react";
```

`ParticipantApp` handles registration, consent, matchmaking, readiness, connection, and optional audio before handing a `GameSession` to the game renderer. The game owns its domain types, visual design, controls, language, assets, and completion presentation.

`GameSession` exposes role-specific `observation`, the nullable most recent `transition`, optional `availableActions`, conversation, presence, completion, voice capability, and high-level operations. A transition contains the accepted actor and action; presentation code may ignore it and render only the observation. The session does not expose authoritative state, generic events, provider credentials, or dashboard configuration.

Use `new ParticipantClient({ baseUrl })` for the HTTP lifecycle of a custom non-React participant application. It retains the participant credential and returns presentation-neutral experiment, room, and authenticated one-use `GameSessionPlan` or `AudioSessionPlan` values with JavaScript-style field names. Implement the documented WebSocket protocol from that plan when the complete `ParticipantApp` lifecycle is unsuitable; raw server-message DTOs and socket helpers are intentionally not package exports.

## Presentation boundary

The package supplies platform behavior, not game presentation. Human-readable labels, animation, maps, dialogue layout, and accessibility choices belong to the participant application. Rust games and agents exchange only structured domain observations, actions, messages, and completion.

Speechmatics and TTS credentials and provider selection remain server-owned. Participant code receives only narrow capabilities and session status. Do not call provider APIs directly.

## Local development

```bash
npm install
npm test
npm run build
```

A game normally depends on a published version. For temporary local development, install this checkout by absolute path and restore the versioned dependency before committing.

## Publishing

```bash
make publish-js-client-dry-run
make publish-js-client
```
