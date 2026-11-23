//! Deterministic assertions against a component and its recovery representation.

use anyhow::{ensure, Context, Result};
use parlando::{
    component::{ComponentValue, GameFrame, GameSession, LoadedGame},
    PlayerRole,
};
use serde::Deserialize;
use serde_json::Value;
use std::{collections::HashSet, path::Path};

/// Complete expected game output at one committed frame.
#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct Frame {
    a: Value,
    b: Value,
    #[serde(default, deserialize_with = "present_completion")]
    completion: Option<Value>,
}

/// Distinguishes an explicit null completion from the absence of a result.
fn present_completion<'de, D: serde::Deserializer<'de>>(
    source: D,
) -> Result<Option<Value>, D::Error> {
    Value::deserialize(source).map(Some)
}

impl Frame {
    /// Takes a comparable snapshot of both views and the terminal result.
    fn capture(state: &GameFrame) -> Result<Self> {
        Ok(Self {
            a: serde_json::to_value(state.observation(PlayerRole::A))?,
            b: serde_json::to_value(state.observation(PlayerRole::B))?,
            completion: state.completion().map(serde_json::to_value).transpose()?,
        })
    }

    /// Compares both independently projected views and the terminal result.
    fn verify(&self, actual: &Self) -> Result<()> {
        for (role, expected, observed) in [
            (PlayerRole::A, &self.a, &actual.a),
            (PlayerRole::B, &self.b, &actual.b),
        ] {
            ensure!(
                observed == expected,
                "role {}: expected {expected}, got {observed}",
                role.as_str()
            );
        }
        ensure!(
            actual.completion == self.completion,
            "expected completion {:?}, got {:?}",
            self.completion,
            actual.completion
        );
        Ok(())
    }
}

/// A committed frame and its private transition data from one accepted action.
#[derive(Debug, PartialEq)]
struct Accepted {
    frame: Frame,
    metadata: Option<Value>,
}

impl Accepted {
    /// Captures all observable outputs of an accepted action through the game trait.
    fn capture(after: &GameFrame) -> Result<Self> {
        Ok(Self {
            frame: Frame::capture(after)?,
            metadata: after.transition_metadata(),
        })
    }

    /// Checks recovery and rejected actions leave subsequent accepted outputs equivalent.
    fn verify_control(&self, control: &Self) -> Result<()> {
        ensure!(
            self == control,
            "accepted output diverged from the uninterrupted control: expected {control:?}, got {self:?}"
        );
        Ok(())
    }
}

/// One action and its expected effect on the committed state.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Turn {
    actor: PlayerRole,
    action: Value,
    #[serde(default)]
    reject: Option<String>,
    #[serde(default)]
    recover: bool,
    expect: Frame,
}

/// A seeded component session with expected frames at start and after every attempt.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Scenario {
    name: String,
    seed: u64,
    config: Value,
    initial: Frame,
    steps: Vec<Turn>,
}

/// Parses all declarations before running individual scenarios.
pub(super) fn read(path: &Path) -> Result<Vec<Scenario>> {
    let input =
        std::fs::read(path).with_context(|| format!("reading scenario file {}", path.display()))?;
    let scenarios: Vec<Scenario> = serde_yaml::from_slice(&input)
        .with_context(|| format!("parsing scenario file {}", path.display()))?;
    let mut names = HashSet::new();
    for scenario in &scenarios {
        ensure!(
            !scenario.name.trim().is_empty() && names.insert(&scenario.name),
            "scenario names must be non-empty and unique"
        );
        ensure!(
            !scenario.steps.is_empty(),
            "scenario {:?} requires at least one action",
            scenario.name
        );
    }
    ensure!(
        !scenarios.is_empty(),
        "scenario file requires at least one scenario"
    );
    Ok(scenarios)
}

