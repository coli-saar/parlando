use parlando_game_sdk::{Game, GameValue, Rejection, Role};
use serde::{Deserialize, Serialize};

/// The four stable semantic choices available to the learning policy.
#[derive(Clone, Copy, Debug, GameValue, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Choice {
    Oak,
    Pine,
    Birch,
    Elm,
}

impl Choice {
    /// Returns all choices in the stable order used by the policy head.
    pub const fn all() -> [Self; 4] {
        [Self::Oak, Self::Pine, Self::Birch, Self::Elm]
    }
}

/// One compiled cue-to-choice level selected by experiment configuration.
#[derive(Clone, Copy, Debug, GameValue, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Dax,
    Wug,
    Kiki,
    Zorp,
}

impl Level {
    /// Returns the cue visible to player A.
    pub const fn cue(self) -> &'static str {
        match self {
            Self::Dax => "dax",
            Self::Wug => "wug",
            Self::Kiki => "kiki",
            Self::Zorp => "zorp",
        }
    }

    /// Returns the authoritative choice hidden from both role observations.
    pub const fn correct_choice(self) -> Choice {
        match self {
            Self::Dax => Choice::Oak,
            Self::Wug => Choice::Pine,
            Self::Kiki => Choice::Birch,
            Self::Zorp => Choice::Elm,
        }
    }
}

/// Strict game configuration containing only a compiled level selector.
#[derive(Clone, Debug, GameValue, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CueChoiceConfig {
    pub level: Level,
}

/// The three authoritative phases of one two-action session.
#[derive(Clone, Debug, GameValue, Deserialize, Serialize)]
pub enum CueChoiceState {
    AwaitingDeal {
        level: Level,
        nonce: String,
    },
    Choosing {
        level: Level,
        nonce: String,
    },
    Complete {
        level: Level,
        nonce: String,
        chosen: Choice,
        correct: bool,
    },
}

/// Typed actions accepted from the dealer and learner roles.
#[derive(Clone, Debug, GameValue, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CueChoiceAction {
    Deal,
    Choose { choice: Choice },
}

/// Complete role-safe observation; player B never receives the private cue.
#[derive(Clone, Debug, GameValue, Deserialize, Serialize)]
#[serde(tag = "view", rename_all = "snake_case")]
pub enum CueChoiceObservation {
    Waiting,
    ReadyToDeal,
    Cue {
        cue: String,
        trial_nonce: String,
        available_actions: Vec<CueChoiceAction>,
    },
    DealerWaiting,
    Finished {
        chosen: Choice,
        correct: bool,
    },
}

/// Shared terminal result safe for delivery to both roles.
#[derive(Clone, Debug, GameValue, Deserialize, Serialize)]
pub struct CueChoiceCompletion {
    pub chosen: Choice,
    pub correct: bool,
}

impl Game for CueChoiceState {
    type Config = CueChoiceConfig;
    type Action = CueChoiceAction;
    type ViewA<'a> = CueChoiceObservation;
    type ViewB<'a> = CueChoiceObservation;
    type Completion<'a> = CueChoiceCompletion;
    type Metadata<'a> = ();

    /// Creates a deterministic nonce without revealing the correct choice.
    fn init(config: Self::Config, seed: u64) -> Result<Self, String> {
        Ok(Self::AwaitingDeal {
            level: config.level,
            nonce: format!("trial-{seed:016x}"),
        })
    }

    /// Applies the phase- and role-authorized transition atomically.
    fn step(&mut self, actor: Role, action: Self::Action) -> Result<(), Rejection> {
        let next = match (&*self, action, actor) {
            (CueChoiceState::AwaitingDeal { level, nonce }, CueChoiceAction::Deal, Role::B) => {
                CueChoiceState::Choosing {
                    level: *level,
                    nonce: nonce.clone(),
                }
            }
            (
                CueChoiceState::Choosing { level, nonce },
                CueChoiceAction::Choose { choice },
                Role::A,
            ) => CueChoiceState::Complete {
                level: *level,
                nonce: nonce.clone(),
                chosen: choice,
                correct: choice == level.correct_choice(),
            },
            (CueChoiceState::Complete { .. }, _, _) => return Err(Rejection::new("game_complete")),
            (_, CueChoiceAction::Deal, Role::A) | (_, CueChoiceAction::Choose { .. }, Role::B) => {
                return Err(Rejection::new("wrong_role"))
            }
            _ => return Err(Rejection::new("wrong_phase")),
        };
        *self = next;
        Ok(())
    }

