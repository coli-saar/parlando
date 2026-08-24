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

Use `session.role`, `observation`, `availableActions`, `conversation`, `presence`, `completed`, `completion`, `outcome`, `interactionEnabled`, `voiceEnabled`, `voiceStatus`, `voicePreflight`, `sendAction`, `sendMessage`, `setMicrophoneMuted`, and `leave` as needed. Never branch on whether the peer is human or an agent. A game leave control only calls `session.leave`; it must not close sockets, navigate, clear local state, or render its own withdrawal screen. The shared client immediately removes the active game, waits for the runtime's durable recipient-specific outcome, and then renders the standard terminal surface with any applicable Prolific return instructions.

Gate normal action/chat controls on `interactionEnabled`. `ParticipantApp` automatically replaces the game with the terminal shell when `completed` becomes true. When normal completion has game-specific meaning, pass `renderCompletion={(completion) => ...}` so the game controls the content inside that shell and styles the shell through its lifecycle CSS. State plainly whether the result is a success, loss, draw, or neutral finish; use the game's own vocabulary, summarize the meaningful result, and acknowledge cooperative success when appropriate. Do not label a win merely “complete.” Direct and Prolific participants must receive the same game-ending content. `ParticipantApp` conditionally appends its premade Prolific handoff widget after that content, so game code must not render provider instructions, completion codes, recruitment links, or separate provider-specific end screens. Use the shared structured completion for public results such as winner and scores, and the final observation for any role-private terminal facts before termination.

Use `MicrophoneMuteButton` and `MicrophoneLevelMeter` for the active-game voice panel (mute control + level feedback). `TranscriptionProgress` is rendered by `ParticipantApp` itself during the pre-join lobby/preflight screen — do not add it again inside `renderGame`. Delegate microphone setup, transport, STT, TTS playback, credentials, and reconnection to the SDK/runtime. Do not add WebRTC, custom audio sockets/worklets, browser provider clients, or custom startup protocol handling.

Generated CSS must style both `ParticipantApp` lifecycle markup and the active game, including `.parlando-decline-consent`, `.parlando-partner-reconnect`, `.parlando-session-outcome`, `.parlando-game-completion`, `.parlando-recruitment-handoff`, `.parlando-completion-code-row`, `.parlando-completion-code`, `.parlando-copy-completion-code`, and `.parlando-copy-status`. The terminal shell must look intentional and consistent with the game's palette, typography, spacing, borders, and responsive layout. Give completion content a clear visual hierarchy, keep the Prolific handoff visually distinct but integrated, render the completion code as a readable token, and keep its premade copy button immediately adjacent at desktop and mobile widths. Style both the copy button and return link as obvious actions with visible keyboard focus; do not reimplement clipboard behavior in game code. Style `.parlando-decline-consent` as a clearly secondary action beside the primary waiting-room button, with an equally visible keyboard-focus state; the SDK renders it only for Prolific intake with configured consent items. Preserve accessibility attributes and visible focus/disabled states. The reconnect widget is self-explanatory and owns its countdown; games may import `PartnerReconnectNotice` for nonstandard shells but must not reimplement its timer. A transformed microphone meter child needs block layout, full width/height, left transform origin, and a visible background. `MicrophoneMuteButton` toggles a `muted`/`live` class itself; give both a visually distinct style (not just the bare `button` default) so the mute state is glanceable, not only readable from its label text.

Keep lifecycle cards content-sized at desktop widths and full-width within a bounded mobile gutter. If the application wrapper uses flexbox with a viewport-height minimum, set its cross-axis alignment explicitly so the startup card is not stretched to the viewport. Check the unprepared and prepared voice layouts, long consent prose, disabled primary action, direct and Prolific action rows, waiting-room readiness, reconnect notice, normal completion, and exceptional completion at desktop and narrow-mobile widths.

Test rendering from observation, action/message submission, optional action catalogues, terminal state, and voice capability branches.
