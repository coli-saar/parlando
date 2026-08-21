use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use axum::Router;
use futures_util::{SinkExt, StreamExt};
use parlando::{
    agent::grpc::Factory as RemoteGrpcAgentFactory,
    agent::{
        Agent, Context as AgentContext, Definition as AgentDefinition, Factory as AgentFactory,
        Identity as AgentIdentity, Response as AgentResponse,
    },
    test_support::{
        build_router,
        remote_agent_pb::{
            agent_service_server::{AgentService, AgentServiceServer},
            AgentResponse as PbAgentResponse, CreateAgentRequest, CreateAgentResponse,
            FinishRequest, ObserveMessageRequest, ObserveResponse, ObserveTransitionRequest,
            RespondRequest, RespondResponse, ShutdownRequest, ShutdownResponse, StartRequest,
        },
        AgentsConfig, AgentsMode, AudioChunk, ConsentItemConfig, DatabaseConfig, DirectConfig,
        ExperimentConfig, ExperimentIdentityConfig, HumanVsAgentConfig, ServeOptions,
        SpeechmaticsConfig, StreamingTtsProvider, TranscriptionConfig, VoiceConfig,
    },
    ActionRejection, Game, GameInitializationContext, PlayerRole,
};
use prost_types::{value::Kind, Struct, Value as ProstValue};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio_stream::wrappers::TcpListenerStream;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tonic::{transport::Server, Request as TonicRequest, Response as TonicResponse, Status};

type TestSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

#[derive(Clone, Debug, Deserialize, Serialize)]
struct DummyState {
    actions: usize,
    done: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum DummyAction {
    Mark { finish: bool },
}

#[derive(Clone, Debug, Serialize)]
struct DummyObservation {
    role: String,
    actions: usize,
    done: bool,
}

#[derive(Clone, Debug, Serialize)]
struct DummySummary {
    done: bool,
    actions: usize,
}

#[derive(Clone)]
struct DummyAdapter;

struct DummyGameFactory;

impl parlando::GameFactory for DummyGameFactory {
    type Game = DummyAdapter;

    /// Creates one stateless integration-test game.
    fn create(&self, _context: parlando::GameSessionContext) -> Result<DummyAdapter> {
        Ok(DummyAdapter)
    }
}

impl Game for DummyAdapter {
    type Config = Value;
    type State = DummyState;
    type Action = DummyAction;
    type Observation = DummyObservation;
    type Completion = DummySummary;

    /// Creates one stateless test adapter for an admitted session.
    fn initial_state(
        &self,
        _context: GameInitializationContext<'_, Self::Config>,
    ) -> Result<Self::State> {
        Ok(DummyState {
            actions: 0,
            done: false,
        })
    }

    fn apply_action(
        &self,
        state: &Self::State,
        action: &Self::Action,
        _actor: PlayerRole,
    ) -> std::result::Result<Self::State, ActionRejection> {
        if state.done {
            return Err(ActionRejection::new("game_complete"));
        }
        let DummyAction::Mark { finish } = action;
        Ok(DummyState {
            actions: state.actions + 1,
            done: *finish,
        })
    }

    fn observation(&self, state: &Self::State, player: PlayerRole) -> Self::Observation {
        DummyObservation {
            role: player.as_str().to_string(),
            actions: state.actions,
            done: state.done,
        }
    }

    fn available_actions(
        &self,
        _state: &Self::State,
        _player: PlayerRole,
    ) -> Option<Vec<Self::Action>> {
        Some(vec![
            DummyAction::Mark { finish: false },
            DummyAction::Mark { finish: true },
        ])
    }

    fn completion(&self, state: &Self::State) -> Option<Self::Completion> {
        state.done.then(|| DummySummary {
            done: state.done,
            actions: state.actions,
        })
    }
}

struct ScriptedAgent {
    script: VecDeque<Option<AgentResponse<DummyAction>>>,
}

#[async_trait]
impl Agent<DummyAdapter> for ScriptedAgent {
    async fn start(&mut self, observation: DummyObservation) -> Result<()> {
        assert_eq!(observation.role, "B");
        Ok(())
    }

