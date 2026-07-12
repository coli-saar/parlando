# Browser Client Protocol

The browser client receives the game-specific Rust values as JSON. A game author therefore needs to design both sides together:

- Rust structs and enums define the authoritative state, action, observation, event, and summary.
- Serde attributes define the wire names.
- TypeScript types should mirror the JSON shape, not necessarily the Rust field names.
- The React or plain JavaScript UI renders observations and sends action JSON back to the server.

The reusable `@coli-saar/parlando-client` package supplies HTTP helpers, WebSocket helpers, audio-session helpers, and generic protocol types. The game client supplies game-specific types and rendering.

## Rust To JSON Naming

Rust structs commonly use snake_case fields. The JSON wire format follows serde rules:

- a field without `#[serde(rename = "...")]` keeps its Rust field name.
- `#[serde(rename = "someName")]` uses that exact JSON key.
- `#[serde(rename_all = "camelCase")]` converts all fields in that struct or enum variant.
- enum variants with `#[serde(tag = "type")]` serialize as JSON objects with a `type` discriminator.

Example Rust action:

```rust
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(tag = "type")]
pub enum SpaceAction {
    #[serde(rename = "moveStep")]
    MoveStep { player: String, direction: String },
    #[serde(rename = "setValve")]
    SetValve {
        player: String,
        valve: String,
        open: bool,
    },
}
```

Matching JSON actions:

```json
{ "type": "moveStep", "player": "A", "direction": "left" }
```

```json
{ "type": "setValve", "player": "B", "valve": "C", "open": true }
```

Matching TypeScript type:

```ts
type GameAction =
  | { type: "moveStep"; player: "A" | "B"; direction: "up" | "down" | "left" | "right" }
  | { type: "setValve"; player: "A" | "B"; valve: "A" | "C" | "floodgate"; open: boolean };
```

If a Rust field is renamed, mirror the renamed JSON field in TypeScript. For example, the demo game uses Rust field `override_held` with `#[serde(rename = "overrideHeld")]`; the TypeScript field is `overrideHeld`.

Two additional details matter in browser code:

- Rust integer and floating-point values both arrive as JavaScript `number` values.
- fields annotated with `skip_serializing_if = "Option::is_none"` are omitted when absent; fields without that annotation may appear as `null`.

Write UI code that handles both omitted optional properties and explicit `null` when a reusable protocol type allows it.

## Setup Flow

A typical browser flow is:

1. `GET /api/config` to read public study settings, consent text, and enabled audio/conversation features.
2. `POST /api/participants` or `POST /api/direct/start` to create a participant session.
3. `POST /api/consent` if consent is required.
4. `POST /api/rooms` plus `POST /api/rooms/{room_id}/join`, or `POST /api/matchmaking/join`.
5. `POST /api/rooms/{room_id}/audio-session` if voice is enabled.
6. Connect to `/ws/game/{room_id}?participantSessionId=...`.
7. Wait for `roleAssigned` before showing active game controls.

The reusable JavaScript client wraps these HTTP calls, but the game UI still decides how to arrange screens and when to move from setup to waiting to active play.

## HTTP Room Payloads

Room creation, joining, direct-entry, and matchmaking responses include game-specific payloads:

```ts
interface RoomResponse<TState, TObservation, TAction, TEvent> {
  room_id: string;
  participant_session_id: string;
  role: "A" | "B" | string;
  state?: TState | null;
  observation?: TObservation | null;
  available_actions?: TAction[];
  events?: TEvent[];
  conversation?: ConversationMessage[];
}
```

Use `observation` for rendering the participant view. `state` may be present for compatibility or admin-like flows, but a participant UI should not rely on hidden information being absent from `state`; privacy should be enforced by rendering from `observation`.

For waiting-room flows, the first participant may receive `status: "waiting"` without game payloads. The game starts when the WebSocket receives `roleAssigned`.

`available_actions` has three different states:

- omitted or `null`: the game does not provide this affordance in that payload.
- `[]`: the game provides the affordance and this player currently has no listed actions.
- `[ ... ]`: the game provides concrete action objects that the UI may render as controls.

The server still validates every submitted action, including actions selected from `available_actions`.

## WebSocket Messages

Open a room socket with:

```ts
const socket = new WebSocket(
  socketUrl(roomId, participantSessionId, apiBaseUrl)
);
```

The URL shape is:

```text
/ws/game/{room_id}?participantSessionId={participant_session_id}
```

### `roleAssigned`

This is the game-start or reconnect payload for a participant:

```json
{
  "type": "roleAssigned",
  "room_id": "room-123",
  "participant_session_id": "participant-session-abc",
  "role": "A",
  "observation": {
    "role": "A",
    "players": {
      "A": { "room": "power", "position": { "x": 2, "y": 2 }, "plateHeld": false },
      "B": { "room": "valve", "position": { "x": 9, "y": 6 }, "plateHeld": false }
    },
    "privateKnowledge": ["Blue fuse wakes the bus feed."],
    "systems": { "readyToLaunch": false }
  },
  "available_actions": [
    { "type": "moveStep", "player": "A", "direction": "left" }
  ],
  "events": [],
  "conversation": []
}
```

