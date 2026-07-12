---
name: generate-parlando-game
description: Generate complete Parlando dialogue games from a study/game description, including the Rust GameAdapter crate, JavaScript or React browser client, config files, build files, tests, and run/deploy instructions. Use when a user wants to create, scaffold, port, or iterate on a Parlando game using the rust-server and js-client packages.
---

# Generate Parlando Game

Use this skill to turn a game or experiment idea into a working Parlando game. Assume normal generated game projects depend on released registry packages: `parlando-server` from crates.io and `@coli-saar/parlando-client` from npm. Do not make generated game projects depend on sibling `rust-server` or `js-client` source directories unless the user is explicitly working inside the Parlando monorepo or asks for a temporary local debug override. Prefer the user's target repository layout, but generate a complete, runnable project when no layout exists.

Global docs may be referenced at:

- https://github.com/coli-saar/parlando
- https://github.com/coli-saar/parlando/tree/main/docs

If the local Parlando docs are present, prefer them over GitHub because they match the checked-out package versions.

## Reference Scan

Before generating code in a Parlando checkout, read the relevant local files:

- `docs/building-games.md` for the adapter contract and game design checklist
- `docs/client-protocol.md` for browser JSON and WebSocket message shapes
- `docs/running-and-deployment.md` for config, local run, Docker, and Render conventions
- `space-game/server/` and `space-game/` for a complete example, when present

## First Questions

Ask for missing information only when it affects the core game contract. Otherwise make a reasonable prototype and state the assumptions.

Capture:

- game name, study goal, and success/completion condition
- roles, normally `A` and `B`
- private information each role receives
- shared state visible to both roles
- legal actions and validation rules
- whether controls come from server `available_actions` or client-side UI logic
- events/log messages participants should see
- summary/export fields needed for analysis
- local-only, Docker, Render, or other deployment target
- human-vs-human, human-vs-agent, or both
- text chat, voice, transcription, and TTS requirements

## Output Shape

Generate both halves of the game and all build/run files:

```text
<game-slug>/
|-- README.md
|-- Makefile
|-- config/
|   |-- experiment.local.yaml
|   `-- experiment.render.example.yaml
|-- server/
|   |-- Cargo.toml
|   `-- src/
|       |-- main.rs
|       |-- lib.rs
|       |-- agents.rs
|       `-- game/
|           |-- mod.rs
|           |-- state_engine.rs
|           `-- adapter.rs
`-- client/
    |-- package.json
    |-- tsconfig.json
    |-- vite.config.ts
    |-- index.html
    `-- web/
        `-- src/
            |-- App.tsx
            |-- main.tsx
            |-- styles.css
            `-- game/
                |-- types.ts
                |-- stateEngine.ts
                `-- stateEngine.test.ts
