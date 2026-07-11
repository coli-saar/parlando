# Steps 1-4 Foundation Decisions

## Context

The first Parlando implementation checkpoint covers the Rust workspace skeleton, protocol/config foundation, core server traits/app state, and the first storage backend. The server and game crate are linked into one binary, so game internals should stay typed and only serialize at HTTP, WebSocket, and persistence boundaries.

## Chosen Approach

- Keep two crates: `parlando-server` for reusable runtime behavior and `parlando-space-game` for concrete game logic.
- Use associated types on `GameAdapter` for `State`, `Action`, `Observation`, `Event`, and `Summary`.
- Parse client JSON into a typed game action once at the server boundary.
- Keep YAML compatibility with the Python-era config files, including `extends`, `include/includes`, optional includes, environment substitution, and relative path resolution.
- Use a general `ExperimentStore` trait for durable evaluation data, with SQLite as the first implementation.
- Model experiments, durable participant identities, sessions, session-local participants, consent declarations, and ordered session events relationally.
- Keep `MemoryState` only as an active-game runtime cache for WebSockets, broadcasts, and in-progress game execution.

## Tradeoffs

- A typed adapter interface is more verbose than `serde_json::Value`, but it prevents accidental JSON-shaped game logic inside the linked binary.
- The evaluation schema is more specific than a generic event log, but it gives researchers stable keys for experiment/session exports.
- A single `session_events` table stores session activity once. It avoids redundant category/audit tables while still supporting ordered reconstruction and event-type filtering.
- The config loader accepts legacy Python fields instead of rejecting them so existing YAML files remain usable during migration.

## Risks And Follow-Up

- Recovery of full active runtime state from the evaluation database is still future work.
- The runtime cache must not become a hidden persistence layer; later steps should keep moving semantic writes into `ExperimentStore`.
- Endpoint-level tests should expand beyond storage-level tests as participant, session, WebSocket, transcript, and agent flows mature.
- Config validation should remain compatible with existing YAML files as real deployments are tested.
- Public rustdoc and function comments need to continue expanding as each later step is implemented.
