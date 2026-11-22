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
  type GameSnapshot,
} from "@coli-saar/parlando-client/react";
```

`ParticipantApp` handles registration, consent, matchmaking, readiness, connection, conversation, microphone controls, leaving, replay, and terminal outcomes. It hands a `GameSnapshot` to the game renderer. The game owns its domain types, game surface, task controls, language, assets, and completion presentation.

`GameSnapshot` exposes role-specific `observation`, the nullable most recent `transition`, optional `availableActions`, `interactionEnabled`, and `act`. A transition contains the accepted actor and action; presentation code may ignore it and render only the observation. The snapshot does not expose authoritative state, transport state, provider credentials, or dashboard configuration. The same renderer receives live and reconstructed replay snapshots.

Use `new ParticipantClient({ baseUrl })` for the HTTP lifecycle of a custom non-React participant application. It retains the participant credential and returns presentation-neutral experiment, room, and authenticated one-use `GameSessionPlan` or `AudioSessionPlan` values with JavaScript-style field names. Implement the documented WebSocket protocol from that plan when the complete `ParticipantApp` lifecycle is unsuitable; raw server-message DTOs and socket helpers are intentionally not package exports.

## Presentation boundary

The package supplies platform behavior and platform widgets, while the game supplies its game surface. Maps, task-specific labels, and game animation belong to the game. Conversation, microphone, connection, leave, and replay controls belong to Parlando and expose stable `parlando-*` styling hooks.

`ParticipantApp` displays a communication widget below the game by default. The experiment's `communication.mode` selects typed chat or the microphone controls. To place the widget in your UI, set `communicationPlacement="game"` on `ParticipantApp` and render one `<CommunicationWidget />` inside `renderGame`. Import both components from `@coli-saar/parlando-client/react`. The widget includes default styling. Scope game CSS to its `.parlando-communication` root to customize its appearance. See the [game developer guide](../docs/web/content/manual/build/create-game.md#place-communication-beside-the-game) for a layout example.

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
