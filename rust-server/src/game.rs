use anyhow::Result;
use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;

/// Describes one configuration input accepted by a compiled agent factory.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct AgentConfigFieldDescriptor {
    /// Stable object key written beneath `agents.human_vs_agent.config`.
    pub key: String,
    /// Human-readable field label shown by the administrator dashboard.
    pub label: String,
    /// Brief explanation of the value's purpose.
    pub help: String,
    /// Browser control kind, such as `text` or `url`.
    pub kind: String,
    /// Whether an administrator must supply a non-empty value.
    pub required: bool,
    /// Default value inserted when this factory is selected.
    pub default_value: Value,
}

/// Describes one agent implementation compiled into a concrete game server.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct AgentFactoryDescriptor {
    /// Stable selector persisted in `agents.human_vs_agent.factory`.
    pub id: String,
    /// Concise name shown in the agent selector.
    pub display_name: String,
    /// Explanation of where the agent runs and how it behaves.
    pub description: String,
    /// Factory-specific structured configuration inputs.
    pub config_fields: Vec<AgentConfigFieldDescriptor>,
}

/// Immutable identity of the one game implementation compiled into a server process.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct GameDescriptor {
    /// Stable machine-readable identifier shared by releases of this game.
    pub id: String,
    /// Human-readable game name displayed throughout the administrator dashboard.
    pub display_name: String,
    /// Exact semantic version used to decide whether an experiment can be activated.
    pub version: semver::Version,
    /// Diagnostic build provenance which does not affect activation compatibility.
    pub build_manifest: Value,
}

impl GameDescriptor {
    /// Validates the stable process identity before the server opens its dashboard.
    pub fn validate(&self) -> Result<()> {
        if self.id.is_empty()
            || self.id.chars().count() > 128
            || !self
                .id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        {
            anyhow::bail!(
                "game id must contain 1 to 128 letters, digits, dots, dashes, or underscores"
            );
        }
        if self.display_name.trim().is_empty() {
            anyhow::bail!("game display name must not be empty");
        }
        Ok(())
    }
}

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

/// Identifies a participant's active player seat in a room.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Seat {
    /// The first active player seat.
    #[serde(rename = "A")]
    A,
    /// The second active player seat.
    #[serde(rename = "B")]
    B,
}

impl Seat {
    /// Returns the wire-format seat string expected by the browser protocol.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::A => "A",
            Self::B => "B",
        }
    }

    /// Converts a seat into the equivalent player role.
    pub fn player_role(self) -> PlayerRole {
        match self {
            Self::A => PlayerRole::A,
            Self::B => PlayerRole::B,
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
    /// Lists the agent factories compiled into this game binary.
    fn agent_factories(&self) -> Vec<AgentFactoryDescriptor> {
        Vec::new()
    }
    /// Validates game-owned per-experiment configuration before it is saved or activated.
    fn validate_config(&self, _config: &Value) -> Result<()> {
        Ok(())
    }
    /// Creates initial state using validated game-owned per-experiment configuration.
    fn initial_state_with_config(&self, config: &Value) -> Result<Self::State> {
        self.validate_config(config)?;
        Ok(self.initial_state())
    }
    /// Parses a browser-provided JSON action into the game-specific action type.
    fn parse_action(&self, action: Value) -> Result<Self::Action> {
        Ok(serde_json::from_value(action)?)
    }
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
    /// Lists currently available actions for a player role, when the game can
    /// provide that player-facing affordance.
    ///
    /// `None` means this game does not provide an available-action list. An
    /// empty `Some(vec![])` means the game does provide the affordance and the
    /// player currently has no listed actions. The server still validates every
    /// submitted action through `validate_action`; this method only supplies
    /// player-facing UI and agent hints.
    fn available_actions(
        &self,
        _state: &Self::State,
        _player: PlayerRole,
    ) -> Option<Vec<Self::Action>> {
        None
    }
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
