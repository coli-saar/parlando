//! Composes the live HTTP and WebSocket server for a loaded game component. Installation services
//! share the store, administration, and runtime registry across experiment revisions.
//! Each mounted revision owns its configuration and live rooms. Routing selects the
//! owning revision before participant and agent operations access its state.

use std::{
    collections::{HashMap, HashSet},
    future::Future,
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    sync::{Arc, OnceLock, Weak},
    time::{Duration, Instant},
};

use anyhow::{anyhow, bail, Context, Result};
use axum::{
    extract::{
        ws::{Message, WebSocket},
        ConnectInfo, Extension, Path, Query, Request, State, WebSocketUpgrade,
    },
    http::{
        header::{
            AUTHORIZATION, CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_TYPE, COOKIE, HOST, ORIGIN,
            SET_COOKIE,
        },
        HeaderMap, HeaderName, HeaderValue, Method, StatusCode,
    },
    middleware::{self, Next},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
    Json, Router,
};
use futures_util::{SinkExt, StreamExt as FuturesStreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::{broadcast, mpsc, Mutex, RwLock};
use tower::limit::ConcurrencyLimitLayer;
use tower_http::{
    cors::{AllowOrigin, CorsLayer},
    limit::RequestBodyLimitLayer,
    services::{ServeDir, ServeFile},
    timeout::TimeoutLayer,
};

use crate::{
    agents::{
        configuration_fingerprint, Agent, AgentContext, AgentFactory, AgentResponse,
        SharedAgentFactory,
    },
    audio::{
        AudioFrame, AudioOutbound, AudioSessionRegistry, SharedAudioSessions, AUDIO_CHANNELS,
        AUDIO_FRAME_DURATION_MS, AUDIO_PROTOCOL_VERSION, AUDIO_SAMPLE_RATE,
    },
    audio_publisher::{AgentAudioPublisher, SessionAgentAudioPublisher},
    auth::{
        AdminAuthenticator, AdminLoginResult, AdminSession, ParticipantAuthenticator,
        ParticipantPrincipal, UpgradePurpose, UpgradeTicketClaims, UpgradeTicketStore,
        ADMIN_ABSOLUTE_SECONDS,
    },
    component::{ComponentValue, GameFrame, GameSession, LoadedGame},
    config::{AgentsMode, ExperimentConfig, StoredExperimentConfig},
    game::{
        parse_game_config, ActionRejection, AgentDefinition, GameMetadata, PlayerRole, SecretValues,
    },
    identity::{new_id, session_code},
    protocol::*,
    storage::ExperimentSnapshot,
    storage::{
        experiment_store_from_url, generated_experiment_id, now_iso, ConsentDeclarationRecord,
        ExperimentRecord, GameSettingsUpdate, ParticipantRecord, ProlificSubmissionRecord,
        SessionEventRecord, SessionParticipantRecord, SessionRecord, SharedExperimentStore,
        StoredGameSettings, TransitionCommit,
    },
    transcription::{
        FinalTranscriptUtterance, SpeechmaticsTranscriptionProvider, TranscriptionEvent,
        TranscriptionInput, TranscriptionProvider, TranscriptionSessionContext,
    },
    tts::{ElevenLabsStreamingTtsProvider, StreamingTtsProvider},
    SessionLogger,
};

/// A constructed agent and the validated settings needed by its runtime task.
struct AgentTaskInput {
    agent: Box<dyn Agent + Send>,
    timeout: f64,
    invalid_action_limit: usize,
    metadata: Value,
}

/// Optional runtime services supplied when mounting an experiment.
#[derive(Clone)]
pub struct ServeOptions {
    /// Display identity and build provenance of the loaded game component.
    #[cfg(any(test, feature = "internal-tools"))]
    pub game_descriptor: Option<GameMetadata>,
    /// Factory used to create one fresh agent per agent participant.
    pub agent_factory: Option<Arc<dyn AgentFactory>>,
    /// Dashboard definitions for every agent factory registered with this server.
    pub agent_definitions: Vec<AgentDefinition>,
    /// Streaming TTS provider used for agent-origin conversation messages.
    pub tts_provider: Option<Arc<dyn StreamingTtsProvider>>,
    /// Optional publisher used to send synthesized agent audio into the session relay.
    pub audio_publisher: Option<Arc<dyn AgentAudioPublisher>>,
    /// Optional server-side STT provider override used by tests or local deployments.
    pub transcription_provider: Option<Arc<dyn TranscriptionProvider>>,
}

impl Default for ServeOptions {
    /// Creates serve options with no agent factory configured.
    fn default() -> Self {
        Self {
            #[cfg(any(test, feature = "internal-tools"))]
            game_descriptor: None,
            agent_factory: None,
            agent_definitions: Vec::new(),
            tts_provider: None,
            audio_publisher: None,
            transcription_provider: None,
        }
    }
}

/// Hashes the exact reviewed participant-information reference and stored consent prose.
fn consent_configuration_hash(config: &ExperimentConfig) -> Result<String, AppError> {
    let presented = json!({
        "participant_information_version": config.direct.participant_information_version,
        "participant_information_url": config.direct.participant_information_url,
        "consents": config.direct.consents,
    });
    let canonical = serde_json::to_string(&presented)
        .map_err(|error| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    Ok(format!("sha256:{}", hex_sha256(&canonical)))
}

/// Applies the experiment-wide safety ceiling to unauthenticated participant creation bursts.
async fn enforce_creation_rate(state: &Arc<AppState>) -> Result<(), AppError> {
    let now = chrono::Utc::now().timestamp();
    let mut attempts = state.participant_creation_window.write().await;
    attempts.record(now).then_some(()).ok_or_else(|| {
        AppError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "Participant creation rate limit exceeded",
        )
    })
}

/// Router cache contains entries only for real experiments that have mounted successfully.
type ExperimentMountCache = RwLock<HashMap<String, routing::ExperimentMounts>>;

/// Process-wide resources shared by catalogue administration and mounted experiments.
struct Installation {
    loaded_game: LoadedGame,
    game_descriptor: GameMetadata,
    version_manifest: Value,
    store: SharedExperimentStore,
    admin_auth: Arc<AdminAuthenticator>,
    game_settings: RwLock<StoredGameSettings>,
    telemetry: Arc<RuntimeTelemetry>,
    runtime_registry: RwLock<HashMap<String, Vec<Weak<AppState>>>>,
    prolific_preflight_cache: RwLock<HashMap<String, ProlificPreflightCacheEntry>>,
    // The dispatcher owns the routers. Keeping this link weak avoids a router/state cycle.
    runtime_router_cache: Weak<ExperimentMountCache>,
    runtime_router_build_locks: [Arc<Mutex<()>>; MOUNT_LOCK_COUNT],
}

/// Configuration and administrative capabilities independent of live session resources.
struct ServerContext {
    installation: Arc<Installation>,
    config: ExperimentConfig,
    agent_definitions: Vec<AgentDefinition>,
    has_transcription_provider: bool,
    has_tts_provider: bool,
}

/// Shared server state used by HTTP handlers, WebSocket tasks, and background agents.
pub(crate) struct AppState {
    context: Arc<ServerContext>,
    /// Typed game-owned configuration validated when this experiment runtime is built.
    pub game_config: ComponentValue,
    pub experiment_id: String,
    /// Immutable configuration revision used for newly created sessions.
    pub config_revision: i64,
    experiment_lifecycle: RwLock<ExperimentLifecycle>,
    /// Participant URL selected when the current intake run started.
    participant_url: RwLock<Option<RunningParticipantUrl>>,
    memory: RwLock<MemoryState>,
    /// Serializes matchmaking reservations without blocking live-session state access.
    session_admission: AdmissionQueue,
    pub session_buses: RwLock<HashMap<String, broadcast::Sender<ServerMessage>>>,
    pub agent_factory: Option<SharedAgentFactory>,
    pub started_agents: RwLock<HashSet<String>>,
    pub tts_provider: Option<Arc<dyn StreamingTtsProvider>>,
    pub audio_publisher: Option<Arc<dyn AgentAudioPublisher>>,
    pub audio_sessions: SharedAudioSessions,
    pub transcription_provider: Option<Arc<dyn TranscriptionProvider>>,
    /// Server-only Prolific adapter sharing HTTP and signing-key caches across admissions.
    prolific_client: RwLock<Option<crate::prolific::ProlificClient>>,
    /// Provider-verified study contract cached by successful activation preflight.
    prolific_study: RwLock<Option<VerifiedProlificStudy>>,
    committed_transcripts: RwLock<HashSet<String>>,
    mount_id: u64,
    retired: std::sync::atomic::AtomicBool,
    participant_auth: Arc<ParticipantAuthenticator>,
    upgrade_tickets: Arc<UpgradeTicketStore>,
    participant_creation_window: Arc<RwLock<ParticipantCreationRate>>,
    chat_submission_budgets: RwLock<HashMap<String, TokenBucket>>,
    rejection_windows: RwLock<HashMap<String, RejectionWindow>>,
    game_connections: ConnectionRegistry,
    audio_connections: ConnectionRegistry,
}

/// Stable participant URL ingredients retained for one open intake run.
#[derive(Clone, Debug, Serialize)]
struct RunningParticipantUrl {
    kind: String,
    participant_id: Option<String>,
    session_id: Option<String>,
}

/// Provider-verified study together with the workspace derived from its project.
#[derive(Clone)]
struct VerifiedProlificStudy {
    study: crate::prolific::Study,
    workspace_id: String,
    refreshed_at: String,
}

/// One provider readiness result keyed by every local input to the study contract.
#[derive(Clone)]
struct ProlificPreflightCacheEntry {
    fingerprint: String,
    checked_at: Instant,
    issues: Vec<String>,
    study: Option<VerifiedProlificStudy>,
}

/// Selects a historical experiment for storage-only administrator handlers.
#[derive(Clone)]
struct AdminExperimentScope(String);

/// Returns the explicitly routed experiment or the runtime's own experiment.
fn admin_experiment_id(
    state: &AppState,
    scope: Option<&Extension<AdminExperimentScope>>,
) -> String {
    scope
        .map(|Extension(scope)| scope.0.clone())
        .unwrap_or_else(|| state.experiment_id.clone())
}

/// Sliding-window participant-creation counters bounded by cleanup on each request.
struct ParticipantCreationRate {
    /// Process-wide creation timestamps used as a final safety ceiling.
    global: Vec<i64>,
    /// Maximum attempts retained inside the sliding window.
    max_attempts: usize,
    /// Width of the sliding window in seconds.
    window_seconds: i64,
}

impl Default for ParticipantCreationRate {
    /// Loads the same build-time limits exposed to operational preflight tools.
    fn default() -> Self {
        let limit = crate::bundled_runtime_limits().participant_creation.clone();
        Self {
            global: Vec::new(),
            max_attempts: limit.max_attempts,
            window_seconds: limit.window_seconds,
        }
    }
}

impl ParticipantCreationRate {
    /// Records one allowed attempt after pruning the configured sliding window.
    fn record(&mut self, now: i64) -> bool {
        self.global
            .retain(|timestamp| *timestamp > now - self.window_seconds);
        if self.global.len() >= self.max_attempts {
            return false;
        }
        self.global.push(now);
        true
    }
}

/// In-memory token bucket for inexpensive pacing that never disconnects a session.
struct TokenBucket {
    capacity: f64,
    refill_per_second: f64,
    tokens: f64,
    last_refill: Instant,
}

impl TokenBucket {
    /// Creates a full bucket with the supplied burst and sustained rates.
    fn new(capacity: f64, refill_per_second: f64) -> Self {
        Self {
            capacity,
            refill_per_second,
            tokens: capacity,
            last_refill: Instant::now(),
        }
    }

    /// Consumes one token after refilling from elapsed wall time.
    fn consume(&mut self) -> bool {
        let now = Instant::now();
        self.tokens = (self.tokens
            + now.duration_since(self.last_refill).as_secs_f64() * self.refill_per_second)
            .min(self.capacity);
        self.last_refill = now;
        if self.tokens < 1.0 {
            return false;
        }
        self.tokens -= 1.0;
        true
    }
}

/// Coalescing state for repeated rejected inputs from the same participant and reason.
struct RejectionWindow {
    first_seen: i64,
    last_seen: i64,
    last_persisted: i64,
    occurrences: u64,
}

/// Expected server-owned reason why an action cannot enter the game transition.
#[derive(Debug)]
struct SubmissionRejection(&'static str);

impl std::fmt::Display for SubmissionRejection {
    /// Formats the stable protocol code for logs.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.0)
    }
}

impl std::error::Error for SubmissionRejection {}

mod intake;
use intake::{
    consent, create_participant, prolific_handoff, prolific_no_consent_url, require_consent,
};
mod admission;
use admission::{create_session, AdmissionQueue};
use construction::{construct_and_join_agent, create_live_session_locked};
use session::construction;
mod participant_state;
use participant_state::{
    action_rejected_message, broadcast_participant_states, error_message, get_participant_state,
    internal_error_message, leave_session, participant_state_response, presence_message,
    send_role_assignment, voice_message,
};
mod conversation;
use conversation::{
    add_voice_diagnostic, commit_conversation_message, commit_final_transcript, ConversationOrigin,
};
mod game_socket;
use game_socket::{game_session, game_socket};
mod audio_socket;
use audio_socket::{audio_session, audio_socket};
mod client_message;
use client_message::handle_client_message;
mod rejected_input;
use rejected_input::{
    flush_rejected_input_aggregates, persist_rejected_action, persist_rejected_input,
};
mod live_agent;
use live_agent::start_agent_task;
mod agent_speech;
use agent_speech::speak_agent_message;
mod readiness;
use readiness::{mark_participant_audio_ready, maybe_start_game, publish_transcription_readiness};
mod events;
use events::{
    persist_event, persist_session_event, persist_session_event_required, session_event_record,
};
mod session_activity;
use session_activity::{
    participant_role, require_session, session_accepts_audio, session_has_started,
    session_is_active, session_transition_lock, shutdown_session_log, touch_session_activity,
    touch_transport_activity,
};
mod maintenance;
use maintenance::{
    finalize_interrupted_sessions, session_lifecycle_is_ended, spawn_security_cleanup,
};
mod connections;
use connections::ConnectionRegistry;
mod occupancy;
use occupancy::{experiment_occupancy, Occupancy};
mod monitoring;
use monitoring::{admin_load, game_connection_health, spawn_load_sampler, track_request_load};
mod liveness;
use liveness::build_session_liveness;

mod admin;
use admin::*;
mod privacy;
use privacy::*;
mod timeline;
use timeline::{admin_event_bundles, important_admin_events};
mod experiment_config;
use experiment_config::{
    apply_experiment_secrets, apply_game_provider_secrets, apply_provider_endpoint_defaults,
    derived_agent_secret_key, experiment_config_from_json, experiment_config_from_json_unvalidated,
    experiment_public_base_url, hex_sha256, hydrated_experiment_config, nonempty_string,
    persistable_config_json, prolific_setup_url, redact_secret_fields,
    validate_game_config_contains_no_secrets,
};
mod agent_configuration;
use agent_configuration::{dashboard_agent_definitions, validate_agent_configuration};
mod activation;
#[cfg(test)]
use activation::prolific_completion_path_issues;
use activation::{
    cached_prolific_activation_preflight, experiment_activation_issues,
    local_testing_issues_for_config, prolific_configuration_complete,
    prolific_local_activation_issues, run_and_cache_prolific_activation_preflight,
};
mod http_security;
use http_security::{
    configured_cors, security_headers, validate_admin_request_origin, validate_websocket_origin,
    websocket_path, CspNonce,
};
mod authentication;
#[cfg(test)]
use authentication::admin_network_allows;
use authentication::{
    authenticated_participant_id, bearer_token, cookie_value, require_admin_auth,
    require_admin_network, require_participant_auth,
};
mod admin_login;
#[cfg(test)]
use admin_login::administrator_cookie;
use admin_login::{admin_entry, admin_login, admin_login_page, admin_logout, admin_setup};

mod corpus;
mod lifecycle;
use corpus::{corpus_experiment_export, export_csv, CORPUS_EXPORT_SCHEMA_V1};
#[cfg(test)]
use corpus::{export_transcript_tokens, retain_export_sessions};
use lifecycle::{require_open_experiment, ExperimentLifecycle};
mod action;
mod session;
use action::submit_action;
mod deadlines;
use deadlines::{due_live_deadline, room_cleanup, RoomCleanup};
mod session_transitions;
use session::{
    session_ready_for_game, speechmatics_readiness_required, AgentObservation, LiveSession,
    MemoryState, SessionLifecycle, SessionParticipant,
};
use session_transitions::{
    abandon_session, depart_session_locked, expire_if_due_locked, expire_live_session,
    expire_live_session_locked, reserve_agent_delivery,
};
mod telemetry;
use telemetry::{
    CapacitySample, ConnectionLiveness, ConnectionLivenessSnapshot, ConnectionSample, LoadSample,
    ParticipantLiveness, RuntimeTelemetry, SessionLiveness,
};

impl AppState {
    /// Returns the shared broadcast channel for one session.
    async fn session_bus(&self, public_session_id: &str) -> broadcast::Sender<ServerMessage> {
        let mut buses = self.session_buses.write().await;
        buses
            .entry(public_session_id.to_string())
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
    /// Constructs an HTTP application error with its response status.
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    /// Builds a missing-resource error.
    fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, message)
    }

    /// Builds an authorization error.
    fn forbidden(message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, message)
    }

    /// Builds an error for invalid request input.
    fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }
}

