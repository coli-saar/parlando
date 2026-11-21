---
title: Rust and React game APIs
---

# Rust and React game APIs

## Implement the session in Rust

The type implementing `parlando_game_sdk::Game` owns the private state of one session. Derive `GameValue` for the domain values that cross the boundary, then call `parlando_game_sdk::export!(MyGame)` in the game crate. The generated adapter describes the game's schema and encodes the results of each call. The host compiles the module once and creates a separate instance for each session.

| SDK method | Game responsibility |
| --- | --- |
| `init(config, seed)` | Validate game-specific settings and create a reproducible initial state. |
| `step(&mut self, actor, action)` | Check the actor and rules before mutating state. Return a stable rejection code on an illegal action. |
| `view_a(&self)`, `view_b(&self)` | Project only what each seat may see. The associated view types can differ and can borrow from the state. |
| `completion(&self)` | Return a shared terminal result when the session ends. |
| `metadata(&self)` | Supply optional role-neutral analysis data for an accepted transition. |
| `checkpoint(&self)`, `restore(bytes)` | Optionally save and restore private session state. With deterministic initialization and actions the host can also recover by replay. |

Initialization and every accepted step return both observations in one frame. Live players and learning agents consume these same observations. If controls need a list of available choices, include it in the appropriate role's view. The game still checks every submitted action.

## Build the React participant application

Import `ParticipantApp` from `@coli-saar/parlando-client/react`. It manages participant information,
consent, waiting, readiness, reconnection, voice preparation, and terminal handoff before and after
your game screen.

```tsx
import {
  ParticipantApp,
  type GameSnapshot,
} from "@coli-saar/parlando-client/react";

type Snapshot = GameSnapshot<MyObservation, MyAction>;

function GameView({ snapshot }: { snapshot: Snapshot }) {
  return (
    <main>
      <pre>{JSON.stringify(snapshot.observation, null, 2)}</pre>
      <button disabled={!snapshot.interactionEnabled} onClick={() => snapshot.act({ type: "ready" })}>
        Ready
      </button>
    </main>
  );
}

export default function App() {
  return (
    <ParticipantApp<MyObservation, MyAction, MyCompletion>
      renderGame={(snapshot) => <GameView snapshot={snapshot} />}
      renderCompletion={(completion) => <Result value={completion} />}
    />
  );
}
```

`GameSnapshot` provides the current role and observation, the most recent accepted transition,
optional available actions, interaction status, and `act`. Disable controls when
`interactionEnabled` is false. Parlando renders and operates conversation, connection,
microphone, leave, and replay widgets outside the game surface. The client never receives
authoritative state or provider credentials.

Great Tree switches its entire game view on the observation's `role` field. Crown buttons send
`setSun` actions; Root buttons send `setFlow` actions. Its custom completion renderer turns the
shared list of flowering limbs into a success screen.

## Test the public boundary

At minimum, test:

- configuration validation and deterministic seeded initialization;
- every action's role authorization and invalid-state rejection;
- both observation projections, including absence of opponent-private information;
- completion and any role-neutral transition metadata;
- the React renderer using observations rather than authoritative state;
- controls while interaction is disabled or reconnecting; and
- communication delivery without a game-state transition.

Run server and client tests before each data-collection deployment. The files under
`games/great-tree/server` and `games/great-tree/client/web` provide complete examples.
