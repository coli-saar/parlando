//! Small component for exercising live HTTP and storage behavior.

use std::collections::BTreeMap;

use parlando_game_sdk::{Game, GameValue, Rejection, Role};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize, Deserialize)]
struct Fixture {
    done: bool,
    checkpointed: bool,
    loss: bool,
    total: Option<u32>,
}

#[derive(GameValue)]
struct View {
    done: bool,
    role: &'static str,
    total: Option<u32>,
}

#[derive(GameValue)]
struct Completion {
    done: bool,
    outcome: &'static str,
    dyad_score: i64,
    player_scores: BTreeMap<String, i64>,
}

impl Fixture {
    /// Projects the active test mode's state with the private recipient label.
    fn view(&self, role: &'static str) -> View {
        View {
            done: self.done,
            role,
            total: self.total,
        }
    }
}

impl Game for Fixture {
    type Config = Value;
    type Action = Value;
    type ViewA<'a> = View;
    type ViewB<'a> = View;
    type Completion<'a> = Completion;
    type Metadata<'a> = ();

    /// Selects fixture rules from the experiment configuration.
    fn init(config: Value, _: u64) -> Result<Self, String> {
        Ok(Self {
            done: false,
            checkpointed: config["checkpointed"] == true,
            loss: config["loss"] == true,
            total: (config["counter"] == true).then_some(0),
        })
    }

    /// Adds one counter increment or accepts a finish request.
    fn step(&mut self, _: Role, action: Value) -> Result<(), Rejection> {
        if let Some(total) = &mut self.total {
            let increment = action
                .as_u64()
                .and_then(|value| u32::try_from(value).ok())
                .ok_or_else(|| Rejection::new("invalid_tiny_action"))?;
            *total = total
                .checked_add(increment)
                .ok_or_else(|| Rejection::new("invalid_tiny_action"))?;
        } else {
            if action["invalid"] == true {
                return Err(Rejection::new("invalid_tiny_action"));
            }
            self.done = action["finish"]
                .as_bool()
                .ok_or_else(|| Rejection::new("invalid_action"))?;
        }
        Ok(())
    }

    /// Provides role A's view.
    fn view_a(&self) -> View {
        self.view("A")
    }

    /// Provides role B's view.
    fn view_b(&self) -> View {
        self.view("B")
    }

    /// Reports the fixture outcome independently of infrastructure completion.
    fn completion(&self) -> Option<Completion> {
        self.done.then(|| {
            let (outcome, dyad_score, a, b) = if self.loss {
                ("loss", 0, 0, 0)
            } else {
                ("success", 10, 6, 4)
            };
            Completion {
                done: self.done,
                outcome,
                dyad_score,
                player_scores: [("A".into(), a), ("B".into(), b)].into(),
            }
        })
    }

    /// Snapshots checkpoint-mode turns for administrative recovery tests.
    fn checkpoint(&self) -> Option<Vec<u8>> {
        (self.checkpointed && self.total != Some(0))
            .then(|| serde_json::to_vec(self).expect("serializable fixture"))
    }

    /// Restores the fixture from its private JSON checkpoint bytes.
    fn restore(checkpoint: &[u8]) -> Result<Self, String> {
        serde_json::from_slice(checkpoint).map_err(|error| error.to_string())
    }
}

parlando_game_sdk::export!(Fixture);
