use std::{collections::BTreeMap, marker::PhantomData, time::Duration};

use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use prost_types::{value::Kind, ListValue, NullValue, Struct, Value as ProstValue};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::{Number, Value};
use tonic::transport::Channel;

use crate::{
    agents::{
        AgentFactory, AgentInitContext, AgentParticipantIdentity, AgentResponse,
        AgentUtteranceKind, GameAgent,
    },
    game::{GameAdapter, PlayerRole},
};

/// Generated protobuf types and gRPC service clients for remote agents.
pub mod pb {
    tonic::include_proto!("parlando.agent.v2");
}

use pb::{
    agent_service_client::AgentServiceClient, CreateAgentRequest, DecisionRequest,
    ObserveActionRequest, ObserveMessageRequest, ObserveStateRequest, UtteranceKind,
};

/// Configuration for a remote gRPC agent backend.
#[derive(Clone, Debug)]
pub struct RemoteGrpcAgentConfig {
    /// HTTP/2 endpoint for the remote agent service, such as `http://127.0.0.1:50051`.
    pub endpoint: String,
    /// Stable human-readable agent name stored in initialization requests.
    pub agent_name: String,
    /// Stable agent version or fingerprint stored in initialization requests.
    pub agent_version: Option<String>,
    /// Remote-agent protocol version expected by this server.
    pub protocol_version: String,
    /// Per-request timeout for create and act calls.
    pub request_timeout: Duration,
}

impl RemoteGrpcAgentConfig {
    /// Creates a remote-agent configuration with conservative default metadata.
    pub fn new(endpoint: impl Into<String>, agent_name: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            agent_name: agent_name.into(),
            agent_version: None,
            protocol_version: "parlando-agent-v2".to_string(),
            request_timeout: Duration::from_secs(5),
        }
    }
}

/// Agent factory that adapts a gRPC service to Parlando's normal in-process trait.
pub struct RemoteGrpcAgentFactory<A: GameAdapter> {
    config: RemoteGrpcAgentConfig,
    _adapter: PhantomData<A>,
}

impl<A: GameAdapter> RemoteGrpcAgentFactory<A> {
    /// Creates a factory that will instantiate one remote agent per room participant.
    pub fn new(config: RemoteGrpcAgentConfig) -> Self {
        Self {
            config,
            _adapter: PhantomData,
        }
    }
}

impl<A: GameAdapter> AgentFactory<A> for RemoteGrpcAgentFactory<A> {
    /// Creates one lazy remote-agent handle for a room participant.
    fn create(&self, context: AgentInitContext) -> Result<Box<dyn GameAgent<A> + Send>> {
        Ok(Box::new(RemoteGrpcAgent::<A> {
            config: self.config.clone(),
            init_context: context,
            client: None,
            agent_id: None,
            _adapter: PhantomData,
        }))
    }

    /// Returns durable identity metadata for remote gRPC agents.
    fn participant_identity(&self) -> AgentParticipantIdentity {
        let mut metadata = serde_json::json!({
            "agent_type": "remote_grpc",
            "protocol_version": self.config.protocol_version,
            "agent_name": self.config.agent_name,
        });
        if let Some(agent_version) = self.config.agent_version.as_deref() {
            metadata["agent_version"] = serde_json::json!(agent_version);
        }
        AgentParticipantIdentity {
            identity_provider: "remote_grpc".to_string(),
            external_id: Some(
                self.config
                    .agent_version
                    .as_ref()
                    .map(|version| format!("{}@{}", self.config.agent_name, version))
                    .unwrap_or_else(|| self.config.agent_name.clone()),
            ),
            metadata,
        }
    }
}

/// Per-room remote gRPC agent instance.
pub struct RemoteGrpcAgent<A: GameAdapter> {
    config: RemoteGrpcAgentConfig,
    init_context: AgentInitContext,
    client: Option<AgentServiceClient<Channel>>,
    agent_id: Option<String>,
    _adapter: PhantomData<A>,
}

