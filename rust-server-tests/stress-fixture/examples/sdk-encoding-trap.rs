//! Exercises host recovery when a typed game mutates state and its view fails to encode.

#![cfg(target_arch = "wasm32")]

use parlando_game_sdk::{export, Game, GameValue, Rejection, Role};

/// The input cases for a normal transition and a transition with a broken view.
#[derive(GameValue)]
#[serde(rename_all = "snake_case")]
enum Action {
    Increment,
    InvalidView,
}

/// Counts accepted transitions and lets a test action produce a non-finite view.
struct Fixture {
    count: u64,
    invalid_view: bool,
}

impl Game for Fixture {
    type Config = ();
    type Action = Action;
    type ViewA<'a> = f64;
    type ViewB<'a> = u64;
    type Completion<'a> = ();
    type Metadata<'a> = ();

    /// Begins at the recorded seed with two encodable views.
    fn init(_: (), seed: u64) -> Result<Self, String> {
        Ok(Self {
            count: seed,
            invalid_view: false,
        })
    }

    /// Commits an increment before the SDK tries to encode either view.
    fn step(&mut self, _: Role, action: Action) -> Result<(), Rejection> {
        self.count += 1;
        self.invalid_view = matches!(action, Action::InvalidView);
        Ok(())
    }

    /// Produces a non-finite float if the action injected an encoding failure.
    fn view_a(&self) -> f64 {
        if self.invalid_view {
            f64::NAN
        } else {
            self.count as f64
        }
    }

    /// Returns the count as an integer for precise replay assertions.
    fn view_b(&self) -> u64 {
        self.count
    }
}

export!(Fixture);
