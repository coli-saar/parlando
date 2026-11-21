//! Native fixture mechanics and backend selection for the stress harness.

use std::path::Path;

use anyhow::{bail, Context, Result};
use clap::ValueEnum;
use parlando::{
    ActionRejection, Game, GameFactory, GameInitializationContext, GameSessionContext, PlayerRole,
};
use parlando_stress_fixture::{
    self as fixture, StressAction, StressCompletion, StressObservation, StressRole, StressState,
};
use serde::Serialize;
use serde_json::Value;

/// Selects which implementation runs the shared deterministic fixture rules.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub(super) enum GameBackend {
    Native,
    Wasm,
}

impl GameBackend {
    /// Names the backend in the hidden server command line and run reports.
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Wasm => "wasm",
        }
    }

    /// Validates startup options before the supervisor starts either child.
    pub(super) fn validate(self, wasm_module: Option<&Path>) -> Result<()> {
        match (self, wasm_module) {
            (Self::Native, None) => Ok(()),
            (Self::Native, Some(_)) => bail!("--wasm-module requires --game-backend wasm"),
            (Self::Wasm, _) if !cfg!(feature = "stress-wasm") => {
                bail!("--game-backend wasm requires building runtime-stress with --features stress-wasm")
            }
            (Self::Wasm, None) => bail!("--game-backend wasm requires --wasm-module"),
            (Self::Wasm, Some(path)) => {
                let metadata = path
                    .metadata()
                    .with_context(|| format!("reading Wasm component path {}", path.display()))?;
                if !metadata.is_file() {
                    bail!("Wasm component path {} must be a file", path.display());
                }
                Ok(())
            }
        }
    }
}

/// Creates a native fixture game for each admitted session.
pub(super) struct StressGameFactory;

impl GameFactory for StressGameFactory {
    type Game = StressGame;

    /// Returns the native mechanics implementation.
    fn create(&self, _: GameSessionContext) -> Result<StressGame> {
        Ok(StressGame)
    }
}

/// Executes the shared fixture rules as native Rust.
pub(super) struct StressGame;

/// Converts the public runtime role to the fixture's dependency-free role.
fn fixture_role(role: PlayerRole) -> StressRole {
    match role {
        PlayerRole::A => StressRole::A,
        PlayerRole::B => StressRole::B,
    }
}

impl Game for StressGame {
    type Config = Value;
    type State = StressState;
    type Action = StressAction;
    type Observation = StressObservation;
    type Completion = StressCompletion;

    /// Builds the authoritative state using the shared fixture mechanics.
    fn initial_state(&self, _: GameInitializationContext<'_, Value>) -> Result<StressState> {
        Ok(fixture::initial_state())
    }

    /// Runs the deterministic transition and preserves its rule rejection code.
    fn apply_action(
        &self,
        state: &StressState,
        action: &StressAction,
        actor: PlayerRole,
    ) -> std::result::Result<StressState, ActionRejection> {
        fixture::apply_action(state, action, fixture_role(actor)).map_err(ActionRejection::new)
    }

    /// Projects the state for the recipient role.
    fn observation(&self, state: &StressState, player: PlayerRole) -> StressObservation {
        fixture::observation(state, fixture_role(player))
    }

    /// Fetches the terminal summary.
    fn completion(&self, state: &StressState) -> Option<StressCompletion> {
        fixture::completion(state)
    }
}
