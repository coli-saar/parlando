use std::{collections::BTreeMap, env, fmt, sync::Arc, time::Duration};

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
    agent_experiment::{CheckpointId, RLAgent, RLTrainingContext, TrainingBatch, TrajectoryStep},
    agents::{Agent, AgentContext, AgentFactory, AgentIdentity, AgentResponse},
    game::{
        AgentConfigField, AgentConfigValue, AgentDefinition, Game, PlayerRole, SecretPurpose,
        StringFormat,
    },
};

/// Generated protobuf types and gRPC service clients for remote agents.
pub mod pb {
    tonic::include_proto!("parlando.agent.v4");
}

/// Generated protobuf types and gRPC client for remote learners.
pub mod learner_pb {
    tonic::include_proto!("parlando.rl.v1");
}

use pb::{
    agent_service_client::AgentServiceClient, CreateAgentRequest, FinishRequest,
    ObserveMessageRequest, ObserveTransitionRequest, RespondRequest, ShutdownRequest, StartRequest,
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
    /// Required stable implementation release identifier stored in initialization requests.
    pub agent_version: String,
    /// Remote-agent protocol version expected by this server.
    #[serde(default = "default_protocol_version")]
    pub protocol_version: String,
    /// Agent-owned configuration forwarded without local interpretation.
    #[serde(default = "empty_json_object")]
    pub config: Value,
    /// Per-request timeout for create and act calls.
    #[serde(skip, default = "default_request_timeout")]
    pub request_timeout: Duration,
    #[serde(skip)]
    auth_token: Option<String>,
}

