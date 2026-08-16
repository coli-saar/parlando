use anyhow::Result;
use parlando::{ActionRejection, Game, PlayerRole};
use serde::{Deserialize, Serialize};

use super::state_engine::{
    apply_action, available_actions, derive_systems, initial_state, validate_action,
    FilteredKnowledge, SpaceAction, SpaceCompletion, SpaceGameState, SpaceObservation,
};

/// Game-owned configuration; Space Game currently has no configurable mechanics.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SpaceGameConfig {}

#[derive(Clone, Debug, Default)]
/// Defines the Space Game mechanics used by the reusable Parlando runtime.
pub struct SpaceGame;

impl SpaceGame {
    /// Creates the stateless game used by every experiment runtime in this process.
    pub fn new() -> Self {
        Self
    }
}

impl Game for SpaceGame {
    type Config = SpaceGameConfig;
    type State = SpaceGameState;
    type Action = SpaceAction;
    type Observation = SpaceObservation;
    type Completion = SpaceCompletion;

    fn initial_state(&self, _config: &Self::Config, _seed: u64) -> Result<Self::State> {
        Ok(initial_state())
    }

    fn apply_action(
        &self,
        state: &Self::State,
        action: &Self::Action,
        actor: PlayerRole,
    ) -> std::result::Result<Self::State, ActionRejection> {
        validate_action(state, action, actor.as_str()).map_err(|error| {
            let code = if action.player() != Some(actor.as_str()) {
                "wrong_role"
            } else {
                "action_unavailable"
            };
            debug_assert!(!error.to_string().is_empty());
            ActionRejection::new(code)
        })?;
        apply_action(state, action).map_err(|_| ActionRejection::new("invalid_action"))
    }

    fn observation(&self, state: &Self::State, player: PlayerRole) -> Self::Observation {
        SpaceObservation {
            players: state.players.clone(),
            fuses: state.fuses.clone(),
            breakers: state.breakers.clone(),
            valves: state.valves.clone(),
            override_held: state.override_held,
            battery: state.battery.clone(),
            relay: state.relay.clone(),
            pressure_drained: state.pressure_drained,
            oxygen_fan_tripped: state.oxygen_fan_tripped,
            visual_effects: state.visual_effects.clone(),
            beacon_launched: state.beacon_launched,
            role: player.as_str().to_string(),
            systems: derive_systems(state),
            knowledge: FilteredKnowledge {
                a: if player == PlayerRole::A {
                    state.knowledge.a.clone()
                } else {
                    vec![]
                },
                b: if player == PlayerRole::B {
                    state.knowledge.b.clone()
                } else {
                    vec![]
                },
            },
            private_knowledge: if player == PlayerRole::A {
                state.knowledge.a.clone()
            } else {
                state.knowledge.b.clone()
            },
            log: vec![],
        }
    }

    fn available_actions(
        &self,
        state: &Self::State,
        player: PlayerRole,
    ) -> Option<Vec<Self::Action>> {
        Some(available_actions(state, player.as_str()))
    }

    fn completion(&self, state: &Self::State) -> Option<Self::Completion> {
        state.beacon_launched.then(|| SpaceCompletion {
            beacon_launched: state.beacon_launched,
            systems: derive_systems(state),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Confirms the dashboard cannot save game-owned keys that Space Game would ignore.
    #[test]
    fn game_configuration_rejects_unknown_options() {
        let _: SpaceGameConfig = serde_json::from_value(serde_json::json!({})).unwrap();
        assert!(
            serde_json::from_value::<SpaceGameConfig>(serde_json::json!({"difficulty": 2}))
                .is_err()
        );
    }
}
