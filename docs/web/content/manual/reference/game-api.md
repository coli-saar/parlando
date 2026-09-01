---
title: Rust and React game APIs
---

# Rust and React game APIs

A Parlando game combines deterministic Rust mechanics with a participant-facing browser
application. Parlando owns sessions, pairing, storage, consent, and communication; the game
owns task state, actions, role-specific observations, and completion. Great Tree is the worked
example for both APIs.

Start with [Create a game](../build/create-game/) when designing a task. This chapter is the compact
API reference for implementing that design.

## Define the Rust mechanics

Implement `parlando::Game` with serializable types for configuration, authoritative state, actions,
role-specific observations, and shared completion:

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

`State` is authoritative and is never sent to a participant. `Observation` is the complete task
information visible to one role. Great Tree's state contains the hidden mapping between limbs and
roots, while its Crown observation contains only limbs and its Root observation only roots. Test
the serialized observations, not merely the browser, for private-information leaks.

`apply_action` must verify the actor and every state-dependent rule before returning a new state.
Use a stable `ActionRejection` code for an expected refusal. The operation should be deterministic
and perform no I/O. Initialize stochastic state from the recorded `seed` and retain the result in
the authoritative state.

`Completion` is a shared structured terminal value delivered to both roles and retained for
analysis. Include only facts safe for both participants. Put role-private terminal facts in the
final role-specific observation.

### Optional mechanics methods

`available_actions` may enumerate a role's current action affordances. `None` means the action space
is not enumerated; `Some(vec![])` means the game enumerates it and none are currently available.
Every submitted action still passes through `apply_action`.

`transition_metadata` may attach role-neutral structured analysis fields to an accepted transition.
Do not put viewer-specific prose or animation instructions there. Parlando already records the
actor and action.

`GameInitializationContext` also contains experiment-owned game secrets. Secret values are exposed
only to trusted compiled game code, have redacted debug output, and are not sent to clients. Avoid
using a secret when ordinary typed configuration is sufficient.

## Construct one game per session

Implement `GameFactory` to create a session-local game value and to validate semantic constraints
on the typed game configuration:

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

The factory creates a separate game value for each session. `GameSessionContext` also supplies a
session logger when the game needs task-specific diagnostic messages.

## Start the server

Give the compiled game a stable ID, participant-facing name, semantic version, and build manifest.
Then attach the participant build and optional agent factories:

```rust
use parlando::{GameMetadata, Server};

let metadata = GameMetadata {
    id: "my-game".into(),
    name: "My Game".into(),
    version: env!("CARGO_PKG_VERSION").parse()?,
    build_manifest: serde_json::json!({
        "name": env!("CARGO_PKG_NAME"),
        "version": env!("CARGO_PKG_VERSION")
    }),
};

Server::new(MyGameFactory, metadata)?
    .database_url("sqlite:///./my-game.sqlite")
    .participant_app("../client/dist")
    .serve("127.0.0.1:8000".parse()?)
    .await?;
```

Register an in-process or remote agent with `.agent(factory)?`. Do not encode pairing, consent,
privacy, lifecycle limits, transcription, or speech synthesis in this builder; administrators
configure those per experiment.

## Build the React participant application

Import `ParticipantApp` from `@coli-saar/parlando-client/react`. It manages participant information,
consent, waiting, readiness, reconnection, voice preparation, and terminal handoff before and after
your game screen.

```tsx
import {
  ParticipantApp,
  type GameSession,
} from "@coli-saar/parlando-client/react";

type Session = GameSession<MyObservation, MyAction, MyCompletion>;

function GameView({ session }: { session: Session }) {
  return (
    <main>
      <pre>{JSON.stringify(session.observation, null, 2)}</pre>
      <button onClick={() => session.sendAction({ type: "ready" })}>
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

`GameSession` provides the current role and observation, the most recent accepted transition,
optional available actions, conversation, presence, connection state, voice controls, and the
methods `sendAction`, `sendMessage`, and `leave`. Disable controls when `interactionEnabled` is
false. The client never receives authoritative state or provider credentials.

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
