use std::{collections::{HashMap, HashSet}, net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};

use anyhow::{anyhow, Result};
use axum::{
    extract::{Path, Query, State, WebSocketUpgrade, ws::{Message, WebSocket}},
    http::StatusCode,
    response::{IntoResponse, Response, Sse},
    routing::{get, post},
    Json, Router,
};
use futures_util::{SinkExt, StreamExt as FuturesStreamExt};
use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::{broadcast, RwLock};
use tokio_stream::wrappers::BroadcastStream;
use tower_http::{cors::CorsLayer, services::{ServeDir, ServeFile}};

use crate::{
    agents::{AgentActContext, AgentFactory, AgentInitContext, AgentResult, SharedAgentFactory},
    config::{AgentsMode, ExperimentConfig},
    game::{GameAdapter, PlayerRole, Seat},
    identity::{new_id, room_code},
    livekit::{create_livekit_token, livekit_identity},
    protocol::*,
    speechmatics::create_speechmatics_temporary_key,
    storage::{event_store_from_url, now_iso, GameRoom, MemoryState, RoomParticipant, SharedEventStore, TimelineEvent, TranscriptSegment},
};

#[derive(Clone, Default)]
pub struct ServeOptions<A: GameAdapter> {
    pub agent_factory: Option<Arc<dyn AgentFactory<A>>>,
}

pub struct AppState<A: GameAdapter> {
    pub adapter: A,
    pub config: ExperimentConfig,
    pub memory: RwLock<MemoryState<A::State>>,
    pub events: SharedEventStore,
    pub room_buses: RwLock<HashMap<String, broadcast::Sender<ServerMessage>>>,
    pub transcript_buses: RwLock<HashMap<String, broadcast::Sender<Value>>>,
    pub agent_factory: Option<SharedAgentFactory<A>>,
    pub started_agents: RwLock<HashSet<String>>,
}

impl<A: GameAdapter> AppState<A> {
    async fn record(&self, event: TimelineEvent) {
        if let Err(error) = self.events.record(event).await {
            tracing::warn!(%error, "failed to persist timeline event");
        }
    }

    async fn room_bus(&self, room_id: &str) -> broadcast::Sender<ServerMessage> {
        let mut buses = self.room_buses.write().await;
        buses
            .entry(room_id.to_string())
            .or_insert_with(|| broadcast::channel(256).0)
            .clone()
    }

    async fn transcript_bus(&self, room_id: &str) -> broadcast::Sender<Value> {
        let mut buses = self.transcript_buses.write().await;
        buses
            .entry(room_id.to_string())
            .or_insert_with(|| broadcast::channel(256).0)
            .clone()
    }
}

#[derive(Debug)]
pub struct AppError {
    status: StatusCode,
    message: String,
}

