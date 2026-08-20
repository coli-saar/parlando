use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::game::{AgentDefinition, Game, PlayerRole, SecretValues};

/// Stable semantic identity recorded for participants created by an agent factory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentIdentity {
    /// Human-readable implementation or model name.
    pub name: String,
    /// Required stable implementation release identifier.
    pub version: String,
}

impl AgentIdentity {
    /// Rejects identities that cannot provide stable participant provenance.
    pub(crate) fn validate(&self) -> Result<()> {
        anyhow::ensure!(!self.name.trim().is_empty(), "agent name must not be empty");
        anyhow::ensure!(
            !self.version.trim().is_empty(),
            "agent version must not be empty"
        );
        Ok(())
    }
}

/// Information supplied when a factory creates one session-local agent.
///
/// Construction happens before the runtime creates the game state and delivers
/// the initial observation through [`Agent::start`]. `settings` contains only
/// values owned by the selected agent factory.
#[derive(Clone, Debug)]
pub struct AgentContext {
    /// The player role controlled by this agent.
    pub role: PlayerRole,
    /// Recorded deterministic seed selected for this agent instance.
    pub seed: u64,
    /// Agent-specific settings entered through the dashboard.
    pub settings: Value,
    /// Secrets consumed only by the local factory or transport adapter.
    pub factory_secrets: SecretValues,
    /// Secrets explicitly authorized for delivery to the agent instance.
    pub agent_instance_secrets: SecretValues,
}

/// Computes the versioned canonical fingerprint of validated non-secret settings.
pub fn configuration_fingerprint(factory_id: &str, settings: &Value) -> Result<String> {
    let document = serde_json::json!({"version": 1, "factory": factory_id, "settings": settings});
    let bytes = serde_json::to_vec(&canonical_value(&document))?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}

/// Recursively sorts object keys while preserving array order.
fn canonical_value(value: &Value) -> Value {
    match value {
        Value::Object(object) => {
            let mut entries = object.iter().collect::<Vec<_>>();
            entries.sort_by(|(left, _), (right, _)| left.cmp(right));
            Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key.clone(), canonical_value(value)))
                    .collect(),
            )
        }
        Value::Array(values) => Value::Array(values.iter().map(canonical_value).collect()),
        _ => value.clone(),
    }
}

/// One non-empty output produced by an agent decision.
///
/// Messages are communication between the two players and do not change game
/// state. Actions pass through the same game-rule validation as human actions.
#[derive(Clone, Debug, PartialEq)]
pub enum AgentResponse<Action> {
    /// Proposes one game action without sending a player message.
    Action(Action),
    /// Sends one player message without proposing a game action.
    Message(String),
    /// Proposes an action and sends a message after that action is accepted.
    ActionAndMessage { action: Action, message: String },
}

impl<Action> AgentResponse<Action> {
    /// Creates an action-only response.
    pub fn action(action: Action) -> Self {
        Self::Action(action)
    }

    /// Creates a message-only response.
    pub fn message(message: impl Into<String>) -> Self {
        Self::Message(message.into())
    }

    /// Creates a response whose message is sent only after its action is accepted.
    pub fn action_and_message(action: Action, message: impl Into<String>) -> Self {
        Self::ActionAndMessage {
            action,
            message: message.into(),
        }
    }

    /// Separates the response into the runtime's ordered action and message operations.
    pub(crate) fn into_parts(self) -> (Option<Action>, Option<String>) {
        match self {
            Self::Action(action) => (Some(action), None),
            Self::Message(message) => (None, Some(message)),
            Self::ActionAndMessage { action, message } => (Some(action), Some(message)),
        }
    }
}

/// Mutable agent instance controlling one player role in one game session.
#[async_trait]
pub trait Agent<G: Game>: Send {
    /// Starts play by delivering the first complete observation for this role.
    ///
    /// The runtime calls this only after the factory has finished constructing
    /// the agent, so implementations may load models or establish remote resources
    /// before they receive game information.
    async fn start(&mut self, _initial_observation: G::Observation) -> Result<()> {
        Ok(())
    }

    /// Observes one accepted action and the complete resulting information for this role.
    async fn observe_transition(
        &mut self,
        _actor: PlayerRole,
        _action: G::Action,
        _observation: G::Observation,
    ) -> Result<()> {
        Ok(())
    }

    /// Observes one message sent by the other player.
    async fn observe_message(&mut self, _sender: PlayerRole, _text: String) -> Result<()> {
        Ok(())
    }

    /// Receives the same shared terminal result delivered to both human players.
    async fn finish(&mut self, _completion: G::Completion) -> Result<()> {
        Ok(())
    }

    /// Optionally produces one non-empty action, message, or combined response.
    ///
    /// `available_actions` is an optional affordance rather than an authorization
    /// guarantee. Every returned action is checked by the game before it changes
    /// state. `None` means that the agent chooses not to respond now.
    async fn respond(
        &mut self,
        available_actions: Option<Vec<G::Action>>,
    ) -> Result<Option<AgentResponse<G::Action>>>;

    /// Releases remote or per-session resources before the runtime drops the agent.
    async fn shutdown(&mut self) -> Result<()> {
        Ok(())
    }
}

/// Registers one compiled agent implementation and creates its session instances.
#[async_trait]
pub trait AgentFactory<G: Game>: Send + Sync + 'static {
    /// Returns the stable dashboard definition for this factory.
    fn definition(&self) -> AgentDefinition;

    /// Instantiates one mutable agent before game-state delivery begins.
    async fn create(&self, context: AgentContext) -> Result<Box<dyn Agent<G> + Send>>;

    /// Returns the required semantic identity stored for participants created by this factory.
    fn identity(&self, settings: &Value) -> Result<AgentIdentity>;
}

/// Internal shared ownership used by experiment runtimes.
pub(crate) type SharedAgentFactory<G> = Arc<dyn AgentFactory<G>>;

#[cfg(test)]
mod tests {
    use super::{configuration_fingerprint, AgentIdentity, AgentResponse};
    use serde_json::json;

    /// Confirms every response variant represents at least one runtime operation.
    #[test]
    fn response_variants_have_no_empty_state() {
        assert_eq!(AgentResponse::action(3).into_parts(), (Some(3), None));
        assert_eq!(
            AgentResponse::<u8>::message("hello").into_parts(),
            (None, Some("hello".to_string()))
        );
        assert_eq!(
            AgentResponse::action_and_message(4, "moving").into_parts(),
            (Some(4), Some("moving".to_string()))
        );
    }

    /// Canonical object ordering is stable while semantic changes remain distinct.
    #[test]
    fn fingerprints_canonicalize_objects_and_include_factory() {
        let first =
            configuration_fingerprint("factory", &json!({"b": 2, "a": {"y": 1, "x": 0}})).unwrap();
        let second =
            configuration_fingerprint("factory", &json!({"a": {"x": 0, "y": 1}, "b": 2})).unwrap();
        assert_eq!(first, second);
        assert!(first.starts_with("sha256:"));
        assert_ne!(
            first,
            configuration_fingerprint("other", &json!({"b": 2, "a": {"y": 1, "x": 0}})).unwrap()
        );
    }

    /// New automated participants must always identify an implementation release.
    #[test]
    fn agent_identity_requires_name_and_version() {
        assert!(AgentIdentity {
            name: "Agent".to_string(),
            version: String::new(),
        }
        .validate()
        .is_err());
        assert!(AgentIdentity {
            name: String::new(),
            version: "1".to_string(),
        }
        .validate()
        .is_err());
    }
}
