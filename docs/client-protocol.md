# Participant client protocol

Parlando's Rust server is frontend-neutral. React, another browser framework, a native application, or an agent-operated client may implement this HTTP/JSON/WebSocket protocol. `@coli-saar/parlando-client` and its `ParticipantApp` React component are conveniences, not server dependencies.

The protocol accepts player actions and carries accepted actions, role-specific resulting observations, player messages, narrow readiness state, and completion. Human players and agents receive the same accepted-transition information. A frontend may ignore the action when it wants to present only the resulting observation. The protocol never carries authoritative game state, generic presentation events, peer controller type, provider credentials, storage records, or dashboard configuration.

## Domain values

Rust `Action`, `Observation`, and `Completion` types define their JSON shapes through Serde. A client mirrors those shapes. The server supplies the authenticated `PlayerRole` separately and validates every submitted action.

`available_actions` is nullable. `null` means the game does not enumerate its action space; `[]` means it does and no action is currently available. Neither value bypasses `Game::apply_action`.

## HTTP lifecycle

Participant routes are scoped to `/e/{experiment_id}` when a process hosts multiple experiments. A client normally performs these operations:

1. `GET /api/config` for participant-visible experiment information.
2. `POST /api/participants` with `{}` to register.
3. `POST /api/consent` with consent decisions when required.
4. `POST /api/sessions` with `{}` to join matchmaking.
5. `POST /api/sessions/{public_session_id}/audio-session` when voice is enabled.
6. `POST /api/sessions/{public_session_id}/game-session` for a one-use WebSocket ticket.
7. Open the returned game WebSocket and wait for `session_started`.

Registration returns an opaque bearer credential and a participant-facing random identifier:

```json
{
  "participant_credential": "opaque-secret",
  "participant_id": "curious-otter"
}
```

Keep the bearer credential in memory. Send it in the `Authorization: Bearer …` header for subsequent HTTP calls. Never put it in a URL, browser history, log, or durable browser storage. A game-session plan instead returns a short-lived one-use ticket:

```json
{
  "websocket_url": "/ws/game/room-123",
  "token": "one-use-ticket"
}
```

Append only this ticket as the WebSocket `token` query parameter. Neither the participant credential nor an internal participant-session identifier appears in participant game data.

The session-join response contains `public_session_id`, `role`, optional `presence`, optional `observation`, and `available_actions`. Observation can be absent while matchmaking is incomplete. The WebSocket `session_started` message is the authoritative signal that active rendering may begin.

The public config contains only experiment lifecycle status, optional institution and participant-information references, consent copy, and the narrow `voice.enabled` capability. Pairing, capacity, provider selection, storage, privacy, and lifecycle limits remain server-owned.

## Versioning

Every server WebSocket message has this envelope:

```ts
{ protocol_version: 1, type: string, ...payload }
```

A client must reject an unsupported `protocol_version`; it must not guess field meanings.

## Server messages

The following variants are exhaustive for protocol version 1. Fields shown as `null` are present, not omitted.

### Session start

```json
{
  "protocol_version": 1,
  "type": "session_started",
  "public_session_id": "room-123",
  "role": "A",
  "observation": { "turn": 1 },
  "available_actions": []
}
```

`role` is exactly `"A"` or `"B"`. This message is targeted to its recipient; it carries no transport identity.

### Accepted transition

```json
{
  "protocol_version": 1,
  "type": "transition",
  "public_session_id": "room-123",
  "actor": "B",
  "action": { "type": "choose", "value": 3 },
  "observation": { "turn": 2 },
  "available_actions": null
}
```

Replace the current observation and action affordance with these values. `actor` identifies who caused the transition, while `action` is the accepted game-specific action delivered to both roles. The `observation` remains role-specific and authoritative for what this recipient may know about state. Delivery of an action does not require the frontend to display it.

### Player message

```json
{
  "protocol_version": 1,
  "type": "message",
  "public_session_id": "room-123",
  "message": {
    "id": "msg-123",
    "sender": "B",
    "text": "Your turn",
    "input": "text",
    "created_at": "2026-08-16T09:00:00Z"
  }
}
```

