use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::game::GameAdapter;

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
    /// Chooses the agent's next action and/or message from the player-facing view.
    ///
    /// The `available_actions` value is the same optional affordance sent to the
    /// human UI for this role. `None` means the game does not provide this
    /// affordance; `Some(vec![])` means it does and the player currently has no
    /// listed actions. Any returned action is still validated by the server
    /// before it can affect game state.
    async fn act(
        &mut self,
        observation: A::Observation,
        available_actions: Option<Vec<A::Action>>,
    ) -> Result<AgentResult<A::Action>>;
}

/// Creates fresh game agent instances for individual agent participants.
pub trait AgentFactory<A: GameAdapter>: Send + Sync + 'static {
    /// Instantiates one mutable agent for a room participant.
    fn create(&self, context: AgentInitContext) -> Result<Box<dyn GameAgent<A> + Send>>;
}

/// Shared trait-object handle used by the reusable server.
pub type SharedAgentFactory<A> = Arc<dyn AgentFactory<A>>;
