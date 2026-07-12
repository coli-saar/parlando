use std::{
    collections::{HashMap, HashSet},
    future::Future,
    net::SocketAddr,
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use anyhow::{anyhow, Result};
use axum::{
    extract::{
        ws::{Message, WebSocket},
        Path, Query, State, WebSocketUpgrade,
    },
    http::StatusCode,
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use futures_util::{SinkExt, StreamExt as FuturesStreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::{broadcast, RwLock};
use tower_http::{
    cors::CorsLayer,
    services::{ServeDir, ServeFile},
};

use crate::{
    agents::{AgentFactory, AgentInitContext, AgentResult, SharedAgentFactory},
    audio_publisher::AgentAudioPublisher,
    audio_session::{AudioSessionContext, DefaultAudioSessionPlanner, TranscriptionReadiness},
    config::{AgentsMode, ExperimentConfig},
    game::{GameAdapter, PlayerRole, Seat},
    identity::{new_id, room_code},
    livekit::{create_livekit_token, livekit_identity},
    protocol::*,
    storage::{
        experiment_store_from_url, generated_experiment_id, now_iso, ConsentDeclarationRecord,
        ExperimentRecord, GameRoom, MemoryState, ParticipantRecord, RoomParticipant,
        SessionEventRecord, SessionParticipantRecord, SessionRecord, SharedExperimentStore,
        TranscriptSegment,
    },
    tts::{ElevenLabsStreamingTtsProvider, StreamingTtsProvider},
};

/// Optional runtime components supplied by the game-specific binary.
#[derive(Clone)]
pub struct ServeOptions<A: GameAdapter> {
    /// Factory used to create one fresh agent per agent participant.
    pub agent_factory: Option<Arc<dyn AgentFactory<A>>>,
    /// Streaming TTS provider used for agent-origin conversation messages.
    pub tts_provider: Option<Arc<dyn StreamingTtsProvider>>,
    /// Optional publisher used to send synthesized agent audio into RTC.
    pub audio_publisher: Option<Arc<dyn AgentAudioPublisher>>,
    /// Game/client version metadata supplied by the game-specific binary.
    pub game_version_manifest: Option<Value>,
}

impl<A: GameAdapter> Default for ServeOptions<A> {
    /// Creates serve options with no agent factory configured.
    fn default() -> Self {
        Self {
            agent_factory: None,
            tts_provider: None,
            audio_publisher: None,
            game_version_manifest: None,
        }
    }
}

/// Shared server state used by HTTP handlers, WebSocket tasks, and background agents.
pub struct AppState<A: GameAdapter> {
    pub adapter: A,
    pub config: ExperimentConfig,
    pub experiment_id: String,
    pub memory: RwLock<MemoryState<A::State>>,
    pub store: SharedExperimentStore,
    pub room_buses: RwLock<HashMap<String, broadcast::Sender<ServerMessage>>>,
    pub agent_factory: Option<SharedAgentFactory<A>>,
    pub started_agents: RwLock<HashSet<String>>,
    pub tts_provider: Option<Arc<dyn StreamingTtsProvider>>,
    pub audio_publisher: Option<Arc<dyn AgentAudioPublisher>>,
    pub version_manifest: Value,
}

impl<A: GameAdapter> AppState<A> {
    async fn room_bus(&self, room_id: &str) -> broadcast::Sender<ServerMessage> {
        let mut buses = self.room_buses.write().await;
        buses
            .entry(room_id.to_string())
            .or_insert_with(|| broadcast::channel(256).0)
            .clone()
    }
}

/// Logs storage failures while preserving the current active-session request behavior.
async fn persist_event<F, T>(event_name: &'static str, record: F)
where
    F: Future<Output = Result<T>>,
{
    if let Err(error) = record.await {
        tracing::warn!(%error, event_name, "failed to persist event");
    }
}

/// Appends a session event after resolving the session and optional actor from the active cache.
async fn persist_session_event<A: GameAdapter>(
    state: &Arc<AppState<A>>,
    room_id: &str,
    participant_session_id: Option<&str>,
    event_type: &'static str,
    payload: Value,
    game_state: Option<Value>,
) {
    let resolved = {
        let memory = state.memory.read().await;
        let Some(room) = memory.rooms.get(room_id) else {
            return;
        };
        let actor = participant_session_id.and_then(|id| {
            room.participants.get(id).map(|participant| {
                (
                    participant.participant_id,
                    participant.role.as_str().to_string(),
                )
            })
        });
        Some((room.experiment_id.clone(), room.session_id, actor))
    };
    let Some((experiment_id, session_id, actor)) = resolved else {
        return;
    };
    if session_id <= 0 {
        return;
    }
    let (actor_participant_id, actor_role) = actor
        .map(|(participant_id, role)| (Some(participant_id), Some(role)))
        .unwrap_or((None, None));
    persist_event(
        event_type,
        state.store.append_session_event(SessionEventRecord {
            experiment_id,
            session_id,
            event_type: event_type.to_string(),
            actor_participant_id,
            actor_role,
            payload,
            game_state,
        }),
    )
    .await;
}

/// Creates the durable session row and stores its integer id in the active room cache.
async fn persist_created_session<A: GameAdapter>(
    state: &Arc<AppState<A>>,
    room_id: &str,
) -> Result<i64>
where
    A::State: Serialize,
{
    let (experiment_id, mode, status) = {
        let memory = state.memory.read().await;
        let room = memory
            .rooms
            .get(room_id)
            .ok_or_else(|| anyhow!("Room not found."))?;
        (
            room.experiment_id.clone(),
            room.mode.clone(),
            room.status.clone(),
        )
    };
    let session_id = state
        .store
        .create_session(SessionRecord {
            experiment_id,
            room_id: room_id.to_string(),
            mode,
            status,
        })
        .await?;
    {
        let mut memory = state.memory.write().await;
        if let Some(room) = memory.rooms.get_mut(room_id) {
            room.session_id = session_id;
        }
    }
    persist_session_event(
        state,
        room_id,
        None,
        "session_created",
        json!({"room_id": room_id}),
        None,
    )
    .await;
    Ok(session_id)
}

/// Persists one participant's session-local role and connection status.
async fn persist_session_participant<A: GameAdapter>(
    state: &Arc<AppState<A>>,
    room_id: &str,
    participant_session_id: &str,
) -> Result<()> {
    let record = {
        let memory = state.memory.read().await;
        let room = memory
            .rooms
            .get(room_id)
            .ok_or_else(|| anyhow!("Room not found."))?;
        let participant = room
            .participants
            .get(participant_session_id)
            .ok_or_else(|| anyhow!("Participant is not in room."))?;
        SessionParticipantRecord {
            experiment_id: room.experiment_id.clone(),
            session_id: room.session_id,
            participant_id: participant.participant_id,
            participant_session_id: participant_session_id.to_string(),
            role: participant.role.as_str().to_string(),
            connection_status: if participant.connected {
                "connected".to_string()
            } else {
                "joined".to_string()
            },
        }
    };
    state.store.add_session_participant(record).await
}

#[derive(Debug)]
pub struct AppError {
    status: StatusCode,
    message: String,
}

impl AppError {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, message)
    }

    fn forbidden(message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, message)
    }

    fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }
}

impl From<anyhow::Error> for AppError {
    fn from(value: anyhow::Error) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, value.to_string())
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (self.status, self.message).into_response()
    }
}

/// Builds and runs a Parlando HTTP/WebSocket server on the provided socket address.
pub async fn serve<A: GameAdapter>(
    adapter: A,
    config: ExperimentConfig,
    bind_addr: SocketAddr,
    options: ServeOptions<A>,
) -> Result<()> {
    let router = build_router(adapter, config, options).await?;
    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    axum::serve(listener, router).await?;
    Ok(())
}

/// Builds an Axum router for tests or for embedding in a custom server runner.
pub async fn build_router<A: GameAdapter>(
    adapter: A,
    config: ExperimentConfig,
    options: ServeOptions<A>,
) -> Result<Router>
where
    A::State: Serialize,
{
    let store = experiment_store_from_url(&config.database.url).await?;
    let experiment_id = config
        .experiment
        .id
        .clone()
        .unwrap_or_else(generated_experiment_id);
    let version_manifest = version_manifest(options.game_version_manifest.clone());
    store
        .ensure_experiment(ExperimentRecord {
            experiment_id: experiment_id.clone(),
            config: serde_json::to_value(&config)?,
            server_version: Some(env!("CARGO_PKG_VERSION").to_string()),
            version_manifest: Some(version_manifest.clone()),
            status: "active".to_string(),
            notes: None,
        })
        .await?;
    let client_dist = config.server.client_dist_path.as_ref().map(PathBuf::from);
    let tts_provider = if options.tts_provider.is_some() {
        options.tts_provider
    } else if config.tts.enabled {
        Some(
            Arc::new(ElevenLabsStreamingTtsProvider::new(config.tts.clone())?)
                as Arc<dyn StreamingTtsProvider>,
        )
    } else {
        None
    };
    let state = Arc::new(AppState {
        adapter,
        config,
        experiment_id,
        memory: RwLock::new(MemoryState::default()),
        store,
        room_buses: RwLock::new(HashMap::new()),
        agent_factory: options.agent_factory,
        started_agents: RwLock::new(HashSet::new()),
        tts_provider,
        audio_publisher: options.audio_publisher,
        version_manifest,
    });

    let api = Router::new()
        .route("/health", get(health))
        .route("/api/config", get(public_config::<A>))
        .route("/api/participants", post(create_participant::<A>))
        .route("/api/direct/start", post(direct_start::<A>))
        .route("/api/direct/enter", post(direct_enter::<A>))
        .route(
            "/api/direct/wait/:participant_session_id",
            get(direct_wait::<A>),
        )
        .route("/api/consent", post(consent::<A>))
        .route("/api/rooms", post(create_room::<A>))
        .route("/api/rooms/:room_id/join", post(join_room::<A>))
        .route("/api/matchmaking/join", post(join_matchmaking::<A>))
        .route(
            "/api/matchmaking/status/:participant_session_id",
            get(matchmaking_status::<A>),
        )
        .route(
            "/api/rooms/:room_id/livekit-token",
            post(livekit_token::<A>),
        )
        .route(
            "/api/rooms/:room_id/livekit-worker-token",
            post(livekit_worker_token::<A>),
        )
        .route(
            "/api/rooms/:room_id/audio-session",
            post(audio_session::<A>),
        )
        .route("/api/rooms/:room_id/transcripts", post(add_transcript::<A>))
        .route(
            "/api/rooms/:room_id/voice-diagnostics",
            post(add_voice_diagnostic::<A>),
        )
        .route("/admin/games", get(admin_games_page))
        .route("/admin/experiments", get(admin_games_page))
        .route("/api/admin/experiments", get(admin_experiments::<A>))
        .route("/api/admin/experiments", post(admin_create_experiment::<A>))
        .route(
            "/api/admin/experiments/:experiment_id/status",
            post(admin_update_experiment_status::<A>),
        )
        .route("/api/admin/games", get(admin_recent_games::<A>))
        .route("/api/admin/games/:session_id", get(admin_game_detail::<A>))
        .route(
            "/api/admin/games/:session_id/events",
            get(admin_game_events::<A>),
        )
        .route("/api/admin/sessions", get(admin_recent_games::<A>))
        .route(
            "/api/admin/sessions/:session_id",
            get(admin_game_detail::<A>),
        )
        .route(
            "/api/admin/sessions/:session_id/events",
            get(admin_game_events::<A>),
        )
        .route("/api/admin/export", get(admin_export::<A>))
        .route("/ws/game/:room_id", get(game_socket::<A>))
        .layer(CorsLayer::permissive())
        .with_state(state.clone());

    if let Some(dist) = client_dist.filter(|path| path.join("index.html").is_file()) {
        let index = dist.join("index.html");
        Ok(api
            .nest_service("/assets", ServeDir::new(dist.join("assets")))
            .fallback_service(ServeDir::new(dist).fallback(ServeFile::new(index))))
    } else {
        Ok(api)
    }
}

async fn health() -> Json<Value> {
    Json(json!({"status": "ok"}))
}

async fn public_config<A: GameAdapter>(
    State(state): State<Arc<AppState<A>>>,
) -> Json<PublicConfigResponse> {
    let config = &state.config;
    Json(PublicConfigResponse {
        study_name: config.study.name.clone(),
        require_consent: config.direct.require_consent,
        consents: config
            .direct
            .consents
            .iter()
            .map(|item| ConsentItemResponse {
                id: item.id.clone(),
                title: item.title.clone(),
                body_html: item.body_html.clone(),
                required: item.required,
            })
            .collect(),
        livekit: json!({"enabled": config.livekit.enabled, "url": if config.livekit.enabled { Some(config.livekit.url.clone()) } else { None }}),
        transcription: json!({
            "enabled": config.transcription.enabled,
            "provider": config.transcription.provider,
            "model": config.transcription.model,
            "language": config.transcription.language,
            "worker_autostart": config.transcription.worker_autostart,
            "store_audio": config.transcription.store_audio,
        }),
        tts: json!({
            "enabled": config.tts.enabled,
            "provider": config.tts.provider,
            "model": config.tts.model,
            "voice_name": if config.tts.voice_name.is_empty() { None } else { Some(config.tts.voice_name.clone()) },
            "worker_autostart": config.tts.worker_autostart,
        }),
        conversation: json!({
            "enabled": config.conversation.enabled,
            "max_history_messages": config.conversation.max_history_messages,
        }),
    })
}

async fn direct_start<A: GameAdapter>(
    State(state): State<Arc<AppState<A>>>,
    Json(request): Json<DirectStartRequest>,
) -> Result<Json<DirectStartResponse>, AppError> {
    let request = ParticipantCreateRequest {
        source: "direct".to_string(),
        display_name: request.display_name,
        study_id: request.study_id,
        external_id: request.external_id,
        metadata: request.metadata,
    };
    create_participant_inner(state, request).await.map(Json)
}

async fn create_participant<A: GameAdapter>(
    State(state): State<Arc<AppState<A>>>,
    Json(request): Json<ParticipantCreateRequest>,
) -> Result<Json<ParticipantCreateResponse>, AppError> {
    create_participant_inner(state, request).await.map(Json)
}

async fn create_participant_inner<A: GameAdapter>(
    state: Arc<AppState<A>>,
    request: ParticipantCreateRequest,
) -> Result<ParticipantCreateResponse, AppError> {
    if request.source == "direct" && !state.config.direct.enabled {
        return Err(AppError::not_found("Direct mode is disabled."));
    }
    let participant_kind = if request.source == "agent" {
        "agent"
    } else if request.source == "worker" {
        "worker"
    } else {
        "human"
    };
    let identity_provider = request.source.clone();
    let participant_id = state
        .store
        .upsert_participant(ParticipantRecord {
            participant_kind: participant_kind.to_string(),
            identity_provider,
            external_id: request.external_id,
            display_name: request.display_name.clone(),
            metadata: request.metadata,
        })
        .await?;
    let participant = {
        let mut memory = state.memory.write().await;
        memory.create_participant(
            participant_id,
            request.source,
            request.display_name,
            Some(
                request
                    .study_id
                    .unwrap_or_else(|| state.config.study.name.clone()),
            ),
        )
    };
    Ok(ParticipantCreateResponse {
        participant_session_id: participant.id,
        source: participant.source,
        display_name: participant.display_name,
    })
}

async fn direct_enter<A: GameAdapter>(
    State(state): State<Arc<AppState<A>>>,
    Json(request): Json<DirectEnterRequest>,
) -> Result<Json<DirectEnterResponse>, AppError>
where
    A::State: Serialize,
{
    let participant = create_participant_inner(
        state.clone(),
        ParticipantCreateRequest {
            source: "direct".to_string(),
            display_name: request.display_name,
            study_id: request.study_id.clone(),
            external_id: request.external_id,
            metadata: request.metadata,
        },
    )
    .await?;
    enter_matchmaking(state, participant.participant_session_id, request.study_id)
        .await
        .map(Json)
}

async fn direct_wait<A: GameAdapter>(
    State(state): State<Arc<AppState<A>>>,
    Path(participant_session_id): Path<String>,
) -> Result<Json<DirectEnterResponse>, AppError>
where
    A::State: Serialize,
{
    matchmaking_status_inner(state, &participant_session_id)
        .await
        .map(Json)
}

async fn consent<A: GameAdapter>(
    State(state): State<Arc<AppState<A>>>,
    Json(request): Json<ConsentRequest>,
) -> Result<Json<Value>, AppError> {
    let mut memory = state.memory.write().await;
    let participant = memory
        .participants
        .get_mut(&request.participant_session_id)
        .ok_or_else(|| AppError::not_found("Participant session not found."))?;
    let participant_id = participant.participant_id;
    participant
        .consent_decisions
        .extend(request.decisions.clone());
    participant.updated_at = now_iso();
    for room in memory.rooms.values_mut() {
        if request
            .room_id
            .as_ref()
            .is_some_and(|room_id| room_id != &room.id)
        {
            continue;
        }
        if let Some(room_participant) = room.participants.get_mut(&request.participant_session_id) {
            room_participant
                .consent_decisions
                .extend(request.decisions.clone());
            room_participant.updated_at = now_iso();
        }
    }
    drop(memory);
    let session_id = if let Some(room_id) = request.room_id.as_ref() {
        state
            .memory
            .read()
            .await
            .rooms
            .get(room_id)
            .map(|room| room.session_id)
    } else {
        None
    };
    for (consent_item_id, accepted) in request.decisions {
        persist_event(
            "consent_declaration",
            state
                .store
                .record_consent_declaration(ConsentDeclarationRecord {
                    experiment_id: state.experiment_id.clone(),
                    session_id,
                    participant_id,
                    consent_item_id,
                    accepted,
                    consent_text_hash: None,
                    metadata: Value::Null,
                }),
        )
        .await;
    }
    Ok(Json(json!({"ok": true})))
}

async fn create_room<A: GameAdapter>(
    State(state): State<Arc<AppState<A>>>,
    Json(request): Json<CreateRoomRequest>,
) -> Result<Json<CreateRoomResponse>, AppError>
where
    A::State: Serialize,
{
    require_consent(&state, &request.participant_session_id).await?;
    let (room_id, role) = {
        let mut memory = state.memory.write().await;
        create_room_locked(
            &state,
            &mut memory,
            request.participant_session_id.clone(),
            request.mode,
            Seat::A,
            Some(state.config.study.name.clone()),
        )
    }?;
    persist_created_session(&state, &room_id).await?;
    persist_session_participant(&state, &room_id, &request.participant_session_id).await?;
    persist_session_event(
        &state,
        &room_id,
        Some(&request.participant_session_id),
        "participant_joined",
        json!({"role": role.as_str()}),
        None,
    )
    .await;
    let response = room_response(
        &state,
        &room_id,
        &request.participant_session_id,
        role,
        vec![],
    )
    .await?;
    Ok(Json(response))
}

async fn join_room<A: GameAdapter>(
    State(state): State<Arc<AppState<A>>>,
    Path(room_id): Path<String>,
    Json(request): Json<JoinRoomRequest>,
) -> Result<Json<JoinRoomResponse>, AppError>
where
    A::State: Serialize,
{
    require_consent(&state, &request.participant_session_id).await?;
    let (role, newly_joined) = {
        let mut memory = state.memory.write().await;
        let existing_role = memory
            .rooms
            .get(&room_id)
            .ok_or_else(|| AppError::not_found("Room not found."))?
            .participants
            .get(&request.participant_session_id)
            .map(|existing| existing.role);
        if let Some(existing_role) = existing_role {
            (existing_role, false)
        } else {
            let consent_decisions = memory
                .participants
                .get(&request.participant_session_id)
                .ok_or_else(|| AppError::not_found("Participant session not found."))?
                .consent_decisions
                .clone();
            let participant_id = memory
                .participants
                .get(&request.participant_session_id)
                .ok_or_else(|| AppError::not_found("Participant session not found."))?
                .participant_id;
            let room = memory
                .rooms
                .get_mut(&room_id)
                .ok_or_else(|| AppError::not_found("Room not found."))?;
            let role = next_role(room)
                .ok_or_else(|| AppError::forbidden("Room already has two players."))?;
            room.participants.insert(
                request.participant_session_id.clone(),
                RoomParticipant {
                    participant_session_id: request.participant_session_id.clone(),
                    participant_id,
                    source: "direct".to_string(),
                    role,
                    connected: false,
                    audio_ready: !speechmatics_readiness_required(&state.config),
                    consent_decisions,
                    joined_at: now_iso(),
                    updated_at: now_iso(),
                },
            );
            (role, true)
        }
    };
    if newly_joined {
        persist_session_participant(&state, &room_id, &request.participant_session_id).await?;
        persist_session_event(
            &state,
            &room_id,
            Some(&request.participant_session_id),
            "participant_joined",
            json!({"role": role.as_str()}),
            None,
        )
        .await;
    }
    Ok(Json(
        room_response(
            &state,
            &room_id,
            &request.participant_session_id,
            role,
            vec![],
        )
        .await?,
    ))
}

async fn join_matchmaking<A: GameAdapter>(
    State(state): State<Arc<AppState<A>>>,
    Json(request): Json<MatchmakingJoinRequest>,
) -> Result<Json<MatchmakingJoinResponse>, AppError>
where
    A::State: Serialize,
{
    enter_matchmaking(
        state,
        request.participant_session_id,
        request.queue.or(request.study_id),
    )
    .await
    .map(Json)
}

async fn matchmaking_status<A: GameAdapter>(
    State(state): State<Arc<AppState<A>>>,
    Path(participant_session_id): Path<String>,
) -> Result<Json<MatchmakingJoinResponse>, AppError>
where
    A::State: Serialize,
{
    matchmaking_status_inner(state, &participant_session_id)
        .await
        .map(Json)
}