`input` is `"text"` or `"voice_transcript"`. It describes the input channel without revealing whether a role is human or automated. A message communicates only between players and does not imply a game transition.

### Presence

```json
{
  "protocol_version": 1,
  "type": "presence",
  "public_session_id": "room-123",
  "presence": {
    "A": { "connected": true, "audioReady": true },
    "B": { "connected": false, "audioReady": false }
  }
}
```

Presence is keyed only by `A` and `B`. It contains no participant or controller identity.

### Voice readiness

```json
{
  "protocol_version": 1,
  "type": "voice_status",
  "public_session_id": "room-123",
  "voice": {
    "audioReady": true,
    "transcriptionReady": true,
    "transcriptionStatus": "Ready"
  }
}
```

These are narrow readiness values, not provider configuration.

### Completion and abandonment

```json
{
  "protocol_version": 1,
  "type": "completed",
  "public_session_id": "room-123",
  "completion": {
    "winner": "A",
    "player_scores": { "A": 7, "B": 5 }
  }
}
```

```json
{
  "protocol_version": 1,
  "type": "abandoned",
  "public_session_id": "room-123",
  "code": "participant_left"
}
```

`completion` is the game's structured `Game::Completion`, shared with both roles for public terminal facts such as win/loss, the winner, and scores. Parlando does not impose a universal result schema. Role-private terminal facts remain in that role's final observation. An abandonment code is presentation-neutral.

### Rejections and failures

```json
{
  "protocol_version": 1,
  "type": "action_rejected",
  "public_session_id": "room-123",
  "code": "wrong_role"
}
```

```json
{
  "protocol_version": 1,
  "type": "error",
  "public_session_id": "room-123",
  "code": "invalid_action",
  "fatal": false
}
```

`action_rejected` represents an expected game-rule or session-state rejection. `error` represents transport or runtime failure. Both are targeted to the player whose input caused them; a peer does not receive another player's malformed-input diagnostics. Codes are stable machine-readable identifiers; clients own human wording and localization.

## Client messages

The game WebSocket accepts:

```json
{ "type": "ready" }
```

```json
{
  "type": "action",
  "action": { "type": "choose", "value": 3 }
}
```

```json
{
  "type": "message",
  "text": "I can take the left side"
}
```

```json
{ "type": "heartbeat" }
```

```json
{ "type": "leave" }
```

`ready` declares transport readiness, `heartbeat` maintains transport liveness without changing meaningful session activity, and `leave` intentionally abandons the session. A player message does not call `Game::apply_action`; an agent may react by later submitting a separate typed action.

These five variants are the complete client protocol. Consent is submitted through the HTTP lifecycle, not through the game WebSocket.

## React session contract

`ParticipantApp` supplies a `GameSession<Observation, Action, Completion>` containing:

- `sessionId`, `role`, `observation`, nullable `transition`, and nullable `availableActions`;
- `conversation` as `PlayerMessage[]` and role-keyed `presence`;
- narrow voice status and capability values;
- `connected`, `completed`, and nullable `completion`; and
- `sendAction`, `sendMessage`, `setMicrophoneMuted`, and `leave`.

`transition` contains the most recent accepted `actor` and `action`; it is `null` for an initial or resynchronized observation. The session contains neither authoritative state nor generic game events. Render human labels, instructions, accessibility text, and animation in the participant application from structured domain values.

## Compatibility map from the old API

| Old | Version 1 clean protocol |
|---|---|
| `roleAssigned` | `session_started` |
| `stateChanged` | `transition` |
| `conversationMessageAdded` | `message` |
| `presenceChanged` | `presence` |
| `voiceStatusChanged` | `voice_status` |
| `partnerWaiting` / `waiting` | Removed; derive waiting from room state and presence |
| client `submitAction` | client `action` |
| client `sendChatMessage` | client `message` |
| `participant_session_id` in participant payloads | Removed |
| `state`, `events`, and conversation snapshots | Removed |
| `summary` / `completionSummary` | `completion` |
