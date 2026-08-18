use anyhow::Result;
use async_trait::async_trait;
use parlando::agent::{
    Agent, Context as AgentContext, Definition as AgentDefinition, Factory as AgentFactory,
    Response,
};

use crate::{Action, GreatTree};

/// An agent that connects and does nothing: it never proposes an action or sends a message.
/// Useful for exercising pairing, observation delivery, and the participant client against a
/// live second player without needing a real playing agent yet.
pub struct IdleAgent;

#[async_trait]
impl Agent<GreatTree> for IdleAgent {
    async fn respond(&mut self, _available_actions: Option<Vec<Action>>) -> Result<Option<Response<Action>>> {
        Ok(None)
    }
}

pub struct IdleAgentFactory;

#[async_trait]
impl AgentFactory<GreatTree> for IdleAgentFactory {
    fn definition(&self) -> AgentDefinition {
        AgentDefinition {
            id: "great_tree.idle".to_string(),
            name: "Idle".to_string(),
            description: "Connects and takes no actions or messages.".to_string(),
            config_fields: Vec::new(),
        }
    }

    async fn create(&self, _context: AgentContext) -> Result<Box<dyn Agent<GreatTree> + Send>> {
        Ok(Box::new(IdleAgent))
    }
}