impl From<anyhow::Error> for AppError {
    /// Converts internal errors to HTTP application failures.
    fn from(value: anyhow::Error) -> Self {
        tracing::error!(error = %value, "internal request failure");
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error")
    }
}

impl IntoResponse for AppError {
    /// Serializes the public error status and message.
    fn into_response(self) -> Response {
        (self.status, self.message).into_response()
    }
}

mod routing;
pub use routing::serve_game;
pub(crate) use routing::{build_deployed_experiment, termination_signal, validate_hosted_config};
#[cfg(any(test, feature = "internal-tools"))]
pub use routing::{build_game_router, build_router};

/// Checks that the experiment store is accepting read-write transactions.
async fn health(State(state): State<Arc<AppState>>) -> Result<Json<Value>, AppError> {
    tokio::time::timeout(
        Duration::from_secs(2),
        state.context.installation.store.health_check(),
    )
    .await
    .map_err(|_| AppError::new(StatusCode::SERVICE_UNAVAILABLE, "storage check timed out"))?
    .map_err(|error| {
        tracing::error!(%error, "health check could not acquire SQLite write transaction");
        AppError::new(StatusCode::SERVICE_UNAVAILABLE, "storage unavailable")
    })?;
    Ok(Json(json!({"status": "ok", "storage": "read_write"})))
}

