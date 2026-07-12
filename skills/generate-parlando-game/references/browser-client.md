# Parlando Browser Client Reference

Generated browser clients consume the published `@coli-saar/parlando-client` package. Do not generate `file:` dependencies on Parlando source directories.

This reference documents only what a game client must supply: game-specific JSON types, UI state, rendering, and action dispatch. Treat startup, consent, audio, transcription, WebSocket lifecycle, HTTP error handling, and reusable widgets as SDK responsibilities.

## Package Manifest

Use the latest published client version discovered by the skill:

```json
{
  "name": "<game-slug>-client",
  "private": true,
  "version": "0.1.0",
  "type": "module",
  "scripts": {
    "build": "tsc && vite build",
    "test": "vitest run",
    "dev": "vite"
  },
  "dependencies": {
    "@coli-saar/parlando-client": "^<latest-version>",
    "react": "^19.0.0",
    "react-dom": "^19.0.0"
  },
  "devDependencies": {
    "@testing-library/react": "^16.0.0",
    "@types/react": "^19.0.0",
    "@types/react-dom": "^19.0.0",
    "@vitejs/plugin-react": "^5.0.0",
    "typescript": "^5.8.0",
    "vite": "^7.0.0",
    "vitest": "^3.0.0"
  }
}
```

Add `livekit-client` only if the generated client imports SDK voice helpers that require it as a peer dependency.

## Game JSON Types

TypeScript types must mirror serde JSON, not Rust syntax:

- Unrenamed Rust fields keep snake_case JSON names.
- `#[serde(rename = "someName")]` uses that exact JSON key.
- `#[serde(rename_all = "camelCase")]` applies to all fields or variants.
- `#[serde(tag = "type")]` enum variants become discriminated JSON objects.
- Optional Rust fields may arrive as `null` or be absent, depending on serde attributes.

Example Rust action:

```rust
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(tag = "type")]
pub enum GameAction {
    #[serde(rename = "chooseCard")]
    ChooseCard { player: String, card_id: String },
    #[serde(rename = "submitGuess")]
    SubmitGuess { player: String, answer: String },
}
```

Matching TypeScript:

```ts
export type PlayerRole = "A" | "B";

export type GameAction =
  | { type: "chooseCard"; player: PlayerRole; card_id: string }
  | { type: "submitGuess"; player: PlayerRole; answer: string };
```

Define matching `GameObservation`, `GameEvent`, and `GameSummary` types. Render from `GameObservation`, not from hidden server state.

## Client Entrypoint

Use the reusable startup gate from `@coli-saar/parlando-client/react` and keep generated code focused on game rendering:

```tsx
import { ParlandoStartupGate, type ActiveParlandoSession } from "@coli-saar/parlando-client/react";

type GameSession = ActiveParlandoSession<GameState, GameObservation, GameAction, GameEvent>;

export function App() {
  return (
    <ParlandoStartupGate<GameState, GameObservation, GameAction, GameEvent>
      labels={{ title: "Game Name" }}
      renderGame={(session) => <GameView session={session} />}
    />
  );
}
```

Omit `labels.title` when the YAML `study.name` is the right participant-facing title. The gate passes an `ActiveParlandoSession` only when the game may render. Generated clients must not implement custom startup lifecycle or generic WebSocket-message handling.

Use these session fields in game UI as needed:

- `role`, `observation`, `availableActions`, and `events` for role-specific game rendering.
- `conversation`, `sendChatMessage`, `voiceStatus`, and `toggleVoice` for game-screen communication controls.
- `sendAction(action)` for game actions.
- `completed`, `connected`, and `leave` for game-screen status and controls.

## Actions And Chat

Submit game actions with:

```ts
session.sendAction(action);
```

Submit text chat, when conversation is enabled, with:

```ts
session.sendChatMessage(text);
```

The server still validates every action. Client-side disabled buttons or filtered controls are only ergonomic hints.

For voice studies, enable the full voice stack in config and keep provider credentials in server config/secrets, never in frontend build variables. The active game screen may display voice state or a mute/unmute control from the session, but should not implement startup audio plumbing.

## Source Layout

Recommended client files:

- `web/src/game/types.ts`: JSON mirror types for action, observation, event, summary, and UI helper types.
- `web/src/game/stateEngine.ts`: optional pure helpers for labels, previews, visual derivations, or UI-only controls.
- `web/src/game/stateEngine.test.ts`: tests for helper logic.
- `web/src/App.tsx`: `ParlandoStartupGate` wrapper, game rendering, and action dispatch.
- `web/src/styles.css`: app styling.

## Browser Checklist

Implement:

- `ParlandoStartupGate` wrapper.
- rendering from role-specific observation.
- action controls that send game actions.
- chat controls if conversation is enabled.
- voice controls/status in the active game only if useful.
