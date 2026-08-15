# Parlando Server Adapter Reference

Generated Rust games should consume the published `parlando-server` crate. Do not generate local path dependencies on Parlando source directories.

## Cargo Manifest

Use the latest published Parlando server version discovered by the skill:

```toml
[package]
name = "<game-slug>-server"
version = "0.1.0"
edition = "2021"
license = "MIT"

[dependencies]
anyhow = "1"
async-trait = "0.1"
clap = { version = "4", features = ["derive", "env"] }
parlando-server = "<latest-version>"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["full"] }
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
```

Add `rand = "0.8"` only when a generated in-process agent needs seeded randomness.

## GameAdapter Contract

Every game implements `parlando_server::GameAdapter`:

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

The default `parse_action` can deserialize JSON into the typed `Action` when the action type implements serde. Only override it for compatibility or custom validation.

Keep the adapter thin. Put experiment semantics in pure functions such as `initial_state`, `validate_action`, `apply_action`, `observe_state_for_player`, `available_actions`, `events_for_action`, `is_complete`, and `completion_summary`; call them from the adapter. This keeps game tests independent of HTTP.

## Data Model

Define serde-serializable associated types:

- `State`: authoritative full state, including private information.
- `Action`: tagged enum or struct submitted by humans and agents.
- `Observation`: role-specific state sent to a participant.
- `Event`: role-visible log entries emitted after accepted actions.
- `Summary`: final completion/export summary, including success/failure or another explicit terminal outcome.

Recommended action pattern:

```rust
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(tag = "type")]
pub enum GameAction {
    #[serde(rename = "chooseCard")]
    ChooseCard { player: String, card_id: String },
    #[serde(rename = "submitGuess")]
    SubmitGuess { player: String, answer: String },
}
```

Use `PlayerRole` from `parlando_server`. Convert with `player.as_str()` when pure helpers use string roles.

## Privacy And Validation

Never send private state to the wrong participant. Enforce privacy in `observe_state`, not in React or CSS.

Treat `available_actions` as optional hints:

- Return `None` when the game does not expose action hints.
- Return `Some(vec![])` when it exposes hints but the player has no listed legal actions.
- Return `Some(actions)` for role-specific action choices.

Always keep `validate_action` authoritative. Browsers and agents cannot bypass server validation.

## Completion Contract

Games signal completion to Parlando through the adapter, not through a separate API call. After each accepted human action and after agent actions, `parlando-server` evaluates `adapter.is_complete(&state)`. When it returns true, Parlando:

- marks the in-memory room status as `completed`.
- calls `adapter.completion_summary(&state)`.
- persists a `session_completed` event and stores the serialized summary on the session record.
- broadcasts a WebSocket `completed` message with the serialized `summary` to connected clients.

Use `State` to record enough terminal information to distinguish game-specific outcomes such as success, failure, timeout, invalid final answer, or abandoned objective. `is_complete` should return true for every terminal outcome, not only successful outcomes. `completion_summary` should serialize the outcome with analysis-friendly fields, for example `outcome: "success" | "failure"`, `reason`, score, move count, final choices, or other study-specific measures.

Do not rely on React state, hidden client-only flags, final-page buttons, room disconnects, or export post-processing to complete a game. The server adapter is authoritative for completion.

## Adapter Skeleton

```rust
use anyhow::Result;
use parlando_server::{GameAdapter, PlayerRole};

#[derive(Clone, Debug, Default)]
pub struct MyGameAdapter;

impl MyGameAdapter {
    pub fn new() -> Self {
        Self
    }
}

impl GameAdapter for MyGameAdapter {
    type State = GameState;
    type Action = GameAction;
    type Observation = GameObservation;
    type Event = GameEvent;
    type Summary = GameSummary;

    fn initial_state(&self) -> Self::State {
        initial_state()
    }

    fn validate_action(&self, state: &Self::State, action: &Self::Action, player: PlayerRole) -> Result<()> {
        validate_action(state, action, player.as_str())
    }

    fn apply_action(&self, state: &Self::State, action: &Self::Action) -> Result<Self::State> {
        apply_action(state, action)
    }

    fn observe_state(&self, state: &Self::State, player: PlayerRole) -> Self::Observation {
        observe_state_for_player(state, player.as_str())
    }

    fn available_actions(&self, state: &Self::State, player: PlayerRole) -> Option<Vec<Self::Action>> {
        Some(available_actions(state, player.as_str()))
    }

    fn events_for_action(&self, before: &Self::State, after: &Self::State, action: &Self::Action, player: PlayerRole) -> Vec<Self::Event> {
        events_for_action(before, after, action, player.as_str())
    }

    fn is_complete(&self, state: &Self::State) -> bool {
        is_complete(state)
    }

    fn completion_summary(&self, state: &Self::State) -> Self::Summary {
        completion_summary(state)
    }
}
```

