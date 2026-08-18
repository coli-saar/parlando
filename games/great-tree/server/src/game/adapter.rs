use anyhow::Result;
use parlando::{ActionRejection, Game, PlayerRole};

use super::bijection;
use super::ids::{LimbId, RootId};
use super::mechanics::{apply_set_flow, apply_set_sun, flowered, running, thawed, FlowRejection};
use super::types::{
    GreatTreeAction, GreatTreeCompletion, GreatTreeConfig, GreatTreeObservation, GreatTreeState,
    LimbView, RootView,
};

/// Root is whichever seat `state.crown_seat` isn't.
fn root_role(state: &GreatTreeState) -> PlayerRole {
    match state.crown_seat.role() {
        PlayerRole::A => PlayerRole::B,
        PlayerRole::B => PlayerRole::A,
    }
}

/// `SetSun` belongs to whichever seat is currently Crown; `SetFlow` to the other seat. Which
/// seat that is comes from `state.crown_seat` (set once at `initial_state` from
/// `GreatTreeConfig::crown_seat`), not a fixed role — see types.rs.
fn role_for_action(state: &GreatTreeState, action: &GreatTreeAction) -> PlayerRole {
    match action {
        GreatTreeAction::SetSun { .. } => state.crown_seat.role(),
        GreatTreeAction::SetFlow { .. } => root_role(state),
    }
}

#[derive(Clone, Debug, Default)]
pub struct GreatTree;

impl GreatTree {
    pub fn new() -> Self {
        Self
    }
}

impl Game for GreatTree {
    type Config = GreatTreeConfig;
    type State = GreatTreeState;
    type Action = GreatTreeAction;
    type Observation = GreatTreeObservation;
    type Completion = GreatTreeCompletion;

    fn validate_config(&self, _config: &Self::Config) -> Result<()> {
        Ok(())
    }

    fn initial_state(&self, config: &Self::Config, seed: u64) -> Result<Self::State> {
        let (starting_limb, bijection) = bijection::generate(seed);
        let starting_root = bijection[starting_limb.index()];
        let mut limb_water = [false; 5];
        limb_water[starting_limb.index()] = true;
        Ok(GreatTreeState {
            crown_seat: config.crown_seat,
            sun_holds: vec![starting_limb],
            water_holds: vec![starting_root],
            limb_water,
            bijection,
        })
    }

    fn apply_action(
        &self,
        state: &Self::State,
        action: &Self::Action,
        actor: PlayerRole,
    ) -> std::result::Result<Self::State, ActionRejection> {
        if actor != role_for_action(state, action) {
            return Err(ActionRejection::new("wrong_role"));
        }
        match *action {
            GreatTreeAction::SetSun { limb, lit } => Ok(apply_set_sun(state, limb, lit)),
            GreatTreeAction::SetFlow { root, open } => {
                apply_set_flow(state, root, open).map_err(|error| match error {
                    FlowRejection::RootFrozen => ActionRejection::new("root_frozen"),
                })
            }
        }
    }

    fn observation(&self, state: &Self::State, role: PlayerRole) -> Self::Observation {
        if role == state.crown_seat.role() {
            let mut limbs = [LimbView { id: LimbId::Spire, sun: false, water: false }; 5];
            for (i, &limb) in LimbId::ALL.iter().enumerate() {
                limbs[i] = LimbView {
                    id: limb,
                    sun: state.sun_holds.contains(&limb),
                    water: state.limb_water[limb.index()],
                };
            }
            GreatTreeObservation::Crown { limbs }
        } else {
            let mut roots = [RootView { id: RootId::Hand, thawed: false, running: false }; 5];
            for (i, &root) in RootId::ALL.iter().enumerate() {
                roots[i] = RootView {
                    id: root,
                    thawed: thawed(state, root),
                    running: running(state, root),
                };
            }
            GreatTreeObservation::Root { roots }
        }
    }

    fn available_actions(
        &self,
        _state: &Self::State,
        _role: PlayerRole,
    ) -> Option<Vec<Self::Action>> {
        // Every gate is always clickable; the client toggles based on what it currently
        // renders and reacts to the `root_frozen` rejection when it happens. No pre-filtered
        // legality list is needed for a game this small (spec §13 explicitly leaves this as an
        // implementation choice, not a gameplay requirement).
        None
    }

    fn transition_metadata(
        &self,
        _before: &Self::State,
        _after: &Self::State,
        _action: &Self::Action,
        _actor: PlayerRole,
    ) -> Option<serde_json::Value> {
        // Deferred per spec §11 — not needed for the game to function; add when a real
        // dashboard/analysis need appears.
        None
    }

