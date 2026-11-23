//! Idle fills a seat without taking any action.
use crate::game::types::{GreatTreeCompletion, GreatTreeObservation};
use crate::Action;
use crate::ScriptedConfig as Config;
use parlando_game_sdk::agent::{self as sdk, Result};

/// Participates in a session without proposing actions or messages.
pub struct IdleAgent;

#[sdk::async_trait]
impl sdk::Agent for IdleAgent {
    type Config = Config;
    type View = GreatTreeObservation;
    type Action = Action;
    type Completion = GreatTreeCompletion;
    /// Identifies the idle participant in the dashboard.
    fn definition() -> sdk::Definition {
        sdk::Definition {
            id: "great_tree.idle",
            name: "Idle",
            description: "Connects and takes no actions or messages.",
            version: "1",
            config_fields: "[]",
        }
    }
    /// Creates a fresh, stateless participant.
    async fn init(_: sdk::InitContext<Config>) -> Result<Self> {
        Ok(Self)
    }
    /// Yields on every decision opportunity.
    async fn respond(&mut self, _: sdk::RespondReason) -> Result<sdk::Response<Action>> {
        Ok(sdk::Response::default())
    }
}
