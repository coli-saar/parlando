# Building a game

A Parlando game has three independent parts: typed Rust mechanics, a participant application, and optional agents. The runtime owns authoritative state and experiment policy. Start with [the design principles](design-principles.md); use [the migration guide](migrating-to-clean-api.md) for a 0.2 game.

## Build and run the package

From a checkout, install the host and Rust build tools, then create a game:

```sh
cargo install --locked --path rust-server --bin parlando
rustup target add wasm32-unknown-unknown
cargo install --locked wasm-tools
parlando new my-game --lang rust
cd my-game
parlando test
parlando build
parlando serve
```

Edit `src/lib.rs` for mechanics, `client/` for the participant interface, and `scenarios.yaml` for host-level examples. The CLI reads `game.yaml` and packages the component with its participant assets. Open `/admin` on the local server to configure an experiment. A shared host accepts the same package via `parlando deploy`.

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

## Build a React participant application

```tsx
import { ParticipantApp, type GameSnapshot } from "@coli-saar/parlando-client/react";

function GameView({ snapshot }: { snapshot: GameSnapshot<MyObservation, MyAction> }) {
  return (
    <main>
      <pre>{JSON.stringify(snapshot.observation, null, 2)}</pre>
      <button disabled={!snapshot.interactionEnabled} onClick={() => snapshot.act({ kind: "ready" })}>Ready</button>
    </main>
  );
}

export default function App() {
  return <ParticipantApp renderGame={(snapshot) => <GameView snapshot={snapshot} />} />;
}
```

The game screen receives only its role-specific observation, optional accepted `transition`, interaction status, and the `act` function. `transition` identifies the most recent accepted actor and action; a frontend that only needs the resulting observation can ignore it. Parlando owns connection status, conversation, microphone controls, leaving, and replay controls. The game can style those widgets through their `parlando-*` classes and CSS custom properties, but it does not implement their behavior.

The same renderer is used for live play and replay. In replay, `interactionEnabled` is false and `act` has no effect. Parlando replaces snapshots from the live channel with snapshots reconstructed from the recorded initial state and accepted actions.

A non-React application can use `ParticipantClient` from the package root or implement the documented [client protocol](client-protocol.md).

## Test the boundary

At minimum, test:

- configuration validation and deterministic seeded initialization;
- every action's role authorization and invalid-state rejection;
- observations for A and B, including absence of private opponent data;
- completion and role-neutral transition metadata;
- the participant application using observations only; and
- message delivery without a game-state transition.
