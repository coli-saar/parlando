use anyhow::Result;
use async_trait::async_trait;
use parlando::{
    agent::{
        Agent, Context as AgentContext, Definition as AgentDefinition, Factory as AgentFactory,
        Identity as AgentIdentity, Response,
    },
    PlayerRole, RewardFunction, RoleRewards,
};
use serde_json::Value;

use crate::game::{CueChoiceAction, CueChoiceCompletion, CueChoiceGame, CueChoiceState};

/// Scripted player-B agent which submits `Deal` exactly once when offered.
pub struct DealerAgent {
    dealt: bool,
}

#[async_trait]
impl Agent<CueChoiceGame> for DealerAgent {
    /// Selects the typed dealer action and otherwise declines to respond.
    async fn respond(
        &mut self,
        available_actions: Option<Vec<CueChoiceAction>>,
    ) -> Result<Option<Response<CueChoiceAction>>> {
        if self.dealt {
            return Ok(None);
        }
        let deal = available_actions
            .unwrap_or_default()
            .into_iter()
            .find(|action| matches!(action, CueChoiceAction::Deal));
        self.dealt = deal.is_some();
        Ok(deal.map(Response::action))
    }
}

/// Factory for fresh scripted dealer agents.
pub struct DealerAgentFactory;

#[async_trait]
impl AgentFactory<CueChoiceGame> for DealerAgentFactory {
    /// Describes the settings-free dealer implementation.
    fn definition(&self) -> AgentDefinition {
        AgentDefinition {
            id: "cue_choice.dealer".to_string(),
            name: "Cue Choice dealer".to_string(),
            description: "Deals the private cue with one deterministic typed action.".to_string(),
            config_fields: Vec::new(),
        }
    }

    /// Creates one session-local dealer and enforces its semantic role.
    async fn create(&self, context: AgentContext) -> Result<Box<dyn Agent<CueChoiceGame> + Send>> {
        anyhow::ensure!(
            context.role == PlayerRole::B,
            "cue-choice dealer must occupy player B"
        );
        Ok(Box::new(DealerAgent { dealt: false }))
    }

    /// Returns stable implementation provenance independent of session state.
    fn identity(&self, _settings: &Value) -> Result<AgentIdentity> {
        Ok(AgentIdentity {
            name: "CueChoiceDealer".to_string(),
            version: "1".to_string(),
        })
    }
}

/// Game-specific correctness reward selected by training YAML.
pub struct CorrectnessReward;

impl RewardFunction<CueChoiceGame> for CorrectnessReward {
    /// Returns the stable registry key used by experiment YAML.
    fn id(&self) -> &'static str {
        "cue_choice.correctness"
    }

    /// Returns semantic reward implementation provenance.
    fn version(&self) -> &'static str {
        "1"
    }

    /// Accepts only the empty parameter object used by this fixed objective.
    fn validate_parameters(&self, parameters: &Value) -> Result<()> {
        anyhow::ensure!(
            parameters
                .as_object()
                .is_some_and(serde_json::Map::is_empty),
            "cue-choice correctness reward accepts no parameters"
        );
        Ok(())
    }

    /// Rewards only the learner and only for its terminal choice.
    fn rewards(
        &self,
        _before: &CueChoiceState,
        _after: &CueChoiceState,
        actor: PlayerRole,
        action: &CueChoiceAction,
        completion: Option<&CueChoiceCompletion>,
        _parameters: &Value,
    ) -> RoleRewards {
        let player_a = match (actor, action, completion) {
            (PlayerRole::A, CueChoiceAction::Choose { .. }, Some(result)) if result.correct => 1.0,
            (PlayerRole::A, CueChoiceAction::Choose { .. }, Some(_)) => -1.0,
            _ => 0.0,
        };
        RoleRewards {
            player_a,
            player_b: 0.0,
        }
    }
}
