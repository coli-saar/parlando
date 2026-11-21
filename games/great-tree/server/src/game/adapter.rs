use parlando_game_sdk::{Rejection as ActionRejection, Role as PlayerRole};

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

/// Owns one tree’s seeded arrangement and current gate positions.
pub struct GreatTree {
    state: GreatTreeState,
}

impl GreatTree {
    /// Deals the starting wet limb and the private mapping for this seed.
    fn deal(config: GreatTreeConfig, seed: u64) -> GreatTreeState {
        let (starting_limb, bijection) = bijection::generate(seed);
        let starting_root = bijection[starting_limb.index()];
        let mut limb_water = [false; 5];
        limb_water[starting_limb.index()] = true;
        GreatTreeState {
            crown_seat: config.crown_seat.unwrap_or_default(),
            sun_holds: vec![starting_limb],
            water_holds: vec![starting_root],
            limb_water,
            bijection,
        }
    }

    /// Applies the action only if its actor owns the relevant controls.
    fn apply_action(
        state: &GreatTreeState,
        action: &GreatTreeAction,
        actor: PlayerRole,
    ) -> std::result::Result<GreatTreeState, ActionRejection> {
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

    /// Projects the corresponding Crown or Root controls from either seat.
    fn observation(state: &GreatTreeState, role: PlayerRole) -> GreatTreeObservation {
        if role == state.crown_seat.role() {
            let mut limbs = [LimbView {
                id: LimbId::Spire,
                sun: false,
                water: false,
            }; 5];
            for (i, &limb) in LimbId::ALL.iter().enumerate() {
                limbs[i] = LimbView {
                    id: limb,
                    sun: state.sun_holds.contains(&limb),
                    water: state.limb_water[limb.index()],
                };
            }
            GreatTreeObservation::Crown { limbs }
        } else {
            let mut roots = [RootView {
                id: RootId::Hand,
                thawed: false,
                running: false,
            }; 5];
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

    /// Reports success after at least three limbs flower.
    fn outcome(state: &GreatTreeState) -> Option<GreatTreeCompletion> {
        let flowered_limbs: Vec<LimbId> = LimbId::ALL
            .iter()
            .copied()
            .filter(|&limb| flowered(state, limb))
            .collect();
        if flowered_limbs.len() >= 3 {
            Some(GreatTreeCompletion { flowered_limbs })
        } else {
            None
        }
    }
}

impl parlando_game_sdk::Game for GreatTree {
    type Config = GreatTreeConfig;
    type Action = GreatTreeAction;
    type ViewA<'a> = GreatTreeObservation;
    type ViewB<'a> = GreatTreeObservation;
    type Completion<'a> = GreatTreeCompletion;
    type Metadata<'a> = ();

    /// Deals deterministic state and initializes the session-local projections.
    fn init(config: Self::Config, seed: u64) -> Result<Self, String> {
        Ok(Self {
            state: Self::deal(config, seed),
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
    use super::super::types::CrownSeat;
    use super::*;

    fn config() -> GreatTreeConfig {
        GreatTreeConfig::default()
    }

    /// Creates test state through the public secret-safe initialization contract.
    fn init(config: &GreatTreeConfig, seed: u64) -> GreatTreeState {
        GreatTree::deal(config.clone(), seed)
    }

    #[test]
    fn same_seed_produces_the_same_initial_state() {
        let a = init(&config(), 7);
        let b = init(&config(), 7);
        assert_eq!(a.sun_holds, b.sun_holds);
        assert_eq!(a.water_holds, b.water_holds);
        assert_eq!(a.bijection, b.bijection);
    }

    #[test]
    fn initial_state_has_exactly_one_flowered_limb() {
        let state = init(&config(), 7);
        let flowered_count = LimbId::ALL
            .iter()
            .filter(|&&limb| flowered(&state, limb))
            .count();
        assert_eq!(flowered_count, 1);
    }

    #[test]
    fn crown_cannot_send_set_flow() {
        let state = init(&config(), 1);
        let result = GreatTree::apply_action(
            &state,
            &GreatTreeAction::SetFlow {
                root: RootId::Hand,
                open: true,
            },
            PlayerRole::A,
        );
        assert_eq!(result.unwrap_err().code(), "wrong_role");
    }

    #[test]
    fn root_cannot_send_set_sun() {
        let state = init(&config(), 1);
        let result = GreatTree::apply_action(
            &state,
            &GreatTreeAction::SetSun {
                limb: LimbId::Spire,
                lit: true,
            },
            PlayerRole::B,
        );
        assert_eq!(result.unwrap_err().code(), "wrong_role");
    }

    #[test]
    fn crown_observation_never_contains_the_string_bijection() {
        let state = init(&config(), 1);
        let obs = GreatTree::observation(&state, PlayerRole::A);
        let json = serde_json::to_string(&obs).unwrap().to_lowercase();
        assert!(!json.contains("bijection"));
        assert!(matches!(obs, GreatTreeObservation::Crown { .. }));
    }

    #[test]
    fn root_observation_never_contains_limb_data() {
        let state = init(&config(), 1);
        let obs = GreatTree::observation(&state, PlayerRole::B);
        assert!(matches!(obs, GreatTreeObservation::Root { .. }));
    }

    #[test]
    fn completion_is_none_below_three_flowered_limbs() {
        let state = init(&config(), 1);
        assert_eq!(GreatTree::outcome(&state), None);
    }

    #[test]
    fn completion_fires_the_instant_a_third_limb_flowers() {
        let mut state = init(&config(), 1);
        // Use the real seeded bijection so this test doesn't hardcode pairings.
        let starting_limb = LimbId::ALL
            .iter()
            .copied()
            .find(|&l| flowered(&state, l))
            .unwrap();
        let others: Vec<LimbId> = LimbId::ALL
            .iter()
            .copied()
            .filter(|&l| l != starting_limb)
            .collect();

        for &limb in &others[0..2] {
            state = apply_set_sun(&state, limb, true);
            let root = state.bijection[limb.index()];
            state = apply_set_flow(&state, root, true).unwrap();
        }

        assert_eq!(GreatTree::outcome(&state).unwrap().flowered_limbs.len(), 3);
    }

    #[test]
    fn default_config_makes_crown_player_a() {
        let state = init(&config(), 1);
        assert!(matches!(
            GreatTree::observation(&state, PlayerRole::A),
            GreatTreeObservation::Crown { .. }
        ));
        assert!(matches!(
            GreatTree::observation(&state, PlayerRole::B),
            GreatTreeObservation::Root { .. }
        ));
    }

    #[test]
    fn crown_seat_b_swaps_which_player_is_crown() {
        let config = GreatTreeConfig {
            crown_seat: Some(CrownSeat::B),
        };
        let state = init(&config, 1);

        assert!(matches!(
            GreatTree::observation(&state, PlayerRole::B),
            GreatTreeObservation::Crown { .. }
        ));
        assert!(matches!(
            GreatTree::observation(&state, PlayerRole::A),
            GreatTreeObservation::Root { .. }
        ));

        // SetSun now belongs to B, not A.
        assert_eq!(
            GreatTree::apply_action(
                &state,
                &GreatTreeAction::SetSun {
                    limb: LimbId::Spire,
                    lit: true
                },
                PlayerRole::A,
            )
            .unwrap_err()
            .code(),
            "wrong_role"
        );
        assert!(GreatTree::apply_action(
            &state,
            &GreatTreeAction::SetSun {
                limb: LimbId::Spire,
                lit: true
            },
            PlayerRole::B,
        )
        .is_ok());

        // SetFlow now belongs to A, not B.
        assert_eq!(
            GreatTree::apply_action(
                &state,
                &GreatTreeAction::SetFlow {
                    root: RootId::Hand,
                    open: true
                },
                PlayerRole::B,
            )
            .unwrap_err()
            .code(),
            "wrong_role"
        );
    }

    #[test]
    fn crown_seat_config_round_trips_through_yaml_like_the_dashboard_editor() {
        // The admin dashboard's "Game configuration" panel round-trips Game::Config through
        // YAML, not JSON directly — confirm the field name and value survive that path too.
        let yaml = "crownSeat: b\n";
        let config: GreatTreeConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.crown_seat, Some(CrownSeat::B));

        let value = parlando_game_sdk::ComponentValue::from_typed(&serde_json::json!({})).unwrap();
        let bytes = value
            .to_schema_messagepack(&<GreatTreeConfig as parlando_game_sdk::GameValue>::schema())
            .unwrap();
        let empty: GreatTreeConfig = parlando_game_sdk::decode_declared(&bytes).unwrap();
        assert_eq!(
            GreatTree::deal(empty, 1).crown_seat,
            CrownSeat::A,
            "omitted field must default to Crown = A"
        );
    }
}
