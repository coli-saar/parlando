# Parlando participant application reference

Consume published `@coli-saar/parlando-client`; do not use a local Parlando path.

Use the same discovered Parlando release as the Rust runtime.

```tsx
import { ParticipantApp, type GameSession } from "@coli-saar/parlando-client/react";

type Session = GameSession<GameObservation, GameAction, GameCompletion>;

export function App() {
  return <ParticipantApp renderGame={(session) => <GameView session={session} />} />;
}
```

Mirror Serde JSON in TypeScript. Define action, observation, and completion types; do not define authoritative client state or a generic event type. Accepted transitions replace the observation and expose the accepted actor and action through nullable `session.transition`; ignore it when the presentation needs only the new observation.

Use `session.role`, `observation`, `availableActions`, `conversation`, `presence`, `completed`, `completion`, `voiceEnabled`, `voiceStatus`, `voicePreflight`, `sendAction`, `sendMessage`, `setMicrophoneMuted`, and `leave` as needed. Never branch on whether the peer is human or an agent.

When `completed` is true, render a terminal screen and disable normal action/chat controls. Use the shared structured completion for public results such as winner and scores, and the final observation for any role-private terminal facts.

Use `MicrophoneMuteButton` and `MicrophoneLevelMeter` for the active-game voice panel (mute control + level feedback). `TranscriptionProgress` is rendered by `ParticipantApp` itself during the pre-join lobby/preflight screen — do not add it again inside `renderGame`. Delegate microphone setup, transport, STT, TTS playback, credentials, and reconnection to the SDK/runtime. Do not add WebRTC, custom audio sockets/worklets, browser provider clients, or custom startup protocol handling.

Generated CSS must style both `ParticipantApp` lifecycle markup and the active game. Preserve accessibility attributes and visible focus/disabled states. A transformed microphone meter child needs block layout, full width/height, left transform origin, and a visible background. `MicrophoneMuteButton` toggles a `muted`/`live` class itself; give both a visually distinct style (not just the bare `button` default) so the mute state is glanceable, not only readable from its label text.

Test rendering from observation, action/message submission, optional action catalogues, terminal state, and voice capability branches.
