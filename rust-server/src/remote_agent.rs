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
    game::{AgentConfigField, AgentConfigValue, AgentDefinition, Game, PlayerRole, StringFormat},
};

/// Generated protobuf types and gRPC service clients for remote agents.
pub mod pb {
    tonic::include_proto!("parlando.agent.v5");
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
    /// Optional YAML mapping forwarded to the remote process as structured settings.
    #[serde(default)]
    pub config_yaml: String,
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
            .field("config_yaml", &self.config_yaml)
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
                    key: "config_yaml".to_string(),
                    label: "Remote configuration".to_string(),
                    help: "Optional YAML mapping delivered to the remote process. Keep credentials in the remote process itself."
                        .to_string(),
                    value: AgentConfigValue::String {
                        format: StringFormat::Yaml,
                    },
                    required: false,
                    default_value: Value::String(String::new()),
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
        remote_config_from_settings(settings)?;
        Ok(AgentIdentity {
            name: "remote-agent".to_string(),
            version: "parlando-agent-v5".to_string(),
        })
    }
}

/// Creates and initializes a remote agent, optionally pinned to a learner checkpoint.
async fn create_remote_instance<A: Game>(
    context: AgentContext,
    checkpoint: Option<CheckpointId>,
) -> Result<Box<dyn Agent<A> + Send>> {
    let config = remote_config_from_settings(&context.settings)?;
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
        let config = remote_config_from_settings(&context.settings)?;
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
            settings: Some(json_to_struct(config.structured_settings()?)?),
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
    let mut config: RemoteGrpcAgentConfig = serde_json::from_value(settings.clone())?;
    config.auth_token = env::var("PARLANDO_REMOTE_AGENT_TOKEN")
        .ok()
        .filter(|token| !token.trim().is_empty());
    config.structured_settings()?;
    Ok(config)
}