    async fn respond(
        &mut self,
        available_actions: Option<Vec<DummyAction>>,
    ) -> Result<Option<AgentResponse<DummyAction>>> {
        assert_eq!(
            available_actions,
            Some(vec![
                DummyAction::Mark { finish: false },
                DummyAction::Mark { finish: true }
            ])
        );
        Ok(self.script.pop_front().unwrap_or(None))
    }
}

struct ScriptedAgentFactory {
    scripts: Mutex<VecDeque<Vec<Option<AgentResponse<DummyAction>>>>>,
}

impl ScriptedAgentFactory {
    fn new(script: Vec<Option<AgentResponse<DummyAction>>>) -> Self {
        Self {
            scripts: Mutex::new(VecDeque::from([script])),
        }
    }
}

#[async_trait]
impl AgentFactory<DummyAdapter> for ScriptedAgentFactory {
    fn definition(&self) -> AgentDefinition {
        AgentDefinition {
            id: "mock".to_string(),
            name: "Scripted test agent".to_string(),
            description: "Used by the integration test.".to_string(),
            config_fields: Vec::new(),
        }
    }

    async fn create(&self, context: AgentContext) -> Result<Box<dyn Agent<DummyAdapter> + Send>> {
        assert_eq!(context.role, PlayerRole::B);
        let script = self.scripts.lock().unwrap().pop_front().unwrap_or_default();
        Ok(Box::new(ScriptedAgent {
            script: script.into(),
        }))
    }

    fn identity(&self, _settings: &Value) -> Result<AgentIdentity> {
        Ok(AgentIdentity {
            name: "ScriptedAgent".to_string(),
            version: "1".to_string(),
        })
    }
}

struct RecordingTts {
    messages: Mutex<Vec<String>>,
}

#[async_trait]
impl StreamingTtsProvider for RecordingTts {
    async fn synthesize(&self, text: &str, _message_id: &str) -> Result<Vec<AudioChunk>> {
        self.messages.lock().unwrap().push(text.to_string());
        Ok(vec![AudioChunk {
            data: vec![1, 2, 3, 4],
            sample_rate: 16000,
            channels: 1,
        }])
    }
}

#[derive(Default)]
struct MockRemoteAgentState {
    create_requests: Mutex<Vec<CreateAgentRequest>>,
    start_requests: Mutex<Vec<StartRequest>>,
    respond_requests: Mutex<Vec<RespondRequest>>,
    finish_requests: Mutex<Vec<FinishRequest>>,
}

#[derive(Clone)]
struct MockRemoteAgentService {
    state: Arc<MockRemoteAgentState>,
}

#[async_trait]
impl AgentService for MockRemoteAgentService {
    async fn create_agent(
        &self,
        request: TonicRequest<CreateAgentRequest>,
    ) -> std::result::Result<TonicResponse<CreateAgentResponse>, Status> {
        self.state
            .create_requests
            .lock()
            .unwrap()
            .push(request.into_inner());
        Ok(TonicResponse::new(CreateAgentResponse {
            agent_id: "remote-agent-1".to_string(),
            session_logs: Vec::new(),
        }))
    }

    async fn start(
        &self,
        request: TonicRequest<StartRequest>,
    ) -> std::result::Result<TonicResponse<ObserveResponse>, Status> {
        self.state
            .start_requests
            .lock()
            .unwrap()
            .push(request.into_inner());
        Ok(TonicResponse::new(ObserveResponse {
            session_logs: Vec::new(),
        }))
    }

    async fn observe_transition(
        &self,
        _request: TonicRequest<ObserveTransitionRequest>,
    ) -> std::result::Result<TonicResponse<ObserveResponse>, Status> {
        Ok(TonicResponse::new(ObserveResponse {
            session_logs: Vec::new(),
        }))
    }

    async fn observe_message(
        &self,
        _request: TonicRequest<ObserveMessageRequest>,
    ) -> std::result::Result<TonicResponse<ObserveResponse>, Status> {
        Ok(TonicResponse::new(ObserveResponse {
            session_logs: Vec::new(),
        }))
    }

    async fn finish(
        &self,
        request: TonicRequest<FinishRequest>,
    ) -> std::result::Result<TonicResponse<ObserveResponse>, Status> {
        self.state
            .finish_requests
            .lock()
            .unwrap()
            .push(request.into_inner());
        Ok(TonicResponse::new(ObserveResponse {
            session_logs: Vec::new(),
        }))
    }

