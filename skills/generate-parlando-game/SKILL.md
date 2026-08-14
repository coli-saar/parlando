---
name: generate-parlando-game
description: Generate Parlando dialogue games using the published parlando-server runtime and @coli-saar/parlando-client React startup gate. Use when creating, porting, or iterating on a Parlando game.
---

# Generate Parlando Game

Use this skill to turn a game or experiment idea into a working Parlando game. Generated game projects depend on the published registry packages: `parlando-server` from crates.io and `@coli-saar/parlando-client` from npm. Prefer the user's target repository layout, but generate a complete, runnable project when no layout exists.

## Reference Scan

Before generating code, read the bundled references that match the requested game:

- `references/server-adapter.md` for the Rust `GameAdapter`, server binary, manifest, and test shape.
- `references/browser-client.md` for JSON naming, HTTP flow, WebSocket messages, actions, and React client wiring.
- `references/config-deployment.md` for experiment YAML, local run, Docker, and Render conventions.
- `references/agents.md` only when the generated project needs server-side agent support, an in-process Rust agent, or remote gRPC agents.

These references are intended to be sufficient for normal game generation. Do not browse or search GitHub documentation for Parlando game-generation details; if the bundled references do not cover something needed for a game, stop and report the missing topic so the skill can be updated.

## Package Versions

Use the latest published Parlando package versions when generating manifests:

- Query crates.io with `cargo info parlando-server` or an equivalent registry lookup, then set `parlando-server = "<latest-version>"`.
- Query npm with `npm view @coli-saar/parlando-client version` or an equivalent registry lookup, then set `"@coli-saar/parlando-client": "^<latest-version>"`.
- If network access is unavailable, fall back to `parlando-server = "0.2.0"` and `"@coli-saar/parlando-client": "^0.2.0"`, and say in the final response that the latest versions could not be checked.

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
- whether the generated project should include server-side agent support
- if an agent should understand/respond to text chat or speech transcripts, and whether the user can provide LLM provider credentials for an LLM-backed agent when that is appropriate
- text chat, voice, transcription, and TTS requirements

## Game Runtime Assumptions

Generated games must be agnostic to the participant mix. A browser instance renders one human player's UI from its role-specific observation, sends that human player's actions, and receives accepted actions/events from the other player through Parlando. The game UI must not need to know whether the other participant is a human using another browser, an in-process agent, or a remote agent.

Generated games also must not decide whether voice is enabled. If the server/session exposes voice capability and asks the client to allow it, the game must allow the SDK-provided voice controls/status to work. If the server/session does not expose voice capability, the game should simply omit or disable voice controls based on session state. Voice service setup, credentials, startup, and policy live in Parlando config/server/client SDK code, not in game-specific logic.

Parlando voice uses one authenticated browser-to-server audio WebSocket per human participant. The SDK captures fixed 24 kHz mono PCM, applies its jitter buffer and playback resampling, and receives partner or agent audio on the same connection. The server relays microphone frames, independently feeds server-side transcription, and publishes agent TTS with real-time pacing. Generated games must not add WebRTC libraries, peer-to-peer negotiation, custom audio WebSockets, AudioWorklets, jitter buffers, frame pacing, browser STT clients, or transcript POST endpoints.

When TTS is enabled, agent utterances should be returned through `AgentResponse.message` so `parlando-server` can create an agent-origin conversation message, synthesize it, and publish it through the configured audio transport. Generated browser code must not call TTS providers directly or duplicate the server-side agent speech pipeline.

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
|   |-- build.rs
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

Every generated game must include explicit completion semantics. Model terminal state in `State` or derived game logic, including whether the terminal outcome is success, failure, timeout, or another game-specific result. `is_complete` is the only signal Parlando needs to mark the room/session complete; it must return true once any terminal success or failure condition has been reached. `completion_summary` must return a serde-serializable `Summary` that includes the terminal outcome and enough durable fields for analysis/export.

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
- create an agent factory from config when the generated project includes agent support
- call `parlando_server::serve`
- read `PORT` from the environment when `--port` is absent

Do not construct an audio publisher in the game binary. When `voice.enabled` and `tts.enabled` are true, `parlando-server` publishes agent audio through its room relay automatically.

Do not add transport dependencies or routes to the generated server. `parlando-server` owns `/api/rooms/{room_id}/audio-session`, opaque one-use audio credentials, `/ws/audio/{room_id}`, PCM validation, bounded room queues, server-side transcription sessions, and agent-audio pacing.

## Agents

If the user asks for human-vs-agent mode, server-side agent support, or a demo agent, generate `agents.rs`.

Support:

- no factory for human-vs-human
- a simple deterministic in-process agent for local smoke tests
- `remote_grpc` when custom Python or external agents are expected

Returned agent actions must be normal typed `Action` values and still pass game validation.

Do not fork game semantics or browser UI around human-vs-human versus human-vs-agent. Agents and humans are both participants that submit the same typed actions and receive the same role-specific observations.

If the game design makes chat relevant, generated agents must observe and respond to participant text messages and speech transcripts. Implement `observe_message` to store the latest relevant utterance with its speaker and modality, and make `maybe_act` use that memory when deciding whether to return an `AgentResponse` with a message, action, or both. Do not generate an agent that only reacts to game actions when the participant is expected to instruct, negotiate with, or ask questions of the agent. If a scripted policy is too brittle for the requested dialogue behavior, ask whether the user can provide LLM provider credentials and generate a remote or server-side LLM-backed agent path that keeps credentials out of browser code.

