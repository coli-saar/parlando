use anyhow::Result;
use parlando::{
    ActionRejection, Game, GameFactory, GameInitializationContext, GameSessionContext, PlayerRole,
};
use serde::{Deserialize, Serialize};

/// The four stable semantic choices available to the learning policy.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
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
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
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
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CueChoiceConfig {
    pub level: Level,
}

/// The three authoritative phases of one two-action session.
#[derive(Clone, Debug, Serialize)]
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
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CueChoiceAction {
    Deal,
    Choose { choice: Choice },
}

/// Complete role-safe observation; player B never receives the private cue.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "view", rename_all = "snake_case")]
pub enum CueChoiceObservation {
    Waiting,
    ReadyToDeal,
    Cue { cue: String, trial_nonce: String },
    DealerWaiting,
    Finished { chosen: Choice, correct: bool },
}

/// Shared terminal result safe for delivery to both roles.
#[derive(Clone, Debug, Serialize)]
pub struct CueChoiceCompletion {
    pub chosen: Choice,
    pub correct: bool,
}

/// Session-local cue-choice mechanics.
pub struct CueChoiceGame;

impl Game for CueChoiceGame {
    type Config = CueChoiceConfig;
    type State = CueChoiceState;
    type Action = CueChoiceAction;
    type Observation = CueChoiceObservation;
    type Completion = CueChoiceCompletion;

    /// Creates a deterministic nonce without revealing the correct choice.
    fn initial_state(
        &self,
        context: GameInitializationContext<'_, Self::Config>,
    ) -> Result<Self::State> {
        Ok(CueChoiceState::AwaitingDeal {
            level: context.config.level,
            nonce: format!("trial-{:016x}", context.seed),
        })
    }

    /// Applies the phase- and role-authorized transition atomically.
    fn apply_action(
        &self,
        state: &Self::State,
        action: &Self::Action,
        actor: PlayerRole,
    ) -> std::result::Result<Self::State, ActionRejection> {
        match (state, action, actor) {
            (
                CueChoiceState::AwaitingDeal { level, nonce },
                CueChoiceAction::Deal,
                PlayerRole::B,
            ) => Ok(CueChoiceState::Choosing {
                level: *level,
                nonce: nonce.clone(),
            }),
            (
                CueChoiceState::Choosing { level, nonce },
                CueChoiceAction::Choose { choice },
                PlayerRole::A,
            ) => Ok(CueChoiceState::Complete {
                level: *level,
                nonce: nonce.clone(),
                chosen: *choice,
                correct: *choice == level.correct_choice(),
            }),
            (CueChoiceState::Complete { .. }, _, _) => Err(ActionRejection::new("game_complete")),
            (_, CueChoiceAction::Deal, PlayerRole::A)
            | (_, CueChoiceAction::Choose { .. }, PlayerRole::B) => {
                Err(ActionRejection::new("wrong_role"))
            }
            _ => Err(ActionRejection::new("wrong_phase")),
        }
    }

    /// Reveals the cue only to the learner while preserving a shared completion result.
    fn observation(&self, state: &Self::State, role: PlayerRole) -> Self::Observation {
        match (state, role) {
            (CueChoiceState::AwaitingDeal { .. }, PlayerRole::A) => CueChoiceObservation::Waiting,
            (CueChoiceState::AwaitingDeal { .. }, PlayerRole::B) => {
                CueChoiceObservation::ReadyToDeal
            }
            (CueChoiceState::Choosing { level, nonce }, PlayerRole::A) => {
                CueChoiceObservation::Cue {
                    cue: level.cue().to_string(),
                    trial_nonce: nonce.clone(),
                }
            }
            (CueChoiceState::Choosing { .. }, PlayerRole::B) => CueChoiceObservation::DealerWaiting,
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

    /// Enumerates the one dealer action or four learner actions in their valid phase.
    fn available_actions(
        &self,
        state: &Self::State,
        role: PlayerRole,
    ) -> Option<Vec<Self::Action>> {
        Some(match (state, role) {
            (CueChoiceState::AwaitingDeal { .. }, PlayerRole::B) => vec![CueChoiceAction::Deal],
            (CueChoiceState::Choosing { .. }, PlayerRole::A) => Choice::all()
                .into_iter()
                .map(|choice| CueChoiceAction::Choose { choice })
                .collect(),
            _ => Vec::new(),
        })
    }

    /// Returns the shared correctness result exactly in the terminal phase.
    fn completion(&self, state: &Self::State) -> Option<Self::Completion> {
        match state {
            CueChoiceState::Complete {
                chosen, correct, ..
            } => Some(CueChoiceCompletion {
                chosen: *chosen,
                correct: *correct,
            }),
            _ => None,
        }
    }
}

/// Reusable constructor for session-local cue-choice games.
pub struct CueChoiceFactory;

impl GameFactory for CueChoiceFactory {
    type Game = CueChoiceGame;

    /// Creates one stateless session-local mechanics value.
    fn create(&self, _context: GameSessionContext) -> Result<Self::Game> {
        Ok(CueChoiceGame)
    }

    /// Rejects no compiled level; serde already enforces the closed catalogue.
    fn validate_config(&self, _config: &CueChoiceConfig) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parlando::SecretValues;

    /// Builds the deterministic initial state used by mechanics tests.
    fn initial(level: Level, seed: u64) -> CueChoiceState {
        CueChoiceGame
            .initial_state(GameInitializationContext {
                config: &CueChoiceConfig { level },
                seed,
                secrets: &SecretValues::default(),
            })
            .unwrap()
    }

    /// Exercises authorization, privacy, action catalogues, and terminal correctness.
    #[test]
    fn complete_session_preserves_private_cue_and_role_rules() {
        let game = CueChoiceGame;
        let state = initial(Level::Dax, 7);
        assert!(matches!(
            game.observation(&state, PlayerRole::B),
            CueChoiceObservation::ReadyToDeal
        ));
        assert!(matches!(
            game.observation(&state, PlayerRole::A),
            CueChoiceObservation::Waiting
        ));
        assert_eq!(
            game.apply_action(&state, &CueChoiceAction::Deal, PlayerRole::A)
                .unwrap_err()
                .code,
            "wrong_role"
        );
        let choosing = game
            .apply_action(&state, &CueChoiceAction::Deal, PlayerRole::B)
            .unwrap();
        assert_eq!(
            game.available_actions(&choosing, PlayerRole::A)
                .unwrap()
                .len(),
            4
        );
        assert!(
            !serde_json::to_string(&game.observation(&choosing, PlayerRole::B))
                .unwrap()
                .contains("dax")
        );
        let complete = game
            .apply_action(
                &choosing,
                &CueChoiceAction::Choose {
                    choice: Choice::Oak,
                },
                PlayerRole::A,
            )
            .unwrap();
        assert!(game.completion(&complete).unwrap().correct);
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