impl Scenario {
    /// Attaches the scenario identity to any assertion or execution failure.
    pub(super) fn run(&self, loaded_game: &LoadedGame) -> Result<()> {
        self.execute(loaded_game)
            .with_context(|| format!("scenario {:?}", self.name))
    }

    /// Applies each turn through the host's gameplay path and actively exercises restoration.
    fn execute(&self, loaded_game: &LoadedGame) -> Result<()> {
        let mut actual = Session::start(loaded_game, self)?;
        let mut control = Session::start(loaded_game, self)?;
        self.initial
            .verify(&Frame::capture(&actual.state)?)
            .context("initial frame")?;
        self.initial
            .verify(&Frame::capture(&control.state)?)
            .context("control initial frame")?;
        for (index, step) in self.steps.iter().enumerate() {
            step.apply(loaded_game, &mut actual, &mut control)
                .with_context(|| format!("step {}", index + 1))?;
        }
        Ok(())
    }
}

/// Keeps each engine cache paired with its own last committed state.
struct Session {
    game: GameSession,
    state: GameFrame,
}

impl Session {
    /// Initializes an independent execution from the scenario's seed and configuration.
    fn start(loaded_game: &LoadedGame, scenario: &Scenario) -> Result<Self> {
        let game = loaded_game.session();
        let state = game.init(
            &ComponentValue::from_typed(&scenario.config)?,
            scenario.seed,
        )?;
        Ok(Self { game, state })
    }

    /// Reloads the committed state into a fresh execution cache.
    fn recover(&mut self, loaded_game: &LoadedGame) -> Result<()> {
        let persisted = serde_json::to_vec(&self.state)?;
        let game = loaded_game.session();
        let state = serde_json::from_slice(&persisted)?;
        game.validate_replay_state(&state)?;
        *self = Self { game, state };
        Ok(())
    }

    /// Commits accepted actions while preserving the output metadata of that transition.
    fn attempt(
        &mut self,
        actor: PlayerRole,
        action: &ComponentValue,
    ) -> Result<std::result::Result<Accepted, parlando::ActionRejection>> {
        match self.game.step(&self.state, action, actor)? {
            Ok(next) => {
                let output = Accepted::capture(&next)?;
                self.state = next;
                Ok(Ok(output))
            }
            Err(rejection) => Ok(Err(rejection)),
        }
    }
}

impl Turn {
    /// Gives rejected actions and recovery only to the session under test.
    fn apply(
        &self,
        loaded_game: &LoadedGame,
        actual: &mut Session,
        control: &mut Session,
    ) -> Result<()> {
        if self.recover {
            actual.recover(loaded_game)?;
        }
        let action = ComponentValue::from_typed(&self.action)?;
        match (actual.attempt(self.actor, &action)?, &self.reject) {
            (Ok(output), None) => {
                let expected = control.attempt(self.actor, &action)?.map_err(|rejection| {
                    anyhow::anyhow!("control rejected the accepted action: {}", rejection.code)
                })?;
                output.verify_control(&expected)?;
                self.expect.verify(&output.frame)
            }
            (Ok(_), Some(expected)) => {
                anyhow::bail!("expected rejection {expected:?}, action was accepted")
            }
            (Err(actual), None) => anyhow::bail!("unexpected rejection {:?}", actual.code),
            (Err(rejection), Some(expected)) => {
                ensure!(
                    &rejection.code == expected,
                    "expected rejection {expected:?}, got {:?}",
                    rejection.code
                );
                self.expect.verify(&Frame::capture(&actual.state)?)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A rejection can change the following metadata while leaving views and completion intact.
    #[test]
    fn control_comparison_rejects_metadata_only_divergence() {
        let frame = || Frame {
            a: Value::Null,
            b: Value::Null,
            completion: None,
        };
        let control = Accepted {
            frame: frame(),
            metadata: Some(Value::Bool(false)),
        };
        let actual = Accepted {
            frame: frame(),
            metadata: Some(Value::Bool(true)),
        };
        assert!(actual.verify_control(&control).is_err());
    }
}
