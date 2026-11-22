//! Live and headless sessions use the same factory and callback interface for software
//! players. A factory receives a role, validated settings, and a session-scoped logger. Its
//! agent receives observations and messages before decisions. A decision may yield, send
//! text, propose an action, or do both. The runtime submits proposed actions to the game.

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::canonical_json::canonical_value;
use crate::component::ComponentValue;
use crate::game::{AgentDefinition, PlayerRole, SecretValues};
use crate::SessionLogger;

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
    /// Logger bound to this session, agent participant, and role.
    pub logger: SessionLogger,
}

/// Computes the versioned canonical fingerprint of validated non-secret settings.
pub fn configuration_fingerprint(factory_id: &str, settings: &Value) -> Result<String> {
    let document = serde_json::json!({"version": 1, "factory": factory_id, "settings": settings});
    let bytes = serde_json::to_vec(&canonical_value(&document))?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}

/// One non-empty output produced by an agent decision.
///
/// Messages are communication between the two players and do not change game
/// state. Actions pass through the same game-rule validation as human actions.
#[derive(Clone, Debug)]
pub enum AgentResponse {
    /// Proposes one game action without sending a player message.
    Action(ComponentValue),
    /// Sends one player message without proposing a game action.
    Message(String),
    /// Proposes an action and sends a message after that action is accepted.
    ActionAndMessage {
        action: ComponentValue,
        message: String,
    },
}

impl AgentResponse {
    /// Creates an action-only response.
    pub fn action(action: ComponentValue) -> Self {
        Self::Action(action)
    }

    /// Creates a message-only response.
    pub fn message(message: impl Into<String>) -> Self {
        Self::Message(message.into())
    }

    /// Creates a response whose message is sent only after its action is accepted.
    pub fn action_and_message(action: ComponentValue, message: impl Into<String>) -> Self {
        Self::ActionAndMessage {
            action,
            message: message.into(),
        }
    }

    /// Projects the action and chat onto the headless scheduler's nonempty output.
    pub(crate) fn from_decision(decision: Decision) -> Option<Self> {
        match (decision.action, decision.message) {
            (Some(action), Some(message)) => {
                Some(AgentResponse::action_and_message(action, message))
            }
            (Some(action), None) => Some(AgentResponse::action(action)),
            (None, Some(message)) => Some(AgentResponse::message(message)),
            (None, None) => None,
        }
    }

    /// Separates the response into the runtime's ordered action and message operations.
    pub(crate) fn into_parts(self) -> (Option<ComponentValue>, Option<String>) {
        match self {
            Self::Action(action) => (Some(action), None),
            Self::Message(message) => (None, Some(message)),
            Self::ActionAndMessage { action, message } => (Some(action), Some(message)),
        }
    }
}

pub use parlando_game_sdk::agent::{Message, RespondReason as Reason, WakeupRequest};

/// A game transition carrying the host's compact action and role-private view.
pub type Transition = parlando_game_sdk::agent::Transition<ComponentValue, ComponentValue>;

/// A single delivery of a transition, a partner message, or both.
pub type Observation = parlando_game_sdk::agent::Observation<ComponentValue, ComponentValue>;

/// A policy response carrying a compact action ready for the game host.
pub type Decision = parlando_game_sdk::agent::Response<ComponentValue>;

/// Mutable agent instance controlling one player role in one game session.
#[async_trait]
pub trait Agent: Send {
    /// Starts play by delivering the first complete observation for this role.
    ///
    /// The runtime calls this only after the factory has finished constructing
    /// the agent, so implementations may load models or establish remote resources
    /// before they receive game information.
    async fn start(&mut self, _initial_observation: ComponentValue) -> Result<()> {
        Ok(())
    }

    /// Delivers a committed transition, chat, or both in order.
    async fn observe(&mut self, observation: Observation) -> Result<()> {
        if let Some(transition) = observation.transition {
            self.observe_transition(transition.actor, transition.action, transition.view)
                .await?;
        }
        if let Some(message) = observation.message {
            self.observe_message(message.sender, message.text).await?;
        }
        Ok(())
    }

    /// Observes one accepted action and the complete resulting information for this role.
    async fn observe_transition(
        &mut self,
        _actor: PlayerRole,
        _action: ComponentValue,
        _observation: ComponentValue,
    ) -> Result<()> {
        Ok(())
    }

    /// Observes one message sent by the other player.
    async fn observe_message(&mut self, _sender: PlayerRole, _text: String) -> Result<()> {
        Ok(())
    }

    /// Receives the same shared terminal result delivered to both human players.
    async fn finish(&mut self, _completion: ComponentValue) -> Result<()> {
        Ok(())
    }

    /// Responds to a decision opportunity with optional action, chat, and timer update.
    async fn decide(&mut self, _reason: Reason) -> Result<Decision> {
        let (action, message) = self
            .respond()
            .await?
            .map(AgentResponse::into_parts)
            .unwrap_or_default();
        Ok(Decision {
            action,
            message,
            wakeup: None,
        })
    }

    /// Optionally produces one action, message, or combined response.
    ///
    /// Use the latest observation to decide whether to act. Every action is
    /// checked by the game before it changes state.
    async fn respond(&mut self) -> Result<Option<AgentResponse>> {
        Ok(None)
    }

    /// Releases remote or per-session resources before the runtime drops the agent.
    async fn shutdown(&mut self) -> Result<()> {
        Ok(())
    }
}

/// Registers one compiled agent implementation and creates its session instances.
#[async_trait]
pub trait AgentFactory: Send + Sync + 'static {
    /// Returns the stable dashboard definition for this factory.
    fn definition(&self) -> AgentDefinition;

    /// Returns the SHA-256 of the loaded agent artifact when the factory executes WASM.
    fn wasm_sha256(&self) -> Option<&str> {
        None
    }

    /// Returns a replacement factory if the host's shared WASM metering mode requires
    /// recompilation. Native and remote implementations use the default and keep their
    /// existing factory.
    #[doc(hidden)]
    fn with_host_wasm_metering(
        &self,
        _metering: crate::component::WasmMetering,
    ) -> Result<Option<Arc<dyn AgentFactory>>> {
        Ok(None)
    }

    /// Instantiates one mutable agent before game-state delivery begins.
    async fn create(&self, context: AgentContext) -> Result<Box<dyn Agent + Send>>;

    /// Returns the required semantic identity stored for participants created by this factory.
    fn identity(&self, settings: &Value) -> Result<AgentIdentity>;
}

/// Internal shared ownership used by experiment runtimes.
pub(crate) type SharedAgentFactory = Arc<dyn AgentFactory>;

#[cfg(test)]
mod tests {
    use super::{configuration_fingerprint, AgentIdentity, AgentResponse};
    use serde_json::json;

    /// Confirms every response variant represents at least one runtime operation.
    #[test]
    fn response_variants_have_no_empty_state() {
        let action = serde_json::from_value::<super::ComponentValue>(json!(3)).unwrap();
        let (selected, message) = AgentResponse::action(action.clone()).into_parts();
        assert_eq!(selected.unwrap().decode::<u8>().unwrap(), 3);
        assert!(message.is_none());
        let (selected, message) = AgentResponse::message("hello").into_parts();
        assert!(selected.is_none());
        assert_eq!(message.as_deref(), Some("hello"));
        let (selected, message) = AgentResponse::action_and_message(action, "moving").into_parts();
        assert_eq!(selected.unwrap().decode::<u8>().unwrap(), 3);
        assert_eq!(message.as_deref(), Some("moving"));
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
