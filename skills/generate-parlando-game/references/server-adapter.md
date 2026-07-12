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
- `Summary`: final completion/export summary.

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

Generated binaries should load config and call `parlando_server::serve`:

```rust
use std::{
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    sync::Arc,
};

use anyhow::Result;
use clap::Parser;
use parlando_server::{serve, ExperimentConfig, LiveKitAgentAudioPublisher, ServeOptions};

#[derive(Debug, Parser)]
struct Cli {
    #[arg(long, short)]
    config: Option<PathBuf>,
    #[arg(long, default_value = "127.0.0.1")]
    host: IpAddr,
    #[arg(long)]
    port: Option<u16>,
    #[arg(long)]
    experiment_id: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let cli = Cli::parse();
    let mut config = if let Some(path) = cli.config {
        ExperimentConfig::from_yaml(path)?
    } else {
        ExperimentConfig::default()
    };
    if cli.experiment_id.is_some() {
        config.experiment.id = cli.experiment_id;
    }
    let port = cli
        .port
        .or_else(|| std::env::var("PORT").ok().and_then(|value| value.parse().ok()))
        .unwrap_or(8000);
    let agent_factory = factory_from_config(&config)?;
    let audio_publisher = if config.livekit.enabled && config.tts.enabled {
        Some(Arc::new(LiveKitAgentAudioPublisher::new(config.livekit.clone())) as _)
    } else {
        None
    };
    serve(
        MyGameAdapter::new(),
        config,
        SocketAddr::new(cli.host, port),
        ServeOptions {
            agent_factory,
            audio_publisher,
            ..ServeOptions::default()
        },
    )
    .await
}
```

If the game has no agents, implement `factory_from_config` as a small helper returning `Ok(None)`.

## Testing

Add pure Rust tests for:

- initial state invariants.
- legal and illegal actions.
- role-specific observations and privacy filtering.
- completion and summary.
- `available_actions` for both roles when used.

When feasible, add a server smoke test that creates a room, connects sockets, submits an action, and checks the resulting observation or export.