/// Returns the default opaque agent configuration.
fn empty_json_object() -> Value {
    Value::Object(serde_json::Map::new())
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
            .field("config", &self.config)
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
    "parlando-agent-v4".to_string()
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
pub struct RemoteAgent;

impl RemoteAgent {
    /// Creates a factory that will instantiate one remote agent per session participant.
    pub fn new() -> Self {
        Self
    }
}

impl Default for RemoteAgent {
    /// Creates the dashboard-configured remote gRPC factory.
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl<A: Game> AgentFactory<A> for RemoteAgent {
    /// Describes the standard remote gRPC agent choice for the dashboard.
    fn definition(&self) -> AgentDefinition {
        AgentDefinition {
            id: "remote_grpc".to_string(),
            name: "Remote gRPC agent".to_string(),
            description: "Connects to an external agent process using the Parlando agent protocol."
                .to_string(),
            config_fields: vec![
                AgentConfigField {
                    key: "config".to_string(),
                    label: "Remote configuration".to_string(),
                    help: "Opaque JSON delivered unchanged to the remote process.".to_string(),
                    value: AgentConfigValue::Json,
                    required: false,
                    default_value: empty_json_object(),
                },
                AgentConfigField {
                    key: "endpoint".to_string(),
                    label: "Agent endpoint".to_string(),
                    help: "HTTP/2 endpoint of the external agent process.".to_string(),
                    value: AgentConfigValue::String {
                        format: StringFormat::Uri,
                    },
                    required: true,
                    default_value: Value::String("http://127.0.0.1:50051".to_string()),
                },
                AgentConfigField {
                    key: "agent_name".to_string(),
                    label: "Agent name".to_string(),
                    help: "Name recorded for the remote agent implementation.".to_string(),
                    value: AgentConfigValue::String {
                        format: StringFormat::Plain,
                    },
                    required: true,
                    default_value: Value::String("remote-agent".to_string()),
                },
                AgentConfigField {
                    key: "agent_version".to_string(),
                    label: "Agent version".to_string(),
                    help: "Required stable implementation release identifier.".to_string(),
                    value: AgentConfigValue::String {
                        format: StringFormat::Plain,
                    },
                    required: true,
                    default_value: Value::Null,
                },
                AgentConfigField {
                    key: "protocol_version".to_string(),
                    label: "Protocol version".to_string(),
                    help: "Remote protocol identifier; normally left at its default.".to_string(),
                    value: AgentConfigValue::String {
                        format: StringFormat::Plain,
                    },
                    required: true,
                    default_value: Value::String(default_protocol_version()),
                },
                AgentConfigField {
                    key: "bearer_token".to_string(),
                    label: "Bearer credential".to_string(),
                    help: "Experiment secret used only to authenticate the transport.".to_string(),
                    value: AgentConfigValue::SecretReference {
                        purpose: SecretPurpose::Factory,
                    },
                    required: false,
                    default_value: Value::Null,
                },
                AgentConfigField {
                    key: "agent_secret".to_string(),
                    label: "Agent-instance secret".to_string(),
                    help: "Optional experiment secret explicitly authorized for delivery to the remote agent.".to_string(),
                    value: AgentConfigValue::SecretReference { purpose: SecretPurpose::AgentInstance },
                    required: false,
                    default_value: Value::Null,
                },
            ],
        }
    }

    /// Creates one lazy remote-agent handle for a session participant.
    async fn create(&self, context: AgentContext) -> Result<Box<dyn Agent<A> + Send>> {
        create_remote_instance(context, None).await
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

/// Creates and initializes a remote agent, optionally pinned to a learner checkpoint.
async fn create_remote_instance<A: Game>(
    context: AgentContext,
    checkpoint: Option<CheckpointId>,
) -> Result<Box<dyn Agent<A> + Send>> {
    let mut config = remote_config_from_settings(&context.settings)?;
    config.auth_token = context
        .factory_secrets
        .get("config.bearer_token")
        .map(str::to_string);
    let mut agent = RemoteAgentInstance {
        config,
        init_context: context,
        client: None,
        agent_id: None,
        checkpoint,
    };
    agent.ensure_created().await?;
    Ok(Box::new(agent))
}

/// Factory view of one immutable checkpoint owned by a remote learner.
struct RemoteCheckpointFactory {
    checkpoint: CheckpointId,
}

#[async_trait]
impl<G: Game> AgentFactory<G> for RemoteCheckpointFactory {
    /// Uses the same transport and opaque configuration schema as [`RemoteAgent`].
    fn definition(&self) -> AgentDefinition {
        <RemoteAgent as AgentFactory<G>>::definition(&RemoteAgent)
    }

    /// Creates an inference agent pinned to this factory's checkpoint.
    async fn create(&self, context: AgentContext) -> Result<Box<dyn Agent<G> + Send>> {
        create_remote_instance(context, Some(self.checkpoint.clone())).await
    }

    /// Returns the remote implementation identity independently of checkpoint identity.
    fn identity(&self, settings: &Value) -> Result<AgentIdentity> {
        <RemoteAgent as AgentFactory<G>>::identity(&RemoteAgent, settings)
    }
}

#[async_trait]
impl<G: Game> RLAgent<G> for RemoteAgent {
    /// Returns the standard remote transport factory selector.
    fn factory_id(&self) -> &str {
        "remote_grpc"
    }

    /// Treats a non-empty YAML string as a learner-owned checkpoint identifier.
    fn resolve_checkpoint(&self, reference: &Value) -> Result<CheckpointId> {
        CheckpointId::new(
            reference
                .as_str()
                .context("remote checkpoint reference must be a string")?,
        )
    }

    /// Exposes an immutable checkpoint through the ordinary agent factory interface.
    fn factory(&self, checkpoint: &CheckpointId) -> Result<Arc<dyn AgentFactory<G>>> {
        Ok(Arc::new(RemoteCheckpointFactory {
            checkpoint: checkpoint.clone(),
        }))
    }

    /// Sends an opaque configuration and a language-neutral trajectory to the learner service.
    async fn train(
        &mut self,
        context: &RLTrainingContext,
        base: &CheckpointId,
        batch: TrainingBatch,
    ) -> Result<CheckpointId> {
        let mut config = remote_config_from_settings(&context.settings)?;
        config.auth_token = context
            .factory_secrets
            .get("config.bearer_token")
            .map(str::to_string);
        validate_remote_endpoint(&config.endpoint, config.auth_token.is_some())?;
        let channel = Channel::from_shared(config.endpoint.clone())?
            .connect()
            .await
            .context("failed to connect to remote learner service")?;
        let authorization = config
            .auth_token
            .as_ref()
            .map(|token| format!("Bearer {token}").parse::<MetadataValue<Ascii>>())
            .transpose()?;
        let mut client = learner_pb::learner_service_client::LearnerServiceClient::with_interceptor(
            channel,
            RemoteAuthInterceptor { authorization },
        );
        let request = learner_pb::TrainRequest {
            update_id: batch.update_id,
            base_checkpoint_id: base.as_str().to_string(),
            completed_epochs: batch.completed_epochs,
            steps: batch
                .steps
                .into_iter()
                .map(trajectory_to_proto)
                .collect::<Result<_>>()?,
            settings: Some(json_to_struct(config.config)?),
        };
        let response = tokio::time::timeout(config.request_timeout, client.train(request))
            .await
            .context("remote learner train timed out")?
            .context("remote learner train failed")?
            .into_inner();
        CheckpointId::new(response.checkpoint_id)
    }
}

/// Parses validated dashboard-owned non-secret settings.
fn remote_config_from_settings(settings: &Value) -> Result<RemoteGrpcAgentConfig> {
    let mut settings = settings.clone();
    settings.as_object_mut().map(|object| {
        object.remove("bearer_token");
        object.remove("agent_secret");
    });
    serde_json::from_value(settings).map_err(Into::into)
}

/// Per-session remote gRPC agent instance.
pub struct RemoteAgentInstance {
    config: RemoteGrpcAgentConfig,
    init_context: AgentContext,
    client: Option<AuthenticatedAgentClient>,
    agent_id: Option<String>,
    checkpoint: Option<CheckpointId>,
}

impl RemoteAgentInstance {
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
            agent_version: self.config.agent_version.clone(),
            role: self.init_context.role.as_str().to_string(),
            seed: self.init_context.seed,
            config: Some(json_to_struct(self.config.config.clone())?),
            agent_instance_secrets: Some(json_to_struct(Value::Object(
                self.init_context
                    .agent_instance_secrets
                    .iter()
                    .map(|(key, value)| (key.clone(), Value::String(value.clone())))
                    .collect(),
            ))?),
            checkpoint_id: self
                .checkpoint
                .as_ref()
                .map(|value| value.as_str().to_string()),
        };
        let response =
            tokio::time::timeout(self.config.request_timeout, client.create_agent(request))
                .await
                .context("remote agent create timed out")?
                .context("remote agent create failed")?
                .into_inner();
        self.record_remote_logs(response.session_logs);
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
            bail!("non-loopback remote-agent TLS requires a configured factory bearer credential")
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
impl<A> Agent<A> for RemoteAgentInstance
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
            .context("remote agent start failed")?
            .into_inner()
            .session_logs
            .into_iter()
            .for_each(|entry| {
                let _ = self.init_context.logger.log(entry);
            });
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
        let logs = tokio::time::timeout(request_timeout, client.observe_transition(request))
            .await
            .context("remote agent observe_transition timed out")?
            .context("remote agent observe_transition failed")?
            .into_inner()
            .session_logs;
        self.record_remote_logs(logs);
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
        let logs = tokio::time::timeout(request_timeout, client.observe_message(request))
            .await
            .context("remote agent observe_message timed out")?
            .context("remote agent observe_message failed")?
            .into_inner()
            .session_logs;
        self.record_remote_logs(logs);
        Ok(())
    }

    /// Sends the shared terminal result to the remote agent before shutdown.
    async fn finish(&mut self, completion: A::Completion) -> Result<()> {
        self.ensure_created().await?;
        let request = FinishRequest {
            agent_id: self.agent_id()?,
            completion: Some(json_to_struct(serde_json::to_value(completion)?)?),
        };
        let logs =
            tokio::time::timeout(self.config.request_timeout, self.client()?.finish(request))
                .await
                .context("remote agent finish timed out")?
                .context("remote agent finish failed")?
                .into_inner()
                .session_logs;
        self.record_remote_logs(logs);
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
        self.record_remote_logs(response.session_logs);
        response.response.map(proto_to_agent_response).transpose()
    }

    /// Releases the corresponding remote server instance on normal completion or cancellation.
    async fn shutdown(&mut self) -> Result<()> {
        let Some(agent_id) = self.agent_id.clone() else {
            return Ok(());
        };
        let request_timeout = self.config.request_timeout;
        let logs = tokio::time::timeout(
            request_timeout,
            self.client()?.shutdown(ShutdownRequest { agent_id }),
        )
        .await
        .context("remote agent shutdown timed out")?
        .context("remote agent shutdown failed")?
        .into_inner()
        .session_logs;
        self.record_remote_logs(logs);
        self.agent_id = None;
        Ok(())
    }
}

impl RemoteAgentInstance {
    /// Writes response-carried remote entries through the locally scoped session logger.
    fn record_remote_logs(&self, entries: Vec<String>) {
        for entry in entries {
            let _ = self.init_context.logger.log(entry);
        }
    }

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
    fn decision_request<Action: Serialize>(
        &self,
        available_actions: Option<Vec<Action>>,
    ) -> Result<RespondRequest> {
        let available_actions_provided = available_actions.is_some();
        let available_actions = available_actions
            .unwrap_or_default()
            .into_iter()
            .map(action_to_struct::<Action>)
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

/// Converts one public trajectory step into the remote learner protocol.
fn trajectory_to_proto(step: TrajectoryStep) -> Result<learner_pb::TrajectoryStep> {
    Ok(learner_pb::TrajectoryStep {
        run_id: step.run_id,
        plan_id: step.plan_id,
        decision: step.decision,
        scenario: step.scenario,
        role: step.role.as_str().to_string(),
        agent: step.agent,
        checkpoint_id: step.checkpoint.as_str().to_string(),
        reward: step.reward,
        reward_version: step.reward_version,
        observation: Some(json_to_prost(step.observation)?),
        available_actions: Some(json_to_prost(step.available_actions)?),
        action: Some(json_to_prost(step.action)?),
        accepted: step.accepted,
        rejection: step
            .rejection
            .map(|value| json_to_struct(serde_json::to_value(value)?))
            .transpose()?,
        rewards: step.rewards.map(|value| learner_pb::RoleRewards {
            player_a: value.player_a,
            player_b: value.player_b,
        }),
        next_observation: Some(json_to_prost(step.next_observation)?),
        terminal: step.terminal,
    })
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

        fn initial_state(
            &self,
            _context: crate::GameInitializationContext<'_, Self::Config>,
        ) -> Result<Self::State> {
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
    fn remote_identity_requires_agent_version() {
        let factory = RemoteAgent::new();
        let error = <RemoteAgent as AgentFactory<TestAdapter>>::identity(
            &factory,
            &serde_json::json!({
                "endpoint": "http://127.0.0.1:50051",
                "agent_name": "python-agent"
            }),
        )
        .unwrap_err();

        assert!(error.to_string().contains("agent_version"));
    }
}
