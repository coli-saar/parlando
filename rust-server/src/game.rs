use anyhow::Result;
use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;

/// Describes one configuration input accepted by a compiled agent factory.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct AgentConfigField {
    /// Stable object key written beneath the selected agent's configuration.
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

/// Describes one agent implementation registered with a game server.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct AgentDefinition {
    /// Stable selector persisted in experiment configuration.
    pub id: String,
    /// Concise name shown in the administrator dashboard.
    pub name: String,
    /// Explanation of where the agent runs and how it behaves.
    pub description: String,
    /// Factory-specific structured configuration inputs.
    pub config_fields: Vec<AgentConfigField>,
}

/// Immutable identity of the game implementation compiled into a server process.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct GameMetadata {
    /// Stable machine-readable identifier shared by releases of this game.
    pub id: String,
    /// Human-readable game name displayed by administration tools.
    pub name: String,
    /// Exact semantic version used to decide whether an experiment can be activated.
    pub version: semver::Version,
    /// Diagnostic build provenance which does not affect activation compatibility.
    pub build_manifest: Value,
}

impl GameMetadata {
    /// Validates the stable process identity before the server opens its dashboard.
    pub(crate) fn validate(&self) -> Result<()> {
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
        if self.name.trim().is_empty() {
            anyhow::bail!("game name must not be empty");
        }
        Ok(())
    }
}

/// Identifies one of the two active player roles in a session.
///
/// Parlando deliberately models two-player games. Either role may be controlled
/// by a human participant or an agent; the role does not encode presentation or
/// participant kind.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum PlayerRole {
    /// The first active player role.
    A,
    /// The second active player role.
    B,
}

impl PlayerRole {
    /// Returns the stable wire-format role name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::A => "A",
            Self::B => "B",
        }
    }
}

/// Identifies a participant's runtime seat in a room.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Seat {
    /// The first active player seat.
    #[serde(rename = "A")]
    A,
    /// The second active player seat.
    #[serde(rename = "B")]
    B,
}

impl Seat {
    /// Returns the stable wire-format seat name.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::A => "A",
            Self::B => "B",
        }
    }

    /// Converts the runtime seat into the public player role.
    pub(crate) fn player_role(self) -> PlayerRole {
        match self {
            Self::A => PlayerRole::A,
            Self::B => PlayerRole::B,
        }
    }
}

/// Machine-readable reason why a game rule rejected a typed action.
///
/// The code is part of the game-specific protocol. It must not contain rendered
/// prose or internal diagnostics: frontends may translate it, agents and logs may
/// inspect it, and the server treats it as an expected rejection rather than a
/// runtime failure.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct ActionRejection {
    /// Stable game-specific reason code, such as `wrong_role`.
    pub code: String,
}

impl ActionRejection {
    /// Creates a rejection from one stable machine-readable reason code.
    pub fn new(code: impl Into<String>) -> Self {
        Self { code: code.into() }
    }
}

impl std::fmt::Display for ActionRejection {
    /// Formats the reason code for diagnostics without inventing presentation text.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.code)
    }
}

impl std::error::Error for ActionRejection {}

/// Defines one two-player game's deterministic mechanics and role information.
///
/// `State`, `Action`, `Observation`, and `Completion` are structured domain data,
/// not rendered human interfaces. The same observations and actions are consumed
/// by browser clients and agents. Given the same configuration, seed, state, and
/// action sequence, an implementation should produce the same results; stochastic
/// games should initialize their random state from `seed` and store it in `State`.
pub trait Game: Send + Sync + 'static {
    /// Game-owned per-experiment configuration edited through the dashboard.
    type Config: Serialize + DeserializeOwned + Send + Sync + 'static;
    /// Authoritative game state owned by the runtime and never sent to participants.
    type State: Serialize + Send + Sync + 'static;
    /// Structured game operation proposed by either player role.
    type Action: Serialize + DeserializeOwned + Clone + Send + Sync + 'static;
    /// Complete game information currently available to one player role.
    type Observation: Serialize + Send + Sync + 'static;
    /// Structured terminal result sent to both players and stored for analysis.
    ///
    /// This shared value may contain public facts such as the winner and scores.
    /// Put role-private terminal facts only in that role's final [`Game::Observation`].
    type Completion: Serialize + Send + Sync + 'static;

    /// Validates semantic constraints on already-deserialized game configuration.
    fn validate_config(&self, _config: &Self::Config) -> Result<()> {
        Ok(())
    }

    /// Creates a fresh authoritative state from game configuration and a recorded seed.
    fn initial_state(&self, config: &Self::Config, seed: u64) -> Result<Self::State>;

    /// Applies one typed action or returns an expected machine-readable rule rejection.
    ///
    /// Implementations must validate the actor and all state-dependent legality in
    /// this operation. The method must not perform I/O. Runtime failures such as
    /// persistence errors are handled separately by the server.
    fn apply_action(
        &self,
        state: &Self::State,
        action: &Self::Action,
        actor: PlayerRole,
    ) -> std::result::Result<Self::State, ActionRejection>;

    /// Returns the complete role-specific information state for one player role.
    fn observation(&self, state: &Self::State, role: PlayerRole) -> Self::Observation;

    /// Lists a role's discrete action affordances when the game provides a catalogue.
    ///
    /// `None` means the action space is not enumerated. `Some(vec![])` means the
    /// game provides an enumeration and no action is currently available. Every
    /// submitted action is still checked by `apply_action`.
    fn available_actions(
        &self,
        _state: &Self::State,
        _role: PlayerRole,
    ) -> Option<Vec<Self::Action>> {
        None
    }

    /// Adds optional role-neutral domain metadata to the durable transition log.
    ///
    /// This value is for analysis and dashboard inspection. It must not contain
    /// viewer-specific prose or assumptions about how a human interface renders
    /// the transition. Parlando always records the actor and action itself.
    fn transition_metadata(
        &self,
        _before: &Self::State,
        _after: &Self::State,
        _action: &Self::Action,
        _actor: PlayerRole,
    ) -> Option<Value> {
        None
    }

    /// Returns the shared structured terminal result, or `None` while play continues.
    ///
    /// The result is delivered unchanged to both players and stored for analysis.
    /// It may contain public game-specific facts such as winner, win/loss outcome,
    /// and scores. Put role-private terminal facts in the role's final observation.
    fn completion(&self, state: &Self::State) -> Option<Self::Completion>;
}

/// Deserializes and semantically validates one stored game configuration value.
pub(crate) fn parse_game_config<G: Game>(game: &G, value: &Value) -> Result<G::Config> {
    let config = serde_json::from_value(value.clone())?;
    game.validate_config(&config)?;
    Ok(config)
}
