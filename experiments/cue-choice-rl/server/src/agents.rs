use anyhow::Result;
use async_trait::async_trait;
use parlando::{
    agent::{
        Agent, Context as AgentContext, Definition as AgentDefinition, Factory as AgentFactory,
        Identity as AgentIdentity, Response,
    },
    component::{ComponentValue, GameFrame},
    PlayerRole, RewardFunction, RoleRewards,
};
use serde_json::Value;

use crate::game::{CueChoiceAction, CueChoiceCompletion, CueChoiceObservation};

/// Scripted player-B agent which submits `Deal` exactly once when offered.
pub struct DealerAgent {
    ready: bool,
}

#[async_trait]
impl Agent for DealerAgent {
    /// Reads the initial dealer view to determine whether a cue can be dealt.
    async fn start(&mut self, observation: ComponentValue) -> Result<()> {
        self.ready = matches!(observation.decode()?, CueChoiceObservation::ReadyToDeal);
        Ok(())
    }

    /// Updates dealer readiness from the current dealer view.
    async fn observe_transition(
        &mut self,
        _actor: PlayerRole,
        _action: ComponentValue,
        observation: ComponentValue,
    ) -> Result<()> {
        self.start(observation).await
    }

    /// Submits a deal only during the game phase that allows it.
    async fn respond(&mut self) -> Result<Option<Response>> {
        self.ready
            .then(|| ComponentValue::from_typed(&CueChoiceAction::Deal).map(Response::action))
            .transpose()
    }
}

/// Factory for fresh scripted dealer agents.
pub struct DealerAgentFactory;

#[async_trait]
impl AgentFactory for DealerAgentFactory {
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
    async fn create(&self, context: AgentContext) -> Result<Box<dyn Agent + Send>> {
        anyhow::ensure!(
            context.role == PlayerRole::B,
            "cue-choice dealer must occupy player B"
        );
        Ok(Box::new(DealerAgent { ready: false }))
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

impl RewardFunction for CorrectnessReward {
    /// Returns the stable registry key used by experiment YAML.
    fn id(&self) -> &'static str {
        "cue_choice.correctness"
    }

    /// Returns semantic reward implementation provenance.
    fn version(&self) -> &'static str {
        "1"
    }

    /// Accepts only the empty parameter object used by this fixed objective.
    fn configure(&self, parameters: &Value) -> Result<Self> {
        anyhow::ensure!(
            parameters
                .as_object()
                .is_some_and(serde_json::Map::is_empty),
            "cue-choice correctness reward accepts no parameters"
        );
        Ok(Self)
    }

    /// Rewards only the learner and only for its terminal choice.
    fn rewards(
        &self,
        _before: &GameFrame,
        _after: &GameFrame,
        actor: PlayerRole,
        _action: &ComponentValue,
        completion: Option<&ComponentValue>,
    ) -> RoleRewards {
        let player_a = match (
            actor,
            completion.and_then(|value| value.decode::<CueChoiceCompletion>().ok()),
        ) {
            (PlayerRole::A, Some(result)) if result.correct => 1.0,
            (PlayerRole::A, Some(_)) => -1.0,
            _ => 0.0,
        };
        RoleRewards {
            player_a,
            player_b: 0.0,
        }
    }
}
