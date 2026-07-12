# Parlando Browser Client Reference

Generated browser clients consume the published `@coli-saar/parlando-client` package. Do not generate `file:` dependencies on Parlando source directories.

This reference documents only what a game client must supply: game-specific JSON types, UI state, rendering, action dispatch, and package-level calls. Treat audio, transcription, WebSocket URL construction, HTTP error handling, and reusable widgets as SDK responsibilities.

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

## Client Flow

Use `ExperimentApiClient` and reusable protocol types from `@coli-saar/parlando-client`:

```ts
import {
  ExperimentApiClient,
  type ConversationMessage,
  type MatchmakingResponse,
  type PublicConfigResponse,
  type RoomResponse,
  type ServerMessage
} from "@coli-saar/parlando-client";
```

Specialize reusable types with the game JSON types:

```ts
type GameRoomResponse = RoomResponse<GameState, GameObservation, GameAction, GameEvent>;
type GameMatchmakingResponse = MatchmakingResponse<GameState, GameObservation, GameAction, GameEvent>;
type GameServerMessage = ServerMessage<GameState, GameObservation, GameAction, GameEvent>;
```

Normal generated clients must use this progression:

```text
Setup screen -> Waiting room -> Game
```

The setup screen collects display name, shows consent items, and lets the participant choose/prepare a microphone when voice is enabled so the browser can request audio permission. It must not display LiveKit, Speechmatics, ElevenLabs, API keys, or other voice-provider configuration. The waiting room is a room-backed state: it starts only after the client has both `participantSessionId` and `roomId`. It shows partner/agent/readiness state and initializes STT/voice immediately. The game screen appears only after the participant has a role assignment and all required readiness gates have passed.

Do not implement the voice flow as "Finding a partner" with no `roomId`. Plain matchmaking may return `status: "waiting"` with `room_id: null`; that is a queue state, not the Parlando voice waiting room, because the client cannot request the room audio session without a room. Voice-enabled generated games must create or join a direct room first, then wait inside that room with Player A visible, Player B/opponent visible as waiting/not connected, and STT initializing as the third readiness participant.

Model the waiting room on the Space Game readiness board. Render stable visible slots for:

- Player A: connected/ready, or waiting/not connected.
- Player B or the configured agent opponent: connected/ready, or waiting/not connected.
- STT/transcription service when voice is enabled: ready, initializing, or disabled/unavailable.

Treat STT as an explicit third readiness participant, not a hidden spinner. The participant should be able to see that the game is waiting for Player A, Player B/agent, or the transcription service. If voice is disabled in public config, omit the STT slot or mark voice as disabled and gate only on the player/opponent slots. Do not show provider credentials or configuration fields in this UI.

Generated clients should support:

1. Load public config with `apiClient.getPublicConfig()`.
2. Render setup with display-name input, consent items, and microphone selection/permission controls if voice is enabled.
3. Create a participant with `apiClient.createParticipant(displayName)`.
4. Submit consent with `apiClient.submitConsent(...)` when public config requires it.
5. Acquire a room before showing the waiting room:
   - If the participant creates a new direct room, call `apiClient.createRoom<...>(participantSessionId, "direct")`. This returns the `roomId` immediately and assigns Player A.
   - If the participant enters an existing direct room by code/link, call `apiClient.joinRoom<...>(roomId, participantSessionId)`.
   - For human-vs-agent matchmaking that returns a matched room immediately, `apiClient.enterMatchmaking<...>(participantSessionId)` is acceptable only if the response includes `room_id`.
   - Do not route voice-enabled human-vs-human participants into a no-room matchmaking queue as the waiting room.
6. Open a room WebSocket with `new WebSocket(apiClient.socketUrl(roomId, participantSessionId))`.
7. Render the waiting room readiness board immediately. Show Player A as connected once the socket/presence confirms it, Player B/opponent as waiting/not connected until presence says otherwise, and STT as initializing until the voice state says ready.
8. If voice/STT is enabled, start audio-session/STT initialization immediately after the waiting-room flow has both `roomId` and `participantSessionId`; do not wait for Player B and do not wait for the game screen.
9. Stay in the waiting room until `roleAssigned` arrives and required readiness gates have passed.
10. Render the game screen and enable game controls.

Speechmatics readiness is a server-side start gate. In the tested server behavior, a Speechmatics voice room does not send `roleAssigned` and a human-vs-agent room does not start the agent until the human participant has requested the room audio session and STT is initialized. Therefore the waiting room must kick off the SDK audio-session/STT setup as soon as the first player creates or joins the room. If a flow returns no `room_id`, it has not reached the waiting room; do not tell the user that STT cannot start from the waiting room. Change the flow so room creation or room join happens before the waiting room.

For the readiness board, derive Player A and Player B status from server presence such as `presenceChanged` messages or the session presence snapshot. Always render both slots, even before both seats are occupied. Derive the STT slot from SDK voice state and `voiceStatusChanged` messages, especially `transcriptionReady` and any transcription/status message exposed by the client package. Keep the user in the waiting room until all visible required slots are ready. In human-vs-agent games, show the agent as the Player B/opponent slot once configured or assigned; do not let the agent startup be invisible to the user.

