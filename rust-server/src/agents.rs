use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::game::{GameAdapter, PlayerRole};

/// Durable identity metadata used when an agent participant is inserted.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AgentParticipantIdentity {
    /// Identity provider stored in the durable participants table.
    pub identity_provider: String,
    /// Stable external id stored in the durable participants table.
    pub external_id: Option<String>,
    /// Non-secret metadata stored with the durable participant row.
    #[serde(default)]
    pub metadata: Value,
}

impl Default for AgentParticipantIdentity {
    /// Creates the default identity used by local in-process agents.
    fn default() -> Self {
        Self {
            identity_provider: "agent".to_string(),
            external_id: None,
            metadata: Value::Null,
        }
    }
}

impl AgentParticipantIdentity {
    /// Returns the configured agent type when the factory supplied one.
    pub fn agent_type(&self) -> Option<&str> {
        self.metadata
            .get("agent_type")
            .or_else(|| self.metadata.get("agent_name"))
            .and_then(Value::as_str)
    }

    /// Returns the configured agent version when the factory supplied one.
    pub fn agent_version(&self) -> Option<&str> {
        self.metadata.get("agent_version").and_then(Value::as_str)
    }
}

/// Context passed to an agent factory when a new agent participant is created.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AgentInitContext {
    /// Session-local player role the agent controls, such as `A` or `B`.
    pub role: String,
    /// Optional deterministic seed supplied by the experiment config.
    pub seed: Option<u64>,
    /// Agent-specific configuration supplied by the game crate.
    #[serde(default)]
    pub config: Value,
}

/// Participant utterance modality observed by an agent.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentUtteranceKind {
    /// Text typed by a participant.
    Typed,
    /// Speech transcript emitted by the voice pipeline.
    Spoken,
    /// Message emitted by an agent participant.
    Agent,
}

/// Response returned by an agent decision method.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AgentResponse<Action> {
    /// Optional participant-facing message to add to the conversation.
    pub message: Option<String>,
    /// Optional game action to submit through the normal validation path.
    pub action: Option<Action>,
}

impl<Action> AgentResponse<Action> {
    /// Returns whether this response contains no message and no action.
    pub fn is_empty(&self) -> bool {
        self.message.is_none() && self.action.is_none()
    }
}

/// A per-room mutable game agent instance.
#[async_trait]
pub trait GameAgent<A: GameAdapter>: Send {
    /// Observes the current role-specific state snapshot.
    async fn observe_state(&mut self, _current_observation: A::Observation) -> Result<()> {
        Ok(())
    }

    /// Observes an accepted game action and the role-specific state snapshot after it.
    async fn observe_action(
        &mut self,
        _actor: PlayerRole,
        _action: A::Action,
        _resulting_observation: A::Observation,
    ) -> Result<()> {
        Ok(())
    }

    /// Observes a conversation utterance from a known player role.
    async fn observe_message(
        &mut self,
        _speaker: PlayerRole,
        _kind: AgentUtteranceKind,
        _text: String,
    ) -> Result<()> {
        Ok(())
    }

    /// Optionally chooses the agent's next action and/or message.
    ///
    /// The `available_actions` value is the same optional affordance sent to the
    /// human UI for this role. `None` means the game does not provide this
    /// affordance; `Some(vec![])` means it does and the player currently has no
    /// listed actions. Any returned action is still validated by the server
    /// before it can affect game state.
    async fn maybe_act(
        &mut self,
        available_actions: Option<Vec<A::Action>>,
    ) -> Result<Option<AgentResponse<A::Action>>>;

    /// Chooses a required non-empty agent response.
    async fn act(
        &mut self,
        available_actions: Option<Vec<A::Action>>,
    ) -> Result<AgentResponse<A::Action>> {
        let response = self
            .maybe_act(available_actions)
            .await?
            .ok_or_else(|| anyhow::anyhow!("agent did not return a required response"))?;
        if response.is_empty() {
            anyhow::bail!("agent returned an empty response");
        }
        Ok(response)
    }

    /// Releases remote or per-session resources when the room agent stops.
    async fn shutdown(&mut self) -> Result<()> {
        Ok(())
    }
}

/// Creates fresh game agent instances for individual agent participants.
pub trait AgentFactory<A: GameAdapter>: Send + Sync + 'static {
    /// Instantiates one mutable agent for a room participant.
    fn create(&self, context: AgentInitContext) -> Result<Box<dyn GameAgent<A> + Send>>;

    /// Returns durable participant identity metadata for agents created by this factory.
    fn participant_identity(&self) -> AgentParticipantIdentity {
        AgentParticipantIdentity::default()
    }
}

/// Shared trait-object handle used by the reusable server.
pub type SharedAgentFactory<A> = Arc<dyn AgentFactory<A>>;
