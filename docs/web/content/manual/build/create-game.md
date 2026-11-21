---
title: Build a dialogue game
description: Specify a two-player task, create its Rust mechanics and React interface, start the server, and verify a local experiment.
---

# Build a dialogue game

The previous chapters ran a compiled game. This chapter constructs one. A finished Parlando game
packages a Wasm component that owns the task and browser assets that present
one player's view. They share the same action and observation shapes, while Parlando supplies participant entry,
pairing, sessions, administration, persistence, and export.

The Great Tree remains the running illustration. Crown sees limbs and controls sunlight; Root sees
roots and controls water; a hidden mapping connects them. The mapping must stay on the server, so
the example exposes the central design problem: deciding what the task knows, what each role sees,
and what each role may do.

## 1. Write the task contract

Before creating files, write down six facts:

1. What shared outcome ends the game?
2. What are the two domain roles, and which information is private to each?
3. Which structured actions can each role submit?
4. When must an action be rejected?
5. What does each role learn after an accepted action?
6. Which terminal facts must be stored for analysis and shown to both roles?

For Great Tree, the shared outcome is three flowering limbs. Crown may set sunlight on a named limb;
Root may open or close a named root. The root-to-limb mapping is private server state. Crown's
observation contains limbs but no roots or mapping; Root's observation contains roots but no limbs
or mapping. Completion contains the flowering limbs because that result is safe for both roles.

Keep administration out of this contract. Recruitment, consent, timeouts, human or agent pairing,
voice, and Prolific belong to an experiment configured after the game runs. They do not belong in
the mechanics.

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

## Build the participant application

Mirror the serialized Rust `Observation`, `Action`, and `Completion` shapes in TypeScript. The
browser never receives `State` or provider credentials. Render the active game inside
`ParticipantApp`:

```tsx
import {
  ParticipantApp,
  type GameSnapshot,
} from "@coli-saar/parlando-client/react";

type Snapshot = GameSnapshot<MyObservation, MyAction>;

function GameView({ snapshot }: { snapshot: Snapshot }) {
  return (
    <main>
      <ObservationView observation={snapshot.observation} />
      <button
        disabled={!snapshot.interactionEnabled}
        onClick={() => snapshot.act({ type: "ready" })}
      >
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

`ParticipantApp` handles participant information, declarations, the waiting countdown, readiness,
reconnection, voice preparation, terminal handoff, and the one-minute human–human voice period after
normal completion. Your view reads `snapshot.observation` and sends actions with
`snapshot.act`. Disable every task control when `interactionEnabled` is false. Parlando owns the
conversation, microphone, connection, and leave controls surrounding the game surface.

Great Tree switches between `CrownView` and `RootView` on the observation's `role` tag. Crown buttons
send `setSun`; Root buttons send `setFlow`. Neither view knows the hidden mapping.

## Verify the public boundary

Mechanics tests should establish deterministic initialization, every legal transition, expected
rejection codes, both observation projections, and completion boundaries. Serialize both
observations in tests and assert that private field names and values are absent.

Interface tests should render each observation variant, inspect the action emitted by every control,
and confirm that controls are disabled during disconnection and after completion. A two-browser
pilot then verifies the complete participant and data-collection path.

Do not treat a plausible screen as sufficient evidence. A browser can look correct while receiving
private fields, sending the wrong action shape, or presenting a completion that was never stored.
The Rust tests establish task meaning; the UI tests establish presentation; the pilot establishes
their integration.

The next chapter attaches an automated player to this same observation and action boundary. The
[API reference](../reference/game-api/) lists optional methods and detailed signatures when the
basic implementation above needs an extension.
