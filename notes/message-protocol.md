# Participant message protocol design note

**Status:** Implemented design note. The user-facing wire reference is
[`docs/client-protocol.md`](../docs/client-protocol.md); the Rust definitions in
[`rust-server/src/protocol.rs`](../rust-server/src/protocol.rs) are authoritative
for serialization.

## Purpose and boundary

The participant message protocol connects a frontend to one running two-player
session. It carries player operations to the Rust server and role-specific game
information back to the frontend. It does not expose the server's authoritative
state or assume that the frontend uses React, a browser, or any particular visual
presentation.

The protocol has three distinct domain channels:

- An **action** proposes a game-state change. The server validates it through
  `Game::apply_action`, records the accepted action, and delivers the accepted
  value to both player roles.
- An **observation** is the complete game information visible to one role. It is
  delivered beside the shared action after a transition.
- A **player message** communicates text between the two players. It does not
  invoke game mechanics or change state by itself.

At termination, the game supplies a shared `Game::Completion` value. This is an
opaque game-specific result, not a Parlando result schema. It may contain public
facts such as win/loss, winner, or scores. A terminal fact that is private to one
role must instead appear in that role's final observation.

## Transport shape

The game channel is an authenticated JSON WebSocket obtained through the HTTP
lifecycle described in the user-facing protocol reference. Client messages are
tagged by `type`. Every server message contains `protocol_version`, currently
`1`, beside its `type` and payload.

Roles are exactly `"A"` and `"B"`. Protocol messages never identify whether a
role is controlled by a human or an agent.

## Client-to-server messages

These five variants are the complete client protocol.

| Type | Fields | Meaning and effect |
|---|---|---|
| `ready` | none | Declares that this participant's game channel is ready. It may allow the session to start when the other requirements are satisfied. |
| `action` | `action` | Submits one game-specific action value. An accepted action may change authoritative state; an invalid action produces a targeted rejection or error. |
| `message` | `text` | Sends text through the player-to-player communication channel. It never calls `Game::apply_action`. |
| `heartbeat` | none | Maintains transport liveness. It is not stored as research activity and does not extend meaningful session activity. |
| `leave` | none | Intentionally leaves and abandons the session. |

Consent is not a game-channel message. The participant submits consent decisions
through the authenticated HTTP endpoint before joining a session.

## Server-to-client messages

| Type | Delivery | Main fields | Meaning |
|---|---|---|---|
| `session_started` | Targeted to one role | `public_session_id`, `role`, `observation`, `available_actions` | Supplies the first complete role-specific observation and starts active rendering. |
| `transition` | Targeted separately to each role | `public_session_id`, `actor`, `action`, `observation`, `available_actions` | Delivers the shared accepted action and replaces that recipient's role-specific observation. |
| `message` | Session participants | `public_session_id`, `message` | Delivers one player message with sender role, text, input channel, identifier, and timestamp. |
| `presence` | Session participants | `public_session_id`, `presence` | Reports narrow connectivity and audio readiness keyed by `A` and `B`. |
| `voice_status` | Session participants | `public_session_id`, `voice` | Reports narrow audio and transcription readiness without provider configuration. |
| `completed` | Both roles | `public_session_id`, `completion` | Delivers the same shared, game-specific terminal result to both players. |
| `abandoned` | Both roles | `public_session_id`, `code` | Reports that a participant intentionally ended the session. |
| `action_rejected` | Targeted to the submitter | `public_session_id`, `code` | Reports an expected game-rule or session-state rejection without ending the session. |
| `error` | Normally targeted to the affected participant | `public_session_id`, `code`, `fatal` | Reports malformed input, transport failure, or runtime failure using presentation-neutral fields. |

`available_actions` is nullable. `null` means that the game does not enumerate
its action space. An empty array means that it does enumerate the action space
and currently offers no actions. Neither value authorizes an action;
`Game::apply_action` remains authoritative.

## Accepted-action sequence

For an accepted non-terminal action, the runtime:

1. validates the typed action and computes the next authoritative state;
2. persists the accepted action, transition metadata, and configured state data;
3. updates the runtime state;
4. queues `observe_transition(actor, action, observation)` with the resulting
   role-specific observation for each Rust or remote agent;
5. sends each participant client a `transition` containing the actor, accepted
   action, and that role's new observation.

The participant and agent interfaces carry the same information. The frontend
may use the action for animation or history, or ignore it and render only the
observation. The action value itself is shared player information; games conceal
a cause by using a non-revealing action value such as `SecretAction`.

For an action that ends the game, the runtime first sends the final role-specific
`transition`, then sends `completed` with the same shared completion payload to
both roles and calls each agent's `finish(completion)` before resource shutdown.
This ordering lets every player process the final observation before the result.

```text
client A                    Rust runtime                    clients A and B
   | action(a)                   |                                |
   |---------------------------->|                                |
   |                        validate + persist                    |
   |                        update state                          |
   |                        project observation(A/B)              |
   |                             |---- transition(a, obs A) ----->|
   |                             |---- transition(a, obs B) ----->|
   |                             |---- completed(result) -------->|
```

## Player-message sequence

A player `message` is persisted and delivered as communication. It does not
produce a game transition. If the other role is controlled by an agent, the
runtime invokes `observe_message(sender, text)`. The agent may later return a
separate action from `respond`, but receiving the message itself cannot change
game state.

## Information that must not cross the boundary

The participant protocol does not carry:

- authoritative `Game::State`;
- generic game events or presentation instructions;
- participant-session identifiers or peer controller type;
- pairing, capacity, lifecycle limits, or other dashboard configuration;
- speech-provider configuration or credentials; or
- storage records and internal diagnostics.

Human-readable labels, rejection explanations, terminal prose, animation, and
localization belong to the frontend. Protocol error and abandonment codes remain
machine-readable identifiers.

## Review checklist

- Each client variant has one purpose and no optional legacy operation fields.
- Both roles receive the same accepted action and independently projected
  observations.
- Both roles receive observations projected independently from the same
  authoritative state.
- Human players and agents receive the same completion payload.
- Role-private terminal information appears only in the final observation.
- A player message cannot call game mechanics or mutate game state.
- Presence does not reveal whether a role is human or automated.
- Consent remains in the HTTP lifecycle rather than the game WebSocket.
- Rust server behavior does not depend on React or the JavaScript client.
