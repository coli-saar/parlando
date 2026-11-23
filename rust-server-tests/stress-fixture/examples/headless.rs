//! Deterministic counter guest for the host's headless experiment lifecycle tests.

#![cfg(target_arch = "wasm32")]

use parlando_game_sdk::{export, Game, GameValue, Rejection, Role};
use serde::Deserialize;

/// Validated scenario configuration.
#[derive(GameValue, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    target: u8,
}

/// Proposed increment. Zero rejects while the maximum traps.
#[derive(GameValue, Deserialize)]
pub struct Action {
    amount: u8,
}

/// Role-safe counter observation.
#[derive(GameValue)]
pub struct Observation {
    count: u8,
    role: &'static str,
}

/// Shared counter at completion.
#[derive(GameValue)]
pub struct Completion {
    count: u8,
}

/// Independent scenario state.
pub struct Counter {
    count: u8,
    target: u8,
}

impl Game for Counter {
    type Config = Config;
    type Action = Action;
    type ViewA<'a> = Observation;
    type ViewB<'a> = Observation;
    type Completion<'a> = Completion;
    type Metadata<'a> = ();

    /// Starts each scenario at zero with its configured goal.
    fn init(config: Config, _seed: u64) -> Result<Self, String> {
        Ok(Self {
            count: 0,
            target: config.target,
        })
    }

    /// Applies a positive increment and exposes separate rejection and trap paths.
    fn step(&mut self, _actor: Role, action: Action) -> Result<(), Rejection> {
        assert_ne!(action.amount, u8::MAX, "guest step failed");
        if action.amount == 0 {
            return Err(Rejection::new("zero_increment"));
        }
        self.count = self.count.saturating_add(action.amount);
        Ok(())
    }

    /// Reveals the count with the recipient's role.
    fn view_a(&self) -> Observation {
        Observation {
            count: self.count,
            role: "A",
        }
    }

    /// Reveals the count with the recipient's role.
    fn view_b(&self) -> Observation {
        Observation {
            count: self.count,
            role: "B",
        }
    }

    /// Reports completion as soon as the target is reached.
    fn completion(&self) -> Option<Completion> {
        (self.count >= self.target).then_some(Completion { count: self.count })
    }
}

export!(Counter);
