---
title: Build a dialogue game
description: Specify a two-player task, create its Rust mechanics and React interface, start the server, and verify a local experiment.
---

# Build a dialogue game

The previous chapters ran a compiled game. This chapter constructs one. A finished Parlando game
has two build products: a Rust executable that owns the task and a set of browser assets that present
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

## 2. Create the project

For a new game, use the repository's `generate-parlando-game` skill. Describe the task contract,
not server internals. For example:

```text
Use $generate-parlando-game to create a two-player tree-tending task.
Crown sees five limbs and controls sunlight. Root sees five roots and controls
water. A hidden one-to-one mapping connects roots to limbs. Both roles must
coordinate to make three limbs flower. Reject controls used by the wrong role.
Include typed communication and deterministic mechanics tests.
```

The generated project contains the same responsibilities as this conceptual layout:

```text
my-game/
├── server/
│   ├── Cargo.toml
│   ├── src/
│   │   ├── game/       Rust domain types, mechanics, Game, and GameFactory
│   │   └── main.rs     game metadata and Parlando server startup
│   └── tests/
├── client/
│   ├── package.json
│   └── web/src/
│       ├── game/       TypeScript game types and role views
│       └── App.tsx     ParticipantApp and the active game screen
└── deployment files
```

The exact file names may differ. The division of responsibility must not: Rust owns task truth;
React owns presentation. The Rust crate depends on `parlando = "0.4.2"`, and the participant
application depends on `@coli-saar/parlando-client` 0.4 and React 19. Keep the Parlando minor
versions aligned.

## 3. Define the Rust domain types

The game exposes five serializable types:

| Type | Purpose |
| --- | --- |
| `Config` | Game variation chosen for an experiment, such as which seat controls Crown |
| `State` | Complete authoritative situation, including private facts |
| `Action` | A player's structured proposal |
| `Observation` | Everything safe to send to one role |
| `Completion` | The shared structured terminal result |

Define the types before writing the mechanics or interface. In Great Tree, `State` contains the hidden
root-to-limb mapping. `Observation` is an enum with separate Crown and Root variants, so a Crown
observation cannot accidentally contain a root field. `Action` is a tagged enum whose serialized
forms resemble:

```json
{ "type": "setSun", "limb": "spire", "lit": true }
{ "type": "setFlow", "root": "hand", "open": true }
```

Use stable domain names in these values. Do not encode button positions, CSS classes, animation
commands, or participant-facing prose as actions. A different interface or an agent must be able to
use the same contract.

## 4. Implement the mechanics and `Game`

Keep state transitions in pure functions where possible, then expose them through `parlando::Game`:

```rust
use anyhow::Result;
use parlando::{ActionRejection, Game, GameInitializationContext, PlayerRole};

impl Game for MyGame {
    type Config = MyConfig;
    type State = MyState;
    type Action = MyAction;
    type Observation = MyObservation;
    type Completion = MyCompletion;

    fn initial_state(
        &self,
        context: GameInitializationContext<'_, Self::Config>,
    ) -> Result<Self::State> {
        initial_state(context.config, context.seed)
    }

    fn apply_action(
        &self,
        state: &Self::State,
        action: &Self::Action,
        actor: PlayerRole,
    ) -> std::result::Result<Self::State, ActionRejection> {
        apply_action(state, action, actor)
    }

    fn observation(&self, state: &Self::State, role: PlayerRole) -> Self::Observation {
        observation(state, role)
    }

    fn completion(&self, state: &Self::State) -> Option<Self::Completion> {
        completion(state)
    }
}
```

`initial_state` receives the experiment's typed game configuration and a recorded seed. Derive all
randomized initial facts from that seed and retain the results in `State`. The same configuration,
seed, and accepted action sequence should reproduce the same trajectory.

`apply_action` must check the actor and every state-dependent rule before returning a new state. An
expected refusal returns an `ActionRejection` with a stable code such as `wrong_role` or
`root_frozen`. The browser may disable an illegal control, but server validation remains necessary
because browsers and agents are not authorities.

