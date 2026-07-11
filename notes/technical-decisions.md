# Steps 1-5 Foundation Decisions

## Context

The first Parlando implementation checkpoint covers the Rust workspace skeleton, protocol/config foundation, core server traits/app state, the first storage backend, and participant/session room creation flows. The server and game crate are linked into one binary, so game internals should stay typed and only serialize at HTTP, WebSocket, and persistence boundaries.

## Chosen Approach

- Keep two crates: `parlando-server` for reusable runtime behavior and `parlando-space-game` for concrete game logic.
- Use associated types on `GameAdapter` for `State`, `Action`, `Observation`, `Event`, and `Summary`.
- Parse client JSON into a typed game action once at the server boundary.
- Keep YAML compatibility with the Python-era config files, including `extends`, `include/includes`, optional includes, environment substitution, and relative path resolution.
- Use a general `ExperimentStore` trait for durable evaluation data, with SQLite as the first implementation.
- Model experiments, durable participant identities, sessions, session-local participants, consent declarations, and ordered session events relationally.
- Keep `MemoryState` only as an active-game runtime cache for WebSockets, broadcasts, and in-progress game execution.
- Preserve client-facing `room_id` and `participant_session_id` while storing evaluation-facing `experiment_id`, integer `session_id`, and integer `participant_id`.
- Cover human-vs-human and human-vs-agent room creation through router-level HTTP tests rather than only unit-level helpers.

## Tradeoffs

- A typed adapter interface is more verbose than `serde_json::Value`, but it prevents accidental JSON-shaped game logic inside the linked binary.
- The evaluation schema is more specific than a generic event log, but it gives researchers stable keys for experiment/session exports.
- A single `session_events` table stores session activity once. It avoids redundant category/audit tables while still supporting ordered reconstruction and event-type filtering.
- The config loader accepts legacy Python fields instead of rejecting them so existing YAML files remain usable during migration.
- Step-5 tests use the Axum router directly, which keeps them fast while still exercising real routes and JSON shapes.

## Risks And Follow-Up

- Recovery of full active runtime state from the evaluation database is still future work.
- The runtime cache must not become a hidden persistence layer; later steps should keep moving semantic writes into `ExperimentStore`.
- Endpoint-level tests should expand beyond storage-level tests as participant, session, WebSocket, transcript, and agent flows mature.
- Config validation should remain compatible with existing YAML files as real deployments are tested.
- Public rustdoc and function comments need to continue expanding as each later step is implemented.

# Step 6 Space Game Boundary Decisions

## Context

The Space Game crate is linked into the same Rust binary as the reusable server. The boundary should therefore be a typed Rust boundary, not an internal JSON boundary, while still preserving the existing TypeScript client protocol at transport edges.

## Chosen Approach

- Keep `GameAdapter` as the only reusable-server interface for game semantics.
- Let `GameAdapter::parse_action` default to serde deserialization so concrete games do not repeat JSON parsing boilerplate.
- Treat `Seat` as exactly `A` or `B`; spectator-era optional role conversion was removed from the server path.
- Keep Space Game-specific `moveCount` inside `SpaceGameState`; the reusable server does not interpret or maintain a generic move counter.
- Keep agent factory selection in `parlando-space-game`, because selectors such as `space_game.back_and_forth` are game-specific.
- Test client JSON compatibility only at protocol-shape assertions; all state transition tests operate on typed Rust structs.

## Tradeoffs

- The Space Game adapter still implements the semantic methods explicitly, which makes the boundary clear at the cost of a small delegating layer.
- Default JSON parsing in the reusable trait reduces boilerplate, but games can still override parsing if they need custom compatibility behavior.
- Removing optional player roles simplifies the runtime and agent loop, but any future observer role must be introduced as a separate server concept rather than silently reusing player seats.

## Risks And Follow-Up

- Remote gRPC agents will need an explicit serialization boundary because they are out of process; that should be isolated to the remote-agent adapter rather than leaking JSON handling into in-process game logic.
- As more games are added, we should revisit whether helper traits for action ownership or observation construction reduce repeated code without hiding game semantics.

# Server-Owned Role Assignment

## Context

The early HTTP request structs exposed a caller-controlled role override for tests and debugging. Production room assignment does not need callers to request a role, and exposing such a field creates edge cases around duplicate roles and invalid session layouts.

## Chosen Approach

- Remove caller-controlled role overrides from the public room create/join protocol.
- Assign role `A` to the room creator and role `B` to the next distinct participant.
- Preserve an existing participant's role on rejoin.
- Reject caller-controlled role fields in room create/join JSON by denying unknown fields on those request bodies.
- Record `participant_joined` only for a new session appearance; rejoining a room returns the current role without appending another join event.

## Tradeoffs

- Tests can no longer construct unusual role layouts through public HTTP APIs, but that is consistent with the production client contract.
- Server-owned assignment is less flexible for manual debugging, but it makes evaluation exports cleaner and prevents impossible duplicate-role sessions.

## Risks And Follow-Up

- If future experiment designs need asymmetric assignment controls, introduce an explicit server-side matchmaking policy rather than a caller-controlled role override.
