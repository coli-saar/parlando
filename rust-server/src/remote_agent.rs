use std::{collections::BTreeMap, env, fmt, marker::PhantomData, time::Duration};

use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use prost_types::{value::Kind, ListValue, NullValue, Struct, Value as ProstValue};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{Number, Value};
use tonic::{
    metadata::{Ascii, MetadataValue},
    service::{interceptor::InterceptedService, Interceptor},
    transport::Channel,
    Request, Status,
};

use crate::{
    agents::{Agent, AgentContext, AgentFactory, AgentIdentity, AgentResponse},
    game::{AgentConfigField, AgentDefinition, Game, PlayerRole},
};

/// Generated protobuf types and gRPC service clients for remote agents.
pub mod pb {
    tonic::include_proto!("parlando.agent.v3");
}

use pb::{
    agent_service_client::AgentServiceClient, CreateAgentRequest, ObserveMessageRequest,
    ObserveTransitionRequest, RespondRequest, ShutdownRequest, StartRequest,
};

/// Configuration for a remote gRPC agent backend.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RemoteGrpcAgentConfig {
    /// HTTP/2 endpoint for the remote agent service, such as `http://127.0.0.1:50051`.
    pub endpoint: String,
    /// Stable human-readable agent name stored in initialization requests.
    #[serde(default = "default_agent_name")]
    pub agent_name: String,
    /// Stable agent version or fingerprint stored in initialization requests.
    pub agent_version: Option<String>,
    /// Remote-agent protocol version expected by this server.
    #[serde(default = "default_protocol_version")]
    pub protocol_version: String,
    /// Per-request timeout for create and act calls.
    #[serde(skip, default = "default_request_timeout")]
    pub request_timeout: Duration,
    #[serde(skip)]
    auth_token: Option<String>,
}

impl fmt::Debug for RemoteGrpcAgentConfig {
    /// Formats non-secret transport settings while redacting bearer authentication material.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RemoteGrpcAgentConfig")
            .field("endpoint", &self.endpoint)
            .field("agent_name", &self.agent_name)
            .field("agent_version", &self.agent_version)
            .field("protocol_version", &self.protocol_version)
            .field("request_timeout", &self.request_timeout)
            .field(
                "auth_token",
                &self.auth_token.as_ref().map(|_| "[REDACTED]"),
            )
            .finish()
    }
}

/// Returns the default timeout used for individual remote-agent requests.
fn default_request_timeout() -> Duration {
    Duration::from_secs(5)
}

/// Returns the default semantic name for a dashboard-configured remote agent.
fn default_agent_name() -> String {
    "remote-agent".to_string()
}

/// Returns the current remote-agent protocol identifier.
fn default_protocol_version() -> String {
    "parlando-agent-v3".to_string()
}

#[derive(Clone)]
struct RemoteAuthInterceptor {
    authorization: Option<MetadataValue<Ascii>>,
}

impl Interceptor for RemoteAuthInterceptor {
    /// Adds the configured bearer credential to every remote-agent RPC.
    fn call(&mut self, mut request: Request<()>) -> std::result::Result<Request<()>, Status> {
        if let Some(authorization) = self.authorization.clone() {
            request
                .metadata_mut()
                .insert("authorization", authorization);
        }
        Ok(request)
    }
}

type AuthenticatedAgentClient =
    AgentServiceClient<InterceptedService<Channel, RemoteAuthInterceptor>>;

/// Agent factory that adapts a gRPC service to Parlando's normal in-process trait.
pub struct RemoteGrpcAgentFactory<A: Game>(PhantomData<A>);

impl<A: Game> RemoteGrpcAgentFactory<A> {
    /// Creates a factory that will instantiate one remote agent per room participant.
    pub fn new() -> Self {
        Self(PhantomData)
    }
}

