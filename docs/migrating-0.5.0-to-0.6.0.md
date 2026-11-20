# Upgrade Parlando from 0.5.0 to 0.6.0

Parlando 0.6.0 uses the same React game renderer for live play and administrator replay.
Upgrade the server and browser client together. A game supplies its role-specific view and
task controls. The participant app supplies conversation and the session controls around it.

## Update both packages

In the game's `Cargo.toml`, set the dependency to `parlando = "0.6.0"`. In its `package.json`,
set `"@coli-saar/parlando-client": "^0.6.0"`. Refresh both lockfiles with Cargo and npm, then
rebuild the server binary and browser bundle.

## Adapt the React renderer in TypeScript

Import `GameSnapshot<Observation, Action>` from `@coli-saar/parlando-client/react` in place of
`GameSession<Observation, Action, Completion>`. Replace `session.sendAction(action)` with
`snapshot.act(action)`. Here is a renderer for a game whose observation exposes a
`ready` flag and whose action type has a `ready` variant.

```tsx
import { ParticipantApp, type GameSnapshot } from "@coli-saar/parlando-client/react";

type Observation = { ready: boolean };
type Action = { type: "ready" };

function GameView({ snapshot }: { snapshot: GameSnapshot<Observation, Action> }) {
  return (
    <button
      disabled={!snapshot.interactionEnabled || snapshot.observation.ready}
      onClick={() => snapshot.act({ type: "ready" })}
    >
      {snapshot.observation.ready ? "Ready" : "Mark ready"}
    </button>
  );
}

export function App() {
  return <ParticipantApp renderGame={(snapshot) => <GameView snapshot={snapshot} />} />;
}
```

The snapshot still provides the session ID, role, observation, latest accepted transition,
and available actions. Replay sets `interactionEnabled` to false and supplies an inert
`act` callback. Keep completion content in the optional `renderCompletion` callback.

Remove controls that depend on the former session fields `conversation`, `presence`,
`voiceStatus`, `voicePreflight`, `voiceEnabled`, or `connected`, or on the former callbacks
`sendMessage`, `setMicrophoneMuted`, or `leave`. Use the shared participant app's controls
and their `parlando-*` styling hooks. The participant HTTP and WebSocket formats and experiment
configuration remain compatible with 0.5.0.

## Make authoritative state readable by replay

Implement Serde's `Deserialize` for the type assigned to `Game::State` in addition to its
existing `Serialize` implementation. For ordinary structs and enums, add
`#[derive(serde::Serialize, serde::Deserialize)]` or extend the existing derive.
Your existing `GameFactory` can continue to construct the session's game.

Replay reads the recorded initial state and reapplies accepted actions in order. The
`STATE_REPRESENTATION_VERSION`, `ACTION_REPRESENTATION_VERSION`, and `RULES_VERSION` constants
default to `"1"`. Keep them stable while the build can read those representations and
produce the same transitions. Advance the relevant constant when its contract changes.
Keep the compiled game's own version stable for a dependency-only upgrade if you want to
continue opening experiments bound to that game version.

## Verify your database and exports

Both releases use SQLite schema version 16. Before replacing the deployment, let live
sessions finish and make a consistent SQLite backup using the SQLite backup API or the
`sqlite3` shell's `.backup` command. On a copy, check that
`SELECT MAX(version) FROM schema_migrations;` returns `16` and
`PRAGMA integrity_check;` returns `ok`. Open the copy with the upgraded game. Check that
experiment and session counts match and that a historical session opens and exports.

Sessions recorded on 0.5.0 remain available in the dashboard, and the replay endpoint reports
that they predate replay recording. For new sessions, the corpus action event's `state` field
is null. Adapt analysis that consumes post-action snapshots to use the recorded actions or
game-specific transition metadata. The corpus also omits setup logs with negative game times.
Session history retains those logs.

## Validate the upgrade

1. Activate an experiment and join from separate browser profiles.
2. Confirm that accepted actions update both views and that shared chat, voice if enabled,
   and leaving work through the participant app.
3. Complete a session, check the configured completion view, and open **Replay** from the
   administrator session detail. Scrub through the actions, switch viewpoints, and verify
   that the replay surface cannot send actions.
4. Export the completed session and check any analysis that depends on the action event shape.
