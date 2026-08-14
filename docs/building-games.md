# Building A Game

A Parlando game is a Rust adapter around your experiment mechanics. The adapter tells the reusable server how to create state, parse and validate actions, produce role-specific observations, emit events, and decide when the game is complete. The browser client then renders the JSON form of those observations and sends the JSON form of your actions back to the server.

Start with the research design before writing code. Identify what each role knows, what each role can do, what counts as progress, how the session ends, and what summary fields you need for analysis. Those choices map directly to Parlando's `State`, `Action`, `Observation`, `Event`, and `Summary` types.

## Adapter Contract

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

## Recommended Shape

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

Keep browser convenience logic separate from authority. The UI can gray out controls, suggest likely actions, or animate possible moves, but the Rust adapter should still reject illegal or out-of-turn actions.

## Action Flow

The normal action path is:

1. A browser or agent proposes an action.
2. The server parses the JSON action into the typed game action.
3. The server calls `validate_action`.
4. If validation passes, the server calls `apply_action`.
5. The server persists the accepted action and the resulting state change.
6. The server computes role-specific observations and events.
7. The server sends targeted WebSocket messages to the affected participants.
8. If `is_complete` is true, the server persists and broadcasts `completion_summary`.

Agents do not bypass this path. A remote Python agent can return an invalid action, but the Rust game server still rejects it.

## Observations And Private Information

`observe_state` is where you decide what each participant can see. For asymmetric information experiments, do not send the full state and ask the client to hide fields. Build a role-specific observation on the server.

In the demo game, each role sees shared device state but only its own private knowledge. The adapter filters private knowledge before sending observations to player A, player B, or an agent controlling either role.

A useful rule: if a participant should not use a field, do not include it in that participant's observation. Do not depend on CSS, React state, or disabled UI controls to hide sensitive task information.

## Available Actions

`available_actions` is optional.

- Return `None` when the game does not provide an action list.
- Return `Some(vec![])` when the game does provide action hints but this player currently has no listed legal moves.
- Return `Some(actions)` when you want the UI or agent to receive a role-specific affordance.

Available actions are hints, not authority. Always keep `validate_action` complete.

This is useful for experiments where an agent or UI can choose from a controlled set of choices. It is less useful for free-form text or continuous actions.

## Building A New Game Crate

A typical new game crate needs:

1. Define `State`, `Action`, `Observation`, `Event`, and `Summary`.
2. Write pure transition helpers for initial state, validation, state updates, observations, events, completion, and summary generation.
3. Implement `GameAdapter` by delegating to those helpers.
4. Add any in-process agents and a game-specific `factory_from_config`.
5. Define matching TypeScript types for the JSON form of your action, observation, event, and summary values.
6. Build a browser client using the Parlando JavaScript client package and your game-specific rendering and controls.
7. Create a binary like `space-game/server/src/main.rs` that loads config and calls `serve`.
8. Add integration tests that create rooms, connect sockets, submit actions, and inspect exports.

## Game Design Checklist

Before implementing, write down:

- Roles: currently active player roles are `A` and `B`.
- Participant-visible observation fields for each role.
- Private fields that must never be sent to the other role.
- Action schema and validation rules.
- Whether the UI should use server-provided `available_actions` or compute controls from the observation.
- Transition events that should appear in the participant log.
- Terminal conditions, including success, failure, timeout, or abandonment cases.
- Completion summary fields needed for analysis.
- Fields that should be exported for downstream analysis.

This checklist is also the natural prompt shape for the included `generate-parlando-game` skill: describe the game in these terms, generate a first adapter/client, then iterate.

## Reference Files

- `rust-server/src/game.rs`: game adapter trait.
- `docs/client-protocol.md`: browser JSON contract.
- `space-game/server/src/game/state_engine.rs`: demo typed game model.
- `space-game/server/src/game/adapter.rs`: demo adapter.
- `space-game/server/src/main.rs`: demo binary.
- `space-game/client`: demo browser client.
