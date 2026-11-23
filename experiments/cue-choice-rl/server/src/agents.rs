//! Training reward computed from the terminal game frame.
use crate::game::CueChoiceCompletion;
use anyhow::Result;
use parlando::{
    component::{ComponentValue, GameFrame},
    PlayerRole, RewardFunction, RoleRewards,
};
use serde_json::Value;

/// Game-specific correctness reward selected by training YAML.
pub struct CorrectnessReward;

impl RewardFunction for CorrectnessReward {
    /// Returns the stable registry key used by experiment YAML.
    fn id(&self) -> &'static str {
        "cue_choice.correctness"
    }

    /// Returns semantic reward implementation provenance.
    fn version(&self) -> &'static str {
        "1"
    }

    /// Accepts only the empty parameter object used by this fixed objective.
    fn configure(&self, parameters: &Value) -> Result<Self> {
        anyhow::ensure!(
            parameters
                .as_object()
                .is_some_and(serde_json::Map::is_empty),
            "cue-choice correctness reward accepts no parameters"
        );
        Ok(Self)
    }

    /// Rewards only the learner and only for its terminal choice.
    fn rewards(
        &self,
        _before: &GameFrame,
        _after: &GameFrame,
        actor: PlayerRole,
        _action: &ComponentValue,
        completion: Option<&ComponentValue>,
    ) -> RoleRewards {
        let player_a = match (
            actor,
            completion.and_then(|value| value.decode::<CueChoiceCompletion>().ok()),
        ) {
            (PlayerRole::A, Some(result)) if result.correct => 1.0,
            (PlayerRole::A, Some(_)) => -1.0,
            _ => 0.0,
        };
        RoleRewards {
            player_a,
            player_b: 0.0,
        }
    }
}
