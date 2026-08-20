use serde::{Deserialize, Serialize};

/// The five limb shapes in Crown's domain. Fixed for every session; only the
/// bijection to roots (see `bijection.rs`) and which limb starts flowered vary.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LimbId {
    Spire,
    Hook,
    Fork,
    Cradle,
    Nub,
}

impl LimbId {
    pub const ALL: [LimbId; 5] = [
        LimbId::Spire,
        LimbId::Hook,
        LimbId::Fork,
        LimbId::Cradle,
        LimbId::Nub,
    ];

    pub fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|&id| id == self)
            .expect("LimbId::ALL is exhaustive")
    }
}

/// The five root shapes in Root's domain. Same fixed/varying split as `LimbId`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RootId {
    Hand,
    Knot,
    Tip,
    Swollen,
    Deep,
}

impl RootId {
    pub const ALL: [RootId; 5] = [
        RootId::Hand,
        RootId::Knot,
        RootId::Tip,
        RootId::Swollen,
        RootId::Deep,
    ];

    pub fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|&id| id == self)
            .expect("RootId::ALL is exhaustive")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limb_index_round_trips_through_all() {
        for (i, &id) in LimbId::ALL.iter().enumerate() {
            assert_eq!(id.index(), i);
        }
    }

    #[test]
    fn root_index_round_trips_through_all() {
        for (i, &id) in RootId::ALL.iter().enumerate() {
            assert_eq!(id.index(), i);
        }
    }

    #[test]
    fn limb_id_serializes_as_lowercase_camel_case() {
        assert_eq!(serde_json::to_string(&LimbId::Spire).unwrap(), "\"spire\"");
        assert_eq!(
            serde_json::to_string(&LimbId::Cradle).unwrap(),
            "\"cradle\""
        );
    }

    #[test]
    fn root_id_serializes_as_lowercase_camel_case() {
        assert_eq!(
            serde_json::to_string(&RootId::Swollen).unwrap(),
            "\"swollen\""
        );
    }
}