async fn enter_matchmaking<A: GameAdapter>(
    state: Arc<AppState<A>>,
    participant_session_id: String,
    queue: Option<String>,
) -> Result<MatchmakingJoinResponse, AppError>
where
    A::State: Serialize,
{
    require_consent(&state, &participant_session_id).await?;
    let matched = matchmaking_status_inner(state.clone(), &participant_session_id).await?;
    if matched.status == "matched" {
        return Ok(matched);
    }

    if state.config.agents.mode == AgentsMode::HumanVsAgent {
        let factory_identity = state
            .agent_factory
            .as_ref()
            .map(|factory| factory.participant_identity())
            .unwrap_or_default();
        let configured_agent = state.config.agents.human_vs_agent.as_ref();
        let external_id = factory_identity.external_id.clone().or_else(|| {
            configured_agent
                .and_then(|config| config.factory.clone())
                .or_else(|| Some("agent".to_string()))
        });
        let metadata = if factory_identity.metadata.is_null() {
            configured_agent
                .map(|config| config.config.clone())
                .unwrap_or(Value::Null)
        } else {
            factory_identity.metadata.clone()
        };
        let agent_metadata = metadata.clone();
        let agent_participant_db_id = state
            .store
            .upsert_participant(ParticipantRecord {
                participant_kind: "agent".to_string(),
                identity_provider: factory_identity.identity_provider,
                external_id,
                display_name: Some("Agent".to_string()),
                metadata,
            })
            .await?;
        let (room_id, role, agent_participant_id) = {
            let mut memory = state.memory.write().await;
            let (room_id, role) = create_room_locked(
                &state,
                &mut memory,
                participant_session_id.clone(),
                "direct".to_string(),
                Seat::A,
                Some(queue.unwrap_or_else(|| state.config.study.name.clone())),
            )?;
            let agent = memory.create_participant(
                agent_participant_db_id,
                "agent".to_string(),
                Some("Agent".to_string()),
                Some(state.config.study.name.clone()),
            );
            let room = memory.rooms.get_mut(&room_id).expect("room exists");
            room.participants.insert(
                agent.id.clone(),
                RoomParticipant {
                    participant_session_id: agent.id.clone(),
                    participant_id: agent.participant_id,
                    source: "agent".to_string(),
                    role: Seat::B,
                    connected: true,
                    audio_ready: true,
                    consent_decisions: HashMap::new(),
                    joined_at: now_iso(),
                    updated_at: now_iso(),
                },
            );
            (room_id, role, agent.id)
        };
        persist_created_session(&state, &room_id).await?;
        persist_session_participant(&state, &room_id, &participant_session_id).await?;
        persist_session_participant(&state, &room_id, &agent_participant_id).await?;
        persist_session_event(
            &state,
            &room_id,
            Some(&participant_session_id),
            "participant_joined",
            json!({"role": role.as_str()}),
            None,
        )
        .await;
        persist_session_event(
            &state,
            &room_id,
            Some(&agent_participant_id),
            "participant_joined",
            json!({
                "role": "B",
                "kind": "agent",
                "agent": agent_event_metadata(&agent_metadata),
            }),
            None,
        )
        .await;
        maybe_start_room_agents(state.clone(), &room_id).await;
        return matched_response(&state, &room_id, &participant_session_id, role).await;
    }

    let queue_name = {
        let memory = state.memory.read().await;
        let participant = memory
            .participants
            .get(&participant_session_id)
            .ok_or_else(|| AppError::not_found("Participant session not found."))?;
        queue
            .or_else(|| participant.study_id.clone())
            .unwrap_or_else(|| state.config.study.name.clone())
    };

    let maybe_first = {
        let mut memory = state.memory.write().await;
        let queue = memory
            .matchmaking_queues
            .entry(queue_name.clone())
            .or_default();
        if queue.contains(&participant_session_id) {
            None
        } else if queue.is_empty() {
            queue.push(participant_session_id.clone());
            let (room_id, role) = create_room_locked(
                &state,
                &mut memory,
                participant_session_id.clone(),
                "direct".to_string(),
                Seat::A,
                Some(queue_name),
            )?;
            drop(memory);
            persist_created_session(&state, &room_id).await?;
            persist_session_participant(&state, &room_id, &participant_session_id).await?;
            persist_session_event(
                &state,
                &room_id,
                Some(&participant_session_id),
                "participant_joined",
                json!({"role": role.as_str()}),
                None,
            )
            .await;
            let source = state
                .memory
                .read()
                .await
                .participants
                .get(&participant_session_id)
                .map(|p| p.source.clone());
            return Ok(MatchmakingJoinResponse {
                status: "waiting".to_string(),
                participant_session_id,
                source,
                room_id: None,
                role: None,
                state: None,
                observation: None,
                available_actions: None,
                events: vec![],
                conversation: vec![],
            });
        } else {
            Some(queue.remove(0))
        }
    };

    let first = maybe_first.expect("second participant has first");
    let (room_id, role, created_room) = {
        let mut memory = state.memory.write().await;
        let (room_id, created_room) = if let Some(room) = memory.room_for_participant(&first) {
            (room.id.clone(), false)
        } else {
            let room = create_room_locked(
                &state,
                &mut memory,
                first.clone(),
                "direct".to_string(),
                Seat::A,
                Some(queue_name),
            )?;
            (room.0, true)
        };
        let second_participant_id = memory.participants[&participant_session_id].participant_id;
        let room = memory.rooms.get_mut(&room_id).expect("room exists");
        room.participants.insert(
            participant_session_id.clone(),
            RoomParticipant {
                participant_session_id: participant_session_id.clone(),
                participant_id: second_participant_id,
                source: "direct".to_string(),
                role: Seat::B,
                connected: false,
                audio_ready: !speechmatics_readiness_required(&state.config),
                consent_decisions: HashMap::new(),
                joined_at: now_iso(),
                updated_at: now_iso(),
            },
        );
        (room_id, Seat::B, created_room)
    };
    if created_room {
        persist_created_session(&state, &room_id).await?;
        persist_session_participant(&state, &room_id, &first).await?;
    }
    persist_session_participant(&state, &room_id, &participant_session_id).await?;
    persist_session_event(
        &state,
        &room_id,
        Some(&participant_session_id),
        "participant_joined",
        json!({"role": "B"}),
        None,
    )
    .await;
    matched_response(&state, &room_id, &participant_session_id, role).await
}

fn create_room_locked<A: GameAdapter>(
    state: &AppState<A>,
    memory: &mut MemoryState<A::State>,
    participant_session_id: String,
    mode: String,
    role: Seat,
    study_id: Option<String>,
) -> Result<(String, Seat), AppError> {
    let participant = memory
        .participants
        .get(&participant_session_id)
        .ok_or_else(|| AppError::not_found("Participant session not found."))?
        .clone();
    let mut room_id = room_code();
    while memory.rooms.contains_key(&room_id) {
        room_id = room_code();
    }
    let mut participants = HashMap::new();
    participants.insert(
        participant_session_id.clone(),
        RoomParticipant {
            participant_session_id,
            participant_id: participant.participant_id,
            source: participant.source,
            role,
            connected: false,
            audio_ready: !speechmatics_readiness_required(&state.config),
            consent_decisions: participant.consent_decisions,
            joined_at: now_iso(),
            updated_at: now_iso(),
        },
    );
    memory.rooms.insert(
        room_id.clone(),
        GameRoom {
            id: room_id.clone(),
            experiment_id: state.experiment_id.clone(),
            session_id: 0,
            mode,
            state: state.adapter.initial_state(),
            status: "waiting".to_string(),
            study_id,
            participants,
            created_at: now_iso(),
            updated_at: now_iso(),
        },
    );
    Ok((room_id, role))
}

async fn matchmaking_status_inner<A: GameAdapter>(
    state: Arc<AppState<A>>,
    participant_session_id: &str,
) -> Result<MatchmakingJoinResponse, AppError>
where
    A::State: Serialize,
{
    let existing = {
        let memory = state.memory.read().await;
        let participant = memory
            .participants
            .get(participant_session_id)
            .ok_or_else(|| AppError::not_found("Participant session not found."))?;
        if let Some(room) = memory.room_for_participant(participant_session_id) {
            let role = room.participants[participant_session_id].role;
            let matched = room.participants.values().any(|p| p.role == Seat::A)
                && room.participants.values().any(|p| p.role == Seat::B);
            Some((room.id.clone(), role, participant.source.clone(), matched))
        } else {
            None
        }
    };
    if let Some((room_id, role, source, matched)) = existing {
        if !matched {
            return Ok(MatchmakingJoinResponse {
                status: "waiting".to_string(),
                participant_session_id: participant_session_id.to_string(),
                source: Some(source),
                room_id: None,
                role: None,
                state: None,
                observation: None,
                available_actions: None,
                events: vec![],
                conversation: vec![],
            });
        }
        let response =
            room_response(&state, &room_id, participant_session_id, role, vec![]).await?;
        Ok(MatchmakingJoinResponse {
            status: "matched".to_string(),
            participant_session_id: participant_session_id.to_string(),
            source: Some(source),
            room_id: Some(response.room_id),
            role: Some(response.role),
            state: response.state,
            observation: response.observation,
            available_actions: response.available_actions,
            events: response.events,
            conversation: response.conversation,
        })
    } else {
        let source = state
            .memory
            .read()
            .await
            .participants
            .get(participant_session_id)
            .map(|p| p.source.clone());
        Ok(MatchmakingJoinResponse {
            status: "waiting".to_string(),
            participant_session_id: participant_session_id.to_string(),
            source,
            room_id: None,
            role: None,
            state: None,
            observation: None,
            available_actions: None,
            events: vec![],
            conversation: vec![],
        })
    }
}

async fn matched_response<A: GameAdapter>(
    state: &Arc<AppState<A>>,
    room_id: &str,
    participant_session_id: &str,
    role: Seat,
) -> Result<MatchmakingJoinResponse, AppError>
where
    A::State: Serialize,
{
    let source = state
        .memory
        .read()
        .await
        .participants
        .get(participant_session_id)
        .map(|p| p.source.clone());
    let response = room_response(state, room_id, participant_session_id, role, vec![]).await?;
    Ok(MatchmakingJoinResponse {
        status: "matched".to_string(),
        participant_session_id: participant_session_id.to_string(),
        source,
        room_id: Some(response.room_id),
        role: Some(response.role),
        state: response.state,
        observation: response.observation,
        available_actions: response.available_actions,
        events: response.events,
        conversation: response.conversation,
    })
}

async fn room_response<A: GameAdapter>(
    state: &Arc<AppState<A>>,
    room_id: &str,
    participant_session_id: &str,
    role: Seat,
    events: Vec<Value>,
) -> Result<RoomResponse, AppError>
where
    A::State: Serialize,
{
    let (observation, available_actions, events) = {
        let memory = state.memory.read().await;
        let room = memory
            .rooms
            .get(room_id)
            .ok_or_else(|| AppError::not_found("Room not found."))?;
        if room.status == "waiting" {
            return Ok(RoomResponse {
                room_id: room_id.to_string(),
                participant_session_id: participant_session_id.to_string(),
                role: role.as_str().to_string(),
                state: None,
                observation: None,
                available_actions: None,
                events: vec![],
                conversation: vec![],
            });
        }
        let player = role.player_role();
        (
            Some(protocol_json(
                &state.adapter.observe_state(&room.state, player),
            )?),
            state
                .adapter
                .available_actions(&room.state, player)
                .map(|actions| {
                    actions
                        .into_iter()
                        .map(|action| protocol_json(&action))
                        .collect::<Result<Vec<_>, _>>()
                })
                .transpose()?,
            events,
        )
    };
    Ok(RoomResponse {
        room_id: room_id.to_string(),
        participant_session_id: participant_session_id.to_string(),
        role: role.as_str().to_string(),
        state: None,
        observation,
        available_actions,
        events,
        conversation: vec![],
    })
}

async fn livekit_token<A: GameAdapter>(
    State(state): State<Arc<AppState<A>>>,
    Path(room_id): Path<String>,
    Json(request): Json<LiveKitTokenRequest>,
) -> Result<Json<LiveKitTokenResponse>, AppError> {
    let role = participant_role(&state, &room_id, &request.participant_session_id).await?;
    if !state.config.livekit.enabled {
        return Ok(Json(LiveKitTokenResponse::disabled()));
    }
    let token = create_livekit_token(
        &state.config.livekit,
        &room_id,
        role.as_str(),
        &request.participant_session_id,
        3600,
    )
    .map_err(|error| AppError::bad_request(error.to_string()))?;
    Ok(Json(LiveKitTokenResponse {
        enabled: true,
        url: Some(state.config.livekit.url.clone()),
        token: Some(token),
        identity: Some(livekit_identity(
            &room_id,
            role.as_str(),
            &request.participant_session_id,
        )),
    }))
}

async fn livekit_worker_token<A: GameAdapter>(
    State(state): State<Arc<AppState<A>>>,
    Path(room_id): Path<String>,
    Json(request): Json<LiveKitWorkerTokenRequest>,
) -> Result<Json<LiveKitTokenResponse>, AppError> {
    require_room(&state, &room_id).await?;
    if !state.config.livekit.enabled {
        return Ok(Json(LiveKitTokenResponse::disabled()));
    }
    let participant_session_id = format!("worker-{}", request.role);
    let token = create_livekit_token(
        &state.config.livekit,
        &room_id,
        &request.role,
        &participant_session_id,
        3600,
    )
    .map_err(|error| AppError::bad_request(error.to_string()))?;
    Ok(Json(LiveKitTokenResponse {
        enabled: true,
        url: Some(state.config.livekit.url.clone()),
        token: Some(token),
        identity: Some(livekit_identity(
            &room_id,
            &request.role,
            &participant_session_id,
        )),
    }))
}

async fn audio_session<A: GameAdapter>(
    State(state): State<Arc<AppState<A>>>,
    Path(room_id): Path<String>,
    Json(request): Json<AudioSessionRequest>,
) -> Result<Json<AudioSessionPlanResponse>, AppError> {
    let role = participant_role(&state, &room_id, &request.participant_session_id).await?;
    let planned = DefaultAudioSessionPlanner::plan(AudioSessionContext {
        config: &state.config,
        room_id: &room_id,
        role: role.as_str(),
        participant_session_id: &request.participant_session_id,
        token_ttl_seconds: 3600,
    })
    .await?;
    if planned.transcription_readiness == TranscriptionReadiness::SatisfiedByPlan {
        mark_participant_audio_ready(&state, &room_id, &request.participant_session_id).await;
        let _ = state
            .room_bus(&room_id)
            .await
            .send(voice_message(&state, &room_id).await);
        maybe_start_game(state.clone(), &room_id).await;
    }
    Ok(Json(planned.response))
}

async fn add_transcript<A: GameAdapter>(
    State(state): State<Arc<AppState<A>>>,
    Path(room_id): Path<String>,
    Json(segment): Json<TranscriptSegmentIn>,
) -> Result<Json<TranscriptSegment>, AppError> {
    let role = participant_role(&state, &room_id, &segment.participant_session_id).await?;
    let stored = TranscriptSegment {
        id: new_id("tr"),
        room_id: room_id.clone(),
        participant_session_id: segment.participant_session_id.clone(),
        player: role.as_str().to_string(),
        start_time_ms: segment.start_time_ms,
        end_time_ms: segment.end_time_ms,
        text: segment.text.clone(),
        metadata: segment.metadata.clone(),
        created_at: now_iso(),
    };
    let message = ConversationMessageResponse {
        id: new_id("msg"),
        room_id: room_id.clone(),
        sender_participant_session_id: Some(stored.participant_session_id.clone()),
        sender_role: Some(stored.player.clone()),
        text: stored.text.clone(),
        origin: "voice_transcript".to_string(),
        source_message_id: Some(stored.id.clone()),
        metadata: json!({
            "start_time_ms": stored.start_time_ms,
            "end_time_ms": stored.end_time_ms,
            "transcript_created_at": stored.created_at,
            "client_metadata": stored.metadata,
        }),
        created_at: now_iso(),
    };
    persist_session_event(
        &state,
        &room_id,
        Some(&stored.participant_session_id),
        "transcript_segment",
        serde_json::to_value(&stored).unwrap(),
        None,
    )
    .await;
    persist_session_event(
        &state,
        &room_id,
        message.sender_participant_session_id.as_deref(),
        "conversation_message",
        serde_json::to_value(&message).unwrap(),
        None,
    )
    .await;
    let _ = state.room_bus(&room_id).await.send(ServerMessage {
        conversation_message: Some(message),
        room_id: Some(room_id.clone()),
        ..ServerMessage::new("conversationMessageAdded")
    });
    Ok(Json(stored))
}

async fn add_voice_diagnostic<A: GameAdapter>(
    State(state): State<Arc<AppState<A>>>,
    Path(room_id): Path<String>,
    Json(diagnostic): Json<VoiceDiagnosticIn>,
) -> Result<Json<Value>, AppError> {
    participant_role(&state, &room_id, &diagnostic.participant_session_id).await?;
    let stored = json!({
        "id": new_id("vdiag"),
        "room_id": room_id,
        "participant_session_id": diagnostic.participant_session_id,
        "event": diagnostic.event,
        "metadata": diagnostic.metadata,
        "created_at": now_iso(),
    });
    persist_session_event(
        &state,
        stored["room_id"].as_str().unwrap_or(""),
        stored["participant_session_id"].as_str(),
        "voice_diagnostic",
        stored.clone(),
        None,
    )
    .await;
    Ok(Json(stored))
}

async fn add_conversation<A: GameAdapter>(
    State(state): State<Arc<AppState<A>>>,
    Path(room_id): Path<String>,
    Json(input): Json<ConversationMessageIn>,
) -> Result<Json<ConversationMessageResponse>, AppError> {
    require_room(&state, &room_id).await?;
    let mut sender_participant_session_id = None;
    let mut sender_role = None;
    if let Some(candidate) = input
        .metadata
        .get("sender_participant_session_id")
        .and_then(Value::as_str)
    {
        sender_participant_session_id = Some(candidate.to_string());
        sender_role = Some(
            participant_role(&state, &room_id, candidate)
                .await?
                .as_str()
                .to_string(),
        );
    } else if let Some(role) = input.metadata.get("sender_role").and_then(Value::as_str) {
        sender_role = Some(role.to_string());
    }
    let message = ConversationMessageResponse {
        id: new_id("msg"),
        room_id: room_id.clone(),
        sender_participant_session_id,
        sender_role,
        text: input.text,
        origin: input.origin,
        source_message_id: input.source_message_id,
        metadata: input.metadata,
        created_at: now_iso(),
    };
    persist_session_event(
        &state,
        &room_id,
        message.sender_participant_session_id.as_deref(),
        "conversation_message",
        serde_json::to_value(&message).unwrap(),
        None,
    )
    .await;
    let _ = state.room_bus(&room_id).await.send(ServerMessage {
        room_id: Some(room_id),
        conversation_message: Some(message.clone()),
        ..ServerMessage::new("conversationMessageAdded")
    });
    Ok(Json(message))
}

