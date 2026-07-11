use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::game::GameAdapter;

/// Context passed to an agent factory when a new agent participant is created.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AgentInitContext {
    pub role: String,
    pub room_id: String,
    pub participant_session_id: String,
    pub game_index: i64,
    pub seed: Option<u64>,
    #[serde(default)]
    pub config: Value,
}

/// Per-turn context passed to a game agent's `act` method.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct AgentActContext {
    pub role: String,
    pub room_id: String,
    pub participant_session_id: String,
    pub game_event_count: usize,
    pub invalid_actions: usize,
    pub last_error: Option<String>,
    pub completed: bool,
    #[serde(default)]
    pub conversation: Vec<Value>,
}

/// Result returned by a game agent after observing a room state.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentResult<Action> {
    /// The agent intentionally does nothing this turn.
    None,
    /// The agent submits an action without a chat message.
    Action(Action),
    /// The agent sends a chat message without a game action.
    Message(String),
    /// The agent sends a chat message and submits a game action.
    ActionWithMessage { action: Action, message: String },
}

/// A per-room mutable game agent instance.
#[async_trait]
pub trait GameAgent<A: GameAdapter>: Send {
    /// Chooses the agent's next action and/or message from a typed observation.
    async fn act(
        &mut self,
        observation: A::Observation,
        available_actions: Vec<A::Action>,
        context: AgentActContext,
    ) -> Result<AgentResult<A::Action>>;
}

/// Creates fresh game agent instances for individual agent participants.
pub trait AgentFactory<A: GameAdapter>: Send + Sync + 'static {
    /// Instantiates one mutable agent for a room participant.
    fn create(&self, context: AgentInitContext) -> Result<Box<dyn GameAgent<A> + Send>>;
}

/// Shared trait-object handle used by the reusable server.
pub type SharedAgentFactory<A> = Arc<dyn AgentFactory<A>>;
