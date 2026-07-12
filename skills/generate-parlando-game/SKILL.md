---
name: generate-parlando-game
description: Generate complete Parlando dialogue games from a study/game description, including the Rust GameAdapter crate, JavaScript or React browser client, config files, build files, tests, and run/deploy instructions. Use when a user wants to create, scaffold, port, or iterate on a Parlando game using the published parlando-server and @coli-saar/parlando-client packages.
---

# Generate Parlando Game

Use this skill to turn a game or experiment idea into a working Parlando game. Generated game projects depend on the published registry packages: `parlando-server` from crates.io and `@coli-saar/parlando-client` from npm. Prefer the user's target repository layout, but generate a complete, runnable project when no layout exists.

## Reference Scan

Before generating code, read the bundled references that match the requested game:

- `references/server-adapter.md` for the Rust `GameAdapter`, server binary, manifest, and test shape.
- `references/browser-client.md` for JSON naming, HTTP flow, WebSocket messages, actions, and React client wiring.
- `references/config-deployment.md` for experiment YAML, local run, Docker, and Render conventions.
- `references/agents.md` only when the game needs human-vs-agent, an in-process Rust agent, or remote gRPC agents.

These references are intended to be sufficient for normal game generation. Do not browse or search GitHub documentation for Parlando game-generation details; if the bundled references do not cover something needed for a game, stop and report the missing topic so the skill can be updated.

## Package Versions

Use the latest published Parlando package versions when generating manifests:

- Query crates.io with `cargo info parlando-server` or an equivalent registry lookup, then set `parlando-server = "<latest-version>"`.
- Query npm with `npm view @coli-saar/parlando-client version` or an equivalent registry lookup, then set `"@coli-saar/parlando-client": "^<latest-version>"`.
- If network access is unavailable, fall back to `parlando-server = "0.1.0"` and `"@coli-saar/parlando-client": "^0.1.0"`, and say in the final response that the latest versions could not be checked.

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

Treat the generated game as a registry-package consumer:

- install the generated Rust game server binary with `cargo install --path server --root .local` or an equivalent Cargo install command
- put `.local/bin` on `PATH` when running locally
- depend on the latest published `parlando-server` and `@coli-saar/parlando-client` versions

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

- setup screen with display name, consent items, and microphone preparation when voice is enabled
- room-backed waiting room readiness board with visible rows/cards for Player A, Player B/agent, and STT/voice when voice is enabled; missing participants must be shown as waiting/not connected
- direct room create/join before the waiting room for voice-enabled human-vs-human games, so the first player has a `roomId` while waiting for the second player
- immediate audio-session/STT initialization from the waiting room as soon as the first player has `roomId` and `participantSessionId`; do not wait for Player B and do not wait for the game screen
- game screen only after role assignment and required readiness gates
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
- direct room create/join for voice-enabled games; room codes and/or matchmaking only when appropriate for the requested study flow
- `server.public_base_url: http://localhost:8000`
- `server.client_dist_path: client/dist`
- local SQLite under `.local/`
- voice, transcription, and TTS disabled unless requested
- when voice is requested, full-stack LiveKit, Speechmatics, and ElevenLabs fields via a private overlay
- consent items under `direct.require_consent` and `direct.consents`, and make the setup UI display them
- conversation enabled by default

Generate `config/experiment.render.example.yaml` with:

- env vars for public URL, experiment id, and secrets
- SQLite path on `/data`
- `server.client_dist_path: /app/client-dist`
- safe comments explaining which secrets are required only for voice deployments

## Build Files

Generate:

- Rust `Cargo.toml` using the latest published `parlando-server` crate version, plus `anyhow`, `clap`, `serde`, `serde_json`, `tokio`, `tracing-subscriber`, and optional `async-trait`/`rand`.
- client `package.json` using the latest published `@coli-saar/parlando-client` npm package version, React, Vite, TypeScript, and Vitest.
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
- exact config YAML path to edit for local settings, and if voice/TTS/transcription is enabled, the private YAML/secret path that should hold LiveKit, Speechmatics, and ElevenLabs values
- deployment notes for the requested target
- agent run commands when generating a Rust or Python gRPC agent
- confirmation that the client follows setup screen -> waiting room -> game
- confirmation that voice-enabled clients create/join a room before the waiting room, never treat a no-room matchmaking queue as the waiting room, and start Speechmatics/STT immediately after room creation/join
- assumptions made about the game design or package layout
