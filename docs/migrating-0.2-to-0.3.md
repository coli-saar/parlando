# Migrate a Parlando game from 0.2 to 0.3

Parlando 0.3 is a coordinated breaking release of the Rust runtime and browser client. Upgrade both packages and port the game to the new public boundaries in one change. Do not mix a 0.2 runtime with a 0.3 client, and do not add compatibility aliases for removed APIs.

This guide gives the migration workflow. [Migrating Games to the Clean Parlando API](migrating-to-clean-api.md) is the detailed symbol and behavior reference; use it when a step below identifies an affected part of your game.

## 1. Establish a baseline

Before editing, run the game's existing Rust, JavaScript, Python-agent, and end-to-end tests. Record any existing failures so they are not confused with migration regressions. Preserve the game's domain behavior: legal actions, role-private information, terminal conditions, agent decisions, and participant presentation should change only where the 0.3 contract requires it.

Search the game for 0.2 dependencies and removed API names:

```sh
rg -n 'parlando-server|parlando_server|ParlandoStartupGate|GameAdapter|GameAgent|ExperimentApiClient|ActiveParlandoSession|completionSummary|sendChatMessage|observe_state|observe_action|maybe_act'
```

The search results define the migration surface. Generated code, tests, deployment manifests, and documentation count as consumers and must be migrated with application code.

## 2. Update both dependencies

Replace the Rust package rather than renaming it locally:

```toml
# Cargo.toml
[dependencies]
parlando = "0.3.0"
```

Remove the old `parlando-server` dependency. In Rust source, replace `parlando_server::...` with `parlando::...`:

```rust
use parlando::{ActionRejection, Game, GameMetadata, PlayerRole, Server};
```

Update the participant client in the same change:

```json
{
  "dependencies": {
    "@coli-saar/parlando-client": "^0.3.0"
  }
}
```

Refresh both lockfiles after changing the manifests:

```sh
cargo update
npm install
```

For monorepo development, a local path dependency may replace the registry version temporarily. Confirm that the resolved local package itself reports version 0.3.0; a path does not guarantee the expected API version.

## 3. Port the Rust game boundary

Implement `Game` directly and expose `Config`, authoritative `State`, player `Action`, role-specific `Observation`, and shared `Completion` types. The important behavioral changes are:

- `apply_action` performs authorization, validation, and transition atomically;
- `observation` is the only player-visible projection of authoritative state;
- `completion` combines terminal detection with the shared terminal value;
- accepted actions are visible to both roles, together with each role's resulting observation; and
- presentation events are removed, while optional `transition_metadata` is role-neutral analysis data.

Use the [Rust game mapping and target implementation](migrating-to-clean-api.md#rust-game) for the complete method and associated-type conversion. Do not expose serialized `State` as a migration shortcut. Add any participant-visible fact to `Observation`, filtering private information by `PlayerRole`.

## 4. Port agents and server construction

If the game has agents, migrate their factories and lifecycle callbacks before changing runtime wiring. The 0.3 runtime constructs an agent first, calls `start` with the initial role-safe observation, reports later accepted actions through `observe_transition`, reports player text through `observe_message`, and sends the shared terminal value to `finish`. Follow the [Rust agent](migrating-to-clean-api.md#rust-agents) or [Python remote-agent](migrating-to-clean-api.md#python-remote-agents) mapping as appropriate.

Construct the process through `Server::new(game, metadata)`, register optional agent factories on the server, configure only process bootstrap values, and call `serve`. The [server-construction migration](migrating-to-clean-api.md#server-construction) lists internal configuration, provider, router, storage, and protocol types that downstream games must delete rather than replace.

Experiment creation and configuration now belong to the administrator dashboard. If the 0.2 game has a checked-in experiment YAML file, transfer its supported values through the dashboard; do not keep a bootstrap loader in game code.

## 5. Port the participant application

For a React application, render the game through `ParticipantApp` and a typed `GameSession`:

```tsx
import { ParticipantApp, type GameSession } from "@coli-saar/parlando-client/react";

type Session = GameSession<GameObservation, GameAction, GameCompletion>;

export function App() {
  return <ParticipantApp renderGame={(session: Session) => <GameView session={session} />} />;
}
```

Render authoritative game information from `session.observation`. Use `session.transition` only for presentation that needs the most recent accepted actor and action. Send actions with `sendAction`, messages with `sendMessage`, and render terminal state from `completed` and `completion`.

The [JavaScript and React migration table](migrating-to-clean-api.md#javascript-and-react) covers renamed components, session fields, microphone controls, and lower-level client methods. A custom WebSocket client must also migrate to protocol version 1 and the message shapes listed there. Do not import raw protocol, audio-controller, credential, or socket implementation types from the package.

## 6. Delete obsolete code

Run the baseline search again and inspect every remaining match. Historical documentation may mention 0.2 names, but executable code must not retain them. In particular:

- delete event types and state-delivery fallbacks;
- delete game-owned experiment, provider, pairing, privacy, and session configuration;
- delete compatibility aliases and adapters around removed public names;
- port or delete tests that assert obsolete behavior; and
- keep `test_support` and internal modules out of downstream game crates.

Use the longer [deletion pass](migrating-to-clean-api.md#deletion-pass) when auditing a large or generated game.

## 7. Verify the migrated game

At minimum, run:

```sh
cargo fmt --check
cargo test
npm test
npm run build
```

Also run Python-agent and end-to-end suites when the game has those components. The migration is complete only when tests demonstrate:

- deterministic initialization from the same configuration and seed;
- rejection of illegal actor/action combinations with stable codes;
- no disclosure of one role's private state to the other role;
- terminal results and final observations for success and failure paths;
- unchanged state when players only exchange messages;
- correct agent lifecycle and terminal delivery; and
- a participant UI that renders observations, submits actions and messages, and disables normal controls after completion.

Finally, inspect resolved dependency versions:

```sh
cargo tree -i parlando
npm ls @coli-saar/parlando-client
```

Both must resolve to the intended 0.3 release or to an explicitly chosen local 0.3 checkout.

## Instructions for an automated migration agent

An LLM or other migration agent should execute the sections above in order and obey these constraints:

1. Inventory the whole game repository before editing, including manifests, lockfiles, generated sources, tests, documentation, deployment files, Rust or Python agents, and custom protocol code.
2. Preserve game semantics and role privacy. Do not infer new rules or expose authoritative state to make the frontend compile.
3. Treat [Migrating Games to the Clean Parlando API](migrating-to-clean-api.md) as the authoritative mapping for removed 0.2 symbols and behavior.
4. Replace obsolete code with the 0.3 abstraction or delete it. Never create compatibility aliases, duplicate state channels, generic presentation events, or game-owned runtime configuration.
5. Update `parlando` and `@coli-saar/parlando-client` together, refresh lockfiles, and verify resolved versions.
6. Run the relevant language and end-to-end suites. Report changed files, commands, results, remaining failures, configuration that must be transferred through the dashboard, and any assumption that could affect game behavior.

If a removed API has no mapping in the detailed guide, stop and identify the required capability. Do not import an internal Parlando module or reconstruct a removed public implementation seam.
