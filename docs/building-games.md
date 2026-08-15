# Building a game

A Parlando game is a purpose-built browser application together with the Rust
mechanics that drive it. The browser application gives participants the concrete
world in which they interact: it may contain maps, cards, diagrams, documents,
animations, chat, audio, or any other web interface suited to the study. The Rust
game crate defines how that world changes and what each role may observe.

Start with the research design before writing code. Sketch the participant
experience, identify what each role knows and can do, decide how progress and
completion should appear, and list the outcomes needed for analysis. These
choices become the client interface and the game's `State`, `Action`,
`Observation`, `Event`, and `Summary` types.

## Design the browser game

The browser game owns all game-specific presentation and interaction. You can use
React, another web framework, Canvas, SVG, WebGL, or ordinary HTML controls. The
included Space Game uses React, but its station layout and controls are an
example, not a template that other games must resemble.

Most projects use `@coli-saar/parlando-client` for participant setup, consent,
matchmaking, WebSockets, chat, and optional audio. Its React startup gate can
carry participants through those shared stages and then hand the assigned role,
observation, events, and connection to the custom game UI. Projects that need a
different setup experience can use the lower-level SDK helpers or the documented
protocol directly.

Keep game-specific assets, language, visual state, and interaction logic in the
game client. Define TypeScript types for every game value that crosses the
browser boundary, and render the participant's role-specific `Observation`.

![Space Game participant interface showing a custom map, controls, role-specific
information, events, and communication](images/space-game-interface.jpg)

## Define the adapter contract

Every game implements `GameAdapter` from `rust-server/src/game.rs`.

```rust
pub trait GameAdapter {
    type State;
    type Action;
    type Observation;
    type Event;
    type Summary;

    fn initial_state(&self) -> Self::State;
    fn parse_action(&self, action: serde_json::Value) -> anyhow::Result<Self::Action>;
    fn validate_action(&self, state: &Self::State, action: &Self::Action, player: PlayerRole) -> anyhow::Result<()>;
    fn apply_action(&self, state: &Self::State, action: &Self::Action) -> anyhow::Result<Self::State>;
    fn observe_state(&self, state: &Self::State, player: PlayerRole) -> Self::Observation;
    fn available_actions(&self, state: &Self::State, player: PlayerRole) -> Option<Vec<Self::Action>>;
    fn events_for_action(&self, before: &Self::State, after: &Self::State, action: &Self::Action, player: PlayerRole) -> Vec<Self::Event>;
    fn is_complete(&self, state: &Self::State) -> bool;
    fn completion_summary(&self, state: &Self::State) -> Self::Summary;
}
```

Most games should use serde-serializable Rust structs and enums for all associated types. The default `parse_action` implementation can deserialize JSON into the typed action enum, so games only need a custom parser when they have compatibility constraints.

The Rust types are only half of the contract. For every game-specific Rust type that crosses the browser boundary, define the matching TypeScript type and check the serialized JSON names. See [Browser Client Protocol](client-protocol.md) for concrete examples.

## Declare the compiled game

The server binary must identify the one game implementation it contains with a
`GameDescriptor`: a stable machine id, display name, semantic version, and build
manifest. Use the game crate's package version as the semantic version rather than
maintaining a second version string.

This version is operational, not merely diagnostic. New experiments are bound to
it, sessions record it, and a process refuses to activate experiments created for
another version. When game code changes, build a new binary and clone any
configuration that should be reused. Do not claim compatibility by reusing an old
version number.

The binary calls `serve_game`, passing the adapter, bootstrap settings,
`GameDescriptor`, listener address, and a factory that constructs configuration-
dependent runtime components such as agents. See
`space-game/server/src/main.rs` for the complete entry point.

## Keep the mechanics easy to test

Keep the adapter thin. Put game semantics in pure functions and call those functions from the adapter.