impl<A: Game> Default for RemoteGrpcAgentFactory<A> {
    /// Creates the dashboard-configured remote gRPC factory.
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl<A: Game> AgentFactory<A> for RemoteGrpcAgentFactory<A> {
    /// Describes the standard remote gRPC agent choice for the dashboard.
    fn definition(&self) -> AgentDefinition {
        AgentDefinition {
            id: "remote_grpc".to_string(),
            name: "Remote gRPC agent".to_string(),
            description: "Connects to an external agent process using the Parlando agent protocol."
                .to_string(),
            config_fields: vec![
                AgentConfigField {
                    key: "endpoint".to_string(),
                    label: "Agent endpoint".to_string(),
                    help: "HTTP/2 endpoint of the external agent process.".to_string(),
                    kind: "url".to_string(),
                    required: true,
                    default_value: Value::String("http://127.0.0.1:50051".to_string()),
                },
                AgentConfigField {
                    key: "agent_name".to_string(),
                    label: "Agent name".to_string(),
                    help: "Name recorded for the remote agent implementation.".to_string(),
                    kind: "text".to_string(),
                    required: true,
                    default_value: Value::String("remote-agent".to_string()),
                },
            ],
        }
    }

    /// Creates one lazy remote-agent handle for a room participant.
    async fn create(&self, context: AgentContext) -> Result<Box<dyn Agent<A> + Send>> {
        let config = remote_config_from_settings(&context.settings)?;
        let mut agent = RemoteGrpcAgent::<A> {
            config,
            init_context: context,
            client: None,
            agent_id: None,
            _adapter: PhantomData,
        };
        agent.ensure_created().await?;
        Ok(Box::new(agent))
    }

    /// Returns durable identity metadata for remote gRPC agents.
    fn identity(&self, settings: &Value) -> Result<AgentIdentity> {
        let config = remote_config_from_settings(settings)?;
        Ok(AgentIdentity {
            name: config.agent_name,
            version: config.agent_version,
        })
    }
}

/// Parses dashboard-owned settings and injects the process-local credential.
fn remote_config_from_settings(settings: &Value) -> Result<RemoteGrpcAgentConfig> {
    let mut config: RemoteGrpcAgentConfig = serde_json::from_value(settings.clone())?;
    config.auth_token = env::var("PARLANDO_REMOTE_AGENT_TOKEN").ok();
    Ok(config)
}

/// Per-room remote gRPC agent instance.
pub struct RemoteGrpcAgent<A: Game> {
    config: RemoteGrpcAgentConfig,
    init_context: AgentContext,
    client: Option<AuthenticatedAgentClient>,
    agent_id: Option<String>,
    _adapter: PhantomData<A>,
}

impl<A> RemoteGrpcAgent<A>
where
    A: Game,
{
    /// Connects to the remote service and sends the create-agent request once.
    async fn ensure_created(&mut self) -> Result<()> {
        if self.agent_id.is_some() {
            return Ok(());
        }
        validate_remote_endpoint(&self.config.endpoint, self.config.auth_token.is_some())?;
        let channel = Channel::from_shared(self.config.endpoint.clone())
            .context("invalid remote agent endpoint")?
            .connect()
            .await
            .context("failed to connect to remote agent service")?;
        let authorization = self
            .config
            .auth_token
            .as_ref()
            .map(|token| format!("Bearer {token}").parse::<MetadataValue<Ascii>>())
            .transpose()
            .context("remote-agent bearer token is not valid metadata")?;
        let mut client =
            AgentServiceClient::with_interceptor(channel, RemoteAuthInterceptor { authorization });
        let request = CreateAgentRequest {
            protocol_version: self.config.protocol_version.clone(),
            agent_name: self.config.agent_name.clone(),
            agent_version: self.config.agent_version.clone().unwrap_or_default(),
            role: self.init_context.role.as_str().to_string(),
            seed: self.init_context.seed,
            config: Some(json_to_struct(self.init_context.settings.clone())?),
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

/// Rejects cleartext remote-agent transport unless it is confined to loopback development.
fn validate_remote_endpoint(endpoint: &str, has_auth_token: bool) -> Result<()> {
    let uri = endpoint
        .parse::<http::Uri>()
        .context("invalid remote agent endpoint")?;
    match uri.scheme_str() {
        Some("https") if has_auth_token => {}
        Some("https") => {
            bail!("non-loopback remote-agent TLS requires PARLANDO_REMOTE_AGENT_TOKEN")
        }
        Some("http") => {
            let host = uri
                .host()
                .ok_or_else(|| anyhow!("remote agent endpoint has no host"))?;
            if matches!(host, "localhost" | "127.0.0.1" | "::1") {
                // Literal loopback cleartext is the development-only exception.
            } else {
                bail!("cleartext remote-agent endpoints are allowed only on loopback; use https")
            }
        }
        _ => bail!("remote agent endpoint must use https, or http on loopback for development"),
    }
    let host = uri
        .host()
        .ok_or_else(|| anyhow!("remote agent endpoint has no host"))?;
    if !matches!(host, "localhost" | "127.0.0.1" | "::1") {
        let allowed = env::var("PARLANDO_REMOTE_AGENT_ALLOWED_HOSTS").unwrap_or_default();
        if !allowed
            .split(',')
            .map(str::trim)
            .any(|candidate| !candidate.is_empty() && candidate.eq_ignore_ascii_case(host))
        {
            bail!("remote agent host is not listed in PARLANDO_REMOTE_AGENT_ALLOWED_HOSTS");
        }
    }
    Ok(())
}

#[async_trait]
impl<A> Agent<A> for RemoteGrpcAgent<A>
where
    A: Game,
    A::Action: DeserializeOwned + Serialize,
{
    /// Delivers the first observation after the remote process has created the agent.
    async fn start(&mut self, initial_observation: A::Observation) -> Result<()> {
        self.ensure_created().await?;
        let request = StartRequest {
            agent_id: self.agent_id()?,
            observation: Some(json_to_struct(serde_json::to_value(initial_observation)?)?),
        };
        tokio::time::timeout(self.config.request_timeout, self.client()?.start(request))
            .await
            .context("remote agent start timed out")?
            .context("remote agent start failed")?;
        Ok(())
    }

    /// Sends an accepted action and resulting observation to the remote agent.
    async fn observe_transition(
        &mut self,
        actor: PlayerRole,
        action: A::Action,
        observation: A::Observation,
    ) -> Result<()> {
        self.ensure_created().await?;
        let agent_id = self.agent_id()?;
        let request_timeout = self.config.request_timeout;
        let client = self.client()?;
        let request = ObserveTransitionRequest {
            agent_id,
            actor: actor.as_str().to_string(),
            action: Some(action_to_struct(action)?),
            observation: Some(json_to_struct(serde_json::to_value(observation)?)?),
        };
        tokio::time::timeout(request_timeout, client.observe_transition(request))
            .await
            .context("remote agent observe_transition timed out")?
            .context("remote agent observe_transition failed")?;
        Ok(())
    }

    /// Sends a conversation utterance to the remote agent.
    async fn observe_message(&mut self, sender: PlayerRole, text: String) -> Result<()> {
        self.ensure_created().await?;
        let agent_id = self.agent_id()?;
        let request_timeout = self.config.request_timeout;
        let client = self.client()?;
        let request = ObserveMessageRequest {
            agent_id,
            sender: sender.as_str().to_string(),
            text,
        };
        tokio::time::timeout(request_timeout, client.observe_message(request))
            .await
            .context("remote agent observe_message timed out")?
            .context("remote agent observe_message failed")?;
        Ok(())
    }

    /// Optionally asks the remote agent for a response.
    async fn respond(
        &mut self,
        available_actions: Option<Vec<A::Action>>,
    ) -> Result<Option<AgentResponse<A::Action>>> {
        self.ensure_created().await?;
        let request = self.decision_request(available_actions)?;
        let request_timeout = self.config.request_timeout;
        let response = tokio::time::timeout(request_timeout, self.client()?.respond(request))
            .await
            .context("remote agent respond timed out")?
            .context("remote agent respond failed")?
            .into_inner();
        response.response.map(proto_to_agent_response).transpose()
    }

    /// Releases the corresponding remote server instance on normal completion or cancellation.
    async fn shutdown(&mut self) -> Result<()> {
        let Some(agent_id) = self.agent_id.clone() else {
            return Ok(());
        };
        let request_timeout = self.config.request_timeout;
        tokio::time::timeout(
            request_timeout,
            self.client()?.shutdown(ShutdownRequest { agent_id }),
        )
        .await
        .context("remote agent shutdown timed out")?
        .context("remote agent shutdown failed")?;
        self.agent_id = None;
        Ok(())
    }
}

impl<A> RemoteGrpcAgent<A>
where
    A: Game,
    A::Action: Serialize,
{
    /// Returns the remote agent id after creation.
    fn agent_id(&self) -> Result<String> {
        self.agent_id
            .clone()
            .ok_or_else(|| anyhow!("remote agent was not created"))
    }

    /// Returns the connected gRPC client after creation.
    fn client(&mut self) -> Result<&mut AuthenticatedAgentClient> {
        self.client
            .as_mut()
            .ok_or_else(|| anyhow!("remote agent client was not connected"))
    }

    /// Builds a decision request with optional available actions.
    fn decision_request(
        &self,
        available_actions: Option<Vec<A::Action>>,
    ) -> Result<RespondRequest> {
        let available_actions_provided = available_actions.is_some();
        let available_actions = available_actions
            .unwrap_or_default()
            .into_iter()
            .map(action_to_struct::<A::Action>)
            .collect::<Result<Vec<_>>>()?;
        Ok(RespondRequest {
            agent_id: self.agent_id()?,
            available_actions_provided,
            available_actions,
        })
    }
}

/// Converts a protobuf agent response into a typed Rust response.
fn proto_to_agent_response<Action: DeserializeOwned>(
    response: pb::AgentResponse,
) -> Result<AgentResponse<Action>> {
    let action = response.action.map(struct_to_action).transpose()?;
    match (action, response.message) {
        (Some(action), Some(message)) => Ok(AgentResponse::action_and_message(action, message)),
        (Some(action), None) => Ok(AgentResponse::action(action)),
        (None, Some(message)) => Ok(AgentResponse::message(message)),
        (None, None) => bail!("remote agent returned an empty response"),
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
    use crate::game::{Game, PlayerRole};

    struct TestAdapter;

    impl Game for TestAdapter {
        type Config = Value;
        type State = Value;
        type Action = Value;
        type Observation = Value;
        type Completion = Value;

        fn initial_state(&self, _config: &Self::Config, _seed: u64) -> Result<Self::State> {
            Ok(Value::Null)
        }

        fn apply_action(
            &self,
            state: &Self::State,
            _action: &Self::Action,
            _actor: PlayerRole,
        ) -> std::result::Result<Self::State, crate::ActionRejection> {
            Ok(state.clone())
        }

        fn observation(&self, state: &Self::State, _player: PlayerRole) -> Self::Observation {
            state.clone()
        }

        fn completion(&self, _state: &Self::State) -> Option<Self::Completion> {
            None
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
        let factory = RemoteGrpcAgentFactory::<TestAdapter>::new();
        let identity = factory
            .identity(&serde_json::json!({
                "endpoint": "http://127.0.0.1:50051",
                "agent_name": "python-agent"
            }))
            .unwrap();

        assert_eq!(identity.name, "python-agent");
        assert_eq!(identity.version, None);
    }
}
