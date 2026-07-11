use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::game::GameAdapter;

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

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentResult<Action> {
    None,
    Action(Action),
    Message(String),
    ActionWithMessage { action: Action, message: String },
}

#[async_trait]
pub trait GameAgent<A: GameAdapter>: Send {
    async fn act(
        &mut self,
        observation: A::Observation,
        available_actions: Vec<A::Action>,
        context: AgentActContext,
    ) -> Result<AgentResult<A::Action>>;
}

pub trait AgentFactory<A: GameAdapter>: Send + Sync + 'static {
    fn create(&self, context: AgentInitContext) -> Result<Box<dyn GameAgent<A> + Send>>;
}

pub type SharedAgentFactory<A> = Arc<dyn AgentFactory<A>>;