impl<A> RemoteGrpcAgent<A>
where
    A: GameAdapter,
{
    /// Connects to the remote service and sends the create-agent request once.
    async fn ensure_created(&mut self) -> Result<()> {
        if self.agent_id.is_some() {
            return Ok(());
        }
        let channel = Channel::from_shared(self.config.endpoint.clone())
            .context("invalid remote agent endpoint")?
            .connect()
            .await
            .context("failed to connect to remote agent service")?;
        let mut client = AgentServiceClient::new(channel);
        let request = CreateAgentRequest {
            protocol_version: self.config.protocol_version.clone(),
            agent_name: self.config.agent_name.clone(),
            agent_version: self.config.agent_version.clone().unwrap_or_default(),
            role: self.init_context.role.clone(),
            seed: self.init_context.seed,
            config: Some(json_to_struct(self.init_context.config.clone())?),
        };
        let response =
            tokio::time::timeout(self.config.request_timeout, client.create_agent(request))
                .await
                .context("remote agent create timed out")?
                .context("remote agent create failed")?
                .into_inner();
        if response.agent_id.is_empty() {
            bail!("remote agent returned an empty agent_id");
        }
        self.client = Some(client);
        self.agent_id = Some(response.agent_id);
        Ok(())
    }
}

#[async_trait]
impl<A> GameAgent<A> for RemoteGrpcAgent<A>
where
    A: GameAdapter,
    A::Action: DeserializeOwned + Serialize,
{
    /// Sends the current state snapshot to the remote agent.
    async fn observe_state(&mut self, current_observation: A::Observation) -> Result<()> {
        self.ensure_created().await?;
        let agent_id = self.agent_id()?;
        let request_timeout = self.config.request_timeout;
        let client = self.client()?;
        let request = ObserveStateRequest {
            agent_id,
            current_observation: Some(json_to_struct(serde_json::to_value(current_observation)?)?),
        };
        tokio::time::timeout(request_timeout, client.observe_state(request))
            .await
            .context("remote agent observe_state timed out")?
            .context("remote agent observe_state failed")?;
        Ok(())
    }

    /// Sends an accepted action and resulting state snapshot to the remote agent.
    async fn observe_action(
        &mut self,
        actor: PlayerRole,
        action: A::Action,
        resulting_observation: A::Observation,
    ) -> Result<()> {
        self.ensure_created().await?;
        let agent_id = self.agent_id()?;
        let request_timeout = self.config.request_timeout;
        let client = self.client()?;
        let request = ObserveActionRequest {
            agent_id,
            actor: actor.as_str().to_string(),
            action: Some(action_to_struct(action)?),
            resulting_observation: Some(json_to_struct(serde_json::to_value(
                resulting_observation,
            )?)?),
        };
        tokio::time::timeout(request_timeout, client.observe_action(request))
            .await
            .context("remote agent observe_action timed out")?
            .context("remote agent observe_action failed")?;
        Ok(())
    }

    /// Sends a conversation utterance to the remote agent.
    async fn observe_message(
        &mut self,
        speaker: PlayerRole,
        kind: AgentUtteranceKind,
        text: String,
    ) -> Result<()> {
        self.ensure_created().await?;
        let agent_id = self.agent_id()?;
        let request_timeout = self.config.request_timeout;
        let client = self.client()?;
        let request = ObserveMessageRequest {
            agent_id,
            speaker: speaker.as_str().to_string(),
            kind: utterance_kind_to_proto(kind) as i32,
            text,
        };
        tokio::time::timeout(request_timeout, client.observe_message(request))
            .await
            .context("remote agent observe_message timed out")?
            .context("remote agent observe_message failed")?;
        Ok(())
    }

    /// Optionally asks the remote agent for a response.
    async fn maybe_act(
        &mut self,
        available_actions: Option<Vec<A::Action>>,
    ) -> Result<Option<AgentResponse<A::Action>>> {
        self.ensure_created().await?;
        let request = self.decision_request(available_actions)?;
        let request_timeout = self.config.request_timeout;
        let response = tokio::time::timeout(request_timeout, self.client()?.maybe_act(request))
            .await
            .context("remote agent maybe_act timed out")?
            .context("remote agent maybe_act failed")?
            .into_inner();
        response.response.map(proto_to_agent_response).transpose()
    }

    /// Requires the remote agent to return a response.
    async fn act(
        &mut self,
        available_actions: Option<Vec<A::Action>>,
    ) -> Result<AgentResponse<A::Action>> {
        self.ensure_created().await?;
        let request = self.decision_request(available_actions)?;
        let request_timeout = self.config.request_timeout;
        let response = tokio::time::timeout(request_timeout, self.client()?.act(request))
            .await
            .context("remote agent act timed out")?
            .context("remote agent act failed")?
            .into_inner();
        proto_to_agent_response(response)
    }
}

