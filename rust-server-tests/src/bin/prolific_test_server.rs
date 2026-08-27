//! Minimal compiled Parlando game used only by the standalone Prolific test runner.

use std::{net::SocketAddr, path::PathBuf};

use anyhow::Result;
use clap::Parser;
use parlando::{
    ActionRejection, Game, GameFactory, GameInitializationContext, GameMetadata,
    GameSessionContext, PlayerRole, Server,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Server-process arguments supplied by the independent scenario runner.
#[derive(Debug, Parser)]
#[command(about = "Dummy Parlando game server for Prolific integration tests")]
struct Args {
    /// Loopback address exposed to the test runner and signed launch audience.
    #[arg(long)]
    bind: SocketAddr,
    /// Fresh SQLite file used only for this test run.
    #[arg(long)]
    database: PathBuf,
}

/// Small deterministic state sufficient for session-start and terminal-outcome scenarios.
#[derive(Clone, Debug, Serialize)]
struct FixtureState {
    actions: u64,
    complete: bool,
}

/// One optional game action used by future completion scenarios.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct FixtureAction {
    finish: bool,
}

/// Role-specific fixture observation.
#[derive(Clone, Debug, Serialize)]
struct FixtureObservation {
    role: String,
    actions: u64,
    complete: bool,
}

/// Shared completion fact returned after a finishing action.
#[derive(Clone, Debug, Serialize)]
struct FixtureCompletion {
    actions: u64,
}

/// Stateless game implementation compiled into this test-only server.
struct FixtureGame;

/// Creates one independent fixture game for each admitted Parlando session.
struct FixtureFactory;

impl GameFactory for FixtureFactory {
    type Game = FixtureGame;

    /// Constructs a stateless game because all mutable facts live in `FixtureState`.
    fn create(&self, _context: GameSessionContext) -> Result<FixtureGame> {
        Ok(FixtureGame)
    }
}

impl Game for FixtureGame {
    type Config = Value;
    type State = FixtureState;
    type Action = FixtureAction;
    type Observation = FixtureObservation;
    type Completion = FixtureCompletion;

    /// Creates the fixed initial state used in every Prolific scenario.
    fn initial_state(
        &self,
        _context: GameInitializationContext<'_, Self::Config>,
    ) -> Result<Self::State> {
        Ok(FixtureState {
            actions: 0,
            complete: false,
        })
    }

    /// Applies a deterministic counter update and optional terminal transition.
    fn apply_action(
        &self,
        state: &Self::State,
        action: &Self::Action,
        _actor: PlayerRole,
    ) -> std::result::Result<Self::State, ActionRejection> {
        if state.complete {
            return Err(ActionRejection::new("session_complete"));
        }
        Ok(FixtureState {
            actions: state.actions + 1,
            complete: action.finish,
        })
    }

    /// Projects the small state together with the recipient's stable role.
    fn observation(&self, state: &Self::State, role: PlayerRole) -> Self::Observation {
        FixtureObservation {
            role: role.as_str().to_string(),
            actions: state.actions,
            complete: state.complete,
        }
    }

    /// Returns one completing and one non-completing action for protocol inspection.
    fn available_actions(
        &self,
        _state: &Self::State,
        _role: PlayerRole,
    ) -> Option<Vec<Self::Action>> {
        Some(vec![
            FixtureAction { finish: false },
            FixtureAction { finish: true },
        ])
    }

    /// Returns a completion record only after a finishing action was accepted.
    fn completion(&self, state: &Self::State) -> Option<Self::Completion> {
        state.complete.then_some(FixtureCompletion {
            actions: state.actions,
        })
    }
}

/// Starts a production Parlando server around the test-only compiled game.
#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    if !args.bind.ip().is_loopback() {
        anyhow::bail!("prolific-test-server may listen only on a loopback address");
    }
    let metadata = GameMetadata {
        id: "prolific-integration-fixture".to_string(),
        name: "Prolific Integration Fixture".to_string(),
        version: semver::Version::new(1, 0, 0),
        build_manifest: json!({"purpose": "prolific-integration-testing"}),
    };
    Server::new(FixtureFactory, metadata)?
        .database_url(format!("sqlite:///{}", args.database.display()))
        .public_url(format!("http://{}", args.bind))
        .serve(args.bind)
        .await
}
