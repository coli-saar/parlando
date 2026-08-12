use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use axum::Router;
use futures_util::{SinkExt, StreamExt};
use parlando_server::{
    agents::{AgentFactory, AgentInitContext, AgentResponse, GameAgent},
    build_router,
    config::{
        AgentsConfig, AgentsMode, ConversationConfig, DatabaseConfig, DirectConfig,
        ExperimentConfig, ExperimentIdentityConfig, HumanVsAgentConfig, LiveKitConfig,
        SpeechmaticsConfig, TranscriptionConfig,
    },
    game::{GameAdapter, PlayerRole},
    protocol::TranscriptSegmentIn,
    remote_agent::{
        pb::{
            agent_service_server::{AgentService, AgentServiceServer},
            AgentResponse as PbAgentResponse, CreateAgentRequest, CreateAgentResponse,
            DecisionRequest, MaybeActResponse, ObserveActionRequest, ObserveMessageRequest,
            ObserveResponse, ObserveStateRequest, ShutdownRequest, ShutdownResponse,
        },
        RemoteGrpcAgentConfig, RemoteGrpcAgentFactory,
    },
    tts::{AudioChunk, StreamingTtsProvider},
    ServeOptions,
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
struct DummyEvent {
    kind: String,
    actions: usize,
}

#[derive(Clone, Debug, Serialize)]
struct DummySummary {
    done: bool,
    actions: usize,
}

#[derive(Clone)]
struct DummyAdapter;

impl GameAdapter for DummyAdapter {
    type State = DummyState;
    type Action = DummyAction;
    type Observation = DummyObservation;
    type Event = DummyEvent;
    type Summary = DummySummary;

    fn initial_state(&self) -> Self::State {
        DummyState {
            actions: 0,
            done: false,
        }
    }

    fn validate_action(
        &self,
        state: &Self::State,
        _action: &Self::Action,
        _player: PlayerRole,
    ) -> Result<()> {
        if state.done {
            bail!("game already complete");
        }
        Ok(())
    }

    fn apply_action(&self, state: &Self::State, action: &Self::Action) -> Result<Self::State> {
        let DummyAction::Mark { finish } = action;
        Ok(DummyState {
            actions: state.actions + 1,
            done: *finish,
        })
    }

    fn observe_state(&self, state: &Self::State, player: PlayerRole) -> Self::Observation {
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

    fn events_for_action(
        &self,
        _before: &Self::State,
        after: &Self::State,
        _action: &Self::Action,
        _player: PlayerRole,
    ) -> Vec<Self::Event> {
        vec![DummyEvent {
            kind: "marked".to_string(),
            actions: after.actions,
        }]
    }

    fn is_complete(&self, state: &Self::State) -> bool {
        state.done
    }

    fn completion_summary(&self, state: &Self::State) -> Self::Summary {
        DummySummary {
            done: state.done,
            actions: state.actions,
        }
    }
}

struct ScriptedAgent {
    script: VecDeque<Option<AgentResponse<DummyAction>>>,
}

#[async_trait]
impl GameAgent<DummyAdapter> for ScriptedAgent {
    async fn observe_state(&mut self, observation: DummyObservation) -> Result<()> {
        assert_eq!(observation.role, "B");
        Ok(())
    }

    async fn maybe_act(
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

impl AgentFactory<DummyAdapter> for ScriptedAgentFactory {
    fn create(&self, context: AgentInitContext) -> Result<Box<dyn GameAgent<DummyAdapter> + Send>> {
        assert_eq!(context.role, "B");
        let script = self.scripts.lock().unwrap().pop_front().unwrap_or_default();
        Ok(Box::new(ScriptedAgent {
            script: script.into(),
        }))
    }
}

struct RecordingTts {
    messages: Mutex<Vec<String>>,
}

#[async_trait]
impl StreamingTtsProvider for RecordingTts {
    async fn synthesize(&self, text: &str, _message_id: &str) -> Result<Vec<AudioChunk>> {
        self.messages.lock().unwrap().push(text.to_string());
        Ok(vec![
            AudioChunk {
                data: vec![1, 2, 3, 4],
                sample_rate: 16000,
                channels: 1,
                final_chunk: false,
            },
            AudioChunk {
                data: vec![],
                sample_rate: 16000,
                channels: 1,
                final_chunk: true,
            },
        ])
    }
}

#[derive(Default)]
struct MockRemoteAgentState {
    create_requests: Mutex<Vec<CreateAgentRequest>>,
    observe_state_requests: Mutex<Vec<ObserveStateRequest>>,
    maybe_act_requests: Mutex<Vec<DecisionRequest>>,
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
        }))
    }

    async fn observe_state(
        &self,
        request: TonicRequest<ObserveStateRequest>,
    ) -> std::result::Result<TonicResponse<ObserveResponse>, Status> {
        self.state
            .observe_state_requests
            .lock()
            .unwrap()
            .push(request.into_inner());
        Ok(TonicResponse::new(ObserveResponse {}))
    }

    async fn observe_action(
        &self,
        _request: TonicRequest<ObserveActionRequest>,
    ) -> std::result::Result<TonicResponse<ObserveResponse>, Status> {
        Ok(TonicResponse::new(ObserveResponse {}))
    }

    async fn observe_message(
        &self,
        _request: TonicRequest<ObserveMessageRequest>,
    ) -> std::result::Result<TonicResponse<ObserveResponse>, Status> {
        Ok(TonicResponse::new(ObserveResponse {}))
    }

    async fn maybe_act(
        &self,
        request: TonicRequest<DecisionRequest>,
    ) -> std::result::Result<TonicResponse<MaybeActResponse>, Status> {
        let decision_count = {
            let mut requests = self.state.maybe_act_requests.lock().unwrap();
            requests.push(request.into_inner());
            requests.len()
        };
        Ok(TonicResponse::new(MaybeActResponse {
            response: Some(PbAgentResponse {
                message: (decision_count == 1).then(|| "hello from remote grpc".to_string()),
                action: Some(dummy_action_struct(decision_count > 1)),
            }),
        }))
    }

    async fn act(
        &self,
        _request: TonicRequest<DecisionRequest>,
    ) -> std::result::Result<TonicResponse<PbAgentResponse>, Status> {
        Ok(TonicResponse::new(PbAgentResponse {
            message: Some("hello from remote grpc".to_string()),
            action: Some(dummy_action_struct(true)),
        }))
    }

    async fn shutdown(
        &self,
        _request: TonicRequest<ShutdownRequest>,
    ) -> std::result::Result<TonicResponse<ShutdownResponse>, Status> {
        Ok(TonicResponse::new(ShutdownResponse {}))
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
    let router = build_router(DummyAdapter, config, options).await?;
    spawn_router(router, temp).await
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
    ExperimentConfig {
        experiment: ExperimentIdentityConfig {
            id: Some("mock-client-smoke".to_string()),
        },
        database: DatabaseConfig {
            url: "sqlite:///:memory:".to_string(),
        },
        direct: DirectConfig {
            require_consent: true,
            ..DirectConfig::default()
        },
        livekit: LiveKitConfig {
            enabled: true,
            url: "wss://livekit.example.test".to_string(),
            api_key: "api-key".to_string(),
            api_secret: "api-secret".to_string(),
        },
        speechmatics: SpeechmaticsConfig {
            enabled: false,
            ..SpeechmaticsConfig::default()
        },
        transcription: TranscriptionConfig {
            enabled: false,
            provider: "livekit".to_string(),
            ..TranscriptionConfig::default()
        },
        conversation: ConversationConfig {
            enabled: true,
            max_history_messages: 50,
        },
        agents: AgentsConfig {
            mode,
            human_vs_agent: Some(HumanVsAgentConfig {
                factory: Some("mock".to_string()),
                act_timeout_seconds: 2.0,
                invalid_action_limit: 2,
                ..HumanVsAgentConfig::default()
            }),
            ..AgentsConfig::default()
        },
        ..ExperimentConfig::default()
    }
}

async fn create_participant(
    client: &reqwest::Client,
    base_url: &str,
    name: &str,
) -> Result<String> {
    let response = client
        .post(format!("{base_url}/api/participants"))
        .json(&json!({"source": "direct", "display_name": name}))
        .send()
        .await?
        .error_for_status()?
        .json::<Value>()
        .await?;
    Ok(response["participant_session_id"]
        .as_str()
        .ok_or_else(|| anyhow!("missing participant_session_id"))?
        .to_string())
}

async fn consent(client: &reqwest::Client, base_url: &str, participant: &str) -> Result<()> {
    client
        .post(format!("{base_url}/api/consent"))
        .json(&json!({
            "participant_session_id": participant,
            "decisions": {"study": true}
        }))
        .send()
        .await?
        .error_for_status()?;
    Ok(())
}

async fn create_room(client: &reqwest::Client, base_url: &str, participant: &str) -> Result<Value> {
    Ok(client
        .post(format!("{base_url}/api/rooms"))
        .json(&json!({"participant_session_id": participant, "mode": "direct"}))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}

async fn join_room(
    client: &reqwest::Client,
    base_url: &str,
    room_id: &str,
    participant: &str,
) -> Result<Value> {
    Ok(client
        .post(format!("{base_url}/api/rooms/{room_id}/join"))
        .json(&json!({"participant_session_id": participant}))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}

async fn ws_connect(ws_base_url: &str, room_id: &str, participant: &str) -> Result<TestSocket> {
    let (socket, _) = connect_async(format!(
        "{ws_base_url}/ws/game/{room_id}?participantSessionId={participant}"
    ))
    .await?;
    Ok(socket)
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
async fn mock_browser_two_human_flow_covers_http_ws_chat_transcript_audio_and_export() -> Result<()>
{
    let server = spawn_server(config(AgentsMode::HumanVsHuman), ServeOptions::default()).await?;
    let client = reqwest::Client::new();
    let a = create_participant(&client, &server.base_url, "A").await?;
    let b = create_participant(&client, &server.base_url, "B").await?;
    consent(&client, &server.base_url, &a).await?;
    consent(&client, &server.base_url, &b).await?;

    let room = create_room(&client, &server.base_url, &a).await?;
    assert_eq!(room["role"], "A");
    assert!(room["available_actions"].is_null());
    let room_id = room["room_id"].as_str().unwrap().to_string();
    let joined = join_room(&client, &server.base_url, &room_id, &b).await?;
    assert_eq!(joined["role"], "B");
    assert!(joined["available_actions"].is_null());

    let audio = client
        .post(format!(
            "{}/api/rooms/{room_id}/audio-session",
            server.base_url
        ))
        .json(&json!({"participant_session_id": a}))
        .send()
        .await?
        .error_for_status()?
        .json::<Value>()
        .await?;
    assert_eq!(audio["enabled"], true);
    assert_eq!(audio["sinks"][0]["id"], "livekit-combined");

    let mut socket_a = ws_connect(&server.ws_base_url, &room_id, &a).await?;
    let mut socket_b = ws_connect(&server.ws_base_url, &room_id, &b).await?;
    let assigned_a = read_ws_type(&mut socket_a, "roleAssigned").await?;
    let assigned_b = read_ws_type(&mut socket_b, "roleAssigned").await?;
    assert_eq!(assigned_a["role"], "A");
    assert_eq!(assigned_b["role"], "B");
    assert_eq!(assigned_a["available_actions"].as_array().unwrap().len(), 2);
    send_ws(&mut socket_a, json!({"type": "ready"})).await?;
    send_ws(&mut socket_b, json!({"type": "ready"})).await?;

    send_ws(
        &mut socket_a,
        json!({"type": "sendChatMessage", "text": "browser chat"}),
    )
    .await?;
    let chat = read_ws_type(&mut socket_b, "conversationMessageAdded").await?;
    assert_eq!(chat["conversation_message"]["origin"], "typed");
    assert_eq!(chat["conversation_message"]["text"], "browser chat");

    let transcript = client
        .post(format!(
            "{}/api/rooms/{room_id}/transcripts",
            server.base_url
        ))
        .json(&TranscriptSegmentIn {
            participant_session_id: a.clone(),
            player: "B".to_string(),
            start_time_ms: 10,
            end_time_ms: 40,
            text: "voice transcript".to_string(),
            metadata: json!({"source": "mock-browser"}),
        })
        .send()
        .await?
        .error_for_status()?
        .json::<Value>()
        .await?;
    assert_eq!(transcript["player"], "A");
    let voice_message = read_ws_type(&mut socket_b, "conversationMessageAdded").await?;
    assert_eq!(
        voice_message["conversation_message"]["origin"],
        "voice_transcript"
    );

    send_ws(
        &mut socket_a,
        json!({"type": "submitAction", "action": {"type": "mark", "finish": true}}),
    )
    .await?;
    let completed_a = read_ws_type(&mut socket_a, "completed").await?;
    let completed_b = read_ws_type(&mut socket_b, "completed").await?;
    assert_eq!(completed_a["summary"]["done"], true);
    assert_eq!(completed_b["summary"]["done"], true);

    let export = client
        .get(format!("{}/api/admin/export", server.base_url))
        .send()
        .await?
        .error_for_status()?
        .json::<Value>()
        .await?;
    let event_types = export["session_events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| event["event_type"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(event_types.contains(&"conversation_message"));
    assert!(event_types.contains(&"transcript_segment"));
    assert!(event_types.contains(&"game_action_accepted"));
    assert!(event_types.contains(&"session_completed"));
    Ok(())
}

#[tokio::test]
async fn mock_browser_human_vs_agent_flow_covers_agent_message_action_and_tts_diagnostics(
) -> Result<()> {
    let tts = Arc::new(RecordingTts {
        messages: Mutex::new(vec![]),
    });
    let factory = Arc::new(ScriptedAgentFactory::new(vec![
        Some(AgentResponse {
            message: Some("agent says hello".to_string()),
            action: Some(DummyAction::Mark { finish: false }),
        }),
        Some(AgentResponse {
            message: None,
            action: Some(DummyAction::Mark { finish: true }),
        }),
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
    let human = create_participant(&client, &server.base_url, "Human").await?;
    consent(&client, &server.base_url, &human).await?;
    let room = create_room(&client, &server.base_url, &human).await?;
    assert_eq!(room["role"], "A");
    let room_id = room["room_id"].as_str().unwrap();

    let mut socket = ws_connect(&server.ws_base_url, room_id, &human).await?;
    let assigned = read_ws_type(&mut socket, "roleAssigned").await?;
    assert_eq!(assigned["role"], "A");
    let message = read_ws_type(&mut socket, "conversationMessageAdded").await?;
    assert_eq!(message["conversation_message"]["origin"], "agent");
    assert_eq!(message["conversation_message"]["text"], "agent says hello");
    let completed = read_ws_type(&mut socket, "completed").await?;
    assert_eq!(completed["summary"]["done"], true);

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

    let export = client
        .get(format!("{}/api/admin/export", server.base_url))
        .send()
        .await?
        .error_for_status()?
        .json::<Value>()
        .await?;
    let events = export["session_events"].as_array().unwrap();
    assert!(events.iter().any(|event| {
        event["event_type"] == "conversation_message" && event["payload"]["origin"] == "agent"
    }));
    assert!(events
        .iter()
        .any(|event| event["event_type"] == "agent_action"));
    assert!(events
        .iter()
        .any(|event| event["event_type"] == "tts_diagnostic"));
    Ok(())
}

#[tokio::test]
async fn mock_browser_human_vs_remote_grpc_agent_flow_uses_normal_runtime_and_persistence(
) -> Result<()> {
    let remote = spawn_mock_remote_agent().await?;
    let mut remote_config = RemoteGrpcAgentConfig::new(&remote.endpoint, "mock-python-agent");
    remote_config.agent_version = Some("test-1".to_string());
    remote_config.request_timeout = Duration::from_secs(2);
    let factory = Arc::new(RemoteGrpcAgentFactory::<DummyAdapter>::new(remote_config));
    let server = spawn_server(
        config(AgentsMode::HumanVsAgent),
        ServeOptions {
            agent_factory: Some(factory),
            ..ServeOptions::default()
        },
    )
    .await?;
    let client = reqwest::Client::new();
    let human = create_participant(&client, &server.base_url, "Human").await?;
    consent(&client, &server.base_url, &human).await?;
    let room = create_room(&client, &server.base_url, &human).await?;
    let room_id = room["room_id"].as_str().unwrap();

    let mut socket = ws_connect(&server.ws_base_url, room_id, &human).await?;
    let assigned = read_ws_type(&mut socket, "roleAssigned").await?;
    assert_eq!(assigned["role"], "A");
    let message = read_ws_type(&mut socket, "conversationMessageAdded").await?;
    assert_eq!(message["conversation_message"]["origin"], "agent");
    assert_eq!(
        message["conversation_message"]["text"],
        "hello from remote grpc"
    );
    let completed = read_ws_type(&mut socket, "completed").await?;
    assert_eq!(completed["summary"]["done"], true);

    let create_requests = remote.state.create_requests.lock().unwrap().clone();
    assert_eq!(create_requests.len(), 1);
    assert_eq!(create_requests[0].protocol_version, "parlando-agent-v2");
    assert_eq!(create_requests[0].agent_name, "mock-python-agent");
    assert_eq!(create_requests[0].agent_version, "test-1");
    assert_eq!(create_requests[0].role, "B");

    let observe_state_requests = remote.state.observe_state_requests.lock().unwrap().clone();
    assert_eq!(observe_state_requests.len(), 1);
    assert_eq!(observe_state_requests[0].agent_id, "remote-agent-1");
    assert_eq!(
        observe_state_requests[0]
            .current_observation
            .as_ref()
            .unwrap()
            .fields
            .get("role")
            .and_then(|value| value.kind.as_ref()),
        Some(&Kind::StringValue("B".to_string()))
    );
    let maybe_act_requests = remote.state.maybe_act_requests.lock().unwrap().clone();
    assert_eq!(maybe_act_requests.len(), 2);
    assert_eq!(maybe_act_requests[0].agent_id, "remote-agent-1");
    assert!(maybe_act_requests[0].available_actions_provided);
    assert_eq!(maybe_act_requests[0].available_actions.len(), 2);

    let export = client
        .get(format!("{}/api/admin/export", server.base_url))
        .send()
        .await?
        .error_for_status()?
        .json::<Value>()
        .await?;
    let events = export["session_events"].as_array().unwrap();
    let remote_participant = export["participants"]
        .as_array()
        .unwrap()
        .iter()
        .find(|participant| participant["participant_kind"] == "agent")
        .expect("remote agent participant is exported");
    assert_eq!(remote_participant["identity_provider"], "remote_grpc");
    assert_eq!(
        remote_participant["external_id"],
        "mock-python-agent@test-1"
    );
    assert_eq!(
        remote_participant["metadata"]["protocol_version"],
        "parlando-agent-v2"
    );
    assert_eq!(remote_participant["metadata"]["agent_type"], "remote_grpc");
    assert_eq!(remote_participant["metadata"]["agent_version"], "test-1");
    assert!(events.iter().any(|event| {
        event["event_type"] == "conversation_message" && event["payload"]["origin"] == "agent"
    }));
    assert!(events
        .iter()
        .any(|event| event["event_type"] == "agent_action"));
    assert!(events
        .iter()
        .any(|event| event["event_type"] == "game_action_accepted"));
    assert!(events
        .iter()
        .any(|event| event["event_type"] == "session_completed"));
    Ok(())
}