    fn completion(&self, state: &Self::State) -> Option<Self::Completion> {
        let flowered_limbs: Vec<LimbId> =
            LimbId::ALL.iter().copied().filter(|&limb| flowered(state, limb)).collect();
        if flowered_limbs.len() >= 3 {
            Some(GreatTreeCompletion { flowered_limbs })
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::types::CrownSeat;

    fn config() -> GreatTreeConfig {
        GreatTreeConfig::default()
    }

    #[test]
    fn same_seed_produces_the_same_initial_state() {
        let game = GreatTree::new();
        let a = game.initial_state(&config(), 7).unwrap();
        let b = game.initial_state(&config(), 7).unwrap();
        assert_eq!(a.sun_holds, b.sun_holds);
        assert_eq!(a.water_holds, b.water_holds);
        assert_eq!(a.bijection, b.bijection);
    }

    #[test]
    fn initial_state_has_exactly_one_flowered_limb() {
        let game = GreatTree::new();
        let state = game.initial_state(&config(), 7).unwrap();
        let flowered_count = LimbId::ALL.iter().filter(|&&limb| flowered(&state, limb)).count();
        assert_eq!(flowered_count, 1);
    }

    #[test]
    fn crown_cannot_send_set_flow() {
        let game = GreatTree::new();
        let state = game.initial_state(&config(), 1).unwrap();
        let result = game.apply_action(
            &state,
            &GreatTreeAction::SetFlow { root: RootId::Hand, open: true },
            PlayerRole::A,
        );
        assert_eq!(result.unwrap_err().code, "wrong_role");
    }

    #[test]
    fn root_cannot_send_set_sun() {
        let game = GreatTree::new();
        let state = game.initial_state(&config(), 1).unwrap();
        let result = game.apply_action(
            &state,
            &GreatTreeAction::SetSun { limb: LimbId::Spire, lit: true },
            PlayerRole::B,
        );
        assert_eq!(result.unwrap_err().code, "wrong_role");
    }

    #[test]
    fn crown_observation_never_contains_the_string_bijection() {
        let game = GreatTree::new();
        let state = game.initial_state(&config(), 1).unwrap();
        let obs = game.observation(&state, PlayerRole::A);
        let json = serde_json::to_string(&obs).unwrap().to_lowercase();
        assert!(!json.contains("bijection"));
        assert!(matches!(obs, GreatTreeObservation::Crown { .. }));
    }

    #[test]
    fn root_observation_never_contains_limb_data() {
        let game = GreatTree::new();
        let state = game.initial_state(&config(), 1).unwrap();
        let obs = game.observation(&state, PlayerRole::B);
        assert!(matches!(obs, GreatTreeObservation::Root { .. }));
    }

    #[test]
    fn completion_is_none_below_three_flowered_limbs() {
        let game = GreatTree::new();
        let state = game.initial_state(&config(), 1).unwrap();
        assert_eq!(game.completion(&state), None);
    }

    #[test]
    fn completion_fires_the_instant_a_third_limb_flowers() {
        let game = GreatTree::new();
        let mut state = game.initial_state(&config(), 1).unwrap();
        // Use the real seeded bijection so this test doesn't hardcode pairings.
        let starting_limb = LimbId::ALL.iter().copied().find(|&l| flowered(&state, l)).unwrap();
        let others: Vec<LimbId> =
            LimbId::ALL.iter().copied().filter(|&l| l != starting_limb).collect();

        for &limb in &others[0..2] {
            state = apply_set_sun(&state, limb, true);
            let root = state.bijection[limb.index()];
            state = apply_set_flow(&state, root, true).unwrap();
        }

        assert_eq!(game.completion(&state).unwrap().flowered_limbs.len(), 3);
    }

    #[test]
    fn default_config_makes_crown_player_a() {
        let game = GreatTree::new();
        let state = game.initial_state(&config(), 1).unwrap();
        assert!(matches!(game.observation(&state, PlayerRole::A), GreatTreeObservation::Crown { .. }));
        assert!(matches!(game.observation(&state, PlayerRole::B), GreatTreeObservation::Root { .. }));
    }

    #[test]
    fn crown_seat_b_swaps_which_player_is_crown() {
        let game = GreatTree::new();
        let config = GreatTreeConfig { crown_seat: CrownSeat::B };
        let state = game.initial_state(&config, 1).unwrap();

        assert!(matches!(game.observation(&state, PlayerRole::B), GreatTreeObservation::Crown { .. }));
        assert!(matches!(game.observation(&state, PlayerRole::A), GreatTreeObservation::Root { .. }));

        // SetSun now belongs to B, not A.
        assert_eq!(
            game.apply_action(
                &state,
                &GreatTreeAction::SetSun { limb: LimbId::Spire, lit: true },
                PlayerRole::A,
            )
            .unwrap_err()
            .code,
            "wrong_role"
        );
        assert!(game
            .apply_action(
                &state,
                &GreatTreeAction::SetSun { limb: LimbId::Spire, lit: true },
                PlayerRole::B,
            )
            .is_ok());

        // SetFlow now belongs to A, not B.
        assert_eq!(
            game.apply_action(
                &state,
                &GreatTreeAction::SetFlow { root: RootId::Hand, open: true },
                PlayerRole::B,
            )
            .unwrap_err()
            .code,
            "wrong_role"
        );
    }

    #[test]
    fn crown_seat_config_round_trips_through_yaml_like_the_dashboard_editor() {
        // The admin dashboard's "Game configuration" panel round-trips Game::Config through
        // YAML, not JSON directly — confirm the field name and value survive that path too.
        let yaml = "crownSeat: b\n";
        let config: GreatTreeConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.crown_seat, CrownSeat::B);

        let empty: GreatTreeConfig = serde_yaml::from_str("{}\n").unwrap();
        assert_eq!(empty.crown_seat, CrownSeat::A, "omitted field must default to Crown = A");
    }
}
