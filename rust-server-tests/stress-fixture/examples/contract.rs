//! Shared Wasm game for participant, remote-agent and recruitment contract tests.
#![cfg(target_arch = "wasm32")]

use parlando_game_sdk::{export, Game, GameValue, Rejection, Role};

/// Accepted counters and the intentional rejection exercise delivery to both seats.
#[derive(GameValue)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Action {
    Mark { finish: bool },
    Reject,
}

/// A participant sees only their role and the shared counter.
#[derive(GameValue)]
pub struct View {
    role: &'static str,
    actions: u32,
    done: bool,
}

/// Completion preserves the accepted action count for exports and replay.
#[derive(GameValue)]
pub struct Outcome {
    done: bool,
    actions: u32,
}

/// Each fixture session starts empty and ends on the first finishing mark.
pub struct Contract {
    actions: u32,
    done: bool,
}

impl Contract {
    /// Projects the same accepted count for the addressed participant.
    fn view(&self, role: &'static str) -> View {
        View {
            role,
            actions: self.actions,
            done: self.done,
        }
    }
}

impl Game for Contract {
    type Config = serde_json::Value;
    type Action = Action;
    type ViewA<'a> = View;
    type ViewB<'a> = View;
    type Completion<'a> = Outcome;
    type Metadata<'a> = ();

    /// Initializes the deterministic fixture with no accepted actions.
    fn init(_: Self::Config, _: u64) -> Result<Self, String> {
        Ok(Self {
            actions: 0,
            done: false,
        })
    }

    /// Applies one accepted mark and leaves state unchanged after rejection.
    fn step(&mut self, _: Role, action: Action) -> Result<(), Rejection> {
        let Action::Mark { finish } = action else {
            return Err(Rejection::new("fixture_rejected"));
        };
        if self.done {
            return Err(Rejection::new("game_complete"));
        }
        self.actions += 1;
        self.done = finish;
        Ok(())
    }

    /// Produces the first role's view.
    fn view_a(&self) -> View {
        self.view("A")
    }

    /// Produces the second role's view.
    fn view_b(&self) -> View {
        self.view("B")
    }

    /// Reports the final accepted action count.
    fn completion(&self) -> Option<Outcome> {
        self.done.then_some(Outcome {
            done: true,
            actions: self.actions,
        })
    }
}
export!(Contract);
