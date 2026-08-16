# Building a game

A Parlando game has three independent parts: typed Rust mechanics, a participant application, and optional agents. The runtime owns authoritative state and experiment policy. Start with [the design principles](design-principles.md); use [the migration guide](migrating-to-clean-api.md) for a 0.2 game.

## Implement `Game`

Define serializable `Config`, `State`, `Action`, `Observation`, and `Completion` types, then implement `parlando::Game`. `Action` and `Completion` must also implement `Clone` because the runtime delivers each accepted action and terminal result to both controllers.

```rust
use anyhow::Result;
use parlando::{ActionRejection, Game, PlayerRole};

#[derive(Clone)]
pub struct MyGame;

impl Game for MyGame {
    type Config = MyConfig;
    type State = MyState;
    type Action = MyAction;
    type Observation = MyObservation;
    type Completion = MyCompletion;

    fn validate_config(&self, config: &Self::Config) -> Result<()> {
        validate_config(config)
    }

    fn initial_state(&self, config: &Self::Config, seed: u64) -> Result<Self::State> {
        initial_state(config, seed)
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

    fn available_actions(
        &self,
        state: &Self::State,
        role: PlayerRole,
    ) -> Option<Vec<Self::Action>> {
        available_actions(state, role)
    }

    fn completion(&self, state: &Self::State) -> Option<Self::Completion> {
        completion(state)
    }
}
```

`State` is authoritative and private to the runtime. `Observation` is the complete ongoing domain information visible to one role. After an accepted action, human players and agents both receive the actor, the shared typed action, and their own resulting observation. Test both observation projections for private-information leaks. If the cause of a transition must remain hidden, use a non-revealing action value such as a `SecretAction` variant and put only permitted consequences in each observation.

`Completion` is the shared structured terminal payload sent to both roles and stored for dashboards and exports. It may contain public facts such as winner and scores. Put role-private terminal facts in the final role-specific observation instead.

`apply_action` must check the actor and every state-dependent rule before returning a new state. Use a stable `ActionRejection` code for an expected rejection. The method should be deterministic and perform no I/O.

`available_actions` is optional. `None` means the action space is not enumerated; `Some(vec![])` means it is enumerated and currently empty. It never replaces `apply_action` validation.

When the dashboard or exported log needs game-specific analysis fields, implement `transition_metadata`. Return only structured, role-neutral domain data. Participant prose and animation belong in the participant application.

## Run the server

```rust
use parlando::{GameMetadata, Server};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let metadata = GameMetadata {
        id: "my-game".into(),
        name: "My Game".into(),
        version: env!("CARGO_PKG_VERSION").parse()?,
        build_manifest: serde_json::json!({}),
    };

    Server::new(MyGame, metadata)?
        .database_url("sqlite:///./my-game.sqlite")
        .participant_app("../client/dist")
        .serve("127.0.0.1:3000".parse()?)
        .await
}
```

Register optional factories with `.agent(factory)?`. The server builder deliberately does not expose pairing, consent, privacy, session limits, transcription, TTS, or participant information and consent copy. Administrators configure those per experiment in the dashboard.

Serving participant assets is optional. The HTTP/JSON/WebSocket protocol does not depend on React or on the JavaScript package, so a custom frontend can use the same server. Today, browser deployments use same-origin assets or place the frontend and server behind one reverse-proxy origin. A focused cross-origin allowlist on `Server` is deferred work; the protocol itself does not require co-hosting.

## Build a React participant application

```tsx
import { ParticipantApp, type GameSession } from "@coli-saar/parlando-client/react";

function GameView({ session }: { session: GameSession<MyObservation, MyAction, MyCompletion> }) {
  return (
    <main>
      <pre>{JSON.stringify(session.observation, null, 2)}</pre>
      <button onClick={() => session.sendAction({ kind: "ready" })}>Ready</button>
    </main>
  );
}

export default function App() {
  return <ParticipantApp renderGame={(session) => <GameView session={session} />} />;
}
```

The game screen uses `observation`, optional `transition`, `availableActions`, `conversation`, `presence`, the shared `completion`, and narrow voice properties. It sends typed actions with `sendAction` and player communication with `sendMessage`. `transition` identifies the most recent accepted actor and action; a frontend that only needs the resulting state can ignore it. The screen never receives authoritative state or generic presentation events.

A non-React application can use `ParticipantClient` from the package root or implement the documented [client protocol](client-protocol.md).

## Test the boundary

At minimum, test:

- configuration validation and deterministic seeded initialization;
- every action's role authorization and invalid-state rejection;
- observations for A and B, including absence of private opponent data;
- completion and role-neutral transition metadata;
- the participant application using observations only; and
- message delivery without a game-state transition.