impl<A> RemoteGrpcAgent<A>
where
    A: GameAdapter,
    A::Action: Serialize,
{
    /// Returns the remote agent id after creation.
    fn agent_id(&self) -> Result<String> {
        self.agent_id
            .clone()
            .ok_or_else(|| anyhow!("remote agent was not created"))
    }

    /// Returns the connected gRPC client after creation.
    fn client(&mut self) -> Result<&mut AgentServiceClient<Channel>> {
        self.client
            .as_mut()
            .ok_or_else(|| anyhow!("remote agent client was not connected"))
    }

    /// Builds a decision request with optional available actions.
    fn decision_request(
        &self,
        available_actions: Option<Vec<A::Action>>,
    ) -> Result<DecisionRequest> {
        let available_actions_provided = available_actions.is_some();
        let available_actions = available_actions
            .unwrap_or_default()
            .into_iter()
            .map(action_to_struct::<A::Action>)
            .collect::<Result<Vec<_>>>()?;
        Ok(DecisionRequest {
            agent_id: self.agent_id()?,
            available_actions_provided,
            available_actions,
        })
    }
}

/// Converts an utterance kind to its protobuf representation.
fn utterance_kind_to_proto(kind: AgentUtteranceKind) -> UtteranceKind {
    match kind {
        AgentUtteranceKind::Typed => UtteranceKind::Typed,
        AgentUtteranceKind::Spoken => UtteranceKind::Spoken,
        AgentUtteranceKind::Agent => UtteranceKind::Agent,
    }
}

/// Converts a protobuf agent response into a typed Rust response.
fn proto_to_agent_response<Action: DeserializeOwned>(
    response: pb::AgentResponse,
) -> Result<AgentResponse<Action>> {
    let response = AgentResponse {
        message: response.message,
        action: response.action.map(struct_to_action).transpose()?,
    };
    if response.is_empty() {
        bail!("remote agent returned an empty response");
    }
    Ok(response)
}

/// Serializes a typed action into a protobuf struct for the remote boundary.
fn action_to_struct<Action: Serialize>(action: Action) -> Result<Struct> {
    json_to_struct(serde_json::to_value(action)?)
}

/// Deserializes a typed action from a protobuf struct returned by a remote agent.
fn struct_to_action<Action: DeserializeOwned>(value: Struct) -> Result<Action> {
    Ok(serde_json::from_value(struct_to_json(value))?)
}

/// Converts a JSON object into a protobuf struct.
fn json_to_struct(value: Value) -> Result<Struct> {
    let Value::Object(object) = value else {
        bail!("remote agent protobuf Struct values must be JSON objects");
    };
    let fields = object
        .into_iter()
        .map(|(key, value)| Ok((key, json_to_prost(value)?)))
        .collect::<Result<BTreeMap<_, _>>>()?;
    Ok(Struct { fields })
}