    /// Projects the learner view with legal choices when the cue is available.
    fn view_a(&self) -> Self::ViewA<'_> {
        self.observation(Role::A)
    }

    /// Projects the dealer phase without disclosing the private cue.
    fn view_b(&self) -> Self::ViewB<'_> {
        self.observation(Role::B)
    }

    /// Returns the shared correctness result exactly in the terminal phase.
    fn completion(&self) -> Option<Self::Completion<'_>> {
        match self {
            Self::Complete {
                chosen, correct, ..
            } => Some(CueChoiceCompletion {
                chosen: *chosen,
                correct: *correct,
            }),
            _ => None,
        }
    }
}

impl CueChoiceState {
    /// Reveals the cue only to the learner while preserving a shared completion result.
    fn observation(&self, role: Role) -> CueChoiceObservation {
        match (self, role) {
            (CueChoiceState::AwaitingDeal { .. }, Role::A) => CueChoiceObservation::Waiting,
            (CueChoiceState::AwaitingDeal { .. }, Role::B) => CueChoiceObservation::ReadyToDeal,
            (CueChoiceState::Choosing { level, nonce }, Role::A) => CueChoiceObservation::Cue {
                cue: level.cue().to_string(),
                trial_nonce: nonce.clone(),
                available_actions: Choice::all()
                    .into_iter()
                    .map(|choice| CueChoiceAction::Choose { choice })
                    .collect(),
            },
            (CueChoiceState::Choosing { .. }, Role::B) => CueChoiceObservation::DealerWaiting,
            (
                CueChoiceState::Complete {
                    chosen, correct, ..
                },
                _,
            ) => CueChoiceObservation::Finished {
                chosen: *chosen,
                correct: *correct,
            },
        }
    }
}

#[cfg(target_arch = "wasm32")]
parlando_game_sdk::export!(CueChoiceState);

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds the deterministic initial state used by mechanics tests.
    fn initial(level: Level, seed: u64) -> CueChoiceState {
        CueChoiceState::init(CueChoiceConfig { level }, seed).unwrap()
    }

    /// Exercises authorization, privacy, action catalogues, and terminal correctness.
    #[test]
    fn complete_session_preserves_private_cue_and_role_rules() {
        let mut game = initial(Level::Dax, 7);
        assert!(matches!(game.view_b(), CueChoiceObservation::ReadyToDeal));
        assert!(matches!(game.view_a(), CueChoiceObservation::Waiting));
        assert_eq!(
            game.step(Role::A, CueChoiceAction::Deal)
                .unwrap_err()
                .code(),
            "wrong_role"
        );
        game.step(Role::B, CueChoiceAction::Deal).unwrap();
        let CueChoiceObservation::Cue {
            available_actions, ..
        } = game.view_a()
        else {
            panic!("dealer did not reveal cue");
        };
        assert_eq!(available_actions.len(), 4);
        assert!(!serde_json::to_string(&game.view_b())
            .unwrap()
            .contains("dax"));
        game.step(
            Role::A,
            CueChoiceAction::Choose {
                choice: Choice::Oak,
            },
        )
        .unwrap();
        assert!(game.completion().unwrap().correct);
    }

    /// Confirms seeds affect only the visible nonce and remain deterministic.
    #[test]
    fn seed_controls_deterministic_nonce() {
        assert_eq!(
            serde_json::to_string(&initial(Level::Wug, 1)).unwrap(),
            serde_json::to_string(&initial(Level::Wug, 1)).unwrap()
        );
        assert_ne!(
            serde_json::to_string(&initial(Level::Wug, 1)).unwrap(),
            serde_json::to_string(&initial(Level::Wug, 2)).unwrap()
        );
    }
}
