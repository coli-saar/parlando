use std::{collections::BTreeMap, marker::PhantomData, time::Duration};

use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use prost_types::{value::Kind, ListValue, NullValue, Struct, Value as ProstValue};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::{Number, Value};
use tonic::transport::Channel;

use crate::{
    agents::{AgentFactory, AgentInitContext, AgentParticipantIdentity, AgentResult, GameAgent},
    game::GameAdapter,
};

/// Generated protobuf types and gRPC service clients for remote agents.
pub mod pb {
    tonic::include_proto!("parlando.agent.v1");
}

use pb::{
    agent_service_client::AgentServiceClient, ActRequest, AgentResultKind, CreateAgentRequest,
};

/// Configuration for a remote gRPC agent backend.
#[derive(Clone, Debug)]
pub struct RemoteGrpcAgentConfig {
    /// HTTP/2 endpoint for the remote agent service, such as `http://127.0.0.1:50051`.
    pub endpoint: String,
    /// Stable human-readable agent name stored in initialization requests.
    pub agent_name: String,
    /// Stable agent version or fingerprint stored in initialization requests.
    pub agent_version: String,
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
            agent_version: "dev".to_string(),
            protocol_version: "parlando-agent-v1".to_string(),
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
        AgentParticipantIdentity {
            identity_provider: "remote_grpc".to_string(),
            external_id: Some(format!(
                "{}@{}",
                self.config.agent_name, self.config.agent_version
            )),
            metadata: serde_json::json!({
                "protocol_version": self.config.protocol_version,
                "agent_name": self.config.agent_name,
                "agent_version": self.config.agent_version,
            }),
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
            agent_version: self.config.agent_version.clone(),
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
    /// Calls the remote agent service with the same role-specific view used by the UI.
    async fn act(
        &mut self,
        observation: A::Observation,
        available_actions: Option<Vec<A::Action>>,
    ) -> Result<AgentResult<A::Action>> {
        self.ensure_created().await?;
        let agent_id = self
            .agent_id
            .clone()
            .ok_or_else(|| anyhow!("remote agent was not created"))?;
        let client = self
            .client
            .as_mut()
            .ok_or_else(|| anyhow!("remote agent client was not connected"))?;
        let available_actions_provided = available_actions.is_some();
        let available_actions = available_actions
            .unwrap_or_default()
            .into_iter()
            .map(action_to_struct::<A::Action>)
            .collect::<Result<Vec<_>>>()?;
        let request = ActRequest {
            agent_id,
            role: self.init_context.role.clone(),
            observation: Some(json_to_struct(serde_json::to_value(observation)?)?),
            available_actions_provided,
            available_actions,
        };
        let response = tokio::time::timeout(self.config.request_timeout, client.act(request))
            .await
            .context("remote agent act timed out")?
            .context("remote agent act failed")?
            .into_inner();
        match AgentResultKind::try_from(response.result_kind)
            .unwrap_or(AgentResultKind::Unspecified)
        {
            AgentResultKind::None => Ok(AgentResult::None),
            AgentResultKind::Message => Ok(AgentResult::Message(response.message)),
            AgentResultKind::Action => {
                let action = response
                    .action
                    .ok_or_else(|| anyhow!("remote agent action result omitted action"))?;
                Ok(AgentResult::Action(struct_to_action(action)?))
            }
            AgentResultKind::ActionWithMessage => {
                let action = response.action.ok_or_else(|| {
                    anyhow!("remote agent action-with-message result omitted action")
                })?;
                Ok(AgentResult::ActionWithMessage {
                    action: struct_to_action(action)?,
                    message: response.message,
                })
            }
            AgentResultKind::Unspecified => bail!("remote agent returned unspecified result kind"),
        }
    }
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
}