impl AppError {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self { status, message: message.into() }
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

pub async fn build_router<A: GameAdapter>(
    adapter: A,
    config: ExperimentConfig,
    options: ServeOptions<A>,
) -> Result<Router>
where
    A::State: Serialize,
{
    let events = event_store_from_url(&config.database.url).await?;
    let client_dist = config.server.client_dist_path.as_ref().map(PathBuf::from);
    let state = Arc::new(AppState {
        adapter,
        config,
        memory: RwLock::new(MemoryState::default()),
        events,
        room_buses: RwLock::new(HashMap::new()),
        transcript_buses: RwLock::new(HashMap::new()),
        agent_factory: options.agent_factory,
        started_agents: RwLock::new(HashSet::new()),
    });

    let api = Router::new()
        .route("/health", get(health))
        .route("/api/config", get(public_config::<A>))
        .route("/api/participants", post(create_participant::<A>))
        .route("/api/direct/start", post(direct_start::<A>))
        .route("/api/direct/enter", post(direct_enter::<A>))
        .route("/api/direct/wait/:participant_session_id", get(direct_wait::<A>))
        .route("/api/consent", post(consent::<A>))
        .route("/api/rooms", post(create_room::<A>))
        .route("/api/rooms/:room_id/join", post(join_room::<A>))
        .route("/api/matchmaking/join", post(join_matchmaking::<A>))
        .route("/api/matchmaking/status/:participant_session_id", get(matchmaking_status::<A>))
        .route("/api/rooms/:room_id/livekit-token", post(livekit_token::<A>))
        .route("/api/rooms/:room_id/livekit-worker-token", post(livekit_worker_token::<A>))
        .route("/api/rooms/:room_id/audio-session", post(audio_session::<A>))
        .route("/api/rooms/:room_id/transcripts", post(add_transcript::<A>).get(get_transcripts::<A>))
        .route("/api/rooms/:room_id/transcripts/stream", get(transcript_stream::<A>))
        .route("/api/rooms/:room_id/transcription-context", get(transcription_context::<A>))
        .route("/api/rooms/:room_id/voice-diagnostics", post(add_voice_diagnostic::<A>))
        .route("/api/rooms/:room_id/conversation", get(get_conversation::<A>).post(add_conversation::<A>))
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

async fn public_config<A: GameAdapter>(State(state): State<Arc<AppState<A>>>) -> Json<PublicConfigResponse> {
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
    let request = ParticipantCreateRequest { source: "direct".to_string(), display_name: request.display_name, study_id: request.study_id };
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
    let participant = {
        let mut memory = state.memory.write().await;
        memory.create_participant(
            request.source,
            request.display_name,
            Some(request.study_id.unwrap_or_else(|| state.config.study.name.clone())),
        )
    };
    state
        .record(TimelineEvent::new(
            "participant_created",
            None,
            Some(participant.id.clone()),
            serde_json::to_value(&participant).unwrap_or(Value::Null),
        ))
        .await;
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
        ParticipantCreateRequest { source: "direct".to_string(), display_name: request.display_name, study_id: request.study_id.clone() },
    )
    .await?;
    enter_matchmaking(state, participant.participant_session_id, request.study_id).await.map(Json)
}

async fn direct_wait<A: GameAdapter>(
    State(state): State<Arc<AppState<A>>>,
    Path(participant_session_id): Path<String>,
) -> Result<Json<DirectEnterResponse>, AppError>
where
    A::State: Serialize,
{
    matchmaking_status_inner(state, &participant_session_id).await.map(Json)
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
    participant.consent_decisions.extend(request.decisions.clone());
    participant.updated_at = now_iso();
    for room in memory.rooms.values_mut() {
        if request.room_id.as_ref().is_some_and(|room_id| room_id != &room.id) {
            continue;
        }
        if let Some(room_participant) = room.participants.get_mut(&request.participant_session_id) {
            room_participant.consent_decisions.extend(request.decisions.clone());
            room_participant.updated_at = now_iso();
        }
    }
    drop(memory);
    state
        .record(TimelineEvent::new(
            "consent_updated",
            request.room_id,
            Some(request.participant_session_id),
            json!({"decisions": request.decisions}),
        ))
        .await;
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
    let force_role = parse_seat(request.force_role.as_deref()).unwrap_or(Seat::A);
    let (room_id, role) = {
        let mut memory = state.memory.write().await;
        create_room_locked(&state, &mut memory, request.participant_session_id.clone(), request.mode, force_role, Some(state.config.study.name.clone()))
    }?;
    state
        .record(TimelineEvent::new("room_created", Some(room_id.clone()), Some(request.participant_session_id.clone()), json!({"role": role.as_str()})))
        .await;
    let response = room_response(&state, &room_id, &request.participant_session_id, role, vec![]).await?;
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
    let role = {
        let mut memory = state.memory.write().await;
        let existing_role = memory
            .rooms
            .get(&room_id)
            .ok_or_else(|| AppError::not_found("Room not found."))?
            .participants
            .get(&request.participant_session_id)
            .map(|existing| existing.role);
        if let Some(existing_role) = existing_role {
            existing_role
        } else {
            let consent_decisions = memory
                .participants
                .get(&request.participant_session_id)
                .ok_or_else(|| AppError::not_found("Participant session not found."))?
                .consent_decisions
                .clone();
            let room = memory.rooms.get_mut(&room_id).ok_or_else(|| AppError::not_found("Room not found."))?;
            let role = parse_seat(request.force_role.as_deref()).unwrap_or_else(|| next_role(room));
            room.participants.insert(
                request.participant_session_id.clone(),
                RoomParticipant {
                    participant_session_id: request.participant_session_id.clone(),
                    role,
                    connected: false,
                    consent_decisions,
                    joined_at: now_iso(),
                    updated_at: now_iso(),
                },
            );
            if room.participants.values().filter(|p| p.role.player_role().is_some()).count() == 2 {
                room.status = "playing".to_string();
            }
            role
        }
    };
    state
        .record(TimelineEvent::new("room_joined", Some(room_id.clone()), Some(request.participant_session_id.clone()), json!({"role": role.as_str()})))
        .await;
    Ok(Json(room_response(&state, &room_id, &request.participant_session_id, role, vec![]).await?))
}

async fn join_matchmaking<A: GameAdapter>(
    State(state): State<Arc<AppState<A>>>,
    Json(request): Json<MatchmakingJoinRequest>,
) -> Result<Json<MatchmakingJoinResponse>, AppError>
where
    A::State: Serialize,
{
    enter_matchmaking(state, request.participant_session_id, request.queue.or(request.study_id)).await.map(Json)
}

async fn matchmaking_status<A: GameAdapter>(
    State(state): State<Arc<AppState<A>>>,
    Path(participant_session_id): Path<String>,
) -> Result<Json<MatchmakingJoinResponse>, AppError>
where
    A::State: Serialize,
{
    matchmaking_status_inner(state, &participant_session_id).await.map(Json)
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
            let agent = memory.create_participant("agent".to_string(), Some("Agent".to_string()), Some(state.config.study.name.clone()));
            let room = memory.rooms.get_mut(&room_id).expect("room exists");
            room.participants.insert(
                agent.id.clone(),
                RoomParticipant {
                    participant_session_id: agent.id.clone(),
                    role: Seat::B,
                    connected: true,
                    consent_decisions: HashMap::new(),
                    joined_at: now_iso(),
                    updated_at: now_iso(),
                },
            );
            room.status = "playing".to_string();
            (room_id, role, agent.id)
        };
        state
            .record(TimelineEvent::new("agent_participant_created", Some(room_id.clone()), Some(agent_participant_id.clone()), json!({"role": "B"})))
            .await;
        maybe_start_agent(state.clone(), room_id.clone(), agent_participant_id, Seat::B).await;
        return matched_response(&state, &room_id, &participant_session_id, role).await;
    }

    let queue_name = {
        let memory = state.memory.read().await;
        let participant = memory
            .participants
            .get(&participant_session_id)
            .ok_or_else(|| AppError::not_found("Participant session not found."))?;
        queue.or_else(|| participant.study_id.clone()).unwrap_or_else(|| state.config.study.name.clone())
    };

    let maybe_first = {
        let mut memory = state.memory.write().await;
        let queue = memory.matchmaking_queues.entry(queue_name.clone()).or_default();
        if queue.contains(&participant_session_id) {
            None
        } else if queue.is_empty() {
            queue.push(participant_session_id.clone());
            let (room_id, role) = create_room_locked(&state, &mut memory, participant_session_id.clone(), "direct".to_string(), Seat::A, Some(queue_name))?;
            return matched_response(&state, &room_id, &participant_session_id, role).await;
        } else {
            Some(queue.remove(0))
        }
    };

    let first = maybe_first.expect("second participant has first");
    let (room_id, role) = {
        let mut memory = state.memory.write().await;
        let room_id = if let Some(room) = memory.room_for_participant(&first) {
            room.id.clone()
        } else {
            create_room_locked(&state, &mut memory, first.clone(), "direct".to_string(), Seat::A, Some(queue_name))?.0
        };
        let room = memory.rooms.get_mut(&room_id).expect("room exists");
        room.participants.insert(
            participant_session_id.clone(),
            RoomParticipant {
                participant_session_id: participant_session_id.clone(),
                role: Seat::B,
                connected: false,
                consent_decisions: HashMap::new(),
                joined_at: now_iso(),
                updated_at: now_iso(),
            },
        );
        room.status = "playing".to_string();
        (room_id, Seat::B)
    };
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
            role,
            connected: false,
            consent_decisions: participant.consent_decisions,
            joined_at: now_iso(),
            updated_at: now_iso(),
        },
    );
    memory.rooms.insert(
        room_id.clone(),
        GameRoom {
            id: room_id.clone(),
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
            Some((room.id.clone(), role, participant.source.clone()))
        } else {
            None
        }
    };
    if let Some((room_id, role, source)) = existing {
        let response = room_response(&state, &room_id, participant_session_id, role, vec![]).await?;
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
        let source = state.memory.read().await.participants.get(participant_session_id).map(|p| p.source.clone());
        Ok(MatchmakingJoinResponse {
            status: "waiting".to_string(),
            participant_session_id: participant_session_id.to_string(),
            source,
            room_id: None,
            role: None,
            state: None,
            observation: None,
            available_actions: vec![],
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
    let source = state.memory.read().await.participants.get(participant_session_id).map(|p| p.source.clone());
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
    let memory = state.memory.read().await;
    let room = memory.rooms.get(room_id).ok_or_else(|| AppError::not_found("Room not found."))?;
    let (observation, available_actions) = if let Some(player) = role.player_role() {
        (
            Some(protocol_json(&state.adapter.observe_state(&room.state, player))?),
            state
                .adapter
                .available_actions(&room.state, player)
                .into_iter()
                .map(|action| protocol_json(&action))
                .collect::<Result<Vec<_>, _>>()?,
        )
    } else {
        (Some(protocol_json(&room.state)?), vec![])
    };
    Ok(RoomResponse {
        room_id: room_id.to_string(),
        participant_session_id: participant_session_id.to_string(),
        role: role.as_str().to_string(),
        state: None,
        observation,
        available_actions,
        events,
        conversation: memory.conversation_for_room(room_id, state.config.conversation.max_history_messages),
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
    let token = create_livekit_token(&state.config.livekit, &room_id, role.as_str(), &request.participant_session_id, 3600)
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    Ok(Json(LiveKitTokenResponse {
        enabled: true,
        url: Some(state.config.livekit.url.clone()),
        token: Some(token),
        identity: Some(livekit_identity(&room_id, role.as_str(), &request.participant_session_id)),
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
    let token = create_livekit_token(&state.config.livekit, &room_id, &request.role, &participant_session_id, 3600)
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    Ok(Json(LiveKitTokenResponse {
        enabled: true,
        url: Some(state.config.livekit.url.clone()),
        token: Some(token),
        identity: Some(livekit_identity(&room_id, &request.role, &participant_session_id)),
    }))
}

async fn audio_session<A: GameAdapter>(
    State(state): State<Arc<AppState<A>>>,
    Path(room_id): Path<String>,
    Json(request): Json<AudioSessionRequest>,
) -> Result<Json<AudioSessionPlanResponse>, AppError> {
    let role = participant_role(&state, &room_id, &request.participant_session_id).await?;
    if !state.config.livekit.enabled {
        return Ok(Json(AudioSessionPlanResponse::disabled()));
    }
    let token = create_livekit_token(&state.config.livekit, &room_id, role.as_str(), &request.participant_session_id, 3600)
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    let identity = livekit_identity(&room_id, role.as_str(), &request.participant_session_id);
    if state.config.transcription.enabled && state.config.transcription.provider == "speechmatics" {
        if !state.config.speechmatics.enabled || state.config.speechmatics.api_key.is_empty() {
            return Ok(Json(AudioSessionPlanResponse::disabled()));
        }
        let key = create_speechmatics_temporary_key(&state.config.speechmatics).await?;
        return Ok(Json(AudioSessionPlanResponse {
            enabled: true,
            capture: json!({"audio": true}),
            sinks: vec![
                AudioSinkPlan {
                    id: "livekit-partner".to_string(),
                    provider: "livekit".to_string(),
                    purposes: vec!["partner-audio".to_string()],
                    transport: "webrtc-room".to_string(),
                    credentials: json!({"enabled": true, "url": state.config.livekit.url, "token": token, "identity": identity}),
                },
                AudioSinkPlan {
                    id: "speechmatics-transcription".to_string(),
                    provider: "speechmatics".to_string(),
                    purposes: vec!["transcription".to_string()],
                    transport: "websocket-stt".to_string(),
                    credentials: json!({
                        "enabled": true,
                        "realtime_url": state.config.speechmatics.realtime_url,
                        "temporary_key": key,
                        "language": state.config.transcription.language,
                        "model": state.config.transcription.model,
                        "max_delay": state.config.speechmatics.max_delay,
                        "enable_partials": state.config.speechmatics.enable_partials,
                        "end_of_utterance_silence_trigger": state.config.speechmatics.end_of_utterance_silence_trigger,
                        "ttl_seconds": state.config.speechmatics.temporary_key_ttl_seconds,
                    }),
                },
            ],
        }));
    }
    Ok(Json(AudioSessionPlanResponse {
        enabled: true,
        capture: json!({"audio": true}),
        sinks: vec![AudioSinkPlan {
            id: "livekit-combined".to_string(),
            provider: "livekit".to_string(),
            purposes: vec!["partner-audio".to_string(), "transcription".to_string()],
            transport: "webrtc-room".to_string(),
            credentials: json!({"enabled": true, "url": state.config.livekit.url, "token": token, "identity": identity}),
        }],
    }))
}

async fn add_transcript<A: GameAdapter>(
    State(state): State<Arc<AppState<A>>>,
    Path(room_id): Path<String>,
    Json(segment): Json<TranscriptSegmentIn>,
) -> Result<Json<TranscriptSegment>, AppError> {
    participant_role(&state, &room_id, &segment.participant_session_id).await?;
    let stored = TranscriptSegment {
        id: new_id("tr"),
        room_id: room_id.clone(),
        participant_session_id: segment.participant_session_id.clone(),
        player: segment.player.clone(),
        start_time_ms: segment.start_time_ms,
        end_time_ms: segment.end_time_ms,
        move_count: segment.move_count,
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
            "move_count": stored.move_count,
            "client_metadata": stored.metadata,
        }),
        created_at: now_iso(),
    };
    {
        let mut memory = state.memory.write().await;
        memory.transcripts.push(stored.clone());
        memory.conversation_messages.push(message.clone());
    }
    state.record(TimelineEvent::new("transcript_posted", Some(room_id.clone()), Some(stored.participant_session_id.clone()), serde_json::to_value(&stored).unwrap())).await;
    state.record(TimelineEvent::new("conversation_message_added", Some(room_id.clone()), message.sender_participant_session_id.clone(), serde_json::to_value(&message).unwrap())).await;
    let _ = state.room_bus(&room_id).await.send(ServerMessage {
        conversation_message: Some(message),
        room_id: Some(room_id.clone()),
        ..ServerMessage::new("conversationMessageAdded")
    });
    let _ = state.transcript_bus(&room_id).await.send(serde_json::to_value(&stored).unwrap());
    Ok(Json(stored))
}

async fn get_transcripts<A: GameAdapter>(
    State(state): State<Arc<AppState<A>>>,
    Path(room_id): Path<String>,
) -> Result<Json<Vec<TranscriptSegment>>, AppError> {
    require_room(&state, &room_id).await?;
    Ok(Json(state.memory.read().await.transcripts.iter().filter(|segment| segment.room_id == room_id).cloned().collect()))
}

async fn transcript_stream<A: GameAdapter>(
    State(state): State<Arc<AppState<A>>>,
    Path(room_id): Path<String>,
) -> Result<Sse<impl futures_util::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>>, AppError> {
    require_room(&state, &room_id).await?;
    let existing = state
        .memory
        .read()
        .await
        .transcripts
        .iter()
        .filter(|segment| segment.room_id == room_id)
        .map(|segment| serde_json::to_value(segment).unwrap())
        .collect::<Vec<_>>();
    let receiver = state.transcript_bus(&room_id).await.subscribe();
    let existing_stream = tokio_stream::iter(existing.into_iter().map(|payload| Ok(axum::response::sse::Event::default().event("transcript").json_data(payload).unwrap())));
    let live_stream = BroadcastStream::new(receiver).filter_map(|item| async move {
        item.ok()
            .and_then(|payload| axum::response::sse::Event::default().event("transcript").json_data(payload).ok())
            .map(Ok)
    });
    Ok(Sse::new(futures_util::StreamExt::chain(existing_stream, live_stream)))
}

async fn transcription_context<A: GameAdapter>(
    State(state): State<Arc<AppState<A>>>,
    Path(room_id): Path<String>,
) -> Result<Json<Value>, AppError> {
    let memory = state.memory.read().await;
    let room = memory.rooms.get(&room_id).ok_or_else(|| AppError::not_found("Room not found."))?;
    let serialized = protocol_json(&room.state)?;
    Ok(Json(json!({"room_id": room_id, "move_count": serialized.get("moveCount").and_then(Value::as_i64).unwrap_or(0)})))
}

async fn add_voice_diagnostic<A: GameAdapter>(
    State(state): State<Arc<AppState<A>>>,
    Path(room_id): Path<String>,
    Json(diagnostic): Json<VoiceDiagnosticIn>,
) -> Result<Json<Value>, AppError> {
    require_room(&state, &room_id).await?;
    let stored = json!({
        "id": new_id("vdiag"),
        "room_id": room_id,
        "participant_session_id": diagnostic.participant_session_id,
        "event": diagnostic.event,
        "metadata": diagnostic.metadata,
        "created_at": now_iso(),
    });
    state.memory.write().await.voice_diagnostics.push(stored.clone());
    state.record(TimelineEvent::new("voice_diagnostic", stored["room_id"].as_str().map(str::to_string), stored["participant_session_id"].as_str().map(str::to_string), stored.clone())).await;
    Ok(Json(stored))
}

async fn get_conversation<A: GameAdapter>(
    State(state): State<Arc<AppState<A>>>,
    Path(room_id): Path<String>,
) -> Result<Json<Vec<ConversationMessageResponse>>, AppError> {
    require_room(&state, &room_id).await?;
    Ok(Json(state.memory.read().await.conversation_for_room(&room_id, state.config.conversation.max_history_messages)))
}

async fn add_conversation<A: GameAdapter>(
    State(state): State<Arc<AppState<A>>>,
    Path(room_id): Path<String>,
    Json(input): Json<ConversationMessageIn>,
) -> Result<Json<ConversationMessageResponse>, AppError> {
    require_room(&state, &room_id).await?;
    let mut sender_participant_session_id = None;
    let mut sender_role = None;
    if let Some(candidate) = input.metadata.get("sender_participant_session_id").and_then(Value::as_str) {
        sender_participant_session_id = Some(candidate.to_string());
        sender_role = Some(participant_role(&state, &room_id, candidate).await?.as_str().to_string());
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
    state.memory.write().await.conversation_messages.push(message.clone());
    state.record(TimelineEvent::new("conversation_message_added", Some(room_id.clone()), message.sender_participant_session_id.clone(), serde_json::to_value(&message).unwrap())).await;
    let _ = state.room_bus(&room_id).await.send(ServerMessage {
        room_id: Some(room_id),
        conversation_message: Some(message.clone()),
        ..ServerMessage::new("conversationMessageAdded")
    });
    Ok(Json(message))
}

async fn admin_export<A: GameAdapter>(State(state): State<Arc<AppState<A>>>) -> Result<Json<Value>, AppError>
where
    A::State: Serialize,
{
    let timeline = state.events.export().await?;
    Ok(Json(state.memory.read().await.export_snapshot(timeline)))
}

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
    Ok(ws.on_upgrade(move |socket| websocket_loop(state, socket, room_id, participant_session_id, role)))
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
    let bus = state.room_bus(&room_id).await;
    let mut receiver = bus.subscribe();
    if let Ok(response) = room_response(&state, &room_id, &participant_session_id, role, vec![]).await {
        let _ = bus.send(ServerMessage {
            room_id: Some(room_id.clone()),
            participant_session_id: Some(participant_session_id.clone()),
            role: Some(role.as_str().to_string()),
            observation: response.observation,
            available_actions: response.available_actions,
            events: response.events,
            conversation: response.conversation,
            ..ServerMessage::new("roleAssigned")
        });
    }
    maybe_start_room_agents(state.clone(), &room_id).await;
    let (mut sender, mut incoming) = socket.split();
    let send_task = tokio::spawn(async move {
        while let Ok(message) = receiver.recv().await {
            if let Ok(text) = serde_json::to_string(&message) {
                if sender.send(Message::Text(text)).await.is_err() {
                    break;
                }
            }
        }
    });
    while let Some(Ok(message)) = FuturesStreamExt::next(&mut incoming).await {
        let Message::Text(text) = message else { continue };
        let Ok(client_message) = serde_json::from_str::<ClientMessage>(&text) else {
            let _ = bus.send(error_message(&room_id, "Invalid client message JSON"));
            continue;
        };
        handle_client_message(state.clone(), &bus, &room_id, &participant_session_id, role, client_message).await;
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
    let _ = bus.send(presence_message(&state, &room_id).await.unwrap_or_else(|| ServerMessage::new("presenceChanged")));
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
            let _ = bus.send(ServerMessage { room_id: Some(room_id.to_string()), ..ServerMessage::new("presenceChanged") });
        }
        "ready" => {
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
            let _ = add_conversation(State(state.clone()), Path(room_id.to_string()), Json(input)).await;
        }
        "submitAction" => {
            let Some(action) = message.action else {
                let _ = bus.send(error_message(room_id, "submitAction requires action"));
                return;
            };
            let action = match state.adapter.parse_action(action) {
                Ok(action) => action,
                Err(error) => {
                    let _ = bus.send(error_message(room_id, &error.to_string()));
                    return;
                }
            };
            match submit_action(state.clone(), room_id, participant_session_id, role, action).await {
                Ok((completed, summary)) => {
                    broadcast_player_views(state.clone(), room_id).await;
                    if completed {
                        let _ = bus.send(ServerMessage { room_id: Some(room_id.to_string()), summary, ..ServerMessage::new("completed") });
                    }
                }
                Err(error) => {
                    let _ = bus.send(error_message(room_id, &error.to_string()));
                }
            }
        }
        other => {
            let _ = bus.send(error_message(room_id, &format!("Unknown message type: {other}")));
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
    let player = role.player_role().ok_or_else(|| anyhow!("Spectators cannot submit game actions."))?;
    let (before, after, events, completed, summary) = {
        let mut memory = state.memory.write().await;
        let room = memory.rooms.get_mut(room_id).ok_or_else(|| anyhow!("Room not found."))?;
        let connected_roles = room.participants.values().filter(|p| p.connected).filter_map(|p| p.role.player_role()).collect::<HashSet<_>>();
        if connected_roles != HashSet::from([PlayerRole::A, PlayerRole::B]) {
            return Err(anyhow!("Room is waiting for both players to connect."));
        }
        let before = room.state.clone();
        state.adapter.validate_action(&room.state, &action, player)?;
        let after = state.adapter.apply_action(&room.state, &action)?;
        let events = state.adapter.events_for_action(&before, &after, &action, player);
        room.state = after.clone();
        room.updated_at = now_iso();
        let completed = state.adapter.is_complete(&room.state);
        if completed {
            room.status = "completed".to_string();
        }
        let summary = completed.then(|| state.adapter.completion_summary(&room.state));
        (before, after, events, completed, summary)
    };
    state.record(TimelineEvent::new(
        "game_action",
        Some(room_id.to_string()),
        Some(participant_session_id.to_string()),
        json!({
            "action": protocol_json(&action)?,
            "before": protocol_json(&before)?,
            "after": protocol_json(&after)?,
            "events": protocol_json(&events)?,
        }),
    )).await;
    Ok((completed, summary.map(|summary| protocol_json(&summary)).transpose()?))
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
            .map(|room| room.participants.values().map(|p| (p.participant_session_id.clone(), p.role)).collect::<Vec<_>>())
            .unwrap_or_default()
    };
    let bus = state.room_bus(room_id).await;
    for (participant_session_id, role) in participants {
        if let Ok(response) = room_response(&state, room_id, &participant_session_id, role, vec![]).await {
            let _ = bus.send(ServerMessage {
                room_id: Some(room_id.to_string()),
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
            .map(|room| {
                room.participants
                    .values()
                    .filter(|participant| participant.role.player_role().is_some())
                    .filter(|participant| {
                        memory
                            .participants
                            .get(&participant.participant_session_id)
                            .is_some_and(|session| session.source == "agent")
                    })
                    .map(|participant| (participant.participant_session_id.clone(), participant.role))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };
    for (participant_session_id, role) in agents {
        maybe_start_agent(state.clone(), room_id.to_string(), participant_session_id, role).await;
    }
}

async fn maybe_start_agent<A: GameAdapter>(state: Arc<AppState<A>>, room_id: String, participant_session_id: String, role: Seat)
where
    A::State: Serialize,
{
    let Some(factory) = state.agent_factory.clone() else { return };
    let key = format!("{room_id}:{participant_session_id}");
    if !state.started_agents.write().await.insert(key) {
        return;
    }
    tokio::spawn(async move {
        let role_name = role.as_str().to_string();
        let context = AgentInitContext {
            role: role_name.clone(),
            room_id: room_id.clone(),
            participant_session_id: participant_session_id.clone(),
            game_index: 0,
            seed: state.config.agents.human_vs_agent.as_ref().and_then(|c| c.seed),
            config: state.config.agents.human_vs_agent.as_ref().map(|c| c.config.clone()).unwrap_or(Value::Null),
        };
        let Ok(mut agent) = factory.create(context) else { return };
        state.record(TimelineEvent::new("agent_started", Some(room_id.clone()), Some(participant_session_id.clone()), json!({"role": role_name}))).await;
        let mut invalid_actions = 0usize;
        let mut last_error = None;
        loop {
            tokio::time::sleep(Duration::from_millis(250)).await;
            let (observation, available_actions, completed, conversation, game_event_count) = {
                let memory = state.memory.read().await;
                let Some(room) = memory.rooms.get(&room_id) else { break };
                let Some(player) = role.player_role() else { break };
                (
                    state.adapter.observe_state(&room.state, player),
                    state.adapter.available_actions(&room.state, player),
                    state.adapter.is_complete(&room.state),
                    memory.conversation_for_room(&room_id, state.config.conversation.max_history_messages).into_iter().map(|m| serde_json::to_value(m).unwrap()).collect::<Vec<_>>(),
                    0,
                )
            };
            if completed {
                break;
            }
            let act_context = AgentActContext {
                role: role_name.clone(),
                room_id: room_id.clone(),
                participant_session_id: participant_session_id.clone(),
                game_event_count,
                invalid_actions,
                last_error: last_error.clone(),
                completed,
                conversation,
            };
            let timeout = state.config.agents.human_vs_agent.as_ref().map(|c| c.act_timeout_seconds).unwrap_or(10.0);
            let result = tokio::time::timeout(Duration::from_secs_f64(timeout), agent.act(observation, available_actions, act_context)).await;
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
                let _ = add_conversation(
                    State(state.clone()),
                    Path(room_id.clone()),
                    Json(ConversationMessageIn {
                        text,
                        origin: "agent".to_string(),
                        source_message_id: None,
                        metadata: json!({"sender_participant_session_id": participant_session_id}),
                    }),
                )
                .await;
            }
            if let Some(action) = action {
                match submit_action(state.clone(), &room_id, &participant_session_id, role, action).await {
                    Ok((completed, summary)) => {
                        broadcast_player_views(state.clone(), &room_id).await;
                        if completed {
                            let _ = state.room_bus(&room_id).await.send(ServerMessage {
                                room_id: Some(room_id.clone()),
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
            let limit = state.config.agents.human_vs_agent.as_ref().map(|c| c.invalid_action_limit).unwrap_or(3);
            if invalid_actions >= limit {
                state.record(TimelineEvent::new("agent_stopped_invalid_actions", Some(room_id.clone()), Some(participant_session_id.clone()), json!({"last_error": last_error}))).await;
                break;
            }
        }
    });
}

async fn require_consent<A: GameAdapter>(state: &Arc<AppState<A>>, participant_session_id: &str) -> Result<(), AppError> {
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
            .filter(|item| !participant.consent_decisions.get(&item.id).copied().unwrap_or(false))
            .map(|item| item.title.clone())
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            return Err(AppError::forbidden(format!("Consent is required before entering the game: {}.", missing.join(", "))));
        }
    }
    Ok(())
}

async fn require_room<A: GameAdapter>(state: &Arc<AppState<A>>, room_id: &str) -> Result<(), AppError> {
    if state.memory.read().await.rooms.contains_key(room_id) {
        Ok(())
    } else {
        Err(AppError::not_found("Room not found."))
    }
}

async fn participant_role<A: GameAdapter>(state: &Arc<AppState<A>>, room_id: &str, participant_session_id: &str) -> Result<Seat, AppError> {
    let memory = state.memory.read().await;
    let room = memory.rooms.get(room_id).ok_or_else(|| AppError::not_found("Room not found."))?;
    room.participants
        .get(participant_session_id)
        .map(|participant| participant.role)
        .ok_or_else(|| AppError::forbidden("Participant is not in this room."))
}

fn parse_seat(value: Option<&str>) -> Option<Seat> {
    match value {
        Some("A") => Some(Seat::A),
        Some("B") => Some(Seat::B),
        Some("spectator") => Some(Seat::Spectator),
        _ => None,
    }
}

fn next_role<S>(room: &GameRoom<S>) -> Seat {
    let roles = room.participants.values().map(|p| p.role).collect::<HashSet<_>>();
    if !roles.contains(&Seat::A) {
        Seat::A
    } else if !roles.contains(&Seat::B) {
        Seat::B
    } else {
        Seat::Spectator
    }
}

async fn presence_message<A: GameAdapter>(state: &Arc<AppState<A>>, room_id: &str) -> Option<ServerMessage> {
    let memory = state.memory.read().await;
    let room = memory.rooms.get(room_id)?;
    Some(ServerMessage {
        room_id: Some(room_id.to_string()),
        presence: Some(json!(room.participants.values().map(|participant| {
            (participant.role.as_str().to_string(), json!({
                "participantSessionId": participant.participant_session_id,
                "connected": participant.connected,
            }))
        }).collect::<serde_json::Map<_, _>>())),
        ..ServerMessage::new("presenceChanged")
    })
}

async fn voice_message<A: GameAdapter>(state: &Arc<AppState<A>>, room_id: &str) -> ServerMessage {
    let active_players = state
        .memory
        .read()
        .await
        .rooms
        .get(room_id)
        .map(|room| room.participants.values().filter(|p| p.role.player_role().is_some()).count())
        .unwrap_or(0);
    ServerMessage {
        room_id: Some(room_id.to_string()),
        voice: Some(json!({
            "audioReady": active_players == 2,
            "transcriptionReady": true,
            "transcriptionStatus": if state.config.transcription.enabled { "Waiting for audio" } else { "Disabled" },
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