#[derive(Clone, Debug, Deserialize)]
struct AdminGamesQuery {
    limit: Option<i64>,
    experiment_id: Option<String>,
    status: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct AdminEventsQuery {
    after: Option<i64>,
    experiment_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct AdminExperimentsQuery {
    limit: Option<i64>,
}

#[derive(Clone, Debug, Deserialize)]
struct AdminCreateExperimentRequest {
    experiment_id: Option<String>,
    status: Option<String>,
    notes: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct AdminUpdateExperimentStatusRequest {
    status: String,
}

#[derive(Clone, Debug, Deserialize)]
struct AdminExportQuery {
    experiment_id: Option<String>,
    session_id: Option<i64>,
    status: Option<String>,
    event_type: Option<String>,
    format: Option<String>,
}

/// Serves the database-backed experiment/session dashboard.
async fn admin_games_page() -> Html<&'static str> {
    Html(ADMIN_GAMES_HTML)
}

/// Returns all known experiments with dashboard aggregates.
async fn admin_experiments<A: GameAdapter>(
    State(state): State<Arc<AppState<A>>>,
    Query(query): Query<AdminExperimentsQuery>,
) -> Result<Json<Value>, AppError> {
    let experiments = state
        .store
        .list_experiments(query.limit.unwrap_or(100))
        .await?;
    Ok(Json(json!({
        "active_experiment_id": state.experiment_id,
        "version_manifest": state.version_manifest,
        "experiments": experiments,
    })))
}

/// Creates a durable draft experiment row using the current server/game manifest.
async fn admin_create_experiment<A: GameAdapter>(
    State(state): State<Arc<AppState<A>>>,
    Json(request): Json<AdminCreateExperimentRequest>,
) -> Result<Json<Value>, AppError> {
    let status = request.status.unwrap_or_else(|| "draft".to_string());
    validate_experiment_status(&status)?;
    let experiment_id = request
        .experiment_id
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(generated_experiment_id);
    state
        .store
        .ensure_experiment(ExperimentRecord {
            experiment_id: experiment_id.clone(),
            config: serde_json::to_value(&state.config).map_err(|error| {
                AppError::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
            })?,
            server_version: Some(env!("CARGO_PKG_VERSION").to_string()),
            version_manifest: Some(state.version_manifest.clone()),
            status,
            notes: request.notes,
        })
        .await?;
    Ok(Json(json!({ "experiment_id": experiment_id })))
}

/// Updates a durable experiment lifecycle status.
async fn admin_update_experiment_status<A: GameAdapter>(
    State(state): State<Arc<AppState<A>>>,
    Path(experiment_id): Path<String>,
    Json(request): Json<AdminUpdateExperimentStatusRequest>,
) -> Result<Json<Value>, AppError> {
    validate_experiment_status(&request.status)?;
    state
        .store
        .update_experiment_status(&experiment_id, &request.status)
        .await?;
    Ok(Json(
        json!({ "experiment_id": experiment_id, "status": request.status }),
    ))
}

/// Returns recent database sessions for the experiment dashboard.
async fn admin_recent_games<A: GameAdapter>(
    State(state): State<Arc<AppState<A>>>,
    Query(query): Query<AdminGamesQuery>,
) -> Result<Json<Value>, AppError> {
    let experiment_id = query
        .experiment_id
        .unwrap_or_else(|| state.experiment_id.clone());
    let sessions = state
        .store
        .recent_sessions(&experiment_id, query.limit.unwrap_or(50))
        .await?;
    let sessions = sessions
        .into_iter()
        .filter(|session| {
            query
                .status
                .as_ref()
                .is_none_or(|status| &session.status == status)
        })
        .collect::<Vec<_>>();
    Ok(Json(json!({
        "experiment_id": experiment_id,
        "games": sessions.clone(),
        "sessions": sessions,
    })))
}

/// Returns one database session's metadata and important event timeline.
async fn admin_game_detail<A: GameAdapter>(
    State(state): State<Arc<AppState<A>>>,
    Path(session_id): Path<i64>,
    Query(query): Query<AdminGamesQuery>,
) -> Result<Json<Value>, AppError> {
    let experiment_id = query
        .experiment_id
        .unwrap_or_else(|| state.experiment_id.clone());
    let exported = state
        .store
        .export_session(&experiment_id, session_id)
        .await?;
    let session = exported["sessions"]
        .as_array()
        .and_then(|sessions| sessions.first())
        .cloned()
        .ok_or_else(|| AppError::not_found("Session not found."))?;
    let participants = state
        .store
        .session_participants(&experiment_id, session_id)
        .await?;
    let events = important_admin_events(
        state
            .store
            .session_events(&experiment_id, session_id, None)
            .await?,
    );
    let event_bundles = admin_event_bundles(&events);
    Ok(Json(json!({
        "experiment_id": experiment_id,
        "session": session,
        "participants": participants,
        "events": events,
        "event_bundles": event_bundles,
    })))
}

/// Returns important database events after an optional event index.
async fn admin_game_events<A: GameAdapter>(
    State(state): State<Arc<AppState<A>>>,
    Path(session_id): Path<i64>,
    Query(query): Query<AdminEventsQuery>,
) -> Result<Json<Value>, AppError> {
    let after = query.after.unwrap_or(0);
    let experiment_id = query
        .experiment_id
        .unwrap_or_else(|| state.experiment_id.clone());
    let all_events = state
        .store
        .session_events(&experiment_id, session_id, None)
        .await?;
    let events = all_events
        .clone()
        .into_iter()
        .filter(|event| event.event_index > after)
        .collect::<Vec<_>>();
    let visible_events = important_admin_events(events);
    let event_bundles = admin_event_bundles(&important_admin_events(all_events));
    Ok(Json(json!({
        "experiment_id": experiment_id,
        "session_id": session_id,
        "events": visible_events,
        "event_bundles": event_bundles,
    })))
}

/// Builds a compact timeline from durable events that matter during monitoring.
fn important_admin_events(events: Vec<crate::storage::StoredSessionEvent>) -> Vec<Value> {
    events
        .into_iter()
        .filter(is_important_admin_event)
        .map(admin_event_summary)
        .collect()
}

/// Groups durable admin events into display rows for the experiment dashboard.
fn admin_event_bundles(events: &[Value]) -> Vec<Value> {
    let mut bundles = Vec::<AdminEventBundle>::new();
    let mut open_bundles = HashMap::<String, usize>::new();
    let mut terminal_event_seen = false;
    for event in events
        .iter()
        .filter(|event| show_admin_event_in_timeline(event))
    {
        let kind = admin_bundle_kind(event);
        let role = admin_event_role(event);
        let key = admin_bundle_key(event, &kind, &role);
        let existing_index = open_bundles.get(&key).copied();
        if let Some(index) = existing_index {
            if can_append_admin_bundle(&bundles[index], event, &kind, &role) {
                bundles[index].events.push(event.clone());
                bundles[index].after_terminal_event =
                    bundles[index].after_terminal_event || terminal_event_seen;
                if admin_bundle_is_closed(&bundles[index]) {
                    open_bundles.remove(&key);
                }
                if admin_event_is_terminal_boundary(event) {
                    terminal_event_seen = true;
                }
                continue;
            }
        }

        let bundle = AdminEventBundle {
            kind: kind.clone(),
            role: role.clone(),
            key: key.clone(),
            events: vec![event.clone()],
            after_terminal_event: terminal_event_seen,
        };
        bundles.push(bundle);
        let index = bundles.len() - 1;
        if !admin_bundle_is_closed(&bundles[index]) {
            open_bundles.insert(key, index);
        }
        if admin_event_is_terminal_boundary(event) {
            terminal_event_seen = true;
        }
    }
    bundles.into_iter().map(admin_bundle_json).collect()
}

#[derive(Clone, Debug)]
struct AdminEventBundle {
    kind: String,
    role: Option<String>,
    key: String,
    events: Vec<Value>,
    after_terminal_event: bool,
}

fn admin_event_is_terminal_boundary(event: &Value) -> bool {
    matches!(
        admin_event_type(event).as_str(),
        "session_completed" | "participant_disconnected"
    )
}

fn admin_event_is_action_request(event: &Value) -> bool {
    matches!(
        admin_event_type(event).as_str(),
        "agent_action" | "game_action_submitted"
    )
}

fn show_admin_event_in_timeline(event: &Value) -> bool {
    if admin_event_type(event) != "voice_diagnostic" {
        return true;
    }
    let text = admin_event_text_value(event);
    text.contains("voice_connect_requested")
        || text.contains("stt_initialized")
        || text.contains("transcription_stream_connecting")
        || text.contains("transcription_stream_started")
        || text.to_ascii_lowercase().contains("fail")
        || text.to_ascii_lowercase().contains("error")
        || text.to_ascii_lowercase().contains("disconnect")
}

fn admin_bundle_kind(event: &Value) -> String {
    match admin_event_type(event).as_str() {
        "agent_action"
        | "game_action_accepted"
        | "game_action_rejected"
        | "game_action_submitted" => "action".to_string(),
        "participant_joined" | "participant_connected" | "participant_disconnected" | "ready" => {
            "participant".to_string()
        }
        "voice_diagnostic" => "voice".to_string(),
        "transcript_segment" => "transcript".to_string(),
        "conversation_message"
            if event
                .get("detail")
                .and_then(|detail| detail.get("origin"))
                .and_then(Value::as_str)
                == Some("voice_transcript") =>
        {
            "transcript".to_string()
        }
        "conversation_message" => "conversation".to_string(),
        other => other.to_string(),
    }
}

fn admin_bundle_key(event: &Value, kind: &str, role: &Option<String>) -> String {
    let role = role.as_deref().unwrap_or("system");
    match kind {
        "action" => format!(
            "action:{role}:{}",
            event
                .get("detail")
                .and_then(|detail| detail.get("action"))
                .map(compact_json)
                .unwrap_or_else(|| "null".to_string())
        ),
        "participant" | "voice" => format!("{kind}:{role}"),
        "transcript" => format!("{kind}:{role}:{}", normalized_transcript_text(event)),
        _ => format!("{kind}:{role}:{}", admin_event_index(event)),
    }
}

fn can_append_admin_bundle(
    bundle: &AdminEventBundle,
    event: &Value,
    kind: &str,
    role: &Option<String>,
) -> bool {
    if bundle.kind != kind {
        return false;
    }
    if (bundle.role.is_some() || role.is_some()) && bundle.role != *role {
        return false;
    }
    match kind {
        "action" => {
            !admin_bundle_is_closed(bundle)
                && bundle.events.first().and_then(admin_event_action) == admin_event_action(event)
                && !(bundle.events.iter().any(admin_event_is_action_request)
                    && admin_event_is_action_request(event))
        }
        "transcript" => {
            bundle
                .events
                .first()
                .map(normalized_transcript_text)
                .unwrap_or_default()
                == normalized_transcript_text(event)
        }
        "participant" | "voice" => true,
        _ => false,
    }
}

fn admin_bundle_is_closed(bundle: &AdminEventBundle) -> bool {
    match bundle.kind.as_str() {
        "action" => bundle.events.iter().any(|event| {
            matches!(
                admin_event_type(event).as_str(),
                "game_action_accepted" | "game_action_rejected"
            )
        }),
        "participant" => bundle.events.iter().any(|event| {
            matches!(
                admin_event_type(event).as_str(),
                "ready" | "participant_disconnected"
            )
        }),
        "voice" => bundle.events.iter().any(|event| {
            let text = admin_event_text_value(event).to_ascii_lowercase();
            text.contains("transcription_stream_started")
                || text.contains("ready")
                || text.contains("fail")
                || text.contains("error")
                || text.contains("disconnect")
        }),
        _ => false,
    }
}

fn admin_bundle_json(bundle: AdminEventBundle) -> Value {
    let first = bundle.events.first().cloned().unwrap_or_else(|| json!({}));
    let last = bundle.events.last().cloned().unwrap_or_else(|| json!({}));
    let steps = if bundle.events.len() > 1 {
        bundle
            .events
            .iter()
            .map(readable_admin_event_step)
            .collect::<Vec<_>>()
            .join(" -> ")
    } else {
        String::new()
    };
    let problem_reason = admin_bundle_problem_reason(&bundle);
    let problem = problem_reason.is_some();
    let action = if bundle.kind == "action" {
        bundle.events.iter().find_map(admin_event_action)
    } else {
        None
    };
    json!({
        "kind": bundle.kind,
        "key": bundle.key,
        "role": bundle.role,
        "first_index": admin_event_index(&first),
        "last_index": admin_event_index(&last),
        "created_at": last.get("created_at").cloned().unwrap_or(Value::Null),
        "title": admin_bundle_title(&bundle),
        "problem": problem,
        "problem_reason": problem_reason,
        "housekeeping": admin_bundle_is_housekeeping(&bundle),
        "steps": steps,
        "text": admin_bundle_text(&bundle),
        "action": action,
        "events": bundle.events,
    })
}

fn admin_bundle_is_housekeeping(bundle: &AdminEventBundle) -> bool {
    matches!(
        bundle.kind.as_str(),
        "participant" | "voice" | "session_created" | "session_completed"
    )
}

fn admin_bundle_problem_reason(bundle: &AdminEventBundle) -> Option<String> {
    if bundle
        .events
        .iter()
        .any(|event| admin_event_type(event).contains("rejected"))
    {
        return Some(admin_rejection_reason(bundle));
    }
    if let Some(text) = bundle.events.iter().find_map(|event| {
        let text = admin_event_text_value(event).to_ascii_lowercase();
        (text.contains("fail")
            || text.contains("error")
            || text.contains("reject")
            || (!bundle.after_terminal_event && text.contains("disconnect")))
        .then(|| admin_event_text_value(event))
    }) {
        return Some(text);
    }
    match bundle.kind.as_str() {
        "action" => (!bundle.after_terminal_event
            && !bundle.events.iter().any(|event| {
                matches!(
                    admin_event_type(event).as_str(),
                    "game_action_accepted" | "game_action_rejected"
                )
            }))
        .then(|| "Action was sent but no accepted/rejected result was logged.".to_string()),
        "voice" => (!bundle.after_terminal_event
            && !bundle.events.iter().any(|event| {
                let text = admin_event_text_value(event).to_ascii_lowercase();
                text.contains("stt_initialized")
                    || text.contains("transcription_stream_started")
                    || text.contains("transcription_ready")
                    || text.contains("ready")
            }))
        .then(|| "Voice setup did not log a ready/started event.".to_string()),
        "participant" => (bundle
            .events
            .iter()
            .any(|event| admin_event_type(event) == "participant_disconnected")
            && !bundle.after_terminal_event)
            .then(|| "Participant disconnected.".to_string()),
        _ => None,
    }
}

fn admin_rejection_reason(bundle: &AdminEventBundle) -> String {
    bundle
        .events
        .iter()
        .find_map(|event| {
            event
                .get("detail")
                .and_then(|detail| detail.get("error"))
                .and_then(Value::as_str)
                .or_else(|| event.get("text").and_then(Value::as_str))
                .map(str::to_string)
        })
        .unwrap_or_else(|| "Action was rejected.".to_string())
}

fn admin_bundle_title(bundle: &AdminEventBundle) -> String {
    match bundle.kind.as_str() {
        "action" => "Action".to_string(),
        "participant" => "Participant".to_string(),
        "voice" => "Voice".to_string(),
        "transcript" => "Voice Message".to_string(),
        "conversation" => "Message".to_string(),
        "session_created" | "session_completed" => "Session".to_string(),
        _ => bundle
            .events
            .first()
            .and_then(|event| event.get("title"))
            .and_then(Value::as_str)
            .unwrap_or("Event")
            .to_string(),
    }
}

fn admin_bundle_text(bundle: &AdminEventBundle) -> String {
    if matches!(
        bundle.kind.as_str(),
        "action" | "participant" | "session_created"
    ) {
        return String::new();
    }
    bundle
        .events
        .iter()
        .filter_map(|event| {
            let text = admin_event_text_value(event);
            (!text.is_empty()).then_some(text)
        })
        .fold(Vec::<String>::new(), |mut texts, text| {
            let normalized = normalize_display_text(&text);
            if !texts
                .iter()
                .any(|existing| normalize_display_text(existing) == normalized)
            {
                texts.push(text);
            }
            texts
        })
        .into_iter()
        .rev()
        .take(2)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join(" · ")
}

fn normalize_display_text(text: &str) -> String {
    text.trim()
        .trim_end_matches(['.', '!', '?'])
        .to_ascii_lowercase()
}

fn readable_admin_event_step(event: &Value) -> String {
    match admin_event_type(event).as_str() {
        "participant_joined" => "Joined".to_string(),
        "participant_connected" => "Connected".to_string(),
        "participant_disconnected" => "Disconnected".to_string(),
        "ready" => "Ready".to_string(),
        "transcript_segment" => "Transcript saved".to_string(),
        "conversation_message"
            if event
                .get("detail")
                .and_then(|detail| detail.get("origin"))
                .and_then(Value::as_str)
                == Some("voice_transcript") =>
        {
            "Transcript displayed".to_string()
        }
        "voice_diagnostic" => admin_event_text_value(event),
        _ => event
            .get("title")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| admin_event_type(event)),
    }
}

fn admin_event_type(event: &Value) -> String {
    event
        .get("event_type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn admin_event_role(event: &Value) -> Option<String> {
    event
        .get("actor_role")
        .and_then(Value::as_str)
        .or_else(|| {
            event
                .get("detail")
                .and_then(|detail| detail.get("player"))
                .and_then(Value::as_str)
        })
        .or_else(|| {
            event
                .get("detail")
                .and_then(|detail| detail.get("sender_role"))
                .and_then(Value::as_str)
        })
        .or_else(|| {
            event
                .get("detail")
                .and_then(|detail| detail.get("role"))
                .and_then(Value::as_str)
        })
        .map(str::to_string)
}

fn admin_event_index(event: &Value) -> i64 {
    event
        .get("event_index")
        .and_then(Value::as_i64)
        .unwrap_or_default()
}

fn admin_event_text_value(event: &Value) -> String {
    event
        .get("text")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            event
                .get("detail")
                .and_then(|detail| detail.get("event"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| admin_event_type(event))
}

fn normalized_transcript_text(event: &Value) -> String {
    event
        .get("text")
        .and_then(Value::as_str)
        .or_else(|| {
            event
                .get("raw")
                .and_then(|raw| raw.get("payload"))
                .and_then(|payload| payload.get("text"))
                .and_then(Value::as_str)
        })
        .unwrap_or_default()
        .trim()
        .trim_end_matches(['.', '!', '?'])
        .to_ascii_lowercase()
}

fn admin_event_action(event: &Value) -> Option<Value> {
    event
        .get("detail")
        .and_then(|detail| detail.get("action"))
        .cloned()
        .or_else(|| {
            event
                .get("raw")
                .and_then(|raw| raw.get("payload"))
                .and_then(|payload| payload.get("action"))
                .cloned()
        })
}

/// Returns whether a stored event should appear in the admin session monitor.
fn is_important_admin_event(event: &crate::storage::StoredSessionEvent) -> bool {
    matches!(
        event.event_type.as_str(),
        "agent_action"
            | "conversation_message"
            | "game_action_accepted"
            | "game_action_rejected"
            | "game_action_submitted"
            | "participant_connected"
            | "participant_disconnected"
            | "participant_joined"
            | "ready"
            | "session_completed"
            | "session_created"
            | "transcript_segment"
            | "voice_diagnostic"
    )
}

/// Converts one durable event row into UI-oriented metadata without game-specific types.
fn admin_event_summary(event: crate::storage::StoredSessionEvent) -> Value {
    let payload = event.payload.clone();
    let title = admin_event_title(&event.event_type, &payload);
    let text = admin_event_text(&event.event_type, &payload);
    let detail = admin_event_detail(&event.event_type, &payload);
    let raw = json!({
        "event_id": event.event_id,
        "experiment_id": event.experiment_id,
        "session_id": event.session_id,
        "event_index": event.event_index,
        "event_type": event.event_type,
        "actor_participant_id": event.actor_participant_id,
        "actor_role": event.actor_role,
        "payload": payload,
        "game_state": event.game_state,
        "created_at": event.created_at,
    });
    json!({
        "event_id": raw["event_id"],
        "event_index": raw["event_index"],
        "event_type": raw["event_type"],
        "actor_participant_id": raw["actor_participant_id"],
        "actor_role": raw["actor_role"],
        "created_at": raw["created_at"],
        "title": title,
        "text": text,
        "detail": detail,
        "raw": raw,
    })
}

/// Produces a human-readable title for one admin timeline event.
fn admin_event_title(event_type: &str, payload: &Value) -> &'static str {
    match event_type {
        "agent_action" => "Agent action",
        "conversation_message" => match payload.get("origin").and_then(Value::as_str) {
            Some("voice_transcript") => "Transcription result",
            Some("agent") => "Agent message",
            _ => "Conversation message",
        },
        "game_action_accepted" => "Action accepted",
        "game_action_rejected" => "Action rejected",
        "game_action_submitted" => "Action submitted",
        "participant_connected" => "Participant connected",
        "participant_disconnected" => "Participant disconnected",
        "participant_joined" => "Participant joined",
        "ready" => "Participant ready",
        "session_completed" => "Session completed",
        "session_created" => "Session created",
        "transcript_segment" => "Transcript segment",
        "voice_diagnostic" => "Voice diagnostic",
        _ => "Event",
    }
}

/// Extracts the primary readable text from one admin timeline event.
fn admin_event_text(event_type: &str, payload: &Value) -> Option<String> {
    match event_type {
        "conversation_message" | "transcript_segment" => payload
            .get("text")
            .and_then(Value::as_str)
            .map(str::to_string),
        "game_action_accepted" | "game_action_submitted" | "agent_action" => payload
            .get("action")
            .map(compact_json)
            .or_else(|| Some(compact_json(payload))),
        "game_action_rejected" => payload
            .get("error")
            .and_then(Value::as_str)
            .map(str::to_string),
        _ => None,
    }
}

/// Keeps compact structured details for one admin timeline event.
fn admin_event_detail(event_type: &str, payload: &Value) -> Value {
    match event_type {
        "conversation_message" => json!({
            "origin": payload.get("origin"),
            "sender_role": payload.get("sender_role"),
            "metadata": payload.get("metadata"),
        }),
        "transcript_segment" => json!({
            "player": payload.get("player"),
            "start_time_ms": payload.get("start_time_ms"),
            "end_time_ms": payload.get("end_time_ms"),
            "metadata": payload.get("metadata"),
        }),
        "game_action_accepted" => json!({
            "action": payload.get("action"),
            "events": payload.get("events"),
        }),
        "game_action_submitted" | "agent_action" => json!({
            "action": payload.get("action"),
        }),
        _ => payload.clone(),
    }
}

/// Serializes JSON into a stable one-line string for timeline labels.
fn compact_json(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "null".to_string())
}

fn agent_event_metadata(metadata: &Value) -> Value {
    let agent_type = metadata
        .get("agent_type")
        .or_else(|| metadata.get("agent_name"))
        .and_then(Value::as_str);
    let agent_version = metadata.get("agent_version").and_then(Value::as_str);
    json!({
        "agent_type": agent_type,
        "agent_version": agent_version,
        "agent_version_missing": agent_version.is_none(),
        "metadata": metadata,
    })
}

async fn admin_export<A: GameAdapter>(
    State(state): State<Arc<AppState<A>>>,
    Query(query): Query<AdminExportQuery>,
) -> Result<Response, AppError>
where
    A::State: Serialize,
{
    let value = filtered_export(&state, &query).await?;
    match query.format.as_deref().unwrap_or("json") {
        "json" => Ok(Json(value).into_response()),
        "yaml" | "yml" => Ok((
            [("content-type", "application/yaml; charset=utf-8")],
            serde_yaml::to_string(&value).map_err(|error| {
                AppError::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
            })?,
        )
            .into_response()),
        "csv" => Ok((
            [("content-type", "text/csv; charset=utf-8")],
            export_csv(&value),
        )
            .into_response()),
        other => Err(AppError::bad_request(format!(
            "Unsupported export format {other:?}."
        ))),
    }
}

fn validate_experiment_status(status: &str) -> Result<(), AppError> {
    if matches!(status, "draft" | "active" | "completed") {
        Ok(())
    } else {
        Err(AppError::bad_request(
            "Experiment status must be draft, active, or completed.",
        ))
    }
}

fn version_manifest(game_manifest: Option<Value>) -> Value {
    let server_warnings =
        local_dependency_warnings(env!("CARGO_MANIFEST_DIR"), include_str!("../Cargo.toml"));
    let mut warnings = server_warnings.clone();
    if let Some(game_warnings) = game_manifest
        .as_ref()
        .and_then(|manifest| manifest.get("warnings"))
        .and_then(Value::as_array)
    {
        warnings.extend(game_warnings.iter().cloned());
    }
    json!({
        "server": {
            "name": env!("CARGO_PKG_NAME"),
            "version": env!("CARGO_PKG_VERSION"),
            "build_time": option_env!("PARLANDO_SERVER_BUILD_TIME"),
            "git_sha": option_env!("PARLANDO_SERVER_GIT_SHA"),
            "git_dirty": option_env!("PARLANDO_SERVER_GIT_DIRTY").unwrap_or("unknown"),
            "repository": option_env!("CARGO_PKG_REPOSITORY"),
            "local_dependency_warnings": server_warnings,
        },
        "game": game_manifest,
        "warnings": warnings,
    })
}

fn local_dependency_warnings(manifest_dir: &str, cargo_toml: &str) -> Vec<Value> {
    cargo_toml
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            !trimmed.starts_with('#') && trimmed.contains("path") && trimmed.contains('=')
        })
        .map(|line| {
            json!({
                "level": "warning",
                "message": "Local path dependency linked into this build; use a published version or pinned Git revision for reproducibility.",
                "manifest_dir": manifest_dir,
                "dependency": line.trim(),
            })
        })
        .collect()
}

async fn filtered_export<A: GameAdapter>(
    state: &Arc<AppState<A>>,
    query: &AdminExportQuery,
) -> Result<Value, AppError>
where
    A::State: Serialize,
{
    let experiment_id = query
        .experiment_id
        .as_deref()
        .unwrap_or(&state.experiment_id);
    let mut exported = if let Some(session_id) = query.session_id {
        state
            .store
            .export_session(experiment_id, session_id)
            .await?
    } else {
        state.store.export_experiment(experiment_id).await?
    };
    if let Some(status) = query.status.as_deref() {
        filter_array_by_string(&mut exported, "sessions", "status", status);
        filter_scoped_tables_to_sessions(&mut exported);
    }
    if let Some(event_type) = query.event_type.as_deref() {
        filter_array_by_string(&mut exported, "session_events", "event_type", event_type);
    }
    Ok(exported)
}

fn filter_array_by_string(exported: &mut Value, table: &str, field: &str, expected: &str) {
    if let Some(rows) = exported.get_mut(table).and_then(Value::as_array_mut) {
        rows.retain(|row| row.get(field).and_then(Value::as_str) == Some(expected));
    }
}

fn filter_scoped_tables_to_sessions(exported: &mut Value) {
    let session_ids = exported
        .get("sessions")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(|row| row.get("session_id").and_then(Value::as_i64))
                .collect::<HashSet<_>>()
        })
        .unwrap_or_default();
    for table in [
        "session_participants",
        "consent_declarations",
        "session_events",
    ] {
        if let Some(rows) = exported.get_mut(table).and_then(Value::as_array_mut) {
            rows.retain(|row| {
                row.get("session_id")
                    .and_then(Value::as_i64)
                    .is_some_and(|id| session_ids.contains(&id))
            });
        }
    }
}

fn export_csv(exported: &Value) -> String {
    let mut output = String::from("table,row_json\n");
    for table in [
        "experiment",
        "participants",
        "sessions",
        "session_participants",
        "consent_declarations",
        "session_events",
    ] {
        let Some(value) = exported.get(table) else {
            continue;
        };
        if let Some(rows) = value.as_array() {
            for row in rows {
                push_csv_row(&mut output, table, row);
            }
        } else if !value.is_null() {
            push_csv_row(&mut output, table, value);
        }
    }
    output
}

fn push_csv_row(output: &mut String, table: &str, row: &Value) {
    output.push_str(&csv_escape(table));
    output.push(',');
    output.push_str(&csv_escape(
        &serde_json::to_string(row).unwrap_or_else(|_| "null".to_string()),
    ));
    output.push('\n');
}

