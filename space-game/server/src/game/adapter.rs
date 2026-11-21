use parlando_game_sdk::{Rejection as ActionRejection, Role as PlayerRole};
use serde::{Deserialize, Serialize};

use super::state_engine::{
    apply_action, available_actions, derive_systems, initial_state, validate_action,
    FilteredKnowledge, SpaceAction, SpaceCompletion, SpaceGameState, SpaceObservation,
};

/// Game-owned configuration; Space Game currently has no configurable mechanics.
#[derive(Clone, Debug, Default, Deserialize, Serialize, parlando_game_sdk::GameValue)]
#[serde(deny_unknown_fields)]
pub struct SpaceGameConfig {}

/// Owns the authoritative Space Game state for one session.
pub struct SpaceGame {
    state: SpaceGameState,
}

impl SpaceGame {
    /// Rejects unauthorized or unavailable commands before applying them.
    fn apply_action(
        state: &SpaceGameState,
        action: &SpaceAction,
        actor: PlayerRole,
    ) -> std::result::Result<SpaceGameState, ActionRejection> {
        validate_action(state, action, actor.as_str()).map_err(|error| {
            let code = if action.player() != actor.as_str() {
                "wrong_role"
            } else {
                "action_unavailable"
            };
            debug_assert!(!error.to_string().is_empty());
            ActionRejection::new(code)
        })?;
        Ok(apply_action(state, action))
    }

    /// Projects the same controls as the rules engine and only this seat’s private knowledge.
    pub(crate) fn observation(state: &SpaceGameState, player: PlayerRole) -> SpaceObservation {
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
            actions: available_actions(state, player.as_str()),
        }
    }

    /// Reports a launched beacon as the terminal outcome.
    pub(crate) fn outcome(state: &SpaceGameState) -> Option<SpaceCompletion> {
        state.beacon_launched.then(|| SpaceCompletion {
            beacon_launched: state.beacon_launched,
            systems: derive_systems(state),
        })
    }
}

impl parlando_game_sdk::Game for SpaceGame {
    type Config = SpaceGameConfig;
    type Action = SpaceAction;
    type ViewA<'a> = SpaceObservation;
    type ViewB<'a> = SpaceObservation;
    type Completion<'a> = SpaceCompletion;
    type Metadata<'a> = ();

    /// Deals deterministic state and initializes the session-local projections.
    fn init(_config: Self::Config, _seed: u64) -> Result<Self, String> {
        Ok(Self {
            state: initial_state(),
        })
    }

    /// Validates and applies one operation before emitting new observations.
    fn step(&mut self, actor: PlayerRole, action: Self::Action) -> Result<(), ActionRejection> {
        self.state = Self::apply_action(&self.state, &action, actor)?;
        Ok(())
    }

    /// Projects information visible to the first seat.
    fn view_a(&self) -> Self::ViewA<'_> {
        Self::observation(&self.state, PlayerRole::A)
    }

    /// Projects information visible to the second seat.
    fn view_b(&self) -> Self::ViewB<'_> {
        Self::observation(&self.state, PlayerRole::B)
    }

    /// Reports the game-defined success outcome when reached.
    fn completion(&self) -> Option<Self::Completion<'_>> {
        Self::outcome(&self.state)
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