/// Converts any JSON value into a protobuf value.
fn json_to_prost(value: Value) -> Result<ProstValue> {
    let kind = match value {
        Value::Null => Kind::NullValue(NullValue::NullValue as i32),
        Value::Bool(value) => Kind::BoolValue(value),
        Value::Number(value) => Kind::NumberValue(
            value
                .as_f64()
                .ok_or_else(|| anyhow!("remote agent number is not finite"))?,
        ),
        Value::String(value) => Kind::StringValue(value),
        Value::Array(values) => Kind::ListValue(ListValue {
            values: values
                .into_iter()
                .map(json_to_prost)
                .collect::<Result<Vec<_>>>()?,
        }),
        Value::Object(values) => Kind::StructValue(json_to_struct(Value::Object(values))?),
    };
    Ok(ProstValue { kind: Some(kind) })
}

/// Converts a protobuf struct back to JSON.
fn struct_to_json(value: Struct) -> Value {
    Value::Object(
        value
            .fields
            .into_iter()
            .map(|(key, value)| (key, prost_to_json(value)))
            .collect(),
    )
}

/// Converts a protobuf value back to JSON.
fn prost_to_json(value: ProstValue) -> Value {
    match value.kind {
        Some(Kind::NullValue(_)) | None => Value::Null,
        Some(Kind::BoolValue(value)) => Value::Bool(value),
        Some(Kind::NumberValue(value)) => {
            if value.is_finite()
                && value.fract() == 0.0
                && value >= i64::MIN as f64
                && value <= i64::MAX as f64
            {
                Value::Number(Number::from(value as i64))
            } else {
                Number::from_f64(value)
                    .map(Value::Number)
                    .unwrap_or(Value::Null)
            }
        }
        Some(Kind::StringValue(value)) => Value::String(value),
        Some(Kind::ListValue(values)) => {
            Value::Array(values.values.into_iter().map(prost_to_json).collect())
        }
        Some(Kind::StructValue(value)) => struct_to_json(value),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::{GameAdapter, PlayerRole};

    struct TestAdapter;

    impl GameAdapter for TestAdapter {
        type State = Value;
        type Action = Value;
        type Observation = Value;
        type Event = Value;
        type Summary = Value;

        fn initial_state(&self) -> Self::State {
            Value::Null
        }

        fn validate_action(
            &self,
            _state: &Self::State,
            _action: &Self::Action,
            _player: PlayerRole,
        ) -> Result<()> {
            Ok(())
        }

        fn apply_action(&self, state: &Self::State, _action: &Self::Action) -> Result<Self::State> {
            Ok(state.clone())
        }

        fn observe_state(&self, state: &Self::State, _player: PlayerRole) -> Self::Observation {
            state.clone()
        }

        fn events_for_action(
            &self,
            _before: &Self::State,
            _after: &Self::State,
            _action: &Self::Action,
            _player: PlayerRole,
        ) -> Vec<Self::Event> {
            vec![]
        }

        fn is_complete(&self, _state: &Self::State) -> bool {
            false
        }

        fn completion_summary(&self, state: &Self::State) -> Self::Summary {
            state.clone()
        }
    }

    #[test]
    fn json_struct_conversion_preserves_nested_values() {
        let original = serde_json::json!({
            "role": "B",
            "score": 2,
            "ready": true,
            "nested": {"items": [null, "x"]}
        });
        let converted = struct_to_json(json_to_struct(original.clone()).unwrap());
        assert_eq!(converted, original);
    }

    #[test]
    fn remote_identity_does_not_invent_missing_agent_version() {
        let factory = RemoteGrpcAgentFactory::<TestAdapter>::new(RemoteGrpcAgentConfig::new(
            "http://127.0.0.1:50051",
            "python-agent",
        ));
        let identity = factory.participant_identity();

        assert_eq!(identity.metadata["agent_type"], "remote_grpc");
        assert_eq!(identity.metadata["agent_name"], "python-agent");
        assert!(identity.metadata.get("agent_version").is_none());
        assert_eq!(identity.external_id.as_deref(), Some("python-agent"));
    }
}