Generated voice games should provide a direct-room entry path by default: a create-room button/link for Player A and a join-room field/link for Player B. If the user asks for matchmaking as well, keep it as an optional queue before room creation; do not call that queue the waiting room and do not claim STT starts there.

If the installed client package does not yet expose `createRoom` or `joinRoom`, do not switch back to no-room matchmaking. Use minimal game-local wrappers around `POST /api/rooms` and `POST /api/rooms/{roomId}/join` with the `RoomResponse` shape above, and report that the generated game should move back to the SDK helpers after upgrading `@coli-saar/parlando-client`.

## Consent UI

Consent is configured in YAML under `direct.require_consent` and `direct.consents`; the frontend receives it through public config. Generated setup screens must display each consent item before joining:

```yaml
direct:
  require_consent: true
  consents:
    - id: study
      title: Study Consent
      body_html: "I agree to participate in this study."
      required: true
```

Frontend requirements:

- Render every consent item from `publicConfig.consents`.
- Display `title` and `body_html`; treat `body_html` as study-authored HTML and render it deliberately, not as plain escaped text if formatting matters.
- Provide a checkbox or equivalent decision control for each item.
- Disable the setup/enter button until all required consent items are accepted.
- Call `apiClient.submitConsent(participantSessionId, decisions)` before room creation, room join, or matchmaking.
- Keep consent UI on the setup screen, not in the active game.

## WebSocket Handling

Handle at least:

```ts
type GameServerMessage =
  | { type: "roleAssigned"; room_id: string; participant_session_id: string; role: string; observation?: GameObservation | null; available_actions?: GameAction[]; events?: GameEvent[]; conversation?: ConversationMessage[] }
  | { type: "stateChanged"; room_id: string; observation?: GameObservation | null; available_actions?: GameAction[]; events?: GameEvent[]; conversation?: ConversationMessage[] }
  | { type: "conversationMessageAdded"; room_id: string; conversation_message: ConversationMessage }
  | { type: "completed"; room_id: string; summary: Record<string, unknown> }
  | { type: "presenceChanged"; room_id?: string; presence?: Record<string, unknown> }
  | { type: "voiceStatusChanged"; room_id?: string; voice?: Record<string, unknown> }
  | { type: "partnerWaiting"; room_id?: string; message?: string }
  | { type: "error"; room_id?: string; message?: string };
```

On `roleAssigned`, move from setup/waiting into active play. On `stateChanged`, replace the current observation, update available actions, and append role-visible events. Keep transient UI state such as selected panels, animations, scroll position, and unsubmitted text separate from server state.

## Actions And Chat

Submit game actions with:

```ts
apiClient.sendAction(socket, action);
```

Submit text chat, when conversation is enabled, with:

```ts
apiClient.sendChatMessage(socket, text);
```

The server still validates every action. Client-side disabled buttons or filtered controls are only ergonomic hints.

## Voice Studies

Do not document or reimplement voice SDK internals in generated games. If voice, transcription, or TTS is requested:

- enable the full voice stack in `config/experiment.*.yaml`: LiveKit, Speechmatics, and ElevenLabs.
- import the needed voice helpers/widgets from `@coli-saar/parlando-client` and its documented subpaths.
- put microphone selection and browser audio-permission preparation on the setup screen; do not show service configuration there.
- put STT/voice readiness in the room-backed waiting room, and start STT initialization there as soon as `roomId` and `participantSessionId` are available.
- for voice-enabled human-vs-human games, prefer direct room create/join over no-room matchmaking so the first player can initialize STT while waiting for the second player.
- start the active game screen only after required voice/transcription readiness has passed.
- keep provider credentials in server config/secrets, never in frontend build variables.

Game-specific voice UI should be limited to placement, labels, and how readiness affects the game screen.

## Source Layout

Recommended client files:

- `web/src/game/types.ts`: JSON mirror types for action, observation, event, summary, and UI helper types.
- `web/src/game/stateEngine.ts`: optional pure helpers for labels, previews, visual derivations, or UI-only controls.
- `web/src/game/stateEngine.test.ts`: tests for helper logic.
- `web/src/App.tsx`: setup flow, WebSocket lifecycle, rendering, and action dispatch.
- `web/src/styles.css`: app styling.

## Browser Checklist

Implement:

- setup screen, plus direct-room create/join UI for voice-enabled games; add matchmaking only when requested.
- setup screen with name input, consent display, and microphone selection/audio permission when voice is enabled.
- room-backed waiting room readiness board with visible Player A, Player B/agent, and STT/transcription slots; begin audio-session/STT setup immediately on room creation/join.
- game screen only after partner/agent assignment and readiness gates.
- `roleAssigned`, `stateChanged`, `conversationMessageAdded`, `completed`, `presenceChanged`, `voiceStatusChanged`, and `error` handlers.
- rendering from role-specific observation.
- action controls that send game actions.
- chat controls if conversation is enabled.
- voice readiness display only if voice is requested.