impl RemoteGrpcAgentConfig {
    /// Parses the optional dashboard YAML into the mapping accepted by the remote protocol.
    fn structured_settings(&self) -> Result<Value> {
        if self.config_yaml.trim().is_empty() {
            return Ok(Value::Object(serde_json::Map::new()));
        }
        let value: Value = serde_yaml::from_str(&self.config_yaml)
            .context("remote agent configuration must be valid YAML")?;
        if !value.is_object() {
            bail!("remote agent configuration must be a YAML mapping");
        }
        Ok(value)
    }
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
            role: self.init_context.role.as_str().to_string(),
            seed: self.init_context.seed,
            config: Some(json_to_struct(self.config.structured_settings()?)?),
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
            bail!("non-loopback remote-agent TLS requires PARLANDO_REMOTE_AGENT_TOKEN")
        }
        Some("http") => {
            let host = uri
                .host()
                .ok_or_else(|| anyhow!("remote agent endpoint has no host"))?;
            if matches!(host, "localhost" | "127.0.0.1" | "::1" | "[::1]") {
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
    if !matches!(host, "localhost" | "127.0.0.1" | "::1" | "[::1]") {
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
        Value::Number(value) => {
            const MAX_EXACT_INTEGER: i128 = 9_007_199_254_740_991;
            if let Some(integer) = value.as_i64() {
                if i128::from(integer).abs() > MAX_EXACT_INTEGER {
                    bail!("remote agent integer exceeds protobuf Struct exact range");
                }
            } else if let Some(integer) = value.as_u64() {
                if i128::from(integer) > MAX_EXACT_INTEGER {
                    bail!("remote agent integer exceeds protobuf Struct exact range");
                }
            }
            Kind::NumberValue(
                value
                    .as_f64()
                    .ok_or_else(|| anyhow!("remote agent number is not finite"))?,
            )
        }
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
    fn remote_identity_is_transport_owned() {
        let factory = RemoteAgent::new();
        let identity = <RemoteAgent as AgentFactory<TestAdapter>>::identity(
            &factory,
            &serde_json::json!({
                "endpoint": "http://127.0.0.1:50051",
                "config_yaml": "model: local\ntemperature: 0.2\n"
            }),
        )
        .unwrap();

        assert_eq!(identity.name, "remote-agent");
        assert_eq!(identity.version, "parlando-agent-v5");
    }

    /// The dashboard contract contains only endpoint and optional YAML settings.
    #[test]
    fn remote_dashboard_definition_has_two_non_secret_fields() {
        let definition = <RemoteAgent as AgentFactory<TestAdapter>>::definition(&RemoteAgent);
        assert_eq!(
            definition
                .config_fields
                .iter()
                .map(|field| field.key.as_str())
                .collect::<Vec<_>>(),
            vec!["endpoint", "config_yaml"]
        );
        let serialized = serde_json::to_value(definition).unwrap();
        assert_eq!(serialized["config_fields"][1]["format"], "yaml");
        assert_eq!(serialized["config_fields"][1]["default_value"], "");
        assert_eq!(serialized["config_fields"][1]["required"], false);
    }

    /// Remote YAML is optional, must be a mapping, and reaches the protocol as structured data.
    #[test]
    fn remote_yaml_settings_are_parsed_as_a_mapping() {
        let empty = remote_config_from_settings(&serde_json::json!({
            "endpoint": "http://127.0.0.1:50051",
            "config_yaml": ""
        }))
        .unwrap();
        assert_eq!(empty.structured_settings().unwrap(), serde_json::json!({}));

        let configured = remote_config_from_settings(&serde_json::json!({
            "endpoint": "http://127.0.0.1:50051",
            "config_yaml": "model: local\nnested:\n  enabled: true\n"
        }))
        .unwrap();
        assert_eq!(
            configured.structured_settings().unwrap(),
            serde_json::json!({"model": "local", "nested": {"enabled": true}})
        );

        for invalid in ["- item\n", "key: [\n"] {
            assert!(remote_config_from_settings(&serde_json::json!({
                "endpoint": "http://127.0.0.1:50051",
                "config_yaml": invalid
            }))
            .is_err());
        }
    }

    /// Endpoint validation permits only literal loopback cleartext and authenticated TLS remotes.
    #[test]
    fn remote_endpoint_validation_rejects_unsafe_transport_shapes() {
        for endpoint in [
            "http://localhost:50051",
            "http://127.0.0.1:50051",
            "http://[::1]:50051",
        ] {
            validate_remote_endpoint(endpoint, false).unwrap();
        }
        for (endpoint, authenticated, expected) in [
            ("http://agent.example:50051", false, "cleartext"),
            (
                "https://agent.example",
                false,
                "PARLANDO_REMOTE_AGENT_TOKEN",
            ),
            ("ftp://localhost/service", true, "must use https"),
            ("not a URI", false, "invalid remote agent endpoint"),
        ] {
            let error = validate_remote_endpoint(endpoint, authenticated)
                .unwrap_err()
                .to_string();
            assert!(
                error.contains(expected),
                "{error:?} did not contain {expected:?}"
            );
        }
    }

    /// Protobuf Struct conversion rejects integers that a double would silently round.
    #[test]
    fn protobuf_conversion_rejects_inexact_large_integers() {
        let exact = serde_json::json!({"integer": 9_007_199_254_740_991_u64});
        assert_eq!(
            struct_to_json(json_to_struct(exact.clone()).unwrap()),
            exact
        );
        for inexact in [
            serde_json::json!({"integer": 9_007_199_254_740_992_u64}),
            serde_json::json!({"integer": -9_007_199_254_740_992_i64}),
        ] {
            assert!(json_to_struct(inexact)
                .unwrap_err()
                .to_string()
                .contains("exact range"));
        }
    }

    /// Malicious non-finite protobuf numbers normalize to JSON null instead of panicking.
    #[test]
    fn protobuf_nonfinite_numbers_fail_closed_to_null() {
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(
                prost_to_json(ProstValue {
                    kind: Some(Kind::NumberValue(value)),
                }),
                Value::Null
            );
        }
    }
}