When TTS is enabled in config, agents should vocalize participant-facing utterances by returning an `AgentResponse` with `message: Some(text)`. The server records the text as an agent conversation message and routes it through the configured TTS provider and room relay. Do not add frontend TTS calls, browser speech synthesis, or game-specific audio publishing.

## TypeScript Contract

Mirror the JSON shape produced by Rust serde, not the Rust field names.

Generate:

- `types.ts` for `PlayerRole`, `GameAction`, `GameObservation`, `GameEvent`, `GameSummary`, and UI helper types
- a client-side pure `stateEngine.ts` only if it helps UI previews/tests; the server remains authoritative
- tests for action generation, derived UI state, and any client-side reducer logic

Use `@coli-saar/parlando-client/react` for the generated app entrypoint. Wrap the game in `ParlandoStartupGate` and render the active game from the `ActiveParlandoSession` passed to `renderGame`; do not generate custom startup lifecycle code. Game-specific rendering, controls, task text, maps, boards, logs, action creation, and derived UI state belong in the generated app.

The generated app should treat `ActiveParlandoSession` as the source of participant capabilities. It may render chat and voice controls from session state, but it must not infer capabilities from the game type, deployment mode, or whether the peer is expected to be a human or an agent.

When `session.completed` is true, the active game UI must render a final/terminal screen instead of normal action controls or game chat input. Use `session.completionSummary` for durable outcome and score fields, and combine it with the latest observation/events for participant-facing detail. The browser does not call a separate "complete game" API; completion is driven by the server adapter's `is_complete` and broadcast back through the SDK.

If transcription/STT is enabled by `session.publicConfig.transcription?.enabled`, the active game screen should show a compact microphone-level widget with an ASR status pill. Prefer the predefined React exports from `@coli-saar/parlando-client/react`: compose `MicLevelMeter` with `session.voicePreflight.micLevel` / `session.voicePreflight.micProbeActive` and `TranscriptionStatusChip` with `session.voiceStatus`. Use `TranscriptionProgress` when a fuller progress display is useful. Style the exported widgets in `web/src/styles.css`.

## Browser Styling

Generated clients must style both the active game screen and the shared startup screens rendered by `ParlandoStartupGate`. The SDK supplies uniform startup markup and lifecycle behavior, but the generated app owns the CSS that makes those screens look integrated with the game.

In `web/src/styles.css`, include polished, responsive styles for the startup classes emitted by the SDK:

- `lobby-panel`, `lobby-heading`, `lobby-copy`, `platform-label`, `online-error`
- `lobby-actions`, including buttons, inputs, and selects
- `consent-list` and `consent-row`
- `seat-grid` and `seat-ready`
- `voice-preflight`, `mic-device-label`, `mic-meter`, and `mic-meter-track`
- `transcription-progress` and `transcription-progress-track`

Keep the startup screen visually consistent with the generated game's theme, but preserve the SDK's class names and accessibility attributes. Startup controls must be readable and usable on mobile and desktop, with stable spacing, clear disabled states, visible focus states, and no overlapping text.

When styling `MicLevelMeter`, ensure the scaled bar is visible. The SDK renders a child element inside `.mic-meter-track` and updates it with `transform: scaleX(...)`; generated CSS must give that child `display: block`, `width: 100%`, `height: 100%`, `transform-origin: left center`, and a visible background color. Without those dimensions, the mic meter can stay invisible even while audio levels update.

## Config Files

Generate `config/experiment.local.yaml` with:

- local experiment id
- `direct.enabled: true`
- `server.public_base_url: http://localhost:8000`
- `server.client_dist_path: client/dist`
- local SQLite under `.local/`
- voice, transcription, and TTS disabled unless requested
- when voice is requested, `voice`, server-side Speechmatics, and ElevenLabs fields via a private overlay
- consent items under `direct.consents`; an empty list skips consent
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
- confirm the server has tests for every terminal success and failure path, including `is_complete`, `completion_summary`, and the serialized summary shape expected by the client/export
- confirm the browser renders a final screen when `session.completed` is true, using `session.completionSummary` for outcome/score fields and hiding or disabling normal action and game chat input
- if chat with an agent is relevant, confirm the agent observes messages/transcripts and has tests or a smoke path showing it responds to participant utterances
- if voice is enabled, confirm generated code delegates audio capture/playback to `ParlandoStartupGate`, contains no custom media transport or browser provider credentials, and config uses 24 kHz PCM with a sensible jitter target such as 100 ms
- `npm install` when dependencies are available
- `npm run build`
- `npm test`
- inspect the CSS for `.mic-meter-track span` or equivalent and confirm the meter bar has `display: block`, `width: 100%`, `height: 100%`, and a visible background so transform-based level updates render
- start the local server if practical and check `GET /health`

If network access or package installation is blocked, still run local formatting/type checks that do not need the network and clearly say what could not be verified.

## Final Response

End with:

- files/directories created or changed
- commands run and whether they passed
- exact local run command
- exact config YAML path to edit for local settings, and if voice/TTS/transcription is enabled, the private YAML/secret path that should hold Speechmatics and ElevenLabs values
- deployment notes for the requested target
- agent run commands when generating a Rust or Python gRPC agent
- confirmation that the browser client delegates startup to `ParlandoStartupGate` from `@coli-saar/parlando-client/react`
- confirmation that game completion is implemented through `is_complete`/`completion_summary`, including how success and failure are represented
- confirmation that the completed game UI renders a final screen from `session.completionSummary` and does not leave normal game controls active
- confirmation that agent text-message handling is implemented when chat or speech interaction with an agent is part of the game
- assumptions made about the game design or package layout