## Main Binary

Generated binaries should declare their compiled game and call `parlando_server::serve_game`:

```rust
use std::net::{IpAddr, SocketAddr};

use anyhow::Result;
use clap::Parser;
use parlando_server::{serve_game, ExperimentConfig, GameDescriptor, ServeOptions};
use serde_json::json;

#[derive(Debug, Parser)]
struct Cli {
    #[arg(long, default_value = "127.0.0.1")]
    host: IpAddr,
    #[arg(long, env = "PORT")]
    port: u16,
    #[arg(long, env = "PARLANDO_DATABASE_URL", default_value = "sqlite:///.local/parlando.sqlite")]
    database_url: String,
    #[arg(long, env = "PARLANDO_CLIENT_DIST", default_value = "client/dist")]
    client_dist: String,
}

/// Starts one compiled game host with database-backed experiment runtimes.
#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let cli = Cli::parse();
    let mut bootstrap = ExperimentConfig::default();
    bootstrap.experiment.id = Some("default".to_string());
    bootstrap.study.name = "My Game".to_string();
    bootstrap.database.url = cli.database_url;
    bootstrap.server.client_dist_path = Some(cli.client_dist);
    bootstrap.server.public_base_url = format!("http://127.0.0.1:{}", cli.port);
    bootstrap.speechmatics.api_key = std::env::var("SPEECHMATICS_API_KEY").unwrap_or_default();
    bootstrap.tts.api_key = std::env::var("ELEVENLABS_API_KEY").unwrap_or_default();
    let descriptor = GameDescriptor {
        id: "my-game".to_string(),
        display_name: "My Game".to_string(),
        version: semver::Version::parse(env!("CARGO_PKG_VERSION"))?,
        build_manifest: json!({"version": env!("CARGO_PKG_VERSION")}),
    };
    serve_game(
        MyGameAdapter::new(),
        bootstrap,
        descriptor,
        SocketAddr::new(cli.host, cli.port),
        |experiment_config| {
            Ok(ServeOptions {
                agent_factory: factory_from_config(experiment_config)?,
                ..ServeOptions::default()
            })
        },
    )
    .await
}
```

If the game has no agents, implement `factory_from_config` as a small helper returning `Ok(None)`.

## Audio Ownership

Generated server crates do not implement media transport. Do not add audio routes, media SDKs, token minting, PCM framing, browser-transcription endpoints, an `AgentAudioPublisher`, or provider clients to the game binary. When enabled by config, `parlando-server` automatically owns:

- opaque one-use room audio credentials;
- `/ws/audio/{room_id}`;
- fixed 24 kHz mono PCM frame validation and partner relay;
- one server-side `TranscriptionProvider` session per human microphone;
- final-utterance persistence and `observe_message` delivery;
- real-time paced agent TTS publication through the room relay.

The game server only supplies `AgentResponse.message` text and role-specific observations. This keeps media behavior identical across generated games and prevents provider secrets from entering game or browser code.

## Testing

Add pure Rust tests for:

- initial state invariants.
- legal and illegal actions.
- role-specific observations and privacy filtering.
- every success and failure completion path, including `is_complete`.
- `completion_summary` serialization, including terminal outcome fields and client/export JSON naming.
- `available_actions` for both roles when used.

When feasible, add a server smoke test that creates a room, connects sockets, submits a terminal action, and checks the resulting WebSocket `completed` message and exported `session_completed` summary.