`roleAssigned` is not just an acknowledgement. It is the first game-state payload that should move the UI from waiting/setup into active play.

### `stateChanged`

This is sent after an accepted action:

```json
{
  "type": "stateChanged",
  "room_id": "room-123",
  "observation": { "...": "role-specific observation" },
  "available_actions": [
    { "type": "runDiagnostic", "player": "A" }
  ],
  "events": [
    { "type": "action", "actor": "A", "text": "You run diagnostics." }
  ],
  "conversation": []
}
```

Render the new `observation`, update controls from `available_actions` if the game provides them, and append any `events` to the local activity log.

### Other Server Messages

```ts
type ServerMessage<TState, TObservation, TAction, TEvent> =
  | { type: "roleAssigned"; room_id: string; participant_session_id: string; role: string; state?: TState | null; observation?: TObservation | null; available_actions?: TAction[]; events?: TEvent[]; conversation?: ConversationMessage[] }
  | { type: "stateChanged"; room_id: string; state?: TState | null; observation?: TObservation | null; available_actions?: TAction[]; events?: TEvent[]; conversation?: ConversationMessage[] }
  | { type: "conversationMessageAdded"; room_id: string; conversation_message: ConversationMessage }
  | { type: "completed"; room_id: string; summary: Record<string, unknown> }
  | { type: "presenceChanged"; room_id?: string; presence?: Record<string, unknown> }
  | { type: "voiceStatusChanged"; room_id?: string; voice?: { audioReady?: boolean; transcriptionReady?: boolean; transcriptionStatus?: string } }
  | { type: "partnerWaiting"; room_id?: string; message?: string }
  | { type: "error"; room_id?: string; message?: string };
```

`conversationMessageAdded` is the live chat/voice transcript path. `completed` carries the game-specific completion summary returned by `completion_summary`.

## Sending Actions

The browser submits a game action over the room socket:

```json
{
  "type": "submitAction",
  "action": { "type": "moveStep", "player": "A", "direction": "left" }
}
```

With `@coli-saar/parlando-client`:

```ts
apiClient.sendAction(socket, {
  type: "moveStep",
  player: assignedRole,
  direction: "left"
});
```

The server parses `action` into the Rust `Action` type and calls `validate_action`. Do not assume a disabled button is enough; the server must reject illegal actions.

## Sending Chat

Text chat uses:

```json
{
  "type": "sendChatMessage",
  "text": "Try the blue fuse first."
}
```

The server persists the message as a conversation event and broadcasts `conversationMessageAdded`.

## TypeScript Mirror Types

For each game, define TypeScript types that mirror the JSON shapes. The demo game uses:

```ts
export type GameAction =
  | { type: "moveStep"; player: PlayerId; direction: Direction }
  | { type: "toggleFuse"; player: PlayerId; color: FuseColor }
  | { type: "launchBeacon"; player: PlayerId }
  | { type: "reset" };

export interface StationObservation {
  players: Record<PlayerId, PlayerState>;
  fuses: Record<FuseColor, boolean>;
  systems?: DerivedSystems;
  role?: PlayerId;
  privateKnowledge?: string[];
}

type GameServerMessage =
  ServerMessage<StationState, StationObservation, GameAction, ObservationEvent>;
```

Keep the TypeScript type in sync with serde output. If you rename a Rust field or enum variant, update the TypeScript type and UI code at the same time.

Recommended source layout for a game client:

- `game/types.ts`: TypeScript mirror types for action, observation, events, summary, and any UI-only helper types.
- `game/stateEngine.ts`: optional client-side helpers for deriving labels, possible controls, or purely visual effects from observations.
- `App.tsx` or equivalent: setup flow, WebSocket lifecycle, rendering, and action dispatch.

Do not duplicate authoritative validation in the browser and assume it is enough. Client-side checks are for ergonomics; server-side `validate_action` remains the source of truth.

## Browser Implementation Checklist

For a new game client, implement:

1. TypeScript mirror types for `Action`, `Observation`, `Event`, and `Summary`.
2. Participant setup screens that call `/api/config`, `/api/participants`, consent if required, and room or matchmaking entry.
3. A WebSocket connection to `/ws/game/{room_id}?participantSessionId=...`.
4. Handling for `roleAssigned`, `stateChanged`, `conversationMessageAdded`, `completed`, `presenceChanged`, `voiceStatusChanged`, and `error`.
5. Rendering from `observation`, not from hidden server state.
6. Action controls that send `{ type: "submitAction", action }`.
7. Chat controls that send `{ type: "sendChatMessage", text }` if conversation is enabled.
8. Audio-session setup through `/api/rooms/{room_id}/audio-session` if voice is enabled.
9. Local UI state for transient display concerns such as selected panels, animations, scroll position, and unsubmitted text.

## External References

- [Serde attributes](https://serde.rs/attributes.html) for Rust JSON naming and tagging.
- [TypeScript unions](https://www.typescriptlang.org/docs/handbook/2/everyday-types.html#union-types) for action and message discriminators.
- [MDN WebSocket API](https://developer.mozilla.org/en-US/docs/Web/API/WebSocket) for browser socket behavior.
