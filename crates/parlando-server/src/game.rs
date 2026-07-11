use anyhow::Result;
use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum PlayerRole {
    A,
    B,
}

impl PlayerRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::A => "A",
            Self::B => "B",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Seat {
    #[serde(rename = "A")]
    A,
    #[serde(rename = "B")]
    B,
    Spectator,
}

impl Seat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::A => "A",
            Self::B => "B",
            Self::Spectator => "spectator",
        }
    }

    pub fn player_role(self) -> Option<PlayerRole> {
        match self {
            Self::A => Some(PlayerRole::A),
            Self::B => Some(PlayerRole::B),
            Self::Spectator => None,
        }
    }
}

pub trait GameAdapter: Send + Sync + 'static {
    type State: Serialize + DeserializeOwned + Clone + Send + Sync + 'static;
    type Action: Serialize + DeserializeOwned + Clone + Send + Sync + 'static;
    type Observation: Serialize + Clone + Send + Sync + 'static;
    type Event: Serialize + Clone + Send + Sync + 'static;
    type Summary: Serialize + Clone + Send + Sync + 'static;

    fn initial_state(&self) -> Self::State;
    fn parse_action(&self, action: Value) -> Result<Self::Action>;
    fn validate_action(&self, state: &Self::State, action: &Self::Action, player: PlayerRole) -> Result<()>;
    fn apply_action(&self, state: &Self::State, action: &Self::Action) -> Result<Self::State>;
    fn observe_state(&self, state: &Self::State, player: PlayerRole) -> Self::Observation;
    fn available_actions(&self, state: &Self::State, player: PlayerRole) -> Vec<Self::Action>;
    fn events_for_action(
        &self,
        before: &Self::State,
        after: &Self::State,
        action: &Self::Action,
        player: PlayerRole,
    ) -> Vec<Self::Event>;
    fn is_complete(&self, state: &Self::State) -> bool;
    fn completion_summary(&self, state: &Self::State) -> Self::Summary;
}