```rust
impl GameAdapter for MyGameAdapter {
    type State = MyState;
    type Action = MyAction;
    type Observation = MyObservation;
    type Event = MyEvent;
    type Summary = MySummary;

    fn initial_state(&self) -> Self::State {
        initial_state()
    }

    fn validate_action(&self, state: &Self::State, action: &Self::Action, player: PlayerRole) -> anyhow::Result<()> {
        validate_action(state, action, player)
    }

    fn apply_action(&self, state: &Self::State, action: &Self::Action) -> anyhow::Result<Self::State> {
        apply_action(state, action)
    }

    fn observe_state(&self, state: &Self::State, player: PlayerRole) -> Self::Observation {
        observe_state_for_player(state, player)
    }

    fn events_for_action(&self, before: &Self::State, after: &Self::State, action: &Self::Action, player: PlayerRole) -> Vec<Self::Event> {
        events_visible_to_player(before, after, action, player)
    }

    fn is_complete(&self, state: &Self::State) -> bool {
        state.finished
    }

    fn completion_summary(&self, state: &Self::State) -> Self::Summary {
        summarize(state)
    }
}
```

This makes most of the game testable without starting the HTTP server.

Use the browser freely for responsive interaction: it can gray out controls,
suggest likely actions, preview consequences, or animate possible moves. Keep the
same validation in the Rust mechanics so browsers and agents follow one rule set
and a stale client receives a clear rejection instead of changing the game
incorrectly.

## Follow an action through the game

The normal action path is:

1. A browser or agent proposes an action.
2. The server parses the JSON action into the typed game action.
3. The server calls `validate_action`.
4. If validation passes, the server calls `apply_action`.
5. The server persists the accepted action and the resulting state change.
6. The server computes role-specific observations and events.
7. The server sends targeted WebSocket messages to the affected participants.
8. If `is_complete` is true, the server persists and broadcasts `completion_summary`.

Agents use the same path. If a remote Python agent proposes an invalid action, the
game rejects it in the same way as an invalid browser action.

## Design observations and private information

`observe_state` is where you decide what each participant can see. For an
asymmetric-information experiment, build a role-specific observation that
contains exactly what the client needs. This gives the UI a clean model to render
and keeps the other role's private task information outside the browser.

In the demo game, each role sees shared device state but only its own private knowledge. The adapter filters private knowledge before sending observations to player A, player B, or an agent controlling either role.

A useful design rule is to omit fields that a participant should not use. CSS,
React state, and disabled controls can shape the interface, while the observation
itself establishes the information available to the role.

## Offer available actions when useful

`available_actions` is optional.

- Return `None` when the game does not provide an action list.
- Return `Some(vec![])` when the game does provide action hints but this player currently has no listed legal moves.
- Return `Some(actions)` when you want the UI or agent to receive a role-specific affordance.

Available actions are an interface affordance rather than a complete validator.
Keep `validate_action` complete so it also covers typed actions, stale clients,
and agents.

This is useful for experiments where an agent or UI can choose from a controlled set of choices. It is less useful for free-form text or continuous actions.

## Build a new game

A typical new game project follows this sequence:

1. Sketch the participant screens and interaction flow.
2. Define matching Rust and TypeScript forms of `Action`, `Observation`, `Event`,
   and `Summary`, plus the complete Rust `State`.
3. Write pure transition helpers for initial state, validation, state updates,
   observations, events, completion, and summary generation.
4. Implement `GameAdapter` by delegating to those helpers.
5. Build the browser game with the Parlando client package and your custom
   rendering, controls, and assets.
6. Add any in-process agents and a game-specific `factory_from_config`.
7. Create a binary like `space-game/server/src/main.rs` that declares a stable
   `GameDescriptor`, supplies process bootstrap settings, and calls `serve_game`.
8. Add tests for the mechanics and integration tests that create rooms, connect
   sockets, submit actions, and inspect exports.

## Game design checklist

Before implementing, write down:

- Roles: currently active player roles are `A` and `B`.
- Participant screens, controls, instructions, and completion experience.
- Participant-visible observation fields for each role.
- Private fields that must never be sent to the other role.
- Action schema and validation rules.
- Whether the UI should use server-provided `available_actions` or compute controls from the observation.
- Transition events that should appear in the participant log.
- Terminal conditions, including success, failure, timeout, or abandonment cases.
- Completion summary fields needed for analysis.
- Fields that should be exported for downstream analysis.

This checklist is also the natural prompt shape for the included `generate-parlando-game` skill: describe the game in these terms, generate a first adapter/client, then iterate.

## Reference files

- `rust-server/src/game.rs`: game adapter trait.
- `docs/client-protocol.md`: browser JSON contract.
- `space-game/server/src/game/state_engine.rs`: demo typed game model.
- `space-game/server/src/game/adapter.rs`: demo adapter.
- `space-game/server/src/main.rs`: demo binary.
- `space-game/client`: demo browser client.