`observation` is the information boundary. Return a value that may safely be serialized and sent in
full. Never send the complete state and rely on React to hide private fields. Recompute the
observation for each role after every accepted action.

`completion` returns `None` while play continues and a shared value when the task ends. Put private
terminal information in the final role observation, not in completion.

## 5. Construct one game for each session

Implement `GameFactory` to validate the typed configuration and construct a session-local game:

```rust
use anyhow::Result;
use parlando::{GameFactory, GameSessionContext};

#[derive(Clone, Copy)]
struct MyGameFactory;

impl GameFactory for MyGameFactory {
    type Game = MyGame;

    fn create(&self, context: GameSessionContext) -> Result<MyGame> {
        Ok(MyGame::new(context.logger))
    }

    fn validate_config(&self, config: &MyConfig) -> Result<()> {
        validate_config(config)
    }
}
```

The factory creates a separate game value for each session. Validation runs when an administrator
checks or saves the experiment's game YAML, so reject impossible configurations there rather than
waiting for a participant to enter.

## 6. Start the Parlando server

The executable identifies the compiled game, selects persistent storage and the participant
application, registers optional compiled agents, and starts the server:

```rust
let metadata = parlando::GameMetadata {
    id: "my-game".into(),
    name: "My Game".into(),
    version: env!("CARGO_PKG_VERSION").parse()?,
    build_manifest: serde_json::json!({
        "name": env!("CARGO_PKG_NAME"),
        "version": env!("CARGO_PKG_VERSION")
    }),
};

parlando::Server::new(MyGameFactory, metadata)?
    .database_url("sqlite:///./my-game.sqlite")
    .participant_app("../client/dist")
    .serve("127.0.0.1:8080".parse()?)
    .await?;
```

Do not construct experiments, consent, pairing, speech providers, or routes in this executable.
Once `/admin` opens, those settings belong to the dashboard and its versioned records.

Increase the Cargo package version when rules, action meaning, information structure, or completion
semantics change. Existing experiments remain attached to the game version that interpreted their
sessions.

## 7. Build the participant application

Mirror the serialized Rust `Observation`, `Action`, and `Completion` shapes in TypeScript. The
browser never receives `State` or provider credentials. Render the active game inside
`ParticipantApp`:

```tsx
import {
  ParticipantApp,
  type GameSession,
} from "@coli-saar/parlando-client/react";

type Session = GameSession<MyObservation, MyAction, MyCompletion>;

function GameView({ session }: { session: Session }) {
  return (
    <main>
      <ObservationView observation={session.observation} />
      <button
        disabled={!session.interactionEnabled}
        onClick={() => session.sendAction({ type: "ready" })}
      >
        Ready
      </button>
    </main>
  );
}

export default function App() {
  return (
    <ParticipantApp<MyObservation, MyAction, MyCompletion>
      renderGame={(session) => <GameView session={session} />}
      renderCompletion={(completion) => <Result value={completion} />}
    />
  );
}
```

`ParticipantApp` handles participant information, declarations, waiting, readiness, reconnection,
voice preparation, and terminal handoff. Your view reads `session.observation` and sends actions with
`session.sendAction`. Disable every task control when `interactionEnabled` is false. If the game
supports typed dialogue, render `session.conversation` and send text with `session.sendMessage`.

Great Tree switches between `CrownView` and `RootView` on the observation's `role` tag. Crown buttons
send `setSun`; Root buttons send `setFlow`. Neither view knows the hidden mapping.

## 8. Build and run the two components

For the layout above, build the browser assets first and then start the server:

```sh
cd my-game/client
npm install
npm test
npm run build

cd ../server
cargo fmt --check
cargo test
cargo run -- --port 8080 --client-dist ../client/dist
```

Use the exact run command emitted by the generation skill when its layout differs. A successful
start leaves the Rust process running and makes `/admin` available. Create the administrator, then
follow [Run a local pilot](../start/first-experiment/) with this game. Enter game-specific YAML under
**Configuration → Game configuration** before saving the experiment revision.

## 9. Verify the public boundary

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
