# Parlando Browser Client Reference

Generated browser clients consume the published `@coli-saar/parlando-client` package. Do not generate `file:` dependencies on Parlando source directories.

This reference documents only what a game client must supply: game-specific JSON types, UI state, rendering, and action dispatch. Treat startup, consent, audio, transcription, WebSocket lifecycle, HTTP error handling, and reusable widgets as SDK responsibilities.

A generated browser client is participant-mix agnostic. Each browser instance offers UI for exactly one human player, renders that player's role-specific observation, sends that player's actions, and receives accepted actions/events from the other participant through the SDK. The UI must not need to know whether the other participant is another browser user, an in-process agent, or a remote agent.

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
- `completed`, `completionSummary`, `connected`, and `leave` for game-screen status and controls.

Treat these fields as the active capability surface. Do not infer chat or voice availability from the game slug, deployment mode, participant mix, or local assumptions about whether an agent is present.

## Completion UI

`session.completed` becomes true after the server adapter's `is_complete` returns true and Parlando broadcasts the `completed` message. The browser must not implement a separate completion API or decide final success/failure from client-only state.

Generated clients must render a final/terminal screen when `session.completed` is true: hide or disable normal action controls and game chat input, show the participant-facing result from observation/events, and use `session.completionSummary` for durable outcome and score fields. If participants need to see success versus failure, include that terminal outcome in `completion_summary` and optionally mirror participant-specific detail in the role-specific observation or final event text.

## Voice And Transcription Widgets

For active game screens that expose voice or STT state, prefer the reusable exports from `@coli-saar/parlando-client/react`:

```tsx
import {
  MicLevelMeter,
  TranscriptionStatusChip,
  VoiceJoinButton
} from "@coli-saar/parlando-client/react";

function VoiceStrip({ session }: { session: GameSession }) {
  const transcriptionEnabled = Boolean(session.publicConfig.transcription?.enabled);
  return (
    <div className="voice-strip">
      <VoiceJoinButton
        liveKitEnabled={Boolean(session.publicConfig.livekit?.enabled)}
        onToggleVoice={session.toggleVoice}
        voiceStatus={session.voiceStatus}
      />
      {transcriptionEnabled && (
        <>
          <MicLevelMeter
            active={session.voicePreflight.micProbeActive}
            label="Mic"
            level={session.voicePreflight.micLevel}
          />
          <TranscriptionStatusChip voiceStatus={session.voiceStatus} />
        </>
      )}
    </div>
  );
}
```

Use `TranscriptionProgress` from the same package when the UI needs a larger ASR progress display. Generated clients own the CSS for `voice-strip`, `mic-meter`, `mic-meter-track`, and `transcription-chip`, so style them in `web/src/styles.css`.

The mic meter relies on a transform-scaled child inside `.mic-meter-track`. Include CSS equivalent to:

```css
.mic-meter-track {
  overflow: hidden;
}

.mic-meter-track span {
  display: block;
  width: 100%;
  height: 100%;
  transform-origin: left center;
  background: currentColor;
}
```

The exact colors and dimensions should match the game theme, but the child span must have block layout, full dimensions, and a visible background. Otherwise `transform: scaleX(...)` updates can run while the meter appears frozen.

When TTS is enabled, do not synthesize agent speech in the browser. Agent utterances should arrive as normal agent-origin conversation messages and, if the server is configured with TTS and audio publishing, the server will vocalize them through the voice transport.

## Actions And Chat

Submit game actions with:

```ts
session.sendAction(action);
```

Submit text chat, when conversation is enabled, with:

```ts
session.sendChatMessage(text);
```

The server still validates every action. Client-side disabled buttons or filtered controls are only ergonomic hints. This is the same for actions that originated from a human UI and actions that originated from an agent.

For voice studies, enable the full voice stack in config and keep provider credentials in server config/secrets, never in frontend build variables. The active game screen may display voice state or a mute/unmute control from the session, but should not implement startup audio plumbing. It is not the game's job to decide whether voice is enabled: if the server/session exposes voice and asks the client to allow it, the game must allow the SDK-provided voice behavior; otherwise it should omit or disable voice UI according to session state.

## Source Layout

Recommended client files:

- `web/src/game/types.ts`: JSON mirror types for action, observation, event, summary, and UI helper types.
- `web/src/game/stateEngine.ts`: optional pure helpers for labels, previews, visual derivations, or UI-only controls.
- `web/src/game/stateEngine.test.ts`: tests for helper logic.
- `web/src/App.tsx`: `ParlandoStartupGate` wrapper, game rendering, and action dispatch.
- `web/src/styles.css`: active game styling plus startup-screen styling for SDK classes such as `lobby-panel`, `lobby-actions`, `consent-row`, `seat-grid`, `voice-preflight`, `transcription-progress`, `online-error`, and `mic-meter`.

## Startup Styling

`ParlandoStartupGate` provides reusable startup behavior and consistent markup, but generated clients own the stylesheet. Style the SDK startup classes in `web/src/styles.css` so the loading, consent, waiting-room, voice-preflight, transcription-progress, and error states feel native to the generated game instead of appearing unstyled.

Startup styling should include responsive layout, readable form controls, clear disabled and focus states, comfortable consent text, readiness cards, microphone and transcription progress indicators, and mobile-safe spacing. Preserve SDK class names and accessibility attributes.

## Browser Checklist

Implement:

- `ParlandoStartupGate` wrapper.
- startup-screen styles for the classes rendered by `ParlandoStartupGate`.
- rendering from role-specific observation.
- action controls that send game actions.
- chat controls if conversation is enabled.
- voice controls/status in the active game only when exposed by session state and useful.