fn csv_escape(value: &str) -> String {
    if value
        .chars()
        .any(|ch| matches!(ch, ',' | '"' | '\n' | '\r'))
    {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

const ADMIN_GAMES_HTML: &str = r##"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Parlando Experimenter Dashboard</title>
  <style>
    :root { font-family: Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; background: #f6f7f9; color: #182026; }
    * { box-sizing: border-box; }
    body { margin: 0; min-height: 100vh; }
    button { font: inherit; }
    .shell { display: grid; grid-template-columns: minmax(280px, 380px) minmax(0, 1fr); min-height: 100vh; }
    .sidebar { border-right: 1px solid #dde2e7; background: #fff; padding: 18px; overflow: auto; }
    .main { padding: 24px; overflow: auto; }
    .topline { display: flex; align-items: center; justify-content: space-between; gap: 12px; margin-bottom: 18px; }
    .section-title { display: flex; align-items: center; justify-content: space-between; gap: 10px; margin: 18px 0 10px; }
    h1 { font-size: 22px; line-height: 1.2; margin: 0; }
    h2 { font-size: 18px; margin: 0 0 12px; }
    .muted { color: #65717c; }
    .small { font-size: 12px; }
    .refresh, .primary, select, input { border: 1px solid #c9d2da; background: #fff; border-radius: 6px; padding: 7px 10px; }
    .refresh, .primary { cursor: pointer; }
    .primary { background: #185f8f; border-color: #185f8f; color: #fff; font-weight: 700; }
    .refresh:hover, .primary:hover { filter: brightness(0.97); }
    .control-grid { display: grid; gap: 8px; }
    .control-row { display: grid; grid-template-columns: minmax(0, 1fr) auto; gap: 8px; }
    .toggle-row { align-items: center; color: #4e5b66; display: inline-flex; font-size: 13px; gap: 8px; white-space: nowrap; }
    .toggle-row input { height: 16px; width: 16px; }
    .experiment-list { display: grid; gap: 8px; }
    .experiment { width: 100%; text-align: left; border: 1px solid #d9e0e6; border-radius: 8px; background: #fff; padding: 12px; cursor: pointer; }
    .experiment:hover, .experiment.active { border-color: #185f8f; background: #f1f7fb; }
    .warning { border: 1px solid #f2c36b; background: #fff8e8; color: #694500; border-radius: 8px; padding: 10px; margin-top: 10px; }
    .session-list { display: grid; gap: 8px; }
    .session { width: 100%; text-align: left; border: 1px solid #d9e0e6; border-radius: 8px; background: #fff; padding: 12px; cursor: pointer; }
    .session:hover, .session.active { border-color: #2773a8; background: #f2f8fc; }
    .session strong { display: block; color: #151b20; margin-bottom: 4px; overflow-wrap: anywhere; }
    .meta { display: flex; flex-wrap: wrap; gap: 8px; margin-top: 8px; }
    .pill { border: 1px solid #d7dee5; border-radius: 999px; padding: 3px 8px; background: #fff; color: #44515c; font-size: 12px; }
    .pill.warning-pill { border-color: #e5b35a; background: #fff8e8; color: #6f4700; }
    .panel { background: #fff; border: 1px solid #dce2e8; border-radius: 8px; padding: 16px; margin-bottom: 16px; }
    .grid { display: grid; grid-template-columns: repeat(4, minmax(0, 1fr)); gap: 12px; }
    .label { font-size: 12px; color: #66737f; margin-bottom: 4px; }
    .value { font-weight: 650; overflow-wrap: anywhere; }
    .participants { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 10px; }
    .participant { border: 1px solid #dde4ea; border-radius: 8px; padding: 12px; background: #fbfcfd; display: flex; align-items: center; gap: 12px; }
    .participant-body { min-width: 0; }
    .timeline { display: grid; gap: 8px; }
    .event { border-left: 4px solid #9bb6c8; background: #fff; border-radius: 8px; padding: 10px 12px; box-shadow: 0 1px 0 rgba(20, 34, 45, 0.05); }
    .event.action { border-left-color: #287a5c; }
    .event.transcript { border-left-color: #8b5a20; }
    .event.error { border-left-color: #b42318; }
    .event.problem { border-color: #b42318; background: #fff8f7; }
    .event-line { display: grid; grid-template-columns: auto minmax(140px, 1fr) minmax(0, 2fr) auto; align-items: center; gap: 10px; }
    .role-badge { align-items: center; border-radius: 6px; display: inline-flex; font-weight: 800; height: 28px; justify-content: center; min-width: 28px; padding: 0 8px; }
    .role-a { background: #dceef8; color: #135276; }
    .role-b { background: #faead1; color: #73480d; }
    .role-system { background: #e8edf2; color: #4c5964; font-size: 11px; letter-spacing: 0.02em; }
    .event-main { min-width: 0; }
    .event-meta { display: flex; align-items: center; justify-content: flex-end; gap: 10px; min-width: 210px; }
    .event-title { font-weight: 700; }
    .event-text { color: #252f38; overflow: hidden; text-overflow: ellipsis; white-space: normal; }
    .event-text:empty { display: none; }
    .problem-badge { background: #b42318; border-radius: 999px; color: #fff; display: inline-flex; font-size: 11px; font-weight: 800; margin-left: 6px; padding: 2px 7px; text-transform: uppercase; }
    .problem-reason { color: #9f1d16; font-size: 12px; font-weight: 650; margin-top: 5px; overflow-wrap: anywhere; }
    .bundle-steps { color: #66737f; font-size: 12px; margin-top: 4px; }
    .structured-action { display: grid; gap: 3px; min-width: 0; }
    .action-type { color: #151b20; font-weight: 850; margin-bottom: 2px; }
    .action-row { display: grid; grid-template-columns: minmax(76px, auto) minmax(0, 1fr); gap: 8px; }
    .action-key { color: #185f8f; font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; font-size: 12px; font-weight: 800; }
    .action-value { color: #27323a; font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; font-size: 12px; overflow-wrap: anywhere; }
    .event-json-block { border-top: 1px solid #dce2e8; padding-top: 10px; margin-top: 10px; }
    .event-json-title { color: #4e5b66; font-size: 12px; font-weight: 800; margin-bottom: 6px; }
    .expand { border: 1px solid #c9d2da; background: #fff; border-radius: 6px; padding: 4px 8px; cursor: pointer; font-size: 12px; line-height: 1.2; }
    .expand:hover { background: #f0f4f7; }
    .event-json { white-space: pre-wrap; overflow-wrap: anywhere; background: #f5f7f9; border-radius: 6px; padding: 10px; margin: 10px 0 0; font-size: 12px; }
    .event-json pre { margin: 0; white-space: pre-wrap; }
    .event-json[hidden] { display: none; }
    .empty { padding: 28px; text-align: center; color: #66737f; }
    @media (max-width: 860px) {
      .shell { grid-template-columns: 1fr; }
      .sidebar { border-right: 0; border-bottom: 1px solid #dde2e7; max-height: 44vh; }
      .grid, .participants { grid-template-columns: 1fr; }
      .control-row { grid-template-columns: 1fr; }
      .main { padding: 14px; }
      .event-line { grid-template-columns: 34px minmax(0, 1fr); align-items: start; gap: 9px; }
      .role-badge { grid-column: 1; width: 28px; min-width: 28px; padding: 0; }
      .event-main { grid-column: 2; }
      .event-title { display: block; line-height: 1.18; }
      .event-text { grid-column: 2; white-space: normal; overflow-wrap: anywhere; }
      .event-meta { grid-column: 2; justify-content: space-between; min-width: 0; width: 100%; }
    }
  </style>
</head>
<body>
  <div class="shell">
    <aside class="sidebar">
      <div class="topline">
        <h1>Experimenter Dashboard</h1>
        <button class="refresh" id="refreshSessions" title="Refresh sessions">Refresh</button>
      </div>
      <section class="control-grid" aria-label="Experiments">
        <div class="section-title">
          <h2>Experiments</h2>
          <button class="primary" id="createExperiment" title="Create draft experiment">New</button>
        </div>
        <input id="newExperimentId" placeholder="Optional experiment id">
        <div id="experimentList" class="experiment-list"><div class="empty">Loading experiments...</div></div>
      </section>
      <div class="section-title">
        <h2>Sessions</h2>
        <select id="sessionStatusFilter" title="Filter sessions by status">
          <option value="">All</option>
          <option value="waiting">Waiting</option>
          <option value="playing">Active</option>
          <option value="completed">Completed</option>
        </select>
      </div>
      <div id="sessionList" class="session-list"><div class="empty">Loading sessions...</div></div>
    </aside>
    <main class="main">
      <section class="panel">
        <div class="topline">
          <h2>Export</h2>
          <span id="activeExperimentLabel" class="muted small">No experiment selected.</span>
        </div>
        <div class="control-grid">
          <div class="control-row">
            <select id="exportScope" title="Export scope">
              <option value="experiment">Selected experiment</option>
              <option value="session">Selected session</option>
            </select>
            <select id="exportFormat" title="Export format">
              <option value="json">JSON</option>
              <option value="yaml">YAML</option>
              <option value="csv">CSV</option>
            </select>
          </div>
          <div class="control-row">
            <input id="eventTypeFilter" placeholder="Optional event type filter">
            <button class="primary" id="downloadExport">Export</button>
          </div>
        </div>
      </section>
      <section class="panel" id="reproducibility"><div class="empty">Loading version manifest...</div></section>
      <section class="panel" id="summary"><div class="empty">Select a session to inspect metadata and events.</div></section>
      <section class="panel">
        <h2>Players</h2>
        <div id="participants" class="participants"><div class="muted">No game selected.</div></div>
      </section>
      <section>
        <div class="topline">
          <h2>Important Events</h2>
          <label class="toggle-row" title="Show participant, session, and voice setup events">
            <input id="showHousekeeping" type="checkbox">
            <span>Show setup</span>
          </label>
          <span id="liveStatus" class="muted small">Idle</span>
        </div>
        <div id="timeline" class="timeline"></div>
      </section>
    </main>
  </div>
  <script>
    const state = { experiments: [], activeExperimentId: null, sessions: [], selected: null, events: [], eventBundles: [], lastEventIndex: 0, timer: null, versionManifest: null };
    const experimentList = document.getElementById('experimentList');
    const sessionList = document.getElementById('sessionList');
    const summary = document.getElementById('summary');
    const participants = document.getElementById('participants');
    const timeline = document.getElementById('timeline');
    const liveStatus = document.getElementById('liveStatus');
    const reproducibility = document.getElementById('reproducibility');
    const activeExperimentLabel = document.getElementById('activeExperimentLabel');
    const sessionStatusFilter = document.getElementById('sessionStatusFilter');
    const showHousekeeping = document.getElementById('showHousekeeping');

    function fmtTime(value) {
      if (!value) return '-';
      const date = new Date(value);
      return Number.isNaN(date.getTime()) ? value : date.toLocaleString();
    }

    function escapeHtml(value) {
      return String(value ?? '').replace(/[&<>"']/g, ch => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[ch]));
    }

    function renderExperiments() {
      if (!state.experiments.length) {
        experimentList.innerHTML = '<div class="empty">No experiments yet.</div>';
        return;
      }
      experimentList.innerHTML = state.experiments.map(experiment => `
        <button class="experiment ${state.activeExperimentId === experiment.experiment_id ? 'active' : ''}" data-experiment="${escapeHtml(experiment.experiment_id)}">
          <strong>${escapeHtml(experiment.study_name || experiment.experiment_id)}</strong>
          ${experiment.study_name ? `<span class="muted small">${escapeHtml(experiment.experiment_id)}</span>` : ''}
          <span class="meta">
            <span class="pill">${escapeHtml(experiment.status)}</span>
            <span class="pill">${experiment.session_count} sessions</span>
            <span class="pill">${experiment.completed_session_count} completed</span>
          </span>
        </button>
      `).join('');
      experimentList.querySelectorAll('.experiment').forEach(button => {
        button.addEventListener('click', () => {
          state.activeExperimentId = button.dataset.experiment;
          state.selected = null;
          renderExperiments();
          renderReproducibility();
          loadSessions();
        });
      });
    }

    function renderReproducibility() {
      const experiment = state.experiments.find(item => item.experiment_id === state.activeExperimentId);
      const manifest = experiment?.version_manifest || state.versionManifest || {};
      const warnings = manifest.warnings || [];
      activeExperimentLabel.textContent = state.activeExperimentId ? `Experiment ${state.activeExperimentId}` : 'No experiment selected.';
      reproducibility.innerHTML = `
        <h2>Reproducibility</h2>
        <div class="grid">
          <div><div class="label">Server</div><div class="value">${escapeHtml(manifest.server?.version || experiment?.server_version || '-')}</div></div>
          <div><div class="label">Server Git</div><div class="value">${escapeHtml(shortSha(manifest.server?.git_sha))}</div></div>
          <div><div class="label">Game</div><div class="value">${escapeHtml(manifest.game?.version || '-')}</div></div>
          <div><div class="label">Game Git</div><div class="value">${escapeHtml(shortSha(manifest.game?.git_sha))}</div></div>
          <div><div class="label">Client</div><div class="value">${escapeHtml(manifest.game?.client?.version || '-')}</div></div>
          <div><div class="label">Client SDK</div><div class="value">${escapeHtml(manifest.game?.client?.package_version || '-')}</div></div>
        </div>
        ${warnings.length ? warnings.map(warning => `<div class="warning">${escapeHtml(warning.message || warning.dependency || JSON.stringify(warning))}</div>`).join('') : '<p class="muted small">No local development dependency warnings recorded.</p>'}
      `;
    }

    function shortSha(value) {
      if (!value) return '-';
      return String(value).slice(0, 12);
    }

    function renderSessions() {
      if (!state.sessions.length) {
        sessionList.innerHTML = '<div class="empty">No sessions in this experiment yet.</div>';
        return;
      }
      sessionList.innerHTML = state.sessions.map(session => `
        <button class="session ${state.selected === session.session_id ? 'active' : ''}" data-session="${session.session_id}">
          <strong>Session #${session.session_id}</strong>
          <span class="muted small">Room ${escapeHtml(session.room_id)} · ${escapeHtml(fmtTime(session.created_at))}</span>
          <span class="meta">
            <span class="pill">${escapeHtml(session.status)}</span>
            <span class="pill">${session.participant_count} players</span>
            <span class="pill">${session.event_count} events</span>
          </span>
        </button>
      `).join('');
      sessionList.querySelectorAll('.session').forEach(button => {
        button.addEventListener('click', () => selectSession(Number(button.dataset.session)));
      });
    }

    async function loadSessions() {
      const params = new URLSearchParams({ limit: '80' });
      if (state.activeExperimentId) params.set('experiment_id', state.activeExperimentId);
      if (sessionStatusFilter.value) params.set('status', sessionStatusFilter.value);
      const response = await fetch(`/api/admin/sessions?${params}`);
      const data = await response.json();
      state.sessions = data.sessions || data.games || [];
      state.activeExperimentId = data.experiment_id || state.activeExperimentId;
      renderSessions();
      if (!state.selected && state.sessions[0]) selectSession(state.sessions[0].session_id);
    }

    async function loadExperiments() {
      const response = await fetch('/api/admin/experiments?limit=100');
      const data = await response.json();
      state.experiments = data.experiments || [];
      state.versionManifest = data.version_manifest || null;
      state.activeExperimentId = state.activeExperimentId || data.active_experiment_id || state.experiments[0]?.experiment_id || null;
      renderExperiments();
      renderReproducibility();
    }

    async function selectSession(sessionId) {
      state.selected = sessionId;
      state.lastEventIndex = 0;
      renderSessions();
      clearInterval(state.timer);
      liveStatus.textContent = 'Loading';
      const detailParams = new URLSearchParams();
      if (state.activeExperimentId) detailParams.set('experiment_id', state.activeExperimentId);
      const response = await fetch(`/api/admin/sessions/${sessionId}?${detailParams}`);
      const data = await response.json();
      renderSummary(data.session);
      renderParticipants(data.participants || []);
      state.events = [];
      state.eventBundles = data.event_bundles || [];
      mergeEvents(data.events || []);
      renderEventBundles();
      state.timer = setInterval(refreshEvents, 1500);
      liveStatus.textContent = 'Live refresh on';
    }

    function renderSummary(session) {
      summary.innerHTML = `
        <h2>Session #${escapeHtml(session.session_id)}</h2>
        <div class="grid">
          <div><div class="label">Room</div><div class="value">${escapeHtml(session.room_id)}</div></div>
          <div><div class="label">Mode</div><div class="value">${escapeHtml(session.mode)}</div></div>
          <div><div class="label">Status</div><div class="value">${escapeHtml(session.status)}</div></div>
          <div><div class="label">Created</div><div class="value">${escapeHtml(fmtTime(session.created_at))}</div></div>
        </div>
      `;
    }

    function renderParticipants(rows) {
      participants.innerHTML = rows.length ? rows.map(row => `
        <div class="participant">
          ${roleBadge(row.role)}
          <div class="participant-body">
            <div class="value">${escapeHtml(row.display_name || row.participant_session_id)}</div>
            <div class="meta">
              <span class="pill">${escapeHtml(row.connection_status)}</span>
              <span class="pill">${escapeHtml(row.participant_kind || 'participant')}</span>
              ${agentParticipantPills(row)}
            </div>
          </div>
        </div>
      `).join('') : '<div class="muted">No players recorded.</div>';
    }

    function agentParticipantPills(row) {
      if (row.participant_kind !== 'agent') return '';
      const metadata = row.metadata || {};
      const type = metadata.agent_type || metadata.agent_name;
      const version = metadata.agent_version;
      return `
        <span class="pill ${type ? '' : 'warning-pill'}">Agent: ${escapeHtml(type || 'type missing')}</span>
        <span class="pill ${version ? '' : 'warning-pill'}">Version: ${escapeHtml(version || 'missing')}</span>
      `;
    }

    function eventClass(bundle) {
      if (bundle.problem) return 'problem';
      if (bundle.kind === 'action') return 'action';
      if (bundle.kind === 'voice' || bundle.kind === 'transcript') return 'transcript';
      return '';
    }

    // Returns the event's participant role when the durable row exposes one.
    function eventRole(event) {
      return event.actor_role || event.detail?.player || event.detail?.sender_role || event.detail?.role || '';
    }

    // Renders compact participant role markers for rows and event timeline entries.
    function roleBadge(role) {
      const normalized = role === 'A' || role === 'B' ? role : '';
      if (!normalized) return '<span class="role-badge role-system">SYS</span>';
      return `<span class="role-badge role-${normalized.toLowerCase()}">${escapeHtml(normalized)}</span>`;
    }

    // Returns only extra event text that is not already implied by the title and badge.
    function mergeEvents(events) {
      const known = new Set(state.events.map(event => `${event.event_id}:${event.event_index}`));
      for (const event of events) {
        const key = `${event.event_id}:${event.event_index}`;
        if (known.has(key)) continue;
        known.add(key);
        state.events.push(event);
        state.lastEventIndex = Math.max(state.lastEventIndex, event.event_index);
      }
      state.events.sort((left, right) => left.event_index - right.event_index);
    }

    function renderEventBundles() {
      const bundles = (state.eventBundles || []).filter(bundle => showHousekeeping.checked || !bundle.housekeeping);
      timeline.innerHTML = '';
      for (const bundle of bundles) {
        const jsonId = `event-bundle-${bundle.first_index}-${bundle.last_index}`;
        const rawHtml = bundle.events.map(event => `
          <div class="event-json-block">
            <div class="event-json-title">#${event.event_index} ${escapeHtml(event.title || event.event_type)} · ${escapeHtml(fmtTime(event.created_at))}</div>
            ${actionFromEvent(event) ? `<div class="structured-action">${prettyAction(actionFromEvent(event), eventRole(event))}</div>` : ''}
            <pre>${escapeHtml(JSON.stringify(event.raw || event, null, 2))}</pre>
          </div>
        `).join('');
        timeline.insertAdjacentHTML('beforeend', `
          <article class="event ${eventClass(bundle)}">
            <div class="event-line">
              ${roleBadge(bundle.role)}
              <div class="event-main">
                <span class="event-title">${escapeHtml(bundle.title)}${bundle.problem ? '<span class="problem-badge">Problem</span>' : ''}</span>
                <span class="muted small">#${bundle.first_index}${bundle.first_index === bundle.last_index ? '' : `-${bundle.last_index}`}</span>
                ${bundle.steps ? `<div class="bundle-steps">${escapeHtml(bundle.steps)}</div>` : ''}
                ${bundle.problem_reason ? `<div class="problem-reason">${escapeHtml(bundle.problem_reason)}</div>` : ''}
              </div>
              <div class="event-text">${bundle.action ? `<div class="structured-action">${prettyAction(bundle.action, bundle.role)}</div>` : escapeHtml(bundle.text || '')}</div>
              <div class="event-meta">
                <span class="muted small">${escapeHtml(fmtTime(bundle.created_at))}</span>
                <button class="expand" type="button" aria-expanded="false" aria-controls="${jsonId}" data-target="${jsonId}">JSON</button>
              </div>
            </div> 
            <div class="event-json" id="${jsonId}" hidden>${rawHtml}</div>
          </article>
        `);
      }
      if (!timeline.children.length) timeline.innerHTML = '<div class="empty">No action or message events recorded yet.</div>';
    }

    function actionFromEvent(event) {
      return event.detail?.action || event.raw?.payload?.action || event.payload?.action || null;
    }

    function prettyAction(action, role) {
      if (!action || typeof action !== 'object') return escapeHtml(String(action ?? ''));
      const type = action.type;
      const rows = Object.entries(action).filter(([key, value]) => {
        if (key === 'type') return false;
        if (key === 'player' && role && value === role) return false;
        return true;
      }).map(([key, value]) => `
        <div class="action-row">
          <span class="action-key">${escapeHtml(key)}</span>
          <span class="action-value">${escapeHtml(formatActionValue(value))}</span>
        </div>
      `).join('');
      return `${type ? `<strong class="action-type">${escapeHtml(type)}</strong>` : ''}${rows}`;
    }

    function formatActionValue(value) {
      if (value === null) return 'null';
      if (typeof value === 'string' || typeof value === 'number' || typeof value === 'boolean') return String(value);
      return JSON.stringify(value);
    }

    async function refreshEvents() {
      if (!state.selected) return;
      const params = new URLSearchParams({ after: String(state.lastEventIndex) });
      if (state.activeExperimentId) params.set('experiment_id', state.activeExperimentId);
      const response = await fetch(`/api/admin/sessions/${state.selected}/events?${params}`);
      const data = await response.json();
      state.eventBundles = data.event_bundles || state.eventBundles;
      mergeEvents(data.events || []);
      renderEventBundles();
      liveStatus.textContent = `Last checked ${new Date().toLocaleTimeString()}`;
    }

    document.getElementById('refreshSessions').addEventListener('click', loadSessions);
    showHousekeeping.addEventListener('change', renderEventBundles);
    sessionStatusFilter.addEventListener('change', () => {
      state.selected = null;
      loadSessions();
    });
    document.getElementById('createExperiment').addEventListener('click', async () => {
      const input = document.getElementById('newExperimentId');
      const body = { status: 'draft', experiment_id: input.value.trim() || null };
      const response = await fetch('/api/admin/experiments', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(body)
      });
      if (!response.ok) throw new Error(await response.text());
      const data = await response.json();
      input.value = '';
      state.activeExperimentId = data.experiment_id;
      await loadExperiments();
      await loadSessions();
    });
    document.getElementById('downloadExport').addEventListener('click', () => {
      if (!state.activeExperimentId) return;
      const params = new URLSearchParams({
        experiment_id: state.activeExperimentId,
        format: document.getElementById('exportFormat').value
      });
      if (document.getElementById('exportScope').value === 'session' && state.selected) {
        params.set('session_id', String(state.selected));
      }
      const eventType = document.getElementById('eventTypeFilter').value.trim();
      if (eventType) params.set('event_type', eventType);
      window.location.href = `/api/admin/export?${params}`;
    });
    timeline.addEventListener('click', event => {
      const button = event.target.closest('.expand');
      if (!button) return;
      const panel = document.getElementById(button.dataset.target);
      if (!panel) return;
      const expanded = button.getAttribute('aria-expanded') === 'true';
      button.setAttribute('aria-expanded', String(!expanded));
      button.textContent = expanded ? 'JSON' : 'Hide';
      panel.hidden = expanded;
    });
    loadExperiments().then(loadSessions).catch(error => {
      sessionList.innerHTML = `<div class="empty">${escapeHtml(error.message)}</div>`;
    });
  </script>
</body>
</html>
"##;

async fn game_socket<A: GameAdapter>(
    State(state): State<Arc<AppState<A>>>,
    Path(room_id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    ws: WebSocketUpgrade,
) -> Result<Response, AppError>
where
    A::State: Serialize,
{
    let participant_session_id = query
        .get("participantSessionId")
        .ok_or_else(|| AppError::bad_request("participantSessionId query parameter is required"))?
        .clone();
    let role = participant_role(&state, &room_id, &participant_session_id).await?;
    Ok(ws.on_upgrade(move |socket| {
        websocket_loop(state, socket, room_id, participant_session_id, role)
    }))
}

async fn websocket_loop<A: GameAdapter>(
    state: Arc<AppState<A>>,
    socket: WebSocket,
    room_id: String,
    participant_session_id: String,
    role: Seat,
) where
    A::State: Serialize,
{
    {
        let mut memory = state.memory.write().await;
        if let Some(room) = memory.rooms.get_mut(&room_id) {
            if let Some(participant) = room.participants.get_mut(&participant_session_id) {
                participant.connected = true;
                participant.updated_at = now_iso();
            }
        }
    }
    persist_event(
        "participant_connected",
        state.store.update_session_participant_connection(
            &participant_session_id,
            "connected",
            None,
        ),
    )
    .await;
    persist_session_event(
        &state,
        &room_id,
        Some(&participant_session_id),
        "participant_connected",
        Value::Null,
        None,
    )
    .await;
    let bus = state.room_bus(&room_id).await;
    let mut receiver = bus.subscribe();
    if let Some(message) = presence_message(&state, &room_id).await {
        let _ = bus.send(message);
    }
    let _ = bus.send(voice_message(&state, &room_id).await);
    if room_has_started(&state, &room_id).await {
        send_role_assignment(&state, &room_id, &participant_session_id, role).await;
    } else {
        maybe_start_game(state.clone(), &room_id).await;
    }
    let (mut sender, mut incoming) = socket.split();
    let outbound_participant_session_id = participant_session_id.clone();
    let send_task = tokio::spawn(async move {
        while let Ok(message) = receiver.recv().await {
            if message
                .participant_session_id
                .as_ref()
                .is_some_and(|target| target != &outbound_participant_session_id)
            {
                continue;
            }
            if let Ok(text) = serde_json::to_string(&message) {
                if sender.send(Message::Text(text)).await.is_err() {
                    break;
                }
            }
        }
    });
    while let Some(Ok(message)) = FuturesStreamExt::next(&mut incoming).await {
        let Message::Text(text) = message else {
            continue;
        };
        let Ok(client_message) = serde_json::from_str::<ClientMessage>(&text) else {
            let _ = bus.send(error_message(&room_id, "Invalid client message JSON"));
            continue;
        };
        handle_client_message(
            state.clone(),
            &bus,
            &room_id,
            &participant_session_id,
            role,
            client_message,
        )
        .await;
    }
    send_task.abort();
    {
        let mut memory = state.memory.write().await;
        if let Some(room) = memory.rooms.get_mut(&room_id) {
            if let Some(participant) = room.participants.get_mut(&participant_session_id) {
                participant.connected = false;
                participant.updated_at = now_iso();
            }
        }
    }
    persist_event(
        "participant_disconnected",
        state.store.update_session_participant_connection(
            &participant_session_id,
            "disconnected",
            Some(now_iso()),
        ),
    )
    .await;
    persist_session_event(
        &state,
        &room_id,
        Some(&participant_session_id),
        "participant_disconnected",
        Value::Null,
        None,
    )
    .await;
    let _ = bus.send(
        presence_message(&state, &room_id)
            .await
            .unwrap_or_else(|| ServerMessage::new("presenceChanged")),
    );
}

async fn handle_client_message<A: GameAdapter>(
    state: Arc<AppState<A>>,
    bus: &broadcast::Sender<ServerMessage>,
    room_id: &str,
    participant_session_id: &str,
    role: Seat,
    message: ClientMessage,
) where
    A::State: Serialize,
{
    match message.message_type.as_str() {
        "heartbeat" => {
            let _ = bus.send(ServerMessage {
                room_id: Some(room_id.to_string()),
                ..ServerMessage::new("presenceChanged")
            });
        }
        "ready" => {
            persist_session_event(
                &state,
                room_id,
                Some(participant_session_id),
                "ready",
                Value::Null,
                None,
            )
            .await;
            if let Some(message) = presence_message(&state, room_id).await {
                let _ = bus.send(message);
            }
            let _ = bus.send(voice_message(&state, room_id).await);
        }
        "consentUpdated" => {
            if let Some(consent_request) = message.consent {
                let _ = consent(State(state.clone()), Json(consent_request)).await;
            }
        }
        "sendChatMessage" => {
            let input = ConversationMessageIn {
                text: message.text.unwrap_or_default(),
                origin: "typed".to_string(),
                source_message_id: None,
                metadata: json!({"sender_participant_session_id": participant_session_id}),
            };
            let _ = add_conversation(State(state.clone()), Path(room_id.to_string()), Json(input))
                .await;
        }
        "submitAction" => {
            let Some(raw_action) = message.action else {
                persist_session_event(
                    &state,
                    room_id,
                    Some(participant_session_id),
                    "game_action_rejected",
                    json!({"error": "submitAction requires action"}),
                    None,
                )
                .await;
                let _ = bus.send(error_message(room_id, "submitAction requires action"));
                return;
            };
            persist_session_event(
                &state,
                room_id,
                Some(participant_session_id),
                "game_action_submitted",
                json!({"action": raw_action.clone()}),
                None,
            )
            .await;
            let action = match state.adapter.parse_action(raw_action) {
                Ok(action) => action,
                Err(error) => {
                    persist_session_event(
                        &state,
                        room_id,
                        Some(participant_session_id),
                        "game_action_rejected",
                        json!({"error": error.to_string()}),
                        None,
                    )
                    .await;
                    let _ = bus.send(error_message(room_id, &error.to_string()));
                    return;
                }
            };
            match submit_action(state.clone(), room_id, participant_session_id, role, action).await
            {
                Ok((completed, summary)) => {
                    broadcast_player_views(state.clone(), room_id).await;
                    if completed {
                        let _ = bus.send(ServerMessage {
                            room_id: Some(room_id.to_string()),
                            summary,
                            ..ServerMessage::new("completed")
                        });
                    }
                }
                Err(error) => {
                    persist_session_event(
                        &state,
                        room_id,
                        Some(participant_session_id),
                        "game_action_rejected",
                        json!({"error": error.to_string()}),
                        None,
                    )
                    .await;
                    let _ = bus.send(error_message(room_id, &error.to_string()));
                }
            }
        }
        other => {
            let _ = bus.send(error_message(
                room_id,
                &format!("Unknown message type: {other}"),
            ));
        }
    }
}

async fn submit_action<A: GameAdapter>(
    state: Arc<AppState<A>>,
    room_id: &str,
    participant_session_id: &str,
    role: Seat,
    action: A::Action,
) -> Result<(bool, Option<Value>)>
where
    A::State: Serialize,
{
    let player = role.player_role();
    let (before, after, events, completed, summary) = {
        let mut memory = state.memory.write().await;
        let room = memory
            .rooms
            .get_mut(room_id)
            .ok_or_else(|| anyhow!("Room not found."))?;
        if !room_ready_for_game::<A>(&state.config, room) {
            if speechmatics_readiness_required(&state.config) {
                return Err(anyhow!(
                    "Room is waiting for speech transcription to initialize."
                ));
            }
            return Err(anyhow!("Room is waiting for both players to connect."));
        }
        let before = room.state.clone();
        state
            .adapter
            .validate_action(&room.state, &action, player)?;
        let after = state.adapter.apply_action(&room.state, &action)?;
        let events = state
            .adapter
            .events_for_action(&before, &after, &action, player);
        room.state = after.clone();
        room.updated_at = now_iso();
        let completed = state.adapter.is_complete(&room.state);
        if completed {
            room.status = "completed".to_string();
        }
        let summary = completed.then(|| state.adapter.completion_summary(&room.state));
        (before, after, events, completed, summary)
    };
    persist_session_event(
        &state,
        room_id,
        Some(participant_session_id),
        "game_action_accepted",
        json!({
            "action": protocol_json(&action)?,
            "before": protocol_json(&before)?,
            "events": protocol_json(&events)?,
        }),
        Some(protocol_json(&after)?),
    )
    .await;
    persist_session_event(
        &state,
        room_id,
        Some(participant_session_id),
        "state_changed",
        json!({"events": protocol_json(&events)?}),
        Some(protocol_json(&after)?),
    )
    .await;
    let summary = summary.map(|summary| protocol_json(&summary)).transpose()?;
    if completed {
        let session_id = {
            let memory = state.memory.read().await;
            memory.rooms.get(room_id).map(|room| room.session_id)
        };
        if let Some(session_id) = session_id {
            persist_event(
                "session_completed",
                state
                    .store
                    .complete_session(&state.experiment_id, session_id, summary.clone()),
            )
            .await;
            persist_session_event(
                &state,
                room_id,
                Some(participant_session_id),
                "session_completed",
                summary.clone().unwrap_or(Value::Null),
                Some(protocol_json(&after)?),
            )
            .await;
        }
    }
    Ok((completed, summary))
}

async fn broadcast_player_views<A: GameAdapter>(state: Arc<AppState<A>>, room_id: &str)
where
    A::State: Serialize,
{
    let participants = {
        let memory = state.memory.read().await;
        memory
            .rooms
            .get(room_id)
            .map(|room| {
                room.participants
                    .values()
                    .map(|p| (p.participant_session_id.clone(), p.role))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };
    let bus = state.room_bus(room_id).await;
    for (participant_session_id, role) in participants {
        if let Ok(response) =
            room_response(&state, room_id, &participant_session_id, role, vec![]).await
        {
            let _ = bus.send(ServerMessage {
                room_id: Some(room_id.to_string()),
                participant_session_id: Some(participant_session_id.clone()),
                role: Some(role.as_str().to_string()),
                observation: response.observation,
                available_actions: response.available_actions,
                events: response.events,
                conversation: response.conversation,
                ..ServerMessage::new("stateChanged")
            });
        }
    }
}

async fn maybe_start_room_agents<A: GameAdapter>(state: Arc<AppState<A>>, room_id: &str)
where
    A::State: Serialize,
{
    let agents = {
        let memory = state.memory.read().await;
        memory
            .rooms
            .get(room_id)
            .filter(|room| room_ready_for_game::<A>(&state.config, room))
            .map(|room| {
                room.participants
                    .values()
                    .filter(|participant| participant.source == "agent")
                    .map(|participant| {
                        (participant.participant_session_id.clone(), participant.role)
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };
    for (participant_session_id, role) in agents {
        maybe_start_agent(
            state.clone(),
            room_id.to_string(),
            participant_session_id,
            role,
        )
        .await;
    }
}

// Converts one agent message to speech immediately after the agent emits it.
async fn speak_agent_message<A: GameAdapter>(
    state: &Arc<AppState<A>>,
    room_id: &str,
    message: &ConversationMessageResponse,
) {
    let Some(provider) = state.tts_provider.clone() else {
        return;
    };
    persist_tts_diagnostic(
        state,
        room_id,
        "tts_message_started",
        json!({"message_id": message.id, "text": message.text}),
    )
    .await;
    match provider.synthesize(&message.text, &message.id).await {
        Ok(chunks) => {
            let mut saw_audio = false;
            for chunk in &chunks {
                if !chunk.data.is_empty() && !saw_audio {
                    saw_audio = true;
                    persist_tts_diagnostic(
                        state,
                        room_id,
                        "tts_first_audio",
                        json!({
                            "message_id": message.id,
                            "sample_rate": chunk.sample_rate,
                            "channels": chunk.channels,
                        }),
                    )
                    .await;
                }
                persist_tts_diagnostic(
                    state,
                    room_id,
                    "tts_audio_chunk",
                    json!({
                        "message_id": message.id,
                        "bytes": chunk.data.len(),
                        "sample_rate": chunk.sample_rate,
                        "channels": chunk.channels,
                        "final": chunk.final_chunk,
                    }),
                )
                .await;
            }
            if let Some(publisher) = state.audio_publisher.clone() {
                persist_tts_diagnostic(
                    state,
                    room_id,
                    "tts_publish_started",
                    json!({"message_id": message.id}),
                )
                .await;
                match publisher.publish(room_id, &message.id, &chunks).await {
                    Ok(summary) => {
                        persist_tts_diagnostic(
                            state,
                            room_id,
                            "tts_publish_completed",
                            json!({
                                "message_id": message.id,
                                "chunks": summary.chunks_published,
                                "bytes": summary.bytes_published,
                                "sample_rate": summary.sample_rate,
                                "channels": summary.channels,
                            }),
                        )
                        .await;
                    }
                    Err(error) => {
                        persist_tts_diagnostic(
                            state,
                            room_id,
                            "tts_publish_failed",
                            json!({"message_id": message.id, "error": error.to_string()}),
                        )
                        .await;
                    }
                }
            }
            persist_tts_diagnostic(
                state,
                room_id,
                "tts_message_completed",
                json!({"message_id": message.id}),
            )
            .await;
        }
        Err(error) => {
            persist_tts_diagnostic(
                state,
                room_id,
                "tts_message_failed",
                json!({"message_id": message.id, "error": error.to_string()}),
            )
            .await;
        }
    }
}

// Persists one TTS diagnostic event into the evaluation event stream.
async fn persist_tts_diagnostic<A: GameAdapter>(
    state: &Arc<AppState<A>>,
    room_id: &str,
    event: &str,
    metadata: Value,
) {
    persist_session_event(
        state,
        room_id,
        None,
        "tts_diagnostic",
        json!({
            "event": event,
            "metadata": metadata,
            "created_at": now_iso(),
        }),
        None,
    )
    .await;
}

async fn maybe_start_agent<A: GameAdapter>(
    state: Arc<AppState<A>>,
    room_id: String,
    participant_session_id: String,
    role: Seat,
) where
    A::State: Serialize,
{
    let Some(factory) = state.agent_factory.clone() else {
        return;
    };
    let key = format!("{room_id}:{participant_session_id}");
    if !state.started_agents.write().await.insert(key) {
        return;
    }
    tokio::spawn(async move {
        let role_name = role.as_str().to_string();
        let agent_identity = factory.participant_identity();
        let agent_metadata = agent_identity.metadata.clone();
        {
            let mut memory = state.memory.write().await;
            if let Some(room) = memory.rooms.get_mut(&room_id) {
                if let Some(participant) = room.participants.get_mut(&participant_session_id) {
                    participant.connected = true;
                    participant.updated_at = now_iso();
                }
            }
        }
        persist_event(
            "agent_connected",
            state.store.update_session_participant_connection(
                &participant_session_id,
                "connected",
                None,
            ),
        )
        .await;
        persist_session_event(
            &state,
            &room_id,
            Some(&participant_session_id),
            "participant_connected",
            json!({
                "source": "agent",
                "agent": agent_event_metadata(&agent_metadata),
            }),
            None,
        )
        .await;
        let context = AgentInitContext {
            role: role_name.clone(),
            seed: state
                .config
                .agents
                .human_vs_agent
                .as_ref()
                .and_then(|c| c.seed),
            config: state
                .config
                .agents
                .human_vs_agent
                .as_ref()
                .map(|c| c.config.clone())
                .unwrap_or(Value::Null),
        };
        let Ok(mut agent) = factory.create(context) else {
            return;
        };
        persist_session_event(
            &state,
            &room_id,
            Some(&participant_session_id),
            "agent_started",
            json!({
                "role": role_name,
                "agent": agent_event_metadata(&agent_metadata),
            }),
            None,
        )
        .await;
        let mut invalid_actions = 0usize;
        let mut last_error = None;
        loop {
            tokio::time::sleep(Duration::from_millis(250)).await;
            let (observation, available_actions, completed) = {
                let memory = state.memory.read().await;
                let Some(room) = memory.rooms.get(&room_id) else {
                    break;
                };
                let player = role.player_role();
                (
                    state.adapter.observe_state(&room.state, player),
                    state.adapter.available_actions(&room.state, player),
                    state.adapter.is_complete(&room.state),
                )
            };
            if completed {
                break;
            }
            let timeout = state
                .config
                .agents
                .human_vs_agent
                .as_ref()
                .map(|c| c.act_timeout_seconds)
                .unwrap_or(10.0);
            let result = tokio::time::timeout(
                Duration::from_secs_f64(timeout),
                agent.act(observation, available_actions),
            )
            .await;
            let result = match result {
                Ok(Ok(result)) => result,
                Ok(Err(error)) => {
                    last_error = Some(error.to_string());
                    invalid_actions += 1;
                    continue;
                }
                Err(_) => {
                    last_error = Some("agent act timeout".to_string());
                    invalid_actions += 1;
                    continue;
                }
            };
            let (message, action) = match result {
                AgentResult::None => (None, None),
                AgentResult::Action(action) => (None, Some(action)),
                AgentResult::Message(message) => (Some(message), None),
                AgentResult::ActionWithMessage { action, message } => (Some(message), Some(action)),
            };
            if let Some(text) = message {
                if let Ok(Json(message)) = add_conversation(
                    State(state.clone()),
                    Path(room_id.clone()),
                    Json(ConversationMessageIn {
                        text,
                        origin: "agent".to_string(),
                        source_message_id: None,
                        metadata: json!({"sender_participant_session_id": participant_session_id}),
                    }),
                )
                .await
                {
                    speak_agent_message(&state, &room_id, &message).await;
                }
            }
            if let Some(action) = action {
                persist_session_event(
                    &state,
                    &room_id,
                    Some(&participant_session_id),
                    "agent_action",
                    json!({"action": protocol_json(&action).unwrap_or(Value::Null)}),
                    None,
                )
                .await;
                match submit_action(
                    state.clone(),
                    &room_id,
                    &participant_session_id,
                    role,
                    action,
                )
                .await
                {
                    Ok((completed, summary)) => {
                        broadcast_player_views(state.clone(), &room_id).await;
                        if completed {
                            let _ = state.room_bus(&room_id).await.send(ServerMessage {
                                room_id: Some(room_id.clone()),
                                participant_session_id: None,
                                summary,
                                ..ServerMessage::new("completed")
                            });
                            break;
                        }
                    }
                    Err(error) => {
                        invalid_actions += 1;
                        last_error = Some(error.to_string());
                    }
                }
            }
            let limit = state
                .config
                .agents
                .human_vs_agent
                .as_ref()
                .map(|c| c.invalid_action_limit)
                .unwrap_or(3);
            if invalid_actions >= limit {
                persist_session_event(
                    &state,
                    &room_id,
                    Some(&participant_session_id),
                    "agent_error",
                    json!({"last_error": last_error}),
                    None,
                )
                .await;
                break;
            }
        }
    });
}

async fn require_consent<A: GameAdapter>(
    state: &Arc<AppState<A>>,
    participant_session_id: &str,
) -> Result<(), AppError> {
    let memory = state.memory.read().await;
    let participant = memory
        .participants
        .get(participant_session_id)
        .ok_or_else(|| AppError::not_found("Participant session not found."))?;
    if participant.source == "direct" && state.config.direct.require_consent {
        let missing = state
            .config
            .direct
            .consents
            .iter()
            .filter(|item| item.required)
            .filter(|item| {
                !participant
                    .consent_decisions
                    .get(&item.id)
                    .copied()
                    .unwrap_or(false)
            })
            .map(|item| item.title.clone())
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            return Err(AppError::forbidden(format!(
                "Consent is required before entering the game: {}.",
                missing.join(", ")
            )));
        }
    }
    Ok(())
}

async fn require_room<A: GameAdapter>(
    state: &Arc<AppState<A>>,
    room_id: &str,
) -> Result<(), AppError> {
    if state.memory.read().await.rooms.contains_key(room_id) {
        Ok(())
    } else {
        Err(AppError::not_found("Room not found."))
    }
}

async fn participant_role<A: GameAdapter>(
    state: &Arc<AppState<A>>,
    room_id: &str,
    participant_session_id: &str,
) -> Result<Seat, AppError> {
    let memory = state.memory.read().await;
    let room = memory
        .rooms
        .get(room_id)
        .ok_or_else(|| AppError::not_found("Room not found."))?;
    room.participants
        .get(participant_session_id)
        .map(|participant| participant.role)
        .ok_or_else(|| AppError::forbidden("Participant is not in this room."))
}

fn next_role<S>(room: &GameRoom<S>) -> Option<Seat> {
    let roles = room
        .participants
        .values()
        .map(|p| p.role)
        .collect::<HashSet<_>>();
    if !roles.contains(&Seat::A) {
        Some(Seat::A)
    } else if !roles.contains(&Seat::B) {
        Some(Seat::B)
    } else {
        None
    }
}

/// Returns whether room start must wait for Speechmatics setup to finish.
fn speechmatics_readiness_required(config: &ExperimentConfig) -> bool {
    config.transcription.enabled && config.transcription.provider == "speechmatics"
}

/// Returns whether one room participant can take part in game progression.
fn participant_ready_for_game<A: GameAdapter>(
    config: &ExperimentConfig,
    participant: &RoomParticipant,
) -> bool {
    participant.connected
        && (participant.source == "agent"
            || participant.source == "worker"
            || !speechmatics_readiness_required(config)
            || participant.audio_ready)
}

/// Returns whether both player roles are connected and past required audio setup.
fn room_ready_for_game<A: GameAdapter>(
    config: &ExperimentConfig,
    room: &GameRoom<A::State>,
) -> bool {
    let ready_roles = room
        .participants
        .values()
        .filter(|participant| participant_ready_for_game::<A>(config, participant))
        .map(|participant| participant.role.player_role())
        .collect::<HashSet<_>>();
    ready_roles == HashSet::from([PlayerRole::A, PlayerRole::B])
}

/// Marks one participant's room-local audio/STT setup as complete.
async fn mark_participant_audio_ready<A: GameAdapter>(
    state: &Arc<AppState<A>>,
    room_id: &str,
    participant_session_id: &str,
) where
    A::State: Serialize,
{
    let changed = {
        let mut memory = state.memory.write().await;
        memory
            .rooms
            .get_mut(room_id)
            .and_then(|room| room.participants.get_mut(participant_session_id))
            .map(|participant| {
                let changed = !participant.audio_ready;
                participant.audio_ready = true;
                participant.updated_at = now_iso();
                changed
            })
            .unwrap_or(false)
    };
    if changed {
        persist_session_event(
            state,
            room_id,
            Some(participant_session_id),
            "voice_diagnostic",
            json!({"event": "stt_initialized"}),
            None,
        )
        .await;
    }
}

/// Starts a room once all humans/agents and required audio setup are ready.
async fn maybe_start_game<A: GameAdapter>(state: Arc<AppState<A>>, room_id: &str)
where
    A::State: Serialize,
{
    let should_start = {
        let mut memory = state.memory.write().await;
        let Some(room) = memory.rooms.get_mut(room_id) else {
            return;
        };
        if room.status != "waiting" || !room_ready_for_game::<A>(&state.config, room) {
            false
        } else {
            room.status = "playing".to_string();
            room.updated_at = now_iso();
            true
        }
    };
    if !should_start {
        return;
    }

    let participants = {
        let memory = state.memory.read().await;
        memory
            .rooms
            .get(room_id)
            .map(|room| {
                room.participants
                    .values()
                    .map(|participant| {
                        (participant.participant_session_id.clone(), participant.role)
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };
    for (participant_session_id, role) in participants {
        send_role_assignment(&state, room_id, &participant_session_id, role).await;
    }
    maybe_start_room_agents(state, room_id).await;
}

/// Returns whether a room has already left the waiting-room phase.
async fn room_has_started<A: GameAdapter>(state: &Arc<AppState<A>>, room_id: &str) -> bool {
    let memory = state.memory.read().await;
    memory
        .rooms
        .get(room_id)
        .is_some_and(|room| room.status != "waiting")
}

/// Sends the targeted game-start payload for one participant.
async fn send_role_assignment<A: GameAdapter>(
    state: &Arc<AppState<A>>,
    room_id: &str,
    participant_session_id: &str,
    role: Seat,
) where
    A::State: Serialize,
{
    if let Ok(response) = room_response(state, room_id, participant_session_id, role, vec![]).await
    {
        let bus = state.room_bus(room_id).await;
        let _ = bus.send(ServerMessage {
            room_id: Some(room_id.to_string()),
            participant_session_id: Some(participant_session_id.to_string()),
            role: Some(role.as_str().to_string()),
            observation: response.observation,
            available_actions: response.available_actions,
            events: response.events,
            conversation: response.conversation,
            ..ServerMessage::new("roleAssigned")
        });
    }
}

async fn presence_message<A: GameAdapter>(
    state: &Arc<AppState<A>>,
    room_id: &str,
) -> Option<ServerMessage> {
    let memory = state.memory.read().await;
    let room = memory.rooms.get(room_id)?;
    Some(ServerMessage {
        room_id: Some(room_id.to_string()),
        presence: Some(json!(room
            .participants
            .values()
            .map(|participant| {
                (
                    participant.role.as_str().to_string(),
                    json!({
                        "participantSessionId": participant.participant_session_id,
                        "connected": participant.connected,
                        "audioReady": participant.audio_ready,
                    }),
                )
            })
            .collect::<serde_json::Map<_, _>>())),
        ..ServerMessage::new("presenceChanged")
    })
}

async fn voice_message<A: GameAdapter>(state: &Arc<AppState<A>>, room_id: &str) -> ServerMessage {
    let (audio_ready, game_ready) = state
        .memory
        .read()
        .await
        .rooms
        .get(room_id)
        .map(|room| {
            let connected_roles = room
                .participants
                .values()
                .filter(|participant| participant.connected)
                .map(|participant| participant.role.player_role())
                .collect::<HashSet<_>>();
            (
                connected_roles == HashSet::from([PlayerRole::A, PlayerRole::B]),
                room_ready_for_game::<A>(&state.config, room),
            )
        })
        .unwrap_or((false, false));
    let transcription_ready = !state.config.transcription.enabled || game_ready;
    ServerMessage {
        room_id: Some(room_id.to_string()),
        voice: Some(json!({
            "audioReady": audio_ready,
            "transcriptionReady": transcription_ready,
            "transcriptionStatus": if !state.config.transcription.enabled {
                "Disabled"
            } else if transcription_ready {
                "Ready"
            } else {
                "Initializing"
            },
        })),
        ..ServerMessage::new("voiceStatusChanged")
    }
}

fn error_message(room_id: &str, message: &str) -> ServerMessage {
    ServerMessage {
        room_id: Some(room_id.to_string()),
        message: Some(message.to_string()),
        ..ServerMessage::new("error")
    }
}

fn protocol_json<T: Serialize>(value: &T) -> Result<Value> {
    Ok(serde_json::to_value(value)?)
}

#[cfg(test)]
mod tests {
    use anyhow::Result;
    use async_trait::async_trait;
    use axum::{
        body::{to_bytes, Body},
        http::{Request, StatusCode},
    };
    use futures_util::{SinkExt as _, StreamExt as _};
    use serde::{Deserialize, Serialize};
    use serde_json::{json, Value};
    use std::{
        collections::VecDeque,
        fs,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Mutex,
        },
    };
    use tokio::net::TcpListener;
    use tokio_tungstenite::{connect_async, tungstenite::Message as TungsteniteMessage};
    use tower::ServiceExt;

    use crate::agents::{AgentInitContext, GameAgent};
    use crate::config::{
        AgentsConfig, AgentsMode, DatabaseConfig, DirectConfig, ExperimentIdentityConfig,
    };
    use crate::game::PlayerRole;

    use super::*;

    fn admin_test_event(index: i64, event_type: &str, role: Option<&str>, payload: Value) -> Value {
        let stored = crate::storage::StoredSessionEvent {
            event_id: index,
            experiment_id: "experiment".to_string(),
            session_id: 1,
            event_index: index,
            event_type: event_type.to_string(),
            actor_participant_id: None,
            actor_role: role.map(str::to_string),
            payload,
            game_state: None,
            created_at: format!("2026-07-11T20:15:{index:02}.000000+00:00"),
        };
        admin_event_summary(stored)
    }

    #[test]
    fn admin_event_bundles_close_ready_before_later_disconnect() {
        let events = vec![
            admin_test_event(1, "session_created", None, json!({"room_id": "571EBA"})),
            admin_test_event(2, "participant_joined", Some("A"), json!({"role": "A"})),
            admin_test_event(3, "participant_joined", Some("B"), json!({"role": "B"})),
            admin_test_event(4, "participant_connected", Some("A"), Value::Null),
            admin_test_event(5, "ready", Some("A"), Value::Null),
            admin_test_event(
                8,
                "participant_connected",
                Some("B"),
                json!({"source": "agent"}),
            ),
            admin_test_event(133, "participant_disconnected", Some("A"), Value::Null),
        ];

        let bundles = admin_event_bundles(&events);

        let a_ready = bundles
            .iter()
            .find(|bundle| {
                bundle["kind"] == "participant"
                    && bundle["role"] == "A"
                    && bundle["first_index"] == 2
            })
            .unwrap();
        assert_eq!(a_ready["first_index"], 2);
        assert_eq!(a_ready["last_index"], 5);
        assert_eq!(a_ready["problem"], false);
        assert_eq!(a_ready["housekeeping"], true);

        let a_disconnect = bundles
            .iter()
            .find(|bundle| {
                bundle["kind"] == "participant"
                    && bundle["role"] == "A"
                    && bundle["first_index"] == 133
            })
            .unwrap();
        assert_eq!(a_disconnect["first_index"], 133);
        assert_eq!(a_disconnect["last_index"], 133);
        assert_eq!(a_disconnect["problem"], true);
        assert_eq!(a_disconnect["title"], "Participant");
    }

    #[test]
    fn admin_event_bundles_merge_interleaved_action_events_by_action() {
        let action = json!({"type": "moveStep", "player": "B", "direction": "down"});
        let events = vec![
            admin_test_event(70, "agent_action", Some("B"), json!({"action": action})),
            admin_test_event(
                71,
                "game_action_accepted",
                Some("B"),
                json!({"action": {"type": "moveStep", "player": "B", "direction": "down"}}),
            ),
            admin_test_event(
                72,
                "conversation_message",
                Some("A"),
                json!({"origin": "voice_transcript", "sender_role": "A", "text": "Guten Abend."}),
            ),
        ];

        let bundles = admin_event_bundles(&events);

        let accepted = bundles
            .iter()
            .find(|bundle| bundle["title"] == "Action" && bundle["role"] == "B")
            .unwrap();
        assert_eq!(accepted["first_index"], 70);
        assert_eq!(accepted["last_index"], 71);
        assert_eq!(accepted["problem"], false);
        assert_eq!(accepted["housekeeping"], false);
        assert_eq!(accepted["action"]["type"], "moveStep");
        assert_eq!(accepted["action"]["direction"], "down");
    }

    #[test]
    fn admin_event_bundles_hide_routine_voice_rows_but_keep_setup_status() {
        let events = vec![
            admin_test_event(
                6,
                "voice_diagnostic",
                Some("A"),
                json!({"event": "voice_connect_requested"}),
            ),
            admin_test_event(
                7,
                "voice_diagnostic",
                Some("A"),
                json!({"event": "stt_initialized"}),
            ),
            admin_test_event(
                10,
                "voice_diagnostic",
                Some("A"),
                json!({"event": "voice_token_received"}),
            ),
            admin_test_event(
                11,
                "voice_diagnostic",
                Some("A"),
                json!({"event": "transcription_stream_connecting"}),
            ),
            admin_test_event(
                15,
                "voice_diagnostic",
                Some("A"),
                json!({"event": "transcription_stream_started"}),
            ),
            admin_test_event(
                18,
                "voice_diagnostic",
                Some("A"),
                json!({"event": "local_track_published"}),
            ),
        ];

        let bundles = admin_event_bundles(&events);

        assert!(bundles.iter().all(|bundle| !bundle["steps"]
            .as_str()
            .unwrap_or("")
            .contains("voice_token_received")));
        assert!(bundles.iter().all(|bundle| !bundle["steps"]
            .as_str()
            .unwrap_or("")
            .contains("local_track_published")));
        let voice_bundles = bundles
            .iter()
            .filter(|bundle| bundle["kind"] == "voice" && bundle["role"] == "A")
            .collect::<Vec<_>>();
        assert_eq!(voice_bundles.len(), 1);
        let stream_bundle = voice_bundles[0];
        assert_eq!(stream_bundle["first_index"], 6);
        assert_eq!(stream_bundle["last_index"], 15);
        assert_eq!(stream_bundle["title"], "Voice");
        assert_eq!(stream_bundle["problem"], false);
        assert_eq!(stream_bundle["problem_reason"], Value::Null);
        assert_eq!(stream_bundle["housekeeping"], true);
    }

    #[test]
    fn admin_event_bundles_keep_late_teardown_after_completion_non_problematic() {
        let events = vec![
            admin_test_event(131, "session_completed", Some("B"), json!({"done": true})),
            admin_test_event(133, "participant_disconnected", Some("A"), Value::Null),
            admin_test_event(
                134,
                "voice_diagnostic",
                Some("A"),
                json!({"event": "transcription_stream_disconnected"}),
            ),
            admin_test_event(
                135,
                "voice_diagnostic",
                Some("A"),
                json!({"event": "livekit_disconnected"}),
            ),
        ];

        let bundles = admin_event_bundles(&events);

        let participant_teardown = bundles
            .iter()
            .find(|bundle| bundle["kind"] == "participant" && bundle["first_index"] == 133)
            .unwrap();
        assert_eq!(participant_teardown["problem"], false);
        assert_eq!(participant_teardown["problem_reason"], Value::Null);

        let voice_teardown = bundles
            .iter()
            .find(|bundle| bundle["kind"] == "voice" && bundle["first_index"] == 134)
            .unwrap();
        assert_eq!(voice_teardown["problem"], false);
        assert_eq!(voice_teardown["problem_reason"], Value::Null);
    }

    #[test]
    fn admin_event_bundles_do_not_flag_pending_agent_actions_after_disconnect() {
        let events = vec![
            admin_test_event(133, "participant_disconnected", Some("A"), Value::Null),
            admin_test_event(
                136,
                "agent_action",
                Some("B"),
                json!({"action": {"type": "moveStep", "player": "B", "direction": "down"}}),
            ),
            admin_test_event(
                137,
                "agent_action",
                Some("B"),
                json!({"action": {"type": "moveStep", "player": "B", "direction": "up"}}),
            ),
            admin_test_event(
                138,
                "agent_action",
                Some("B"),
                json!({"action": {"type": "moveStep", "player": "B", "direction": "down"}}),
            ),
        ];

        let bundles = admin_event_bundles(&events);

        let disconnect = bundles
            .iter()
            .find(|bundle| bundle["first_index"] == 133)
            .unwrap();
        assert_eq!(disconnect["problem"], true);

        let late_actions = bundles
            .iter()
            .filter(|bundle| bundle["kind"] == "action")
            .collect::<Vec<_>>();
        assert_eq!(late_actions.len(), 3);
        assert!(late_actions
            .iter()
            .all(|bundle| bundle["problem"] == false && bundle["problem_reason"] == Value::Null));
    }

    #[test]
    fn admin_event_bundles_merge_transcript_storage_and_display_rows() {
        let events = vec![
            admin_test_event(
                73,
                "transcript_segment",
                Some("A"),
                json!({"player": "A", "text": "Guten Abend."}),
            ),
            admin_test_event(
                74,
                "conversation_message",
                Some("A"),
                json!({"origin": "voice_transcript", "sender_role": "A", "text": "Guten Abend"}),
            ),
        ];

        let bundles = admin_event_bundles(&events);

        assert_eq!(bundles.len(), 1);
        assert_eq!(bundles[0]["title"], "Voice Message");
        assert_eq!(bundles[0]["first_index"], 73);
        assert_eq!(bundles[0]["last_index"], 74);
        assert_eq!(bundles[0]["text"], "Guten Abend.");
    }

    #[derive(Clone, Debug, Deserialize, Serialize)]
    struct TinyState {
        done: bool,
    }

    #[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
    struct TinyAction {
        finish: bool,
        #[serde(default)]
        invalid: bool,
    }

    #[derive(Clone, Debug, Serialize)]
    struct TinyObservation {
        done: bool,
        role: String,
    }

    #[derive(Clone, Debug, Serialize)]
    struct TinyEvent {
        name: String,
    }

    #[derive(Clone, Debug, Serialize)]
    struct TinySummary {
        done: bool,
    }

    #[derive(Clone)]
    struct TinyAdapter;

    #[derive(Clone)]
    struct NoAvailableActionsAdapter;

    struct NoopAgent;

    #[async_trait]
    impl GameAgent<TinyAdapter> for NoopAgent {
        async fn act(
            &mut self,
            _observation: TinyObservation,
            _available_actions: Option<Vec<TinyAction>>,
        ) -> Result<AgentResult<TinyAction>> {
            Ok(AgentResult::None)
        }
    }

    struct NoopAgentFactory;

    impl AgentFactory<TinyAdapter> for NoopAgentFactory {
        fn create(
            &self,
            _context: AgentInitContext,
        ) -> Result<Box<dyn GameAgent<TinyAdapter> + Send>> {
            Ok(Box::new(NoopAgent))
        }
    }

    struct ScriptedAgent {
        script: VecDeque<AgentResult<TinyAction>>,
    }

    #[async_trait]
    impl GameAgent<TinyAdapter> for ScriptedAgent {
        async fn act(
            &mut self,
            _observation: TinyObservation,
            _available_actions: Option<Vec<TinyAction>>,
        ) -> Result<AgentResult<TinyAction>> {
            Ok(self.script.pop_front().unwrap_or(AgentResult::None))
        }
    }

    struct ScriptedAgentFactory {
        created: AtomicUsize,
        scripts: Mutex<VecDeque<Vec<AgentResult<TinyAction>>>>,
    }

    impl ScriptedAgentFactory {
        // Creates a factory that hands one script to each fresh agent instance.
        fn new(scripts: Vec<Vec<AgentResult<TinyAction>>>) -> Self {
            Self {
                created: AtomicUsize::new(0),
                scripts: Mutex::new(scripts.into()),
            }
        }

        // Returns how many agent instances were created by the server runtime.
        fn created_count(&self) -> usize {
            self.created.load(Ordering::SeqCst)
        }
    }

    impl AgentFactory<TinyAdapter> for ScriptedAgentFactory {
        fn create(
            &self,
            _context: AgentInitContext,
        ) -> Result<Box<dyn GameAgent<TinyAdapter> + Send>> {
            self.created.fetch_add(1, Ordering::SeqCst);
            let script = self.scripts.lock().unwrap().pop_front().unwrap_or_default();
            Ok(Box::new(ScriptedAgent {
                script: script.into(),
            }))
        }
    }

    struct RecordingActionsAgent {
        seen_actions: Arc<Mutex<Vec<Option<Vec<TinyAction>>>>>,
    }

    #[async_trait]
    impl GameAgent<TinyAdapter> for RecordingActionsAgent {
        async fn act(
            &mut self,
            _observation: TinyObservation,
            available_actions: Option<Vec<TinyAction>>,
        ) -> Result<AgentResult<TinyAction>> {
            self.seen_actions.lock().unwrap().push(available_actions);
            Ok(AgentResult::None)
        }
    }

    struct RecordingActionsAgentFactory {
        seen_actions: Arc<Mutex<Vec<Option<Vec<TinyAction>>>>>,
    }

    impl AgentFactory<TinyAdapter> for RecordingActionsAgentFactory {
        fn create(
            &self,
            _context: AgentInitContext,
        ) -> Result<Box<dyn GameAgent<TinyAdapter> + Send>> {
            Ok(Box::new(RecordingActionsAgent {
                seen_actions: self.seen_actions.clone(),
            }))
        }
    }

    struct MockTtsProvider {
        calls: AtomicUsize,
        fail_first: bool,
    }

    struct MockAudioPublisher {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl StreamingTtsProvider for MockTtsProvider {
        async fn synthesize(
            &self,
            _text: &str,
            _message_id: &str,
        ) -> Result<Vec<crate::tts::AudioChunk>> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fail_first && call == 0 {
                return Err(anyhow!("mock tts failure"));
            }
            Ok(vec![
                crate::tts::AudioChunk {
                    data: vec![1, 2, 3],
                    sample_rate: 24000,
                    channels: 1,
                    final_chunk: false,
                },
                crate::tts::AudioChunk {
                    data: vec![],
                    sample_rate: 24000,
                    channels: 1,
                    final_chunk: true,
                },
            ])
        }
    }

    #[async_trait]
    impl crate::audio_publisher::AgentAudioPublisher for MockAudioPublisher {
        async fn publish(
            &self,
            _room_id: &str,
            _message_id: &str,
            chunks: &[crate::tts::AudioChunk],
        ) -> Result<crate::audio_publisher::AudioPublishSummary> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(crate::audio_publisher::AudioPublishSummary {
                chunks_published: chunks.iter().filter(|chunk| !chunk.data.is_empty()).count(),
                bytes_published: chunks.iter().map(|chunk| chunk.data.len()).sum(),
                sample_rate: 24000,
                channels: 1,
            })
        }
    }

    impl GameAdapter for TinyAdapter {
        type State = TinyState;
        type Action = TinyAction;
        type Observation = TinyObservation;
        type Event = TinyEvent;
        type Summary = TinySummary;

        fn initial_state(&self) -> Self::State {
            TinyState { done: false }
        }

        fn validate_action(
            &self,
            _state: &Self::State,
            action: &Self::Action,
            _player: PlayerRole,
        ) -> Result<()> {
            if action.invalid {
                return Err(anyhow!("invalid tiny action"));
            }
            Ok(())
        }

        fn apply_action(&self, _state: &Self::State, action: &Self::Action) -> Result<Self::State> {
            Ok(TinyState {
                done: action.finish,
            })
        }

        fn observe_state(&self, state: &Self::State, player: PlayerRole) -> Self::Observation {
            TinyObservation {
                done: state.done,
                role: player.as_str().to_string(),
            }
        }

        fn available_actions(
            &self,
            _state: &Self::State,
            _player: PlayerRole,
        ) -> Option<Vec<Self::Action>> {
            Some(vec![TinyAction {
                finish: true,
                invalid: false,
            }])
        }

        fn events_for_action(
            &self,
            _before: &Self::State,
            _after: &Self::State,
            _action: &Self::Action,
            _player: PlayerRole,
        ) -> Vec<Self::Event> {
            vec![TinyEvent {
                name: "acted".to_string(),
            }]
        }

        fn is_complete(&self, state: &Self::State) -> bool {
            state.done
        }

        fn completion_summary(&self, state: &Self::State) -> Self::Summary {
            TinySummary { done: state.done }
        }
    }

    impl GameAdapter for NoAvailableActionsAdapter {
        type State = TinyState;
        type Action = TinyAction;
        type Observation = TinyObservation;
        type Event = TinyEvent;
        type Summary = TinySummary;

        fn initial_state(&self) -> Self::State {
            TinyAdapter.initial_state()
        }

        fn validate_action(
            &self,
            state: &Self::State,
            action: &Self::Action,
            player: PlayerRole,
        ) -> Result<()> {
            TinyAdapter.validate_action(state, action, player)
        }

        fn apply_action(&self, state: &Self::State, action: &Self::Action) -> Result<Self::State> {
            TinyAdapter.apply_action(state, action)
        }

        fn observe_state(&self, state: &Self::State, player: PlayerRole) -> Self::Observation {
            TinyAdapter.observe_state(state, player)
        }

        fn events_for_action(
            &self,
            before: &Self::State,
            after: &Self::State,
            action: &Self::Action,
            player: PlayerRole,
        ) -> Vec<Self::Event> {
            TinyAdapter.events_for_action(before, after, action, player)
        }

        fn is_complete(&self, state: &Self::State) -> bool {
            TinyAdapter.is_complete(state)
        }

        fn completion_summary(&self, state: &Self::State) -> Self::Summary {
            TinyAdapter.completion_summary(state)
        }
    }

    #[tokio::test]
    async fn reusable_router_builds_with_typed_adapter() {
        let mut config = ExperimentConfig::default();
        config.database.url = "sqlite:///:memory:".to_string();
        let _router = build_router(
            TinyAdapter,
            config,
            ServeOptions {
                agent_factory: None,
                ..ServeOptions::default()
            },
        )
        .await
        .expect("router builds with a typed adapter");
    }

    fn step_five_config() -> ExperimentConfig {
        ExperimentConfig {
            experiment: ExperimentIdentityConfig {
                id: Some("step5".to_string()),
            },
            database: DatabaseConfig {
                url: "sqlite:///:memory:".to_string(),
            },
            direct: DirectConfig {
                enabled: true,
                allow_room_codes: true,
                allow_matchmaking: true,
                require_consent: true,
                consents: vec![crate::config::ConsentItemConfig {
                    id: "study".to_string(),
                    title: "Study".to_string(),
                    body_html: "Agree?".to_string(),
                    required: true,
                }],
            },
            ..ExperimentConfig::default()
        }
    }

    fn sqlite_config() -> (ExperimentConfig, tempfile::TempDir) {
        let temp = tempfile::tempdir().unwrap();
        let mut config = step_five_config();
        config.database.url = format!(
            "sqlite:///{}",
            temp.path().join("server-core.sqlite").display()
        );
        (config, temp)
    }

    fn livekit_enabled_config() -> ExperimentConfig {
        let mut config = step_five_config();
        config.livekit.enabled = true;
        config.livekit.url = "wss://livekit.example.test".to_string();
        config.livekit.api_key = "livekit-key".to_string();
        config.livekit.api_secret = "livekit-secret".to_string();
        config
    }

    fn human_vs_agent_config() -> ExperimentConfig {
        let mut config = step_five_config();
        config.agents = AgentsConfig {
            mode: AgentsMode::HumanVsAgent,
            human_vs_agent: Some(crate::config::HumanVsAgentConfig {
                act_timeout_seconds: 1.0,
                invalid_action_limit: 2,
                ..Default::default()
            }),
        };
        config
    }

    async fn json_request(
        router: Router,
        method: http::Method,
        path: &str,
        body: Value,
    ) -> (StatusCode, Value) {
        let response = router
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header(http::header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes)
                .unwrap_or_else(|_| json!({"raw": String::from_utf8_lossy(&bytes).to_string()}))
        };
        (status, value)
    }

    // Sends a request and returns the raw response body for static-file assertions.
    async fn raw_request(
        router: Router,
        method: http::Method,
        path: &str,
        body: Body,
    ) -> (StatusCode, String, Option<String>) {
        let response = router
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .body(body)
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let content_type = response
            .headers()
            .get(http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(ToOwned::to_owned);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (
            status,
            String::from_utf8_lossy(&bytes).to_string(),
            content_type,
        )
    }

    async fn create_direct_participant(router: Router, name: &str) -> String {
        let (status, response) = json_request(
            router,
            http::Method::POST,
            "/api/participants",
            json!({"source": "direct", "display_name": name}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        response["participant_session_id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    async fn create_participant_with_payload(router: Router, body: Value) -> String {
        let (status, response) =
            json_request(router, http::Method::POST, "/api/participants", body).await;
        assert_eq!(status, StatusCode::OK);
        response["participant_session_id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    // Starts a real local HTTP server so tests exercise the WebSocket upgrade path.
    async fn spawn_test_server(router: Router) -> (String, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        (format!("http://{addr}"), handle)
    }

    // Starts a local Speechmatics management mock that returns one temporary key.
    async fn spawn_speechmatics_key_server(key: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let router = Router::new().route(
            "/",
            axum::routing::post(move || async move { Json(json!({"key_value": key})) }),
        );
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        format!("http://{addr}")
    }

    // Starts a Speechmatics management mock that intentionally delays key minting.
    async fn spawn_delayed_speechmatics_key_server(key: &'static str, delay: Duration) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let router = Router::new().route(
            "/",
            axum::routing::post(move || async move {
                tokio::time::sleep(delay).await;
                Json(json!({"key_value": key}))
            }),
        );
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        format!("http://{addr}")
    }

    // Reads WebSocket messages until the requested server-message type appears.
    async fn read_ws_type<S>(socket: &mut S, message_type: &str) -> Value
    where
        S: futures_util::Stream<
                Item = Result<TungsteniteMessage, tokio_tungstenite::tungstenite::Error>,
            > + Unpin,
    {
        let deadline = Duration::from_secs(2);
        loop {
            let message = tokio::time::timeout(deadline, socket.next())
                .await
                .expect("timed out waiting for WebSocket message")
                .expect("WebSocket closed before expected message")
                .expect("WebSocket read failed");
            let TungsteniteMessage::Text(text) = message else {
                continue;
            };
            let value: Value = serde_json::from_str(&text).unwrap();
            if value["type"] == message_type {
                return value;
            }
        }
    }

    // Asserts that no message of the given type arrives within a short interval.
    async fn assert_no_ws_type<S>(socket: &mut S, message_type: &str)
    where
        S: futures_util::Stream<
                Item = Result<TungsteniteMessage, tokio_tungstenite::tungstenite::Error>,
            > + Unpin,
    {
        let result = tokio::time::timeout(Duration::from_millis(200), async {
            loop {
                let Some(message) = socket.next().await else {
                    return false;
                };
                let Ok(TungsteniteMessage::Text(text)) = message else {
                    continue;
                };
                let value: Value = serde_json::from_str(&text).unwrap();
                if value["type"] == message_type {
                    return true;
                }
            }
        })
        .await;
        assert!(
            !matches!(result, Ok(true)),
            "unexpected {message_type} WebSocket message"
        );
    }

    // Sends a JSON client message through a WebSocket connection.
    async fn send_ws_json<S>(socket: &mut S, value: Value)
    where
        S: futures_util::Sink<TungsteniteMessage, Error = tokio_tungstenite::tungstenite::Error>
            + Unpin,
    {
        socket
            .send(TungsteniteMessage::Text(value.to_string()))
            .await
            .unwrap();
    }

    // Creates a two-player room and returns the participant sessions plus room id.
    async fn create_joined_room(router: Router) -> (String, String, String) {
        let a = create_direct_participant(router.clone(), "A").await;
        let b = create_direct_participant(router.clone(), "B").await;
        consent_participant(router.clone(), &a).await;
        consent_participant(router.clone(), &b).await;
        let (_, created) = json_request(
            router.clone(),
            http::Method::POST,
            "/api/rooms",
            json!({"participant_session_id": a}),
        )
        .await;
        let room_id = created["room_id"].as_str().unwrap().to_string();
        let (_, _joined) = json_request(
            router,
            http::Method::POST,
            &format!("/api/rooms/{room_id}/join"),
            json!({"participant_session_id": b}),
        )
        .await;
        (
            created["participant_session_id"]
                .as_str()
                .unwrap()
                .to_string(),
            _joined["participant_session_id"]
                .as_str()
                .unwrap()
                .to_string(),
            room_id,
        )
    }

    // Creates one human-vs-agent room through matchmaking and returns the human and room ids.
    async fn create_human_vs_agent_room(router: Router, name: &str) -> (String, String) {
        let human = create_direct_participant(router.clone(), name).await;
        consent_participant(router.clone(), &human).await;
        let (status, matched) = json_request(
            router,
            http::Method::POST,
            "/api/matchmaking/join",
            json!({"participant_session_id": human}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(matched["role"], "A");
        (
            matched["participant_session_id"]
                .as_str()
                .unwrap()
                .to_string(),
            matched["room_id"].as_str().unwrap().to_string(),
        )
    }

    // Polls the evaluation export until an event type appears or the test times out.
    async fn wait_for_export_event(router: Router, event_type: &str) -> Value {
        for _ in 0..20 {
            let (status, export) = json_request(
                router.clone(),
                http::Method::GET,
                "/api/admin/export",
                Value::Null,
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            if export["session_events"]
                .as_array()
                .unwrap()
                .iter()
                .any(|event| event["event_type"] == event_type)
            {
                return export;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("timed out waiting for event type {event_type}");
    }

    // Polls the evaluation export until a TTS diagnostic event appears.
    async fn wait_for_tts_diagnostic(router: Router, diagnostic_event: &str) -> Value {
        for _ in 0..30 {
            let (status, export) = json_request(
                router.clone(),
                http::Method::GET,
                "/api/admin/export",
                Value::Null,
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            if export["session_events"]
                .as_array()
                .unwrap()
                .iter()
                .any(|event| {
                    event["event_type"] == "tts_diagnostic"
                        && event["payload"]["event"] == diagnostic_event
                })
            {
                return export;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("timed out waiting for TTS diagnostic {diagnostic_event}");
    }

    async fn consent_participant(router: Router, participant_session_id: &str) {
        let (status, _response) = json_request(
            router,
            http::Method::POST,
            "/api/consent",
            json!({
                "participant_session_id": participant_session_id,
                "decisions": {"study": true}
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn health_and_public_config_expose_client_bootstrap_shape() {
        let mut config = step_five_config();
        config.study.name = "Bootstrap Study".to_string();
        config.livekit.enabled = true;
        config.livekit.url = "wss://livekit.example.test".to_string();
        config.transcription.enabled = true;
        config.tts.enabled = true;
        config.tts.voice_id = "voice-1".to_string();
        config.tts.api_key = "tts-secret".to_string();
        config.tts.voice_name = "Agent Voice".to_string();
        let router = build_router(TinyAdapter, config, ServeOptions::default())
            .await
            .unwrap();

        let (health_status, health) =
            json_request(router.clone(), http::Method::GET, "/health", Value::Null).await;
        assert_eq!(health_status, StatusCode::OK);
        assert_eq!(health["status"], "ok");

        let (config_status, public_config) =
            json_request(router, http::Method::GET, "/api/config", Value::Null).await;
        assert_eq!(config_status, StatusCode::OK);
        assert_eq!(public_config["study_name"], "Bootstrap Study");
        assert_eq!(public_config["require_consent"], true);
        assert_eq!(public_config["consents"][0]["id"], "study");
        assert_eq!(public_config["livekit"]["enabled"], true);
        assert_eq!(
            public_config["livekit"]["url"],
            "wss://livekit.example.test"
        );
        assert_eq!(public_config["transcription"]["enabled"], true);
        assert_eq!(public_config["tts"]["voice_name"], "Agent Voice");
        assert_eq!(public_config["conversation"]["enabled"], true);
    }

    #[tokio::test]
    async fn static_serving_returns_assets_spa_fallback_and_preserves_api_prefixes() {
        let temp = tempfile::tempdir().unwrap();
        let dist = temp.path().join("dist");
        fs::create_dir_all(dist.join("assets")).unwrap();
        fs::write(dist.join("index.html"), "<main>Parlando client</main>").unwrap();
        fs::write(dist.join("assets/app.js"), "console.log('asset');").unwrap();
        fs::write(temp.path().join("secret.txt"), "do not serve me").unwrap();

        let mut config = step_five_config();
        config.server.client_dist_path = Some(dist.display().to_string());
        let router = build_router(TinyAdapter, config, ServeOptions::default())
            .await
            .unwrap();

        let (root_status, root_body, root_type) =
            raw_request(router.clone(), http::Method::GET, "/", Body::empty()).await;
        assert_eq!(root_status, StatusCode::OK);
        assert!(root_body.contains("Parlando client"));
        assert!(root_type
            .as_deref()
            .is_some_and(|value| value.contains("text/html")));

        let (asset_status, asset_body, asset_type) = raw_request(
            router.clone(),
            http::Method::GET,
            "/assets/app.js",
            Body::empty(),
        )
        .await;
        assert_eq!(asset_status, StatusCode::OK);
        assert_eq!(asset_body, "console.log('asset');");
        assert!(asset_type
            .as_deref()
            .is_some_and(|value| value.contains("javascript")));

        let (fallback_status, fallback_body, _) = raw_request(
            router.clone(),
            http::Method::GET,
            "/room/abc",
            Body::empty(),
        )
        .await;
        assert_eq!(fallback_status, StatusCode::OK);
        assert!(fallback_body.contains("Parlando client"));

        let (api_status, api_body, _) = raw_request(
            router.clone(),
            http::Method::GET,
            "/api/config",
            Body::empty(),
        )
        .await;
        assert_eq!(api_status, StatusCode::OK);
        assert!(api_body.contains("study_name"));
        assert!(!api_body.contains("Parlando client"));

        let (_traversal_status, traversal_body, _) = raw_request(
            router,
            http::Method::GET,
            "/assets/../secret.txt",
            Body::empty(),
        )
        .await;
        assert!(!traversal_body.contains("do not serve me"));
    }

    #[tokio::test]
    async fn audio_session_and_livekit_token_are_disabled_when_livekit_is_disabled() {
        let router = build_router(TinyAdapter, step_five_config(), ServeOptions::default())
            .await
            .unwrap();
        let (a, _b, room_id) = create_joined_room(router.clone()).await;

        let (token_status, token) = json_request(
            router.clone(),
            http::Method::POST,
            &format!("/api/rooms/{room_id}/livekit-token"),
            json!({"participant_session_id": a}),
        )
        .await;
        assert_eq!(token_status, StatusCode::OK);
        assert_eq!(token["enabled"], false);
        assert!(token["token"].is_null());

        let (audio_status, audio) = json_request(
            router,
            http::Method::POST,
            &format!("/api/rooms/{room_id}/audio-session"),
            json!({"participant_session_id": a}),
        )
        .await;
        assert_eq!(audio_status, StatusCode::OK);
        assert_eq!(audio["enabled"], false);
        assert!(audio["sinks"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn livekit_token_worker_token_and_combined_audio_session_match_client_shape() {
        let router = build_router(
            TinyAdapter,
            livekit_enabled_config(),
            ServeOptions::default(),
        )
        .await
        .unwrap();
        let (a, _b, room_id) = create_joined_room(router.clone()).await;

        let (token_status, token) = json_request(
            router.clone(),
            http::Method::POST,
            &format!("/api/rooms/{room_id}/livekit-token"),
            json!({"participant_session_id": a}),
        )
        .await;
        assert_eq!(token_status, StatusCode::OK);
        assert_eq!(token["enabled"], true);
        assert_eq!(token["url"], "wss://livekit.example.test");
        assert_eq!(token["identity"], format!("{room_id}:A:{a}"));
        assert!(token["token"]
            .as_str()
            .is_some_and(|value| !value.is_empty()));

        let (worker_status, worker) = json_request(
            router.clone(),
            http::Method::POST,
            &format!("/api/rooms/{room_id}/livekit-worker-token"),
            json!({"role": "transcription-worker"}),
        )
        .await;
        assert_eq!(worker_status, StatusCode::OK);
        assert_eq!(
            worker["identity"],
            format!("{room_id}:transcription-worker:worker-transcription-worker")
        );

        let (audio_status, audio) = json_request(
            router,
            http::Method::POST,
            &format!("/api/rooms/{room_id}/audio-session"),
            json!({"participant_session_id": a}),
        )
        .await;
        assert_eq!(audio_status, StatusCode::OK);
        assert_eq!(audio["enabled"], true);
        assert_eq!(audio["sinks"].as_array().unwrap().len(), 1);
        assert_eq!(audio["sinks"][0]["id"], "livekit-combined");
        assert_eq!(audio["sinks"][0]["provider"], "livekit");
        assert_eq!(audio["sinks"][0]["transport"], "webrtc-room");
        assert_eq!(
            audio["sinks"][0]["purposes"],
            json!(["partner-audio", "transcription"])
        );
    }

    #[tokio::test]
    async fn speechmatics_audio_session_uses_split_livekit_and_temporary_key_sinks() {
        let mut config = livekit_enabled_config();
        config.transcription.enabled = true;
        config.transcription.provider = "speechmatics".to_string();
        config.transcription.language = "de".to_string();
        config.transcription.model = "test-model".to_string();
        config.speechmatics.enabled = true;
        config.speechmatics.api_key = "permanent-key".to_string();
        config.speechmatics.management_url = spawn_speechmatics_key_server("temporary-key").await;
        config.speechmatics.realtime_url = "wss://speechmatics.example.test/v2".to_string();
        config.speechmatics.temporary_key_ttl_seconds = 321;
        let router = build_router(TinyAdapter, config, ServeOptions::default())
            .await
            .unwrap();
        let (a, _b, room_id) = create_joined_room(router.clone()).await;

        let (audio_status, audio) = json_request(
            router,
            http::Method::POST,
            &format!("/api/rooms/{room_id}/audio-session"),
            json!({"participant_session_id": a}),
        )
        .await;

        assert_eq!(audio_status, StatusCode::OK);
        assert_eq!(audio["enabled"], true);
        assert_eq!(audio["sinks"].as_array().unwrap().len(), 2);
        assert_eq!(audio["sinks"][0]["id"], "livekit-partner");
        assert_eq!(audio["sinks"][0]["purposes"], json!(["partner-audio"]));
        assert_eq!(audio["sinks"][1]["id"], "speechmatics-transcription");
        assert_eq!(audio["sinks"][1]["provider"], "speechmatics");
        assert_eq!(audio["sinks"][1]["transport"], "websocket-stt");
        assert_eq!(
            audio["sinks"][1]["credentials"]["temporary_key"],
            "temporary-key"
        );
        assert_eq!(
            audio["sinks"][1]["credentials"]["realtime_url"],
            "wss://speechmatics.example.test/v2"
        );
        assert_eq!(audio["sinks"][1]["credentials"]["language"], "de");
        assert_eq!(audio["sinks"][1]["credentials"]["model"], "test-model");
        assert_eq!(audio["sinks"][1]["credentials"]["ttl_seconds"], 321);
    }

    #[tokio::test]
    async fn direct_participant_creation_respects_direct_enabled_flag() {
        let mut config = step_five_config();
        config.direct.enabled = false;
        let router = build_router(TinyAdapter, config, ServeOptions::default())
            .await
            .unwrap();

        let (status, response) = json_request(
            router,
            http::Method::POST,
            "/api/participants",
            json!({"source": "direct", "display_name": "Blocked"}),
        )
        .await;

        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(response["raw"], "Direct mode is disabled.");
    }

    #[tokio::test]
    async fn participant_external_identity_reuses_durable_export_row() {
        let router = build_router(TinyAdapter, step_five_config(), ServeOptions::default())
            .await
            .unwrap();
        let first = create_participant_with_payload(
            router.clone(),
            json!({
                "source": "prolific",
                "external_id": "PROLIFIC-1",
                "display_name": "First",
                "metadata": {"cohort": "pilot"}
            }),
        )
        .await;
        let second = create_participant_with_payload(
            router.clone(),
            json!({
                "source": "prolific",
                "external_id": "PROLIFIC-1",
                "display_name": "Return",
                "metadata": {"cohort": "followup"}
            }),
        )
        .await;

        assert_ne!(first, second);
        let (export_status, export) =
            json_request(router, http::Method::GET, "/api/admin/export", Value::Null).await;
        assert_eq!(export_status, StatusCode::OK);
        let prolific_rows = export["participants"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| {
                row["identity_provider"] == "prolific" && row["external_id"] == "PROLIFIC-1"
            })
            .collect::<Vec<_>>();
        assert_eq!(prolific_rows.len(), 1);
    }

    #[tokio::test]
    async fn direct_room_creation_requires_consent_then_assigns_role_a() {
        let router = build_router(TinyAdapter, step_five_config(), ServeOptions::default())
            .await
            .unwrap();
        let participant_session_id =
            create_direct_participant(router.clone(), "Direct Player").await;

        let (blocked_status, _blocked) = json_request(
            router.clone(),
            http::Method::POST,
            "/api/rooms",
            json!({"participant_session_id": participant_session_id}),
        )
        .await;
        assert_eq!(blocked_status, StatusCode::FORBIDDEN);

        consent_participant(router.clone(), &participant_session_id).await;
        let (status, room) = json_request(
            router,
            http::Method::POST,
            "/api/rooms",
            json!({"participant_session_id": participant_session_id}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(room["participant_session_id"], participant_session_id);
        assert_eq!(room["role"], "A");
        assert!(room["room_id"].as_str().is_some_and(|id| !id.is_empty()));
    }

    #[tokio::test]
    async fn room_response_omits_available_actions_when_game_does_not_provide_affordance() {
        let router = build_router(
            NoAvailableActionsAdapter,
            step_five_config(),
            ServeOptions::default(),
        )
        .await
        .unwrap();
        let participant_session_id = create_direct_participant(router.clone(), "No Actions").await;
        consent_participant(router.clone(), &participant_session_id).await;

        let (status, room) = json_request(
            router,
            http::Method::POST,
            "/api/rooms",
            json!({"participant_session_id": participant_session_id, "mode": "direct"}),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert!(room.get("available_actions").is_none());
    }

    #[tokio::test]
    async fn required_consent_must_be_accepted_not_only_declared() {
        let router = build_router(TinyAdapter, step_five_config(), ServeOptions::default())
            .await
            .unwrap();
        let participant_session_id = create_direct_participant(router.clone(), "Nope").await;
        let (consent_status, _consent_response) = json_request(
            router.clone(),
            http::Method::POST,
            "/api/consent",
            json!({
                "participant_session_id": participant_session_id,
                "decisions": {"study": false}
            }),
        )
        .await;
        assert_eq!(consent_status, StatusCode::OK);

        let (room_status, response) = json_request(
            router,
            http::Method::POST,
            "/api/rooms",
            json!({"participant_session_id": participant_session_id}),
        )
        .await;

        assert_eq!(room_status, StatusCode::FORBIDDEN);
        assert!(response["raw"]
            .as_str()
            .unwrap()
            .contains("Consent is required"));
    }

    #[tokio::test]
    async fn room_join_preserves_existing_role_and_rejects_third_player() {
        let router = build_router(TinyAdapter, step_five_config(), ServeOptions::default())
            .await
            .unwrap();
        let a = create_direct_participant(router.clone(), "A").await;
        let b = create_direct_participant(router.clone(), "B").await;
        let c = create_direct_participant(router.clone(), "C").await;
        consent_participant(router.clone(), &a).await;
        consent_participant(router.clone(), &b).await;
        consent_participant(router.clone(), &c).await;

        let (_, created) = json_request(
            router.clone(),
            http::Method::POST,
            "/api/rooms",
            json!({"participant_session_id": a}),
        )
        .await;
        let room_id = created["room_id"].as_str().unwrap().to_string();

        let (join_b_status, join_b) = json_request(
            router.clone(),
            http::Method::POST,
            &format!("/api/rooms/{room_id}/join"),
            json!({"participant_session_id": b}),
        )
        .await;
        assert_eq!(join_b_status, StatusCode::OK);
        assert_eq!(join_b["role"], "B");

        let (rejoin_b_status, rejoin_b) = json_request(
            router.clone(),
            http::Method::POST,
            &format!("/api/rooms/{room_id}/join"),
            json!({"participant_session_id": b}),
        )
        .await;
        assert_eq!(rejoin_b_status, StatusCode::OK);
        assert_eq!(rejoin_b["role"], "B");

        let (join_c_status, _join_c) = json_request(
            router,
            http::Method::POST,
            &format!("/api/rooms/{room_id}/join"),
            json!({"participant_session_id": c}),
        )
        .await;
        assert_eq!(join_c_status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn adversarial_sqlite_rejoin_does_not_duplicate_session_participants() {
        let (config, _temp) = sqlite_config();
        let router = build_router(TinyAdapter, config, ServeOptions::default())
            .await
            .unwrap();
        let a = create_direct_participant(router.clone(), "A").await;
        let b = create_direct_participant(router.clone(), "B").await;
        consent_participant(router.clone(), &a).await;
        consent_participant(router.clone(), &b).await;
        let (_, created) = json_request(
            router.clone(),
            http::Method::POST,
            "/api/rooms",
            json!({"participant_session_id": a}),
        )
        .await;
        let room_id = created["room_id"].as_str().unwrap().to_string();

        for _ in 0..3 {
            let (status, joined) = json_request(
                router.clone(),
                http::Method::POST,
                &format!("/api/rooms/{room_id}/join"),
                json!({"participant_session_id": b}),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(joined["role"], "B");
        }

        let (export_status, export) =
            json_request(router, http::Method::GET, "/api/admin/export", Value::Null).await;
        assert_eq!(export_status, StatusCode::OK);
        assert_eq!(export["session_participants"].as_array().unwrap().len(), 2);
        let participant_joined_count = export["session_events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|event| event["event_type"] == "participant_joined")
            .count();
        assert_eq!(
            participant_joined_count, 2,
            "rejoining must not create a new participant_joined event"
        );
    }

    #[tokio::test]
    async fn adversarial_room_join_requires_known_participant_and_room() {
        let router = build_router(TinyAdapter, step_five_config(), ServeOptions::default())
            .await
            .unwrap();
        let participant = create_direct_participant(router.clone(), "Known").await;
        consent_participant(router.clone(), &participant).await;
        let (missing_room_status, missing_room) = json_request(
            router.clone(),
            http::Method::POST,
            "/api/rooms/not-a-room/join",
            json!({"participant_session_id": participant}),
        )
        .await;
        assert_eq!(missing_room_status, StatusCode::NOT_FOUND);
        assert_eq!(missing_room["raw"], "Room not found.");

        let (missing_participant_status, missing_participant) = json_request(
            router,
            http::Method::POST,
            "/api/rooms/not-a-room/join",
            json!({"participant_session_id": "participant-session-does-not-exist"}),
        )
        .await;
        assert_eq!(missing_participant_status, StatusCode::NOT_FOUND);
        assert_eq!(missing_participant["raw"], "Participant session not found.");
    }

    #[tokio::test]
    async fn adversarial_room_routes_reject_caller_controlled_role_fields() {
        let router = build_router(TinyAdapter, step_five_config(), ServeOptions::default())
            .await
            .unwrap();
        let a = create_direct_participant(router.clone(), "A").await;
        let b = create_direct_participant(router.clone(), "B").await;
        consent_participant(router.clone(), &a).await;
        consent_participant(router.clone(), &b).await;
        let (create_status, create_response) = json_request(
            router.clone(),
            http::Method::POST,
            "/api/rooms",
            json!({"participant_session_id": a, "role": "B"}),
        )
        .await;
        assert_eq!(create_status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(create_response["raw"]
            .as_str()
            .unwrap()
            .contains("unknown field `role`"));

        let (_, created) = json_request(
            router.clone(),
            http::Method::POST,
            "/api/rooms",
            json!({"participant_session_id": a}),
        )
        .await;
        let room_id = created["room_id"].as_str().unwrap().to_string();

        let (join_status, join_response) = json_request(
            router,
            http::Method::POST,
            &format!("/api/rooms/{room_id}/join"),
            json!({"participant_session_id": b, "role": "A"}),
        )
        .await;
        assert_eq!(join_status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(join_response["raw"]
            .as_str()
            .unwrap()
            .contains("unknown field `role`"));
    }

    #[tokio::test]
    async fn room_routes_persist_evaluation_session_and_join_events() {
        let router = build_router(TinyAdapter, step_five_config(), ServeOptions::default())
            .await
            .unwrap();
        let a = create_direct_participant(router.clone(), "A").await;
        let b = create_direct_participant(router.clone(), "B").await;
        consent_participant(router.clone(), &a).await;
        consent_participant(router.clone(), &b).await;
        let (_, created) = json_request(
            router.clone(),
            http::Method::POST,
            "/api/rooms",
            json!({"participant_session_id": a}),
        )
        .await;
        let room_id = created["room_id"].as_str().unwrap().to_string();
        let (_, _joined) = json_request(
            router.clone(),
            http::Method::POST,
            &format!("/api/rooms/{room_id}/join"),
            json!({"participant_session_id": b}),
        )
        .await;

        let (export_status, export) =
            json_request(router, http::Method::GET, "/api/admin/export", Value::Null).await;
        assert_eq!(export_status, StatusCode::OK);
        assert_eq!(export["sessions"].as_array().unwrap().len(), 1);
        assert_eq!(export["sessions"][0]["room_id"], room_id);
        assert_eq!(export["session_participants"].as_array().unwrap().len(), 2);
        let event_types = export["session_events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|event| event["event_type"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            event_types,
            vec![
                "session_created",
                "participant_joined",
                "participant_joined"
            ]
        );
    }

    #[tokio::test]
    async fn websocket_role_assignment_is_targeted_to_one_connection() {
        let router = build_router(TinyAdapter, step_five_config(), ServeOptions::default())
            .await
            .unwrap();
        let (a, b, room_id) = create_joined_room(router.clone()).await;
        let (base_url, server) = spawn_test_server(router).await;
        let host = base_url.trim_start_matches("http://");
        let (mut socket_a, _) = connect_async(format!(
            "ws://{host}/ws/game/{room_id}?participantSessionId={a}"
        ))
        .await
        .unwrap();
        assert_no_ws_type(&mut socket_a, "roleAssigned").await;

        let (mut socket_b, _) = connect_async(format!(
            "ws://{host}/ws/game/{room_id}?participantSessionId={b}"
        ))
        .await
        .unwrap();
        let assigned_a = read_ws_type(&mut socket_a, "roleAssigned").await;
        assert_eq!(assigned_a["participant_session_id"], a);
        assert_eq!(assigned_a["role"], "A");
        let assigned_b = read_ws_type(&mut socket_b, "roleAssigned").await;
        assert_eq!(assigned_b["participant_session_id"], b);
        assert_eq!(assigned_b["role"], "B");
        assert_no_ws_type(&mut socket_a, "roleAssigned").await;
        server.abort();
    }

    #[tokio::test]
    async fn websocket_rejects_actions_until_both_players_are_connected() {
        let router = build_router(TinyAdapter, step_five_config(), ServeOptions::default())
            .await
            .unwrap();
        let (a, _b, room_id) = create_joined_room(router.clone()).await;
        let (base_url, server) = spawn_test_server(router.clone()).await;
        let host = base_url.trim_start_matches("http://");
        let (mut socket_a, _) = connect_async(format!(
            "ws://{host}/ws/game/{room_id}?participantSessionId={a}"
        ))
        .await
        .unwrap();

        send_ws_json(
            &mut socket_a,
            json!({"type": "submitAction", "action": {"finish": false}}),
        )
        .await;
        let error = read_ws_type(&mut socket_a, "error").await;
        assert!(error["message"]
            .as_str()
            .unwrap()
            .contains("waiting for both players"));

        let (export_status, export) =
            json_request(router, http::Method::GET, "/api/admin/export", Value::Null).await;
        assert_eq!(export_status, StatusCode::OK);
        assert!(export["session_events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| {
                event["event_type"] == "game_action_rejected"
                    && event["payload"]["error"]
                        .as_str()
                        .unwrap()
                        .contains("waiting for both players")
            }));
        server.abort();
    }

    #[tokio::test]
    async fn human_human_game_accepts_actions_after_second_human_connects() {
        let router = build_router(TinyAdapter, step_five_config(), ServeOptions::default())
            .await
            .unwrap();
        let (a, b, room_id) = create_joined_room(router.clone()).await;
        let (base_url, server) = spawn_test_server(router).await;
        let host = base_url.trim_start_matches("http://");
        let (mut socket_a, _) = connect_async(format!(
            "ws://{host}/ws/game/{room_id}?participantSessionId={a}"
        ))
        .await
        .unwrap();
        assert_no_ws_type(&mut socket_a, "roleAssigned").await;

        send_ws_json(
            &mut socket_a,
            json!({"type": "submitAction", "action": {"finish": false}}),
        )
        .await;
        let waiting_error = read_ws_type(&mut socket_a, "error").await;
        assert!(waiting_error["message"]
            .as_str()
            .unwrap()
            .contains("waiting for both players"));

        let (mut socket_b, _) = connect_async(format!(
            "ws://{host}/ws/game/{room_id}?participantSessionId={b}"
        ))
        .await
        .unwrap();
        let _assigned_a = read_ws_type(&mut socket_a, "roleAssigned").await;
        let _assigned_b = read_ws_type(&mut socket_b, "roleAssigned").await;
        send_ws_json(
            &mut socket_a,
            json!({"type": "submitAction", "action": {"finish": true}}),
        )
        .await;
        let completed = read_ws_type(&mut socket_a, "completed").await;
        assert_eq!(completed["summary"]["done"], true);
        server.abort();
    }

    #[tokio::test]
    async fn human_agent_waits_for_speechmatics_audio_session_before_agent_start() {
        let mut config = human_vs_agent_config();
        config.livekit.enabled = true;
        config.livekit.url = "wss://livekit.example.test".to_string();
        config.livekit.api_key = "livekit-key".to_string();
        config.livekit.api_secret = "livekit-secret".to_string();
        config.transcription.enabled = true;
        config.transcription.provider = "speechmatics".to_string();
        config.speechmatics.enabled = true;
        config.speechmatics.api_key = "permanent-key".to_string();
        config.speechmatics.management_url =
            spawn_delayed_speechmatics_key_server("temporary-key", Duration::from_millis(500))
                .await;
        config.speechmatics.realtime_url = "wss://speechmatics.example.test/v2".to_string();

        let factory = Arc::new(ScriptedAgentFactory::new(vec![vec![AgentResult::Message(
            "agent starts after stt".to_string(),
        )]]));
        let router = build_router(
            TinyAdapter,
            config,
            ServeOptions {
                agent_factory: Some(factory),
                ..ServeOptions::default()
            },
        )
        .await
        .unwrap();
        let (human, room_id) = create_human_vs_agent_room(router.clone(), "Human").await;
        let (base_url, server) = spawn_test_server(router.clone()).await;
        let host = base_url.trim_start_matches("http://");
        let (mut socket, _) = connect_async(format!(
            "ws://{host}/ws/game/{room_id}?participantSessionId={human}"
        ))
        .await
        .unwrap();
        assert_no_ws_type(&mut socket, "roleAssigned").await;
        assert_no_ws_type(&mut socket, "conversationMessageAdded").await;

        let client = reqwest::Client::new();
        let audio_base_url = base_url.clone();
        let audio_room_id = room_id.clone();
        let audio_human = human.clone();
        let audio_session = tokio::spawn(async move {
            client
                .post(format!(
                    "{audio_base_url}/api/rooms/{audio_room_id}/audio-session"
                ))
                .json(&json!({"participant_session_id": audio_human}))
                .send()
                .await
                .unwrap()
                .error_for_status()
                .unwrap()
                .json::<Value>()
                .await
                .unwrap()
        });
        assert_no_ws_type(&mut socket, "conversationMessageAdded").await;
        let audio = audio_session.await.unwrap();
        assert_eq!(audio["enabled"], true);
        let assigned = read_ws_type(&mut socket, "roleAssigned").await;
        assert_eq!(assigned["role"], "A");
        let message = read_ws_type(&mut socket, "conversationMessageAdded").await;
        assert_eq!(message["conversation_message"]["origin"], "agent");
        assert_eq!(
            message["conversation_message"]["text"],
            "agent starts after stt"
        );
        server.abort();
    }

    #[tokio::test]
    async fn websocket_accepts_actions_chat_completion_and_persists_state_changes() {
        let router = build_router(TinyAdapter, step_five_config(), ServeOptions::default())
            .await
            .unwrap();
        let (a, b, room_id) = create_joined_room(router.clone()).await;
        let (base_url, server) = spawn_test_server(router.clone()).await;
        let host = base_url.trim_start_matches("http://");
        let (mut socket_a, _) = connect_async(format!(
            "ws://{host}/ws/game/{room_id}?participantSessionId={a}"
        ))
        .await
        .unwrap();
        let (mut socket_b, _) = connect_async(format!(
            "ws://{host}/ws/game/{room_id}?participantSessionId={b}"
        ))
        .await
        .unwrap();
        let _assigned_a = read_ws_type(&mut socket_a, "roleAssigned").await;
        let _assigned_b = read_ws_type(&mut socket_b, "roleAssigned").await;

        send_ws_json(
            &mut socket_a,
            json!({"type": "sendChatMessage", "text": "hello from A"}),
        )
        .await;
        let chat_a = read_ws_type(&mut socket_a, "conversationMessageAdded").await;
        let chat_b = read_ws_type(&mut socket_b, "conversationMessageAdded").await;
        assert_eq!(chat_a["conversation_message"]["text"], "hello from A");
        assert_eq!(chat_b["conversation_message"]["origin"], "typed");

        send_ws_json(
            &mut socket_a,
            json!({"type": "submitAction", "action": {"finish": false}}),
        )
        .await;
        let state_a = read_ws_type(&mut socket_a, "stateChanged").await;
        let state_b = read_ws_type(&mut socket_b, "stateChanged").await;
        assert_eq!(state_a["participant_session_id"], a);
        assert_eq!(state_a["role"], "A");
        assert_eq!(state_b["participant_session_id"], b);
        assert_eq!(state_b["role"], "B");
        assert_eq!(state_a["observation"]["done"], false);

        send_ws_json(
            &mut socket_a,
            json!({"type": "submitAction", "action": {"finish": true}}),
        )
        .await;
        let completed_a = read_ws_type(&mut socket_a, "completed").await;
        let completed_b = read_ws_type(&mut socket_b, "completed").await;
        assert_eq!(completed_a["summary"]["done"], true);
        assert_eq!(completed_b["summary"]["done"], true);

        let (export_status, export) =
            json_request(router, http::Method::GET, "/api/admin/export", Value::Null).await;
        assert_eq!(export_status, StatusCode::OK);
        let event_types = export["session_events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|event| event["event_type"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert!(event_types.contains(&"conversation_message"));
        assert!(event_types.contains(&"game_action_accepted"));
        assert!(event_types.contains(&"state_changed"));
        assert!(event_types.contains(&"session_completed"));
        server.abort();
    }

    #[tokio::test]
    async fn transcripts_and_diagnostics_persist_without_public_history_endpoints() {
        let (config, _temp) = sqlite_config();
        let router = build_router(TinyAdapter, config, ServeOptions::default())
            .await
            .unwrap();
        let (a, _b, room_id) = create_joined_room(router.clone()).await;

        let (conversation_get_status, _) = json_request(
            router.clone(),
            http::Method::GET,
            &format!("/api/rooms/{room_id}/conversation"),
            Value::Null,
        )
        .await;
        assert_eq!(conversation_get_status, StatusCode::NOT_FOUND);
        let (conversation_post_status, _) = json_request(
            router.clone(),
            http::Method::POST,
            &format!("/api/rooms/{room_id}/conversation"),
            json!({"text": "typed hello"}),
        )
        .await;
        assert_eq!(conversation_post_status, StatusCode::NOT_FOUND);
        let (transcript_get_status, _) = json_request(
            router.clone(),
            http::Method::GET,
            &format!("/api/rooms/{room_id}/transcripts"),
            Value::Null,
        )
        .await;
        assert_eq!(transcript_get_status, StatusCode::METHOD_NOT_ALLOWED);
        let (transcript_stream_status, _) = json_request(
            router.clone(),
            http::Method::GET,
            &format!("/api/rooms/{room_id}/transcripts/stream"),
            Value::Null,
        )
        .await;
        assert_eq!(transcript_stream_status, StatusCode::NOT_FOUND);
        let (transcription_context_status, _) = json_request(
            router.clone(),
            http::Method::GET,
            &format!("/api/rooms/{room_id}/transcription-context"),
            Value::Null,
        )
        .await;
        assert_eq!(transcription_context_status, StatusCode::NOT_FOUND);

        let (transcript_status, transcript) = json_request(
            router.clone(),
            http::Method::POST,
            &format!("/api/rooms/{room_id}/transcripts"),
            json!({
                "participant_session_id": a,
                "player": "B",
                "start_time_ms": 10,
                "end_time_ms": 40,
                "text": "spoken hello",
                "metadata": {"confidence": 0.9}
            }),
        )
        .await;
        assert_eq!(transcript_status, StatusCode::OK);
        assert_eq!(transcript["player"], "A");

        let (diagnostic_status, diagnostic) = json_request(
            router.clone(),
            http::Method::POST,
            &format!("/api/rooms/{room_id}/voice-diagnostics"),
            json!({
                "participant_session_id": a,
                "event": "mic_started",
                "metadata": {"device": "test"}
            }),
        )
        .await;
        assert_eq!(diagnostic_status, StatusCode::OK);
        assert_eq!(diagnostic["event"], "mic_started");

        let (export_status, export) =
            json_request(router, http::Method::GET, "/api/admin/export", Value::Null).await;
        assert_eq!(export_status, StatusCode::OK);
        let events = export["session_events"].as_array().unwrap();
        assert!(events
            .iter()
            .any(|event| event["event_type"] == "transcript_segment"));
        assert!(events.iter().any(|event| {
            event["event_type"] == "conversation_message"
                && event["payload"]["origin"] == "voice_transcript"
        }));
        assert!(events
            .iter()
            .any(|event| event["event_type"] == "voice_diagnostic"));
    }

    #[tokio::test]
    async fn admin_games_api_reads_actions_and_transcripts_from_database() {
        let (config, _temp) = sqlite_config();
        let router = build_router(TinyAdapter, config, ServeOptions::default())
            .await
            .unwrap();
        let (a, b, room_id) = create_joined_room(router.clone()).await;
        let (base_url, server) = spawn_test_server(router.clone()).await;
        let host = base_url.trim_start_matches("http://");
        let (mut socket_a, _) = connect_async(format!(
            "ws://{host}/ws/game/{room_id}?participantSessionId={a}"
        ))
        .await
        .unwrap();
        let (mut socket_b, _) = connect_async(format!(
            "ws://{host}/ws/game/{room_id}?participantSessionId={b}"
        ))
        .await
        .unwrap();
        let _assigned_a = read_ws_type(&mut socket_a, "roleAssigned").await;
        let _assigned_b = read_ws_type(&mut socket_b, "roleAssigned").await;

        send_ws_json(
            &mut socket_a,
            json!({"type": "submitAction", "action": {"finish": false}}),
        )
        .await;
        let _state_a = read_ws_type(&mut socket_a, "stateChanged").await;
        let _state_b = read_ws_type(&mut socket_b, "stateChanged").await;
        let (transcript_status, _transcript) = json_request(
            router.clone(),
            http::Method::POST,
            &format!("/api/rooms/{room_id}/transcripts"),
            json!({
                "participant_session_id": a,
                "player": "A",
                "start_time_ms": 10,
                "end_time_ms": 40,
                "text": "admin-visible transcript",
                "metadata": {"confidence": 0.95}
            }),
        )
        .await;
        assert_eq!(transcript_status, StatusCode::OK);

        let (games_status, games) = json_request(
            router.clone(),
            http::Method::GET,
            "/api/admin/games",
            Value::Null,
        )
        .await;
        assert_eq!(games_status, StatusCode::OK);
        let session_id = games["games"][0]["session_id"].as_i64().unwrap();
        assert!(games["games"][0]["event_count"].as_i64().unwrap() >= 8);

        let (detail_status, detail) = json_request(
            router.clone(),
            http::Method::GET,
            &format!("/api/admin/games/{session_id}"),
            Value::Null,
        )
        .await;
        assert_eq!(detail_status, StatusCode::OK);
        assert_eq!(detail["participants"].as_array().unwrap().len(), 2);
        let events = detail["events"].as_array().unwrap();
        assert!(events.iter().any(|event| {
            event["event_type"] == "game_action_accepted"
                && event["text"].as_str().unwrap().contains("\"finish\":false")
        }));
        assert!(events.iter().any(|event| {
            event["event_type"] == "transcript_segment"
                && event["text"] == "admin-visible transcript"
        }));

        let after_action = events
            .iter()
            .find(|event| event["event_type"] == "game_action_accepted")
            .and_then(|event| event["event_index"].as_i64())
            .unwrap();
        let (poll_status, poll) = json_request(
            router,
            http::Method::GET,
            &format!("/api/admin/games/{session_id}/events?after={after_action}"),
            Value::Null,
        )
        .await;
        assert_eq!(poll_status, StatusCode::OK);
        assert!(poll["events"].as_array().unwrap().iter().any(|event| {
            event["event_type"] == "transcript_segment"
                && event["text"] == "admin-visible transcript"
        }));
        server.abort();
    }

    #[tokio::test]
    async fn matchmaking_pairs_two_humans_into_one_room() {
        let router = build_router(TinyAdapter, step_five_config(), ServeOptions::default())
            .await
            .unwrap();
        let a = create_direct_participant(router.clone(), "A").await;
        let b = create_direct_participant(router.clone(), "B").await;
        consent_participant(router.clone(), &a).await;
        consent_participant(router.clone(), &b).await;

        let (first_status, first) = json_request(
            router.clone(),
            http::Method::POST,
            "/api/matchmaking/join",
            json!({"participant_session_id": a, "queue": "q1"}),
        )
        .await;
        assert_eq!(first_status, StatusCode::OK);
        assert_eq!(first["status"], "waiting");
        assert!(first["room_id"].is_null());
        assert!(first["role"].is_null());

        let (second_status, second) = json_request(
            router.clone(),
            http::Method::POST,
            "/api/matchmaking/join",
            json!({"participant_session_id": b, "queue": "q1"}),
        )
        .await;
        assert_eq!(second_status, StatusCode::OK);
        assert_eq!(second["status"], "matched");
        assert_eq!(second["role"], "B");
        assert!(second["room_id"].as_str().is_some());

        let (poll_status, first_matched) = json_request(
            router,
            http::Method::GET,
            &format!("/api/matchmaking/status/{a}"),
            Value::Null,
        )
        .await;
        assert_eq!(poll_status, StatusCode::OK);
        assert_eq!(first_matched["status"], "matched");
        assert_eq!(first_matched["role"], "A");
        assert_eq!(first_matched["room_id"], second["room_id"]);
    }

    #[tokio::test]
    async fn human_vs_agent_matchmaking_creates_agent_role_b() {
        let mut config = step_five_config();
        config.agents = AgentsConfig {
            mode: AgentsMode::HumanVsAgent,
            human_vs_agent: Some(Default::default()),
        };
        let router = build_router(
            TinyAdapter,
            config,
            ServeOptions {
                agent_factory: Some(Arc::new(NoopAgentFactory)),
                ..ServeOptions::default()
            },
        )
        .await
        .unwrap();
        let human = create_direct_participant(router.clone(), "Human").await;
        consent_participant(router.clone(), &human).await;

        let (status, matched) = json_request(
            router.clone(),
            http::Method::POST,
            "/api/matchmaking/join",
            json!({"participant_session_id": human}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(matched["status"], "matched");
        assert_eq!(matched["role"], "A");

        let (export_status, export) =
            json_request(router, http::Method::GET, "/api/admin/export", Value::Null).await;
        assert_eq!(export_status, StatusCode::OK);
        let roles = export["session_participants"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["role"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert!(roles.contains(&"A"));
        assert!(roles.contains(&"B"));
        assert!(export["participants"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["participant_kind"] == "agent"));
    }

    #[tokio::test]
    async fn agent_runtime_creates_one_fresh_agent_per_room() {
        let factory = Arc::new(ScriptedAgentFactory::new(vec![vec![], vec![]]));
        let router = build_router(
            TinyAdapter,
            human_vs_agent_config(),
            ServeOptions {
                agent_factory: Some(factory.clone()),
                ..ServeOptions::default()
            },
        )
        .await
        .unwrap();
        let (human_1, room_1) = create_human_vs_agent_room(router.clone(), "Human 1").await;
        let (human_2, room_2) = create_human_vs_agent_room(router.clone(), "Human 2").await;
        let (base_url, server) = spawn_test_server(router).await;
        let host = base_url.trim_start_matches("http://");
        let (mut socket_1, _) = connect_async(format!(
            "ws://{host}/ws/game/{room_1}?participantSessionId={human_1}"
        ))
        .await
        .unwrap();
        let (mut socket_2, _) = connect_async(format!(
            "ws://{host}/ws/game/{room_2}?participantSessionId={human_2}"
        ))
        .await
        .unwrap();
        let _ = read_ws_type(&mut socket_1, "roleAssigned").await;
        let _ = read_ws_type(&mut socket_2, "roleAssigned").await;

        for _ in 0..20 {
            if factory.created_count() == 2 {
                server.abort();
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        server.abort();
        panic!("expected one fresh agent for each room");
    }

    #[tokio::test]
    async fn agent_runtime_receives_same_available_action_affordance_as_ui() {
        let seen_actions = Arc::new(Mutex::new(Vec::new()));
        let router = build_router(
            TinyAdapter,
            human_vs_agent_config(),
            ServeOptions {
                agent_factory: Some(Arc::new(RecordingActionsAgentFactory {
                    seen_actions: seen_actions.clone(),
                })),
                ..ServeOptions::default()
            },
        )
        .await
        .unwrap();
        let (human, room_id) = create_human_vs_agent_room(router.clone(), "Human").await;
        let (base_url, server) = spawn_test_server(router).await;
        let host = base_url.trim_start_matches("http://");
        let (mut socket, _) = connect_async(format!(
            "ws://{host}/ws/game/{room_id}?participantSessionId={human}"
        ))
        .await
        .unwrap();
        let assigned = read_ws_type(&mut socket, "roleAssigned").await;
        let ui_available_actions = assigned["available_actions"].as_array().unwrap();
        assert_eq!(ui_available_actions.len(), 1);
        assert_eq!(ui_available_actions[0]["finish"], true);

        for _ in 0..20 {
            let captured = seen_actions.lock().unwrap().clone();
            if let Some(first_turn_actions) = captured.first() {
                assert_eq!(
                    first_turn_actions,
                    &Some(vec![TinyAction {
                        finish: true,
                        invalid: false,
                    }])
                );
                server.abort();
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        server.abort();
        panic!("expected agent to receive available actions");
    }

    #[tokio::test]
    async fn agent_runtime_persists_messages_and_validated_actions() {
        let factory = Arc::new(ScriptedAgentFactory::new(vec![vec![
            AgentResult::ActionWithMessage {
                action: TinyAction {
                    finish: true,
                    invalid: false,
                },
                message: "agent says hello".to_string(),
            },
        ]]));
        let router = build_router(
            TinyAdapter,
            human_vs_agent_config(),
            ServeOptions {
                agent_factory: Some(factory),
                ..ServeOptions::default()
            },
        )
        .await
        .unwrap();
        let (human, room_id) = create_human_vs_agent_room(router.clone(), "Human").await;
        let (base_url, server) = spawn_test_server(router.clone()).await;
        let host = base_url.trim_start_matches("http://");
        let (mut socket, _) = connect_async(format!(
            "ws://{host}/ws/game/{room_id}?participantSessionId={human}"
        ))
        .await
        .unwrap();
        let _ = read_ws_type(&mut socket, "roleAssigned").await;
        let message = read_ws_type(&mut socket, "conversationMessageAdded").await;
        assert_eq!(message["conversation_message"]["origin"], "agent");
        assert_eq!(message["conversation_message"]["text"], "agent says hello");
        let completed = read_ws_type(&mut socket, "completed").await;
        assert_eq!(completed["summary"]["done"], true);

        let export = wait_for_export_event(router, "session_completed").await;
        let events = export["session_events"].as_array().unwrap();
        assert!(events
            .iter()
            .any(|event| event["event_type"] == "agent_started"));
        assert!(events
            .iter()
            .any(|event| event["event_type"] == "agent_action"));
        assert!(events.iter().any(|event| {
            event["event_type"] == "conversation_message" && event["payload"]["origin"] == "agent"
        }));
        assert!(events
            .iter()
            .any(|event| event["event_type"] == "game_action_accepted"));
        server.abort();
    }

    #[tokio::test]
    async fn agent_runtime_stops_invalid_agents_cleanly() {
        let factory = Arc::new(ScriptedAgentFactory::new(vec![vec![
            AgentResult::Action(TinyAction {
                finish: false,
                invalid: true,
            }),
            AgentResult::Action(TinyAction {
                finish: false,
                invalid: true,
            }),
            AgentResult::Action(TinyAction {
                finish: false,
                invalid: false,
            }),
        ]]));
        let router = build_router(
            TinyAdapter,
            human_vs_agent_config(),
            ServeOptions {
                agent_factory: Some(factory),
                ..ServeOptions::default()
            },
        )
        .await
        .unwrap();
        let (human, room_id) = create_human_vs_agent_room(router.clone(), "Human").await;
        let (base_url, server) = spawn_test_server(router.clone()).await;
        let host = base_url.trim_start_matches("http://");
        let (mut socket, _) = connect_async(format!(
            "ws://{host}/ws/game/{room_id}?participantSessionId={human}"
        ))
        .await
        .unwrap();
        let _ = read_ws_type(&mut socket, "roleAssigned").await;

        let export = wait_for_export_event(router, "agent_error").await;
        let events = export["session_events"].as_array().unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| event["event_type"] == "agent_action")
                .count(),
            2
        );
        assert!(!events
            .iter()
            .any(|event| event["event_type"] == "game_action_accepted"));
        assert!(events.iter().any(|event| {
            event["event_type"] == "agent_error"
                && event["payload"]["last_error"]
                    .as_str()
                    .unwrap()
                    .contains("invalid tiny action")
        }));
        server.abort();
    }

    #[tokio::test]
    async fn agent_tts_records_diagnostics_for_agent_messages() {
        let factory = Arc::new(ScriptedAgentFactory::new(vec![vec![AgentResult::Message(
            "speak this".to_string(),
        )]]));
        let tts_provider = Arc::new(MockTtsProvider {
            calls: AtomicUsize::new(0),
            fail_first: false,
        });
        let audio_publisher = Arc::new(MockAudioPublisher {
            calls: AtomicUsize::new(0),
        });
        let router = build_router(
            TinyAdapter,
            human_vs_agent_config(),
            ServeOptions {
                agent_factory: Some(factory),
                tts_provider: Some(tts_provider),
                audio_publisher: Some(audio_publisher.clone()),
                ..ServeOptions::default()
            },
        )
        .await
        .unwrap();
        let (human, room_id) = create_human_vs_agent_room(router.clone(), "Human").await;
        let (base_url, server) = spawn_test_server(router.clone()).await;
        let host = base_url.trim_start_matches("http://");
        let (mut socket, _) = connect_async(format!(
            "ws://{host}/ws/game/{room_id}?participantSessionId={human}"
        ))
        .await
        .unwrap();
        let _ = read_ws_type(&mut socket, "roleAssigned").await;

        let export = wait_for_tts_diagnostic(router, "tts_message_completed").await;
        let diagnostics = export["session_events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|event| event["event_type"] == "tts_diagnostic")
            .map(|event| event["payload"]["event"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert!(diagnostics.contains(&"tts_message_started"));
        assert!(diagnostics.contains(&"tts_first_audio"));
        assert!(diagnostics.contains(&"tts_audio_chunk"));
        assert!(diagnostics.contains(&"tts_publish_started"));
        assert!(diagnostics.contains(&"tts_publish_completed"));
        assert!(diagnostics.contains(&"tts_message_completed"));
        assert_eq!(audio_publisher.calls.load(Ordering::SeqCst), 1);
        server.abort();
    }

    #[tokio::test]
    async fn agent_tts_continues_after_provider_failure() {
        let factory = Arc::new(ScriptedAgentFactory::new(vec![vec![
            AgentResult::Message("first fails".to_string()),
            AgentResult::Message("second succeeds".to_string()),
        ]]));
        let tts_provider = Arc::new(MockTtsProvider {
            calls: AtomicUsize::new(0),
            fail_first: true,
        });
        let router = build_router(
            TinyAdapter,
            human_vs_agent_config(),
            ServeOptions {
                agent_factory: Some(factory),
                tts_provider: Some(tts_provider),
                ..ServeOptions::default()
            },
        )
        .await
        .unwrap();
        let (human, room_id) = create_human_vs_agent_room(router.clone(), "Human").await;
        let (base_url, server) = spawn_test_server(router.clone()).await;
        let host = base_url.trim_start_matches("http://");
        let (mut socket, _) = connect_async(format!(
            "ws://{host}/ws/game/{room_id}?participantSessionId={human}"
        ))
        .await
        .unwrap();
        let _ = read_ws_type(&mut socket, "roleAssigned").await;

        let export = wait_for_tts_diagnostic(router, "tts_message_completed").await;
        let diagnostics = export["session_events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|event| event["event_type"] == "tts_diagnostic")
            .map(|event| event["payload"]["event"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert!(diagnostics.contains(&"tts_message_failed"));
        assert!(diagnostics.contains(&"tts_message_completed"));
        server.abort();
    }
}
