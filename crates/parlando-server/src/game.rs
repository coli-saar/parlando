use anyhow::Result;
use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;

/// Identifies one of the two active player roles in a room.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum PlayerRole {
    /// The first active player seat.
    A,
    /// The second active player seat.
    B,
}

impl PlayerRole {
    /// Returns the wire-format role string expected by the browser protocol.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::A => "A",
            Self::B => "B",
        }
    }
}

/// Identifies a participant's seat in a room, including non-playing spectators.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Seat {
    /// The first active player seat.
    #[serde(rename = "A")]
    A,
    /// The second active player seat.
    #[serde(rename = "B")]
    B,
    /// A non-playing observer seat.
    Spectator,
}

impl Seat {
    /// Returns the wire-format seat string expected by the browser protocol.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::A => "A",
            Self::B => "B",
            Self::Spectator => "spectator",
        }
    }

    /// Converts an active seat into a player role and returns `None` for spectators.
    pub fn player_role(self) -> Option<PlayerRole> {
        match self {
            Self::A => Some(PlayerRole::A),
            Self::B => Some(PlayerRole::B),
            Self::Spectator => None,
        }
    }
}

/// Connects a concrete game engine to the reusable Parlando server runtime.
///
/// Implementations keep game-specific logic typed inside the linked binary. The
/// reusable server only serializes these associated types at HTTP, WebSocket, and
/// storage boundaries.
pub trait GameAdapter: Send + Sync + 'static {
    /// Authoritative game state type owned by the game crate.
    type State: Serialize + DeserializeOwned + Clone + Send + Sync + 'static;
    /// Game-specific action type submitted by players or agents.
    type Action: Serialize + DeserializeOwned + Clone + Send + Sync + 'static;
    /// Player-specific observation returned to a client or agent.
    type Observation: Serialize + Clone + Send + Sync + 'static;
    /// Game-specific event emitted after an accepted action.
    type Event: Serialize + Clone + Send + Sync + 'static;
    /// Game-specific completion summary.
    type Summary: Serialize + Clone + Send + Sync + 'static;

    /// Creates a fresh authoritative state for a new room.
    fn initial_state(&self) -> Self::State;
    /// Parses a browser-provided JSON action into the game-specific action type.
    fn parse_action(&self, action: Value) -> Result<Self::Action>;
    /// Validates that an action is legal for the given state and player role.
    fn validate_action(
        &self,
        state: &Self::State,
        action: &Self::Action,
        player: PlayerRole,
    ) -> Result<()>;
    /// Applies an already-validated action and returns the next authoritative state.
    fn apply_action(&self, state: &Self::State, action: &Self::Action) -> Result<Self::State>;
    /// Builds a player-specific observation from the authoritative state.
    fn observe_state(&self, state: &Self::State, player: PlayerRole) -> Self::Observation;
    /// Lists currently available actions for a player role.
    fn available_actions(&self, state: &Self::State, player: PlayerRole) -> Vec<Self::Action>;
    /// Computes transition events visible to a player after an accepted action.
    fn events_for_action(
        &self,
        before: &Self::State,
        after: &Self::State,
        action: &Self::Action,
        player: PlayerRole,
    ) -> Vec<Self::Event>;
    /// Returns true once the game has reached its completion condition.
    fn is_complete(&self, state: &Self::State) -> bool;
    /// Builds the game-specific summary persisted and broadcast on completion.
    fn completion_summary(&self, state: &Self::State) -> Self::Summary;
}