    async fn respond(
        &self,
        request: TonicRequest<RespondRequest>,
    ) -> std::result::Result<TonicResponse<RespondResponse>, Status> {
        let decision_count = {
            let mut requests = self.state.respond_requests.lock().unwrap();
            requests.push(request.into_inner());
            requests.len()
        };
        Ok(TonicResponse::new(RespondResponse {
            response: Some(PbAgentResponse {
                message: (decision_count == 1).then(|| "hello from remote grpc".to_string()),
                action: Some(dummy_action_struct(decision_count > 1)),
            }),
            session_logs: Vec::new(),
        }))
    }

    async fn shutdown(
        &self,
        _request: TonicRequest<ShutdownRequest>,
    ) -> std::result::Result<TonicResponse<ShutdownResponse>, Status> {
        Ok(TonicResponse::new(ShutdownResponse {
            session_logs: Vec::new(),
        }))
    }
}

struct TestServer {
    base_url: String,
    ws_base_url: String,
    _temp: TempDir,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct MockGrpcServer {
    endpoint: String,
    state: Arc<MockRemoteAgentState>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for MockGrpcServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn spawn_mock_remote_agent() -> Result<MockGrpcServer> {
    let state = Arc::new(MockRemoteAgentState::default());
    let service = MockRemoteAgentService {
        state: state.clone(),
    };
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let task = tokio::spawn(async move {
        Server::builder()
            .add_service(AgentServiceServer::new(service))
            .serve_with_incoming(TcpListenerStream::new(listener))
            .await
            .unwrap();
    });
    Ok(MockGrpcServer {
        endpoint: format!("http://{addr}"),
        state,
        task,
    })
}

fn dummy_action_struct(finish: bool) -> Struct {
    Struct {
        fields: BTreeMap::from([
            (
                "type".to_string(),
                ProstValue {
                    kind: Some(Kind::StringValue("mark".to_string())),
                },
            ),
            (
                "finish".to_string(),
                ProstValue {
                    kind: Some(Kind::BoolValue(finish)),
                },
            ),
        ]),
    }
}

async fn spawn_server(
    config: ExperimentConfig,
    options: ServeOptions<DummyAdapter>,
) -> Result<TestServer> {
    let temp = TempDir::new()?;
    let mut config = config;
    config.database = DatabaseConfig {
        url: format!("sqlite:///{}", temp.path().join("mock-client.db").display()),
    };
    let router = build_router(DummyGameFactory, config, options).await?;
    let server = spawn_router(router, temp).await?;
    let client = reqwest::Client::new();
    let admin = admin_setup(&client, &server.base_url).await?;
    let response = client
        .post(format!("{}/api/admin/experiment/status", server.base_url))
        .header(reqwest::header::COOKIE, admin.cookie)
        .header("x-csrf-token", admin.csrf_token)
        .json(&json!({"status": "active"}))
        .send()
        .await?;
    anyhow::ensure!(response.status().is_success(), "test activation failed");
    Ok(server)
}

async fn spawn_router(router: Router, temp: TempDir) -> Result<TestServer> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    Ok(TestServer {
        base_url: format!("http://{addr}"),
        ws_base_url: format!("ws://{addr}"),
        _temp: temp,
        task,
    })
}

fn config(mode: AgentsMode) -> ExperimentConfig {
    let human_vs_agent = (mode == AgentsMode::HumanVsAgent).then(|| HumanVsAgentConfig {
        factory: Some("mock".to_string()),
        act_timeout_seconds: 2.0,
        invalid_action_limit: 2,
        ..HumanVsAgentConfig::default()
    });
    ExperimentConfig {
        experiment: ExperimentIdentityConfig {
            id: Some("mock-client-smoke".to_string()),
        },
        database: DatabaseConfig {
            url: "sqlite:///:memory:".to_string(),
        },
        direct: DirectConfig {
            consents: vec![ConsentItemConfig {
                id: "study".to_string(),
                title: "Study consent".to_string(),
                body: "I agree".to_string(),
                required: true,
            }],
            ..DirectConfig::default()
        },
        voice: VoiceConfig {
            enabled: true,
            ..VoiceConfig::default()
        },
        speechmatics: SpeechmaticsConfig::default(),
        transcription: TranscriptionConfig {
            enabled: false,
            provider: "speechmatics".to_string(),
            ..TranscriptionConfig::default()
        },
        agents: AgentsConfig {
            mode,
            human_vs_agent,
        },
        ..ExperimentConfig::default()
    }
}

struct TestParticipant {
    id: String,
    credential: String,
}

async fn create_participant(
    client: &reqwest::Client,
    base_url: &str,
    _name: &str,
) -> Result<TestParticipant> {
    let response = client
        .post(format!("{base_url}/api/participants"))
        .json(&json!({}))
        .send()
        .await?
        .error_for_status()?
        .json::<Value>()
        .await?;
    Ok(TestParticipant {
        id: response["participant_id"]
            .as_str()
            .ok_or_else(|| anyhow!("missing participant_id"))?
            .to_string(),
        credential: response["participant_credential"]
            .as_str()
            .ok_or_else(|| anyhow!("missing participant_credential"))?
            .to_string(),
    })
}

async fn consent(
    client: &reqwest::Client,
    base_url: &str,
    participant: &TestParticipant,
) -> Result<()> {
    client
        .post(format!("{base_url}/api/consent"))
        .bearer_auth(&participant.credential)
        .json(&json!({
            "decisions": {"study": true}
        }))
        .send()
        .await?
        .error_for_status()?;
    Ok(())
}

async fn create_room(
    client: &reqwest::Client,
    base_url: &str,
    participant: &TestParticipant,
) -> Result<Value> {
    Ok(client
        .post(format!("{base_url}/api/sessions"))
        .bearer_auth(&participant.credential)
        .json(&json!({}))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}

async fn ws_connect(
    client: &reqwest::Client,
    server: &TestServer,
    public_session_id: &str,
    participant: &TestParticipant,
) -> Result<TestSocket> {
    let plan = client
        .post(format!(
            "{}/api/sessions/{public_session_id}/game-session",
            server.base_url
        ))
        .bearer_auth(&participant.credential)
        .json(&json!({}))
        .send()
        .await?
        .error_for_status()?
        .json::<Value>()
        .await?;
    let token = plan["token"]
        .as_str()
        .ok_or_else(|| anyhow!("missing game ticket"))?;
    let (socket, _) = connect_async(format!(
        "{}/ws/game/{public_session_id}?token={token}",
        server.ws_base_url
    ))
    .await?;
    Ok(socket)
}

struct TestAdminSession {
    cookie: String,
    csrf_token: String,
}

async fn admin_setup(client: &reqwest::Client, base_url: &str) -> Result<TestAdminSession> {
    admin_auth_request(client, base_url, "setup").await
}

/// Signs in to an administrator credential already created by the test.
async fn admin_login(client: &reqwest::Client, base_url: &str) -> Result<TestAdminSession> {
    admin_auth_request(client, base_url, "login").await
}

/// Submits one setup or login request and extracts its session capability.
async fn admin_auth_request(
    client: &reqwest::Client,
    base_url: &str,
    operation: &str,
) -> Result<TestAdminSession> {
    let response = client
        .post(format!("{base_url}/api/admin/{operation}"))
        .json(&json!({"username": "smoke-admin", "password": "test-password"}))
        .send()
        .await?
        .error_for_status()?;
    let cookie = response
        .headers()
        .get(reqwest::header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .ok_or_else(|| anyhow!("missing administrator cookie"))?
        .to_string();
    let body = response.json::<Value>().await?;
    Ok(TestAdminSession {
        cookie,
        csrf_token: body["csrf_token"]
            .as_str()
            .ok_or_else(|| anyhow!("missing administrator CSRF token"))?
            .to_string(),
    })
}

async fn admin_export(
    client: &reqwest::Client,
    base_url: &str,
    admin: &TestAdminSession,
) -> Result<Value> {
    Ok(client
        .get(format!("{base_url}/api/admin/export"))
        .header(reqwest::header::COOKIE, &admin.cookie)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}

async fn send_ws(socket: &mut TestSocket, payload: Value) -> Result<()> {
    socket.send(Message::Text(payload.to_string())).await?;
    Ok(())
}

async fn read_ws_type(socket: &mut TestSocket, expected: &str) -> Result<Value> {
    for _ in 0..40 {
        let message = tokio::time::timeout(Duration::from_secs(2), socket.next())
            .await?
            .ok_or_else(|| anyhow!("websocket closed"))??;
        if !message.is_text() {
            continue;
        }
        let value: Value = serde_json::from_str(message.to_text()?)?;
        if value["type"] == expected {
            return Ok(value);
        }
        if value["type"] == "error" {
            bail!("unexpected websocket error: {value}");
        }
    }
    bail!("timed out waiting for websocket message type {expected}");
}

#[tokio::test]
async fn public_security_boundaries_reject_anonymous_and_cross_participant_requests() -> Result<()>
{
    let server = spawn_server(config(AgentsMode::HumanVsHuman), ServeOptions::default()).await?;
    let client = reqwest::Client::new();
    let a = create_participant(&client, &server.base_url, "A").await?;
    let b = create_participant(&client, &server.base_url, "B").await?;

    let anonymous_room = client
        .post(format!("{}/api/sessions", server.base_url))
        .json(&json!({"participant_session_id": a.id, "mode": "direct"}))
        .send()
        .await?;
    assert_eq!(anonymous_room.status(), reqwest::StatusCode::UNAUTHORIZED);

    let impersonation = client
        .post(format!("{}/api/consent", server.base_url))
        .bearer_auth(&a.credential)
        .json(&json!({
            "participant_session_id": b.id,
            "decisions": {"study": true}
        }))
        .send()
        .await?;
    assert_eq!(
        impersonation.status(),
        reqwest::StatusCode::UNPROCESSABLE_ENTITY
    );

    let anonymous_export = client
        .get(format!("{}/api/admin/export", server.base_url))
        .send()
        .await?;
    assert_eq!(anonymous_export.status(), reqwest::StatusCode::UNAUTHORIZED);

    let privileged_source = client
        .post(format!("{}/api/participants", server.base_url))
        .json(&json!({"source": "agent"}))
        .send()
        .await?;
    assert_eq!(
        privileged_source.status(),
        reqwest::StatusCode::UNPROCESSABLE_ENTITY
    );

    let admin = admin_login(&client, &server.base_url).await?;
    let missing_csrf = client
        .post(format!("{}/api/admin/experiment/status", server.base_url))
        .header(reqwest::header::COOKIE, &admin.cookie)
        .json(&json!({"status": "inactive"}))
        .send()
        .await?;
    assert_eq!(missing_csrf.status(), reqwest::StatusCode::FORBIDDEN);

    let valid_csrf = client
        .post(format!("{}/api/admin/experiment/status", server.base_url))
        .header(reqwest::header::COOKIE, &admin.cookie)
        .header("x-csrf-token", &admin.csrf_token)
        .json(&json!({"status": "active"}))
        .send()
        .await?;
    assert_eq!(valid_csrf.status(), reqwest::StatusCode::OK);
    Ok(())
}

#[tokio::test]
async fn mock_browser_two_human_flow_covers_http_ws_chat_audio_and_export() -> Result<()> {
    let server = spawn_server(config(AgentsMode::HumanVsHuman), ServeOptions::default()).await?;
    let client = reqwest::Client::new();
    let admin_cookie = admin_login(&client, &server.base_url).await?;
    let a = create_participant(&client, &server.base_url, "A").await?;
    let b = create_participant(&client, &server.base_url, "B").await?;
    consent(&client, &server.base_url, &a).await?;
    consent(&client, &server.base_url, &b).await?;

    let session = create_room(&client, &server.base_url, &a).await?;
    assert_eq!(session["role"], "A");
    assert!(session["available_actions"].is_null());
    let public_session_id = session["public_session_id"].as_str().unwrap().to_string();
    let joined = create_room(&client, &server.base_url, &b).await?;
    assert_eq!(joined["role"], "B");
    assert!(joined["available_actions"].is_null());

    let audio = client
        .post(format!(
            "{}/api/sessions/{public_session_id}/audio-session",
            server.base_url
        ))
        .bearer_auth(&a.credential)
        .json(&json!({}))
        .send()
        .await?
        .error_for_status()?
        .json::<Value>()
        .await?;
    assert_eq!(audio["enabled"], true);
    assert_eq!(audio["protocol_version"], 1);
    assert_eq!(audio["sample_rate_hz"], 24000);
    assert!(audio["token"]
        .as_str()
        .is_some_and(|value| !value.is_empty()));

    let mut socket_a = ws_connect(&client, &server, &public_session_id, &a).await?;
    let mut socket_b = ws_connect(&client, &server, &public_session_id, &b).await?;
    let assigned_a = read_ws_type(&mut socket_a, "session_started").await?;
    let assigned_b = read_ws_type(&mut socket_b, "session_started").await?;
    assert_eq!(assigned_a["role"], "A");
    assert_eq!(assigned_b["role"], "B");
    assert_eq!(assigned_a["available_actions"].as_array().unwrap().len(), 2);
    send_ws(&mut socket_a, json!({"type": "ready"})).await?;
    send_ws(&mut socket_b, json!({"type": "ready"})).await?;

    send_ws(
        &mut socket_a,
        json!({"type": "message", "text": "browser chat"}),
    )
    .await?;
    let chat = read_ws_type(&mut socket_b, "message").await?;
    assert_eq!(chat["message"]["input"], "text");
    assert_eq!(chat["message"]["text"], "browser chat");

    send_ws(
        &mut socket_a,
        json!({"type": "action", "action": {"type": "mark", "finish": true}}),
    )
    .await?;
    let completed_a = read_ws_type(&mut socket_a, "completed").await?;
    let completed_b = read_ws_type(&mut socket_b, "completed").await?;
    assert_eq!(completed_a["completion"]["done"], true);
    assert_eq!(completed_b["completion"]["done"], true);

    let mut export = Value::Null;
    for _ in 0..20 {
        export = admin_export(&client, &server.base_url, &admin_cookie).await?;
        if export["experiment"]["sessions"][0]["events"]
            .as_array()
            .is_some_and(|events| {
                ["message", "action"]
                    .iter()
                    .all(|expected| events.iter().any(|event| event["kind"] == *expected))
            })
            && export["experiment"]["sessions"][0]["completion"]["done"] == true
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let event_types = export["experiment"]["sessions"][0]["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| event["kind"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(event_types.contains(&"message"));
    assert!(event_types.contains(&"action"));
    Ok(())
}

#[tokio::test]
async fn mock_browser_human_vs_agent_flow_covers_agent_message_action_and_tts_diagnostics(
) -> Result<()> {
    let tts = Arc::new(RecordingTts {
        messages: Mutex::new(vec![]),
    });
    let factory = Arc::new(ScriptedAgentFactory::new(vec![
        Some(AgentResponse::action_and_message(
            DummyAction::Mark { finish: false },
            "agent says hello",
        )),
        Some(AgentResponse::action(DummyAction::Mark { finish: true })),
    ]));
    let server = spawn_server(
        config(AgentsMode::HumanVsAgent),
        ServeOptions {
            agent_factory: Some(factory),
            tts_provider: Some(tts.clone()),
            ..ServeOptions::default()
        },
    )
    .await?;
    let client = reqwest::Client::new();
    let admin_cookie = admin_login(&client, &server.base_url).await?;
    let human = create_participant(&client, &server.base_url, "Human").await?;
    consent(&client, &server.base_url, &human).await?;
    let session = create_room(&client, &server.base_url, &human).await?;
    assert_eq!(session["role"], "A");
    let public_session_id = session["public_session_id"].as_str().unwrap();

    let mut socket = ws_connect(&client, &server, public_session_id, &human).await?;
    let assigned = read_ws_type(&mut socket, "session_started").await?;
    assert_eq!(assigned["role"], "A");
    let message = read_ws_type(&mut socket, "message").await?;
    assert_eq!(message["message"]["sender"], "B");
    assert_eq!(message["message"]["input"], "text");
    assert_eq!(message["message"]["text"], "agent says hello");
    send_ws(&mut socket, json!({"type": "message", "text": "continue"})).await?;
    let completed = read_ws_type(&mut socket, "completed").await?;
    assert_eq!(completed["completion"]["done"], true);

    for _ in 0..20 {
        if tts
            .messages
            .lock()
            .unwrap()
            .iter()
            .any(|message| message == "agent says hello")
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let export = admin_export(&client, &server.base_url, &admin_cookie).await?;
    let events = export["experiment"]["sessions"][0]["events"]
        .as_array()
        .unwrap();
    assert!(events
        .iter()
        .any(|event| { event["kind"] == "message" && event["origin"] == "agent" }));
    assert!(events.iter().any(|event| event["kind"] == "action"));
    assert!(!export.to_string().contains("tts_diagnostic"));
    Ok(())
}

#[tokio::test]
async fn mock_browser_human_vs_remote_grpc_agent_flow_uses_normal_runtime_and_persistence(
) -> Result<()> {
    let remote = spawn_mock_remote_agent().await?;
    let factory = Arc::new(RemoteGrpcAgentFactory::<DummyAdapter>::new());
    let mut experiment_config = config(AgentsMode::HumanVsAgent);
    experiment_config
        .agents
        .human_vs_agent
        .as_mut()
        .unwrap()
        .factory = Some("remote_grpc".to_string());
    experiment_config
        .agents
        .human_vs_agent
        .as_mut()
        .unwrap()
        .config = json!({
        "endpoint": remote.endpoint,
        "agent_name": "mock-python-agent",
        "agent_version": "test-1",
        "protocol_version": "parlando-agent-v4"
    });
    let server = spawn_server(
        experiment_config,
        ServeOptions {
            agent_factory: Some(factory),
            ..ServeOptions::default()
        },
    )
    .await?;
    let client = reqwest::Client::new();
    let admin_cookie = admin_login(&client, &server.base_url).await?;
    let human = create_participant(&client, &server.base_url, "Human").await?;
    consent(&client, &server.base_url, &human).await?;
    let session = create_room(&client, &server.base_url, &human).await?;
    let public_session_id = session["public_session_id"].as_str().unwrap();

    let mut socket = ws_connect(&client, &server, public_session_id, &human).await?;
    let assigned = read_ws_type(&mut socket, "session_started").await?;
    assert_eq!(assigned["role"], "A");
    let message = read_ws_type(&mut socket, "message").await?;
    assert_eq!(message["message"]["sender"], "B");
    assert_eq!(message["message"]["input"], "text");
    assert_eq!(message["message"]["text"], "hello from remote grpc");
    send_ws(&mut socket, json!({"type": "message", "text": "continue"})).await?;
    let completed = read_ws_type(&mut socket, "completed").await?;
    assert_eq!(completed["completion"]["done"], true);

    for _ in 0..20 {
        if !remote.state.finish_requests.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    let create_requests = remote.state.create_requests.lock().unwrap().clone();
    assert_eq!(create_requests.len(), 1);
    assert_eq!(create_requests[0].protocol_version, "parlando-agent-v4");
    assert_eq!(create_requests[0].agent_name, "mock-python-agent");
    assert_eq!(create_requests[0].agent_version, "test-1");
    assert_eq!(create_requests[0].role, "B");

    let start_requests = remote.state.start_requests.lock().unwrap().clone();
    assert_eq!(start_requests.len(), 1);
    assert_eq!(start_requests[0].agent_id, "remote-agent-1");
    assert_eq!(
        start_requests[0]
            .observation
            .as_ref()
            .unwrap()
            .fields
            .get("role")
            .and_then(|value| value.kind.as_ref()),
        Some(&Kind::StringValue("B".to_string()))
    );
    let respond_requests = remote.state.respond_requests.lock().unwrap().clone();
    assert_eq!(respond_requests.len(), 2);
    assert_eq!(respond_requests[0].agent_id, "remote-agent-1");
    assert!(respond_requests[0].available_actions_provided);
    assert_eq!(respond_requests[0].available_actions.len(), 2);
    let finish_requests = remote.state.finish_requests.lock().unwrap().clone();
    assert_eq!(finish_requests.len(), 1);
    assert_eq!(finish_requests[0].agent_id, "remote-agent-1");
    assert_eq!(
        finish_requests[0]
            .completion
            .as_ref()
            .unwrap()
            .fields
            .get("done")
            .and_then(|value| value.kind.as_ref()),
        Some(&Kind::BoolValue(true))
    );

    let export = admin_export(&client, &server.base_url, &admin_cookie).await?;
    let events = export["experiment"]["sessions"][0]["events"]
        .as_array()
        .unwrap();
    let remote_participant = export["experiment"]["participants"]
        .as_array()
        .unwrap()
        .iter()
        .find(|participant| participant["kind"] == "agent")
        .expect("remote agent participant is exported");
    assert!(remote_participant["participant_id"]
        .as_str()
        .unwrap()
        .contains("mock-python-agent"));
    assert!(
        remote_participant["agent_identity"]["configuration_fingerprint"]
            .as_str()
            .unwrap()
            .starts_with("sha256:")
    );
    assert_eq!(
        remote_participant["agent_identity"]["agent_name"],
        "mock-python-agent"
    );
    assert_eq!(
        remote_participant["agent_identity"]["agent_version"],
        "test-1"
    );
    assert!(events
        .iter()
        .any(|event| { event["kind"] == "message" && event["origin"] == "agent" }));
    assert!(events.iter().any(|event| event["kind"] == "action"));
    assert_eq!(
        export["experiment"]["sessions"][0]["completion"]["done"],
        true
    );
    Ok(())
}
