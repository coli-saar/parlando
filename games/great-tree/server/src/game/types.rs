use parlando::PlayerRole;
use serde::{Deserialize, Serialize};

use super::ids::{LimbId, RootId};

/// Which seat (`PlayerRole::A` or `PlayerRole::B`) controls Crown for a session. Root is
/// whichever seat this isn't. An administrator sets this per experiment; it has no gameplay
/// effect of its own, it only decides which invitation link is Crown and which is Root.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum CrownSeat {
    A,
    B,
}

impl Default for CrownSeat {
    /// Matches this game's original, still-hardcoded behaviour: Crown = A.
    fn default() -> Self {
        CrownSeat::A
    }
}

impl CrownSeat {
    pub fn role(self) -> PlayerRole {
        match self {
            CrownSeat::A => PlayerRole::A,
            CrownSeat::B => PlayerRole::B,
        }
    }
}

/// Game-owned configuration, edited as YAML in the experiment's "Game configuration" panel in
/// the admin dashboard (`/admin/experiments`).
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct GreatTreeConfig {
    /// Which seat is Crown for this experiment. Defaults to `a` (Crown = Player A), matching
    /// the game's original fixed assignment. Set to `b` to swap the two invitation links'
    /// roles without changing anything else.
    #[serde(default)]
    pub crown_seat: CrownSeat,
}

/// Authoritative, server-only state. Never sent to a participant.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct GreatTreeState {
    /// Which seat is Crown for this session, copied from `GreatTreeConfig::crown_seat` at
    /// `initial_state` (the only point the `Game` trait hands config to game code — see
    /// adapter.rs). Root is whichever seat this isn't.
    pub crown_seat: CrownSeat,
    /// Limbs Crown currently holds sun on, oldest first. `len() <= 3`.
    pub sun_holds: Vec<LimbId>,
    /// Roots Root currently holds open, oldest first. `len() <= 3`. Invariant: every root in
    /// this list is currently thawed (its warming limb is in `sun_holds`) — see mechanics.rs.
    pub water_holds: Vec<RootId>,
    /// Per-limb "water has reached this limb" flag, indexed by `LimbId::index()`. Cleared
    /// whenever Root closes the paired root (explicitly or via FIFO eviction) *and* whenever
    /// Crown removes sun from this limb (explicitly or via FIFO eviction) — losing sun dries
    /// the limb out completely, not just its root. See mechanics.rs for the full reasoning.
    pub limb_water: [bool; 5],
    /// `bijection[limb.index()]` = the one root that limb both warms and is fed by (spec rule
    /// 7). Generated once in `initial_state`; never exposed in any `Observation`.
    pub bijection: [RootId; 5],
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum GreatTreeAction {
    SetSun { limb: LimbId, lit: bool },
    SetFlow { root: RootId, open: bool },
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LimbView {
    pub id: LimbId,
    pub sun: bool,
    pub water: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RootView {
    pub id: RootId,
    pub thawed: bool,
    pub running: bool,
}

/// One Rust type serving both roles' `Game::observation`, discriminated on the wire by `role`.
/// Each variant carries only that role's own five items — Crown's variant has no way to carry
/// root or bijection data even by mistake, which is the point (spec §4's privacy requirement).
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "role", rename_all = "camelCase")]
pub enum GreatTreeObservation {
    Crown { limbs: [LimbView; 5] },
    Root { roots: [RootView; 5] },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GreatTreeCompletion {
    /// Which limbs were flowering at the moment three-of-five was reached (spec §8).
    pub flowered_limbs: Vec<LimbId>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_rejects_unknown_fields() {
        let _: GreatTreeConfig = serde_json::from_value(serde_json::json!({})).unwrap();
        assert!(
            serde_json::from_value::<GreatTreeConfig>(serde_json::json!({"difficulty": 2}))
                .is_err()
        );
    }

    #[test]
    fn set_sun_action_serializes_with_a_type_tag() {
        let action = GreatTreeAction::SetSun {
            limb: LimbId::Hook,
            lit: true,
        };
        let value = serde_json::to_value(&action).unwrap();
        assert_eq!(value["type"], "setSun");
        assert_eq!(value["limb"], "hook");
        assert_eq!(value["lit"], true);
    }

    #[test]
    fn set_flow_action_serializes_with_a_type_tag() {
        let action = GreatTreeAction::SetFlow {
            root: RootId::Deep,
            open: false,
        };
        let value = serde_json::to_value(&action).unwrap();
        assert_eq!(value["type"], "setFlow");
        assert_eq!(value["root"], "deep");
        assert_eq!(value["open"], false);
    }

    #[test]
    fn crown_observation_carries_only_limbs_no_root_or_bijection_keys() {
        let obs = GreatTreeObservation::Crown {
            limbs: [
                LimbView {
                    id: LimbId::Spire,
                    sun: false,
                    water: false,
                },
                LimbView {
                    id: LimbId::Hook,
                    sun: false,
                    water: false,
                },
                LimbView {
                    id: LimbId::Fork,
                    sun: false,
                    water: false,
                },
                LimbView {
                    id: LimbId::Cradle,
                    sun: false,
                    water: false,
                },
                LimbView {
                    id: LimbId::Nub,
                    sun: true,
                    water: true,
                },
            ],
        };
        let value = serde_json::to_value(&obs).unwrap();
        let keys: Vec<&str> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(value["role"], "crown");
        assert!(keys.contains(&"limbs"));
        assert!(
            !keys.contains(&"roots"),
            "Crown observation must never carry root data"
        );
        assert!(!serde_json::to_string(&value)
            .unwrap()
            .to_lowercase()
            .contains("bijection"));
    }

    #[test]
    fn root_observation_carries_only_roots_no_limb_data() {
        let obs = GreatTreeObservation::Root {
            roots: [
                RootView {
                    id: RootId::Hand,
                    thawed: false,
                    running: false,
                },
                RootView {
                    id: RootId::Knot,
                    thawed: false,
                    running: false,
                },
                RootView {
                    id: RootId::Tip,
                    thawed: false,
                    running: false,
                },
                RootView {
                    id: RootId::Swollen,
                    thawed: false,
                    running: false,
                },
                RootView {
                    id: RootId::Deep,
                    thawed: false,
                    running: false,
                },
            ],
        };
        let value = serde_json::to_value(&obs).unwrap();
        let keys: Vec<&str> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(value["role"], "root");
        assert!(keys.contains(&"roots"));
        assert!(
            !keys.contains(&"limbs"),
            "Root observation must never carry limb data"
        );
    }
}