```

If contributing inside the Parlando monorepo, instead mirror the existing demo shape:

- add a Rust crate under `<game-slug>/server`
- depend on the reusable server crate with an appropriate local path to `rust-server`
- add a browser app under `<game-slug>/client`
- include game-local `Makefile`, `config/`, and client source

Outside the monorepo, treat the generated game as a registry-package consumer:

- install the generated Rust game server binary with `cargo install --path server --root .local` or an equivalent Cargo install command
- put `.local/bin` on `PATH` when running locally
- depend on `parlando-server` and `@coli-saar/parlando-client` by released package version
- avoid `../rust-server` and `../js-client` path assumptions
- document temporary absolute local path overrides only for debugging Parlando itself

## Rust Contract

Every game needs a thin adapter around pure game semantics.

Implement:

- `State`
- `Action`
- `Observation`
- `Event`
- `Summary`
- `initial_state`
- `validate_action`
- `apply_action`
- `observe_state_for_player`
- `available_actions` when useful
- `events_for_action`
- `is_complete`
- `completion_summary`

Use serde-serializable Rust structs/enums. Prefer:

```rust
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type")]
pub enum GameAction {
    #[serde(rename = "exampleAction")]
    ExampleAction { player: String },
}
```

Privacy rule: never send private state to the wrong participant. Enforce this in `observe_state`, not in React.

Validation rule: the UI can guide users, but `validate_action` must be the authority.

## Main Binary

Generate a `main.rs` that follows the Parlando server pattern:

- parse `--config`, `--host`, `--port`, and optional `--experiment-id`
- load `ExperimentConfig::from_yaml` or defaults
- create the game adapter
- create an agent factory from config if human-vs-agent is supported
- call `parlando_server::serve`
- read `PORT` from the environment when `--port` is absent

For voice games, include `LiveKitAgentAudioPublisher` only when both `livekit.enabled` and `tts.enabled` are true.

## Agents

If the user asks for human-vs-agent or a demo agent, generate `agents.rs`.

Support:

- no factory for human-vs-human
- a simple deterministic in-process agent for local smoke tests
- `remote_grpc` when custom Python or external agents are expected

Returned agent actions must be normal typed `Action` values and still pass game validation.

## TypeScript Contract

Mirror the JSON shape produced by Rust serde, not the Rust field names.

Generate:

- `types.ts` for `PlayerRole`, `GameAction`, `GameObservation`, `GameEvent`, `GameSummary`, and UI helper types
- a client-side pure `stateEngine.ts` only if it helps UI previews/tests; the server remains authoritative
- tests for action generation, derived UI state, and any client-side reducer logic

Use `@coli-saar/parlando-client` for setup, WebSocket, actions, audio-session status, and reusable React widgets when available. Game-specific rendering, controls, task text, maps, boards, and logs belong in the generated app.

The browser must handle:

- initial setup/direct entry or room creation
- waiting for partner/role assignment
- `roleAssigned`
- `stateChanged`
- `conversationMessageAdded`
- `completed`
- `presenceChanged`
- `voiceStatusChanged`
- `error`
- reconnection-friendly state replacement from server messages

## Config Files

Generate `config/experiment.local.yaml` with:

- local experiment id
- `direct.enabled: true`
- room codes and/or matchmaking
- `server.public_base_url: http://localhost:8000`
- `server.client_dist_path: client/dist`
- local SQLite under `.local/`
- voice, transcription, and TTS disabled unless requested
- conversation enabled by default

Generate `config/experiment.render.example.yaml` with:

- env vars for public URL, experiment id, and secrets
- SQLite path on `/data`
- `server.client_dist_path: /app/client-dist`
- safe comments explaining which secrets are required only for voice deployments

## Build Files

Generate:

- Rust `Cargo.toml` using the released Parlando server crate version, plus `anyhow`, `clap`, `serde`, `serde_json`, `tokio`, `tracing-subscriber`, and optional `async-trait`/`rand`. Use a workspace path only when generating inside the Parlando monorepo. If the user asks for local Parlando debugging, show an absolute-path replacement as a temporary non-release edit.
- client `package.json` using `@coli-saar/parlando-client`, React, Vite, TypeScript, and Vitest. Keep the dependency version normal. If the user asks for local Parlando debugging, document `npm install --no-save file:/absolute/path/to/parlando/js-client` after building `js-client`.
- client-local `vite.config.ts`, `tsconfig.json`, and `index.html`
- `Makefile` targets for `check-client-package`, `install-client-deps`, `install-server`, `build-client`, `build`, `test`, `run`, and `clean`
- Dockerfile and Render config when the user asks for deployable output, or when deployment is part of the prompt

The local Makefile should install dependencies from the registries by default. It should install the generated server binary with Cargo, build the browser client with npm, and run that installed binary.

The production image should build the client, copy the client `dist` to `/app/client-dist`, build/install the generated server binary with Cargo, and start the binary with `--host 0.0.0.0 --config /app/config/experiment.yaml`.

## Validation

After generating code, run the strongest feasible checks:

- `cargo fmt`
- `cargo test` or package-specific Rust tests
- `npm install` when dependencies are available
- `npm run build`
- `npm test`
- start the local server if practical and check `GET /health`

If network access or package installation is blocked, still run local formatting/type checks that do not need the network and clearly say what could not be verified.

## Final Response

End with:

- files/directories created or changed
- commands run and whether they passed
- exact local run command
- deployment notes for the requested target
- assumptions made about the game design or package layout
