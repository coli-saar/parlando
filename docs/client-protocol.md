# Participant client protocol

Parlando exposes one participant lifecycle through both HTTP and the game WebSocket. The lifecycle is independent of transport state: losing a socket can pause a participant, but it does not itself mean that the participant left or that the session ended.

## Participant lifecycle

The complete participant-state inventory is:

```text
Registered -> Waiting -> Active <-> Paused
                  \         \          \
                   ----------> Ended <---
```

More precisely, `Waiting`, `Active`, and `Paused` may transition to `Ended`. `Ended` is absorbing. Repeated snapshots of the same state replace stale payload data; they are reconciliation, not lifecycle transitions.

- `Registered` means the authenticated participant has no session.
- `Waiting` contains the assigned session, role, and presence snapshot.
- `Active` additionally contains the role-specific observation and available actions.
- `Paused` retains the game view and supplies a structured pause reason. Meaningful game input is rejected.
- `Ended` contains one immutable, recipient-specific result, including any recruitment handoff.

Consent is an admission guard on `Registered -> Waiting`; it is not a participant lifecycle state.

## HTTP workflow

Read `GET /api/config` before participant entry. Its `communication.mode` selects the session channel. In `typed` mode the participant can enter directly and exchange text in active play. In `speech` mode the participant prepares the microphone before entry and obtains an authenticated audio plan. A typed experiment returns a disabled audio plan without an upgrade token.

1. `POST /api/participants` creates a participant credential.
2. `POST /api/consent` records required consent decisions.
3. `POST /api/sessions` joins or reuses a session and returns `{ "participant_state": ... }`.
4. `GET /api/participant-state` returns the authoritative snapshot at any time, including after live-room cleanup.
5. `POST /api/sessions/{public_session_id}/leave` atomically ends the session and returns the caller's `Ended` snapshot.

The leave request is idempotent. A client must wait for its HTTP response before closing the socket or navigating away. Disconnecting the WebSocket is not an explicit leave.

All participant-bound HTTP requests use `Authorization: Bearer <participant credential>`. Clients never submit a role or participant identifier as authority.

## Game WebSocket

Obtain a one-use connection plan from `POST /api/sessions/{public_session_id}/game-session`, then open the returned URL with its token. Server envelopes use `protocol_version: 2`.

The server begins every connection with a targeted `participant_state` snapshot. It sends another full snapshot whenever lifecycle changes. Accepted game actions may use compact `transition` deltas between snapshots. Other non-lifecycle payloads are `message`, `presence`, `voice_status`, `action_rejected`, and `error`.

Clients may send only:

- `ready`
- `action`
- `message` for participant-authored text in typed mode. Sending it in speech mode returns a nonfatal `typed_messages_disabled` error.
- `heartbeat`

Explicit leave is HTTP-only. Unknown protocol versions or message variants are errors.

## Session and participant terminal data

The shared session lifecycle is `forming | running | ended`; pause is an orthogonal availability fact. An ended session stores a structured cause and a result for each human role in the same transaction. Participant `Ended` is the role-specific projection of that shared value.

The dashboard exposes the durable `lifecycle` and `session_end` values. The research corpus derives its session-level `metadata.status` from those fields. See [Choose an export](data-and-monitoring.md#choose-an-export) for the v1 mapping.

## Client synchronization

The standard client tracks transport synchronization separately as `connecting | connected | reconnecting`. Game input is enabled only when participant state is `Active` and synchronization is `connected`. After a socket closes, the client reads `/api/participant-state` before reconnecting, so a terminal update cannot be lost in the close race.