/// Returns the public configuration used by the participant client.
async fn public_config(State(state): State<Arc<AppState>>) -> Json<PublicConfigResponse> {
    let config = &state.context.config;
    let game_settings = state
        .context
        .installation
        .game_settings
        .read()
        .await
        .clone();
    let participant_url = state.participant_url.read().await.clone();
    Json(PublicConfigResponse {
        game_name: state.context.installation.game_descriptor.name.clone(),
        experiment_status: state.experiment_lifecycle.read().await.as_str().to_string(),
        institution: nonempty_string(&game_settings.institution),
        participant_information_version: nonempty_string(
            &config.direct.participant_information_version,
        ),
        participant_information_url: nonempty_string(&config.direct.participant_information_url),
        consents: config
            .direct
            .consents
            .iter()
            .map(|item| ConsentItemResponse {
                id: item.id.clone(),
                title: item.title.clone(),
                body: item.body.clone(),
                required: item.required,
            })
            .collect(),
        communication: config.communication,
        recruitment: if config.recruitment.prolific.enabled {
            json!({
                "provider": "prolific",
                "decline_url": prolific_no_consent_url(config, participant_url.as_ref()),
                "return_url": "https://app.prolific.com/submissions",
            })
        } else {
            json!({ "provider": "direct" })
        },
    })
}

use routing::mount_lock::*;

/// Builds the identity and version metadata attached to agent events.
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

/// Combines server and game build provenance and dependency warnings.
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

/// Lists local Cargo dependencies that affect build reproducibility.
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

/// Serializes a typed value for a JSON protocol payload.
fn protocol_json<T: Serialize>(value: &T) -> Result<Value> {
    Ok(serde_json::to_value(value)?)
}

#[cfg(test)]
mod tests;
