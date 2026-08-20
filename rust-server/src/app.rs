use std::{
    collections::{HashMap, HashSet},
    future::Future,
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    sync::{Arc, Weak},
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
    routing::{any, get, post},
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
        AudioFrame, AudioOutbound, AudioRoomRegistry, SharedAudioRooms, AUDIO_CHANNELS,
        AUDIO_FRAME_DURATION_MS, AUDIO_PROTOCOL_VERSION, AUDIO_SAMPLE_RATE,
    },
    audio_publisher::{AgentAudioPublisher, RoomAgentAudioPublisher},
    auth::{
        AdminAuthenticator, AdminLoginResult, AdminSession, ParticipantAuthenticator,
        ParticipantPrincipal, UpgradePurpose, UpgradeTicketClaims, UpgradeTicketStore,
        ADMIN_ABSOLUTE_SECONDS,
    },
    config::{AgentsMode, ConsentItemConfig, ExperimentConfig},
    game::{
        parse_game_config, ActionRejection, AgentConfigField, AgentConfigValue, AgentDefinition,
        Game, GameInitializationContext, GameMetadata, PlayerRole, Seat, SecretValues,
    },
    identity::{new_id, room_code},
    protocol::*,
    storage::{
        experiment_store_from_url, generated_experiment_id, now_iso, ConsentDeclarationRecord,
        ExperimentRecord, GameRoom, MemoryState, ParticipantRecord, RoomParticipant,
        SessionEventRecord, SessionParticipantRecord, SessionRecord, SharedExperimentStore,
        StoredGameSettings, TranscriptSegment,
    },
    transcription::{
        FinalTranscriptUtterance, SpeechmaticsTranscriptionProvider, TranscriptionEvent,
        TranscriptionInput, TranscriptionProvider, TranscriptionSessionContext,
    },
    tts::{ElevenLabsStreamingTtsProvider, StreamingTtsProvider},
};

/// Optional runtime components supplied by the game-specific binary.
#[derive(Clone)]
pub struct ServeOptions<A: Game> {
    /// Identity of the one game implementation compiled into this server process.
    pub game_descriptor: Option<GameMetadata>,
    /// Factory used to create one fresh agent per agent participant.
    pub agent_factory: Option<Arc<dyn AgentFactory<A>>>,
    /// Dashboard definitions for every agent factory registered with this server.
    pub agent_definitions: Vec<AgentDefinition>,
    /// Streaming TTS provider used for agent-origin conversation messages.
    pub tts_provider: Option<Arc<dyn StreamingTtsProvider>>,
    /// Optional publisher used to send synthesized agent audio into the room relay.
    pub audio_publisher: Option<Arc<dyn AgentAudioPublisher>>,
    /// Optional server-side STT provider override used by tests or local deployments.
    pub transcription_provider: Option<Arc<dyn TranscriptionProvider>>,
    /// Game/client version metadata supplied by the game-specific binary.
    pub game_version_manifest: Option<Value>,
}

impl<A: Game> Default for ServeOptions<A> {
    /// Creates serve options with no agent factory configured.
    fn default() -> Self {
        Self {
            game_descriptor: None,
            agent_factory: None,
            agent_definitions: Vec::new(),
            tts_provider: None,
            audio_publisher: None,
            transcription_provider: None,
            game_version_manifest: None,
        }
    }
}

/// Produces the only configuration representation allowed to enter durable storage.
fn persistable_config_json(config: &ExperimentConfig) -> Result<Value> {
    let mut value = serde_json::to_value(config)?;
    if let Some(object) = value.as_object_mut() {
        object.remove("experiment");
        object.remove("server");
        object.remove("database");
    }
    for provider in ["speechmatics", "tts"] {
        if let Some(settings) = value.get_mut(provider).and_then(Value::as_object_mut) {
            settings.remove("api_key");
        }
    }
    if let Some(speechmatics) = value.get_mut("speechmatics").and_then(Value::as_object_mut) {
        speechmatics.remove("realtime_url");
    }
    redact_secret_fields(&mut value);
    Ok(value)
}

/// Rejects credentials embedded in revisioned game options instead of silently redacting them.
fn validate_game_config_contains_no_secrets(value: &Value) -> Result<()> {
    fn visit(value: &Value, path: &mut Vec<String>) -> Result<()> {
        match value {
            Value::Object(object) => {
                for (key, child) in object {
                    path.push(key.clone());
                    let normalized = key.to_ascii_lowercase();
                    let secret = normalized == "private_key"
                        || normalized == "client_secret"
                        || normalized == "access_token"
                        || normalized == "auth_token"
                        || normalized.ends_with("_api_key")
                        || normalized.ends_with("_password")
                        || normalized.ends_with("_secret")
                        || normalized.ends_with("_token")
                        || matches!(
                            normalized.as_str(),
                            "api_key" | "apikey" | "password" | "token" | "secret"
                        );
                    if secret {
                        bail!(
                            "game configuration field {} looks like a credential; add it under Game secrets instead",
                            path.join(".")
                        );
                    }
                    visit(child, path)?;
                    path.pop();
                }
            }
            Value::Array(items) => {
                for item in items {
                    visit(item, path)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    visit(value, &mut Vec::new())
}

/// Restores bootstrap-only fields before strict experiment configuration validation.
fn experiment_config_from_json(
    mut value: Value,
    bootstrap: &ExperimentConfig,
    experiment_id: &str,
) -> Result<ExperimentConfig> {
    let object = value
        .as_object_mut()
        .ok_or_else(|| anyhow!("experiment configuration must be a JSON object"))?;
    object.insert(
        "server".to_string(),
        serde_json::to_value(&bootstrap.server)?,
    );
    object.insert(
        "database".to_string(),
        serde_json::to_value(&bootstrap.database)?,
    );
    let experiment = object
        .entry("experiment")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| anyhow!("experiment configuration identity must be an object"))?;
    experiment.insert("id".to_string(), Value::String(experiment_id.to_string()));
    let speechmatics = object
        .entry("speechmatics")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| anyhow!("speechmatics configuration must be an object"))?;
    speechmatics.insert(
        "api_key".to_string(),
        Value::String(bootstrap.speechmatics.api_key.clone()),
    );
    let tts = object
        .entry("tts")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| anyhow!("TTS configuration must be an object"))?;
    tts.insert(
        "api_key".to_string(),
        Value::String(bootstrap.tts.api_key.clone()),
    );
    let config: ExperimentConfig = serde_json::from_value(value)?;
    config.validate()?;
    Ok(config)
}

/// Applies write-only experiment credentials after loading revisioned non-secret settings.
fn apply_experiment_secrets(config: &mut ExperimentConfig, secrets: &HashMap<String, String>) {
    config.game_secrets = secrets
        .iter()
        .filter_map(|(key, value)| {
            key.strip_prefix("game.")
                .map(|key| (key.to_string(), value.clone()))
        })
        .collect();
}

/// Applies installation-wide provider connections and credentials to one experiment runtime.
fn apply_game_provider_settings(
    config: &mut ExperimentConfig,
    settings: &StoredGameSettings,
    bootstrap_secrets: &HashMap<String, String>,
    stored_secrets: &HashMap<String, String>,
) {
    config.speechmatics.realtime_url = settings.speechmatics_realtime_url.clone();
    config.speechmatics.api_key = stored_secrets
        .get("speechmatics.api_key")
        .or_else(|| bootstrap_secrets.get("speechmatics.api_key"))
        .cloned()
        .unwrap_or_default();
    config.tts.api_key = stored_secrets
        .get("tts.api_key")
        .or_else(|| bootstrap_secrets.get("tts.api_key"))
        .cloned()
        .unwrap_or_default();
}

/// Loads one stored revision with process bootstrap settings and effective credentials applied.
async fn hydrated_experiment_config<A: Game>(
    state: &Arc<AppState<A>>,
    experiment_id: &str,
) -> Result<ExperimentConfig, AppError> {
    let experiment = state
        .store
        .experiment_definition(experiment_id)
        .await?
        .ok_or_else(|| AppError::not_found("Experiment not found."))?;
    let mut config = experiment_config_from_json(experiment.config, &state.config, experiment_id)
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    let experiment_secrets = state.store.experiment_secrets(experiment_id).await?;
    apply_experiment_secrets(&mut config, &experiment_secrets);
    repair_legacy_agent_secret_references(&state.agent_definitions, &mut config);
    let game_secrets = state.store.game_secrets().await?;
    let game_settings = state.game_settings.read().await.clone();
    apply_game_provider_settings(
        &mut config,
        &game_settings,
        &state.bootstrap_secrets,
        &game_secrets,
    );
    Ok(config)
}

/// Repairs references erased by the former credential-shaped-field redactor.
fn repair_legacy_agent_secret_references(
    definitions: &[AgentDefinition],
    config: &mut ExperimentConfig,
) {
    let Some(selected) = config.agents.human_vs_agent.as_mut() else {
        return;
    };
    let Some(factory_id) = selected
        .factory
        .as_deref()
        .or_else(|| definitions.first().map(|definition| definition.id.as_str()))
    else {
        return;
    };
    let Some(definition) = definitions
        .iter()
        .find(|definition| definition.id == factory_id)
    else {
        return;
    };
    let Some(settings) = selected.config.as_object_mut() else {
        return;
    };
    repair_legacy_agent_secret_fields(
        &definition.config_fields,
        settings,
        factory_id,
        "",
        &config.game_secrets,
    );
}

/// Restores only empty references whose dashboard-derived secret actually exists.
fn repair_legacy_agent_secret_fields(
    fields: &[AgentConfigField],
    settings: &mut serde_json::Map<String, Value>,
    factory_id: &str,
    parent_path: &str,
    secrets: &HashMap<String, String>,
) {
    for field in fields {
        let path = if parent_path.is_empty() {
            field.key.clone()
        } else {
            format!("{parent_path}.{}", field.key)
        };
        match &field.value {
            AgentConfigValue::Object { fields } => {
                if let Some(object) = settings.get_mut(&field.key).and_then(Value::as_object_mut) {
                    repair_legacy_agent_secret_fields(fields, object, factory_id, &path, secrets);
                }
            }
            AgentConfigValue::SecretReference { .. } => {
                let missing = settings
                    .get(&field.key)
                    .is_none_or(|value| value.as_str().is_none_or(str::is_empty));
                if !missing {
                    continue;
                }
                let key = derived_agent_secret_key(factory_id, &path);
                if secrets.contains_key(&key) {
                    settings.insert(field.key.clone(), Value::String(format!("game.{key}")));
                }
            }
            _ => {}
        }
    }
}

/// Mirrors the dashboard's stable hidden key for an agent-owned credential.
fn derived_agent_secret_key(factory_id: &str, path: &str) -> String {
    format!("agent_{factory_id}_{path}")
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '_' | '-') {
                character
            } else {
                '_'
            }
        })
        .take(128)
        .collect()
}

/// Computes intake blockers while honoring providers injected by an embedding application.
async fn experiment_activation_issues<A: Game>(
    state: &Arc<AppState<A>>,
    experiment_id: &str,
) -> Result<Vec<String>, AppError> {
    let mut config = hydrated_experiment_config(state, experiment_id).await?;
    Ok(activation_issues_for_config(state, &mut config))
}

/// Computes every runtime-readiness blocker from one hydrated configuration.
fn activation_issues_for_config<A: Game>(
    state: &AppState<A>,
    config: &mut ExperimentConfig,
) -> Vec<String> {
    if state.transcription_provider_is_override {
        config.speechmatics.api_key = "provided-by-runtime".to_string();
    }
    if state.tts_provider_is_override {
        config.tts.api_key = "provided-by-runtime".to_string();
        config.tts.voice_id = "provided-by-runtime".to_string();
    }
    let mut issues = config.activation_issues();
    if let Err(error) = validate_agent_configuration(&state.agent_definitions, &config, true) {
        issues.push(error.to_string());
    }
    issues
}

/// Validates the selected factory settings and, when requested, referenced secret availability.
fn validate_agent_configuration(
    definitions: &[AgentDefinition],
    config: &ExperimentConfig,
    require_secrets: bool,
) -> Result<()> {
    if config.agents.mode != AgentsMode::HumanVsAgent {
        return Ok(());
    }
    let selected = config
        .agents
        .human_vs_agent
        .as_ref()
        .context("human-versus-agent configuration is missing")?;
    let factory_id = selected
        .factory
        .as_deref()
        .or_else(|| definitions.first().map(|definition| definition.id.as_str()))
        .context("no agent factory is selected")?;
    let definition = definitions
        .iter()
        .find(|definition| definition.id == factory_id)
        .with_context(|| format!("unknown agent factory {factory_id:?}"))?;
    let normalized = definition.normalize_settings(&selected.config)?;
    if require_secrets {
        definition.resolve_secrets(&normalized, &config.game_secrets)?;
    }
    Ok(())
}

/// Extracts non-empty process bootstrap credentials for experiment fallback and reveal.
fn bootstrap_secret_values(config: &ExperimentConfig) -> HashMap<String, String> {
    [
        ("speechmatics.api_key", config.speechmatics.api_key.as_str()),
        ("tts.api_key", config.tts.api_key.as_str()),
    ]
    .into_iter()
    .filter(|(_, value)| !value.is_empty())
    .map(|(key, value)| (key.to_string(), value.to_string()))
    .collect()
}

/// Recursively removes credential-shaped fields as a defense-in-depth serialization boundary.
fn redact_secret_fields(value: &mut Value) {
    match value {
        Value::Object(object) => {
            for (key, child) in object.iter_mut() {
                let normalized = key.to_ascii_lowercase();
                if normalized == "private_key"
                    || normalized == "client_secret"
                    || normalized == "access_token"
                    || normalized == "auth_token"
                    || normalized.ends_with("_api_key")
                    || normalized.ends_with("_password")
                    || normalized.ends_with("_secret")
                    || normalized.ends_with("_token")
                    || matches!(
                        normalized.as_str(),
                        "api_key" | "apikey" | "password" | "password_hash" | "token" | "secret"
                    )
                {
                    if !child
                        .as_str()
                        .is_some_and(is_experiment_game_secret_reference)
                    {
                        *child = Value::String(String::new());
                    }
                } else {
                    redact_secret_fields(child);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(redact_secret_fields),
        _ => {}
    }
}

/// Recognizes the non-secret identifier stored by semantic secret-reference fields.
fn is_experiment_game_secret_reference(value: &str) -> bool {
    value.strip_prefix("game.").is_some_and(|key| {
        !key.is_empty()
            && key.len() <= 128
            && key
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    })
}

/// Encodes a SHA-256 digest as lowercase hexadecimal without adding another dependency.
fn hex_sha256(value: &str) -> String {
    Sha256::digest(value.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Returns a cloned configuration string only when it contains a non-whitespace value.
fn nonempty_string(value: &str) -> Option<String> {
    (!value.trim().is_empty()).then(|| value.to_string())
}

/// Resolves legacy dashboard-template placeholders into complete participant-facing prose.
fn expanded_consent_items(
    config: &ExperimentConfig,
    game_settings: &StoredGameSettings,
) -> Vec<ConsentItemConfig> {
    let information_version = config.direct.participant_information_version.trim();
    let institution = game_settings.institution.trim();
    let processing_region = if game_settings.speechmatics_realtime_url.contains("//eu.") {
        "the European Union"
    } else if game_settings.speechmatics_realtime_url.contains("//us.") {
        "the United States"
    } else {
        "the processing region described in the Participant Information and Privacy Notice"
    };
    config
        .direct
        .consents
        .iter()
        .cloned()
        .map(|mut item| {
            if information_version.is_empty() {
                item.body = item.body.replace(
                    ", version {{LOCAL_INFORMATION_VERSION}}, linked above",
                    " linked above",
                );
            } else {
                item.body = item
                    .body
                    .replace("{{LOCAL_INFORMATION_VERSION}}", information_version);
            }
            item.body = item.body.replace(
                "{{INSTITUTION_NAME}}",
                if institution.is_empty() {
                    "the institution responsible for this study"
                } else {
                    institution
                },
            );
            item.body = item
                .body
                .replace("{{SPEECHMATICS_ENTITY_AND_SERVICE}}", "Speechmatics")
                .replace("{{SPEECHMATICS_PROCESSING_REGION}}", processing_region);
            item
        })
        .collect()
}

/// Hashes the exact participant-information reference and expanded consent presentation.
fn consent_configuration_hash(
    config: &ExperimentConfig,
    game_settings: &StoredGameSettings,
) -> Result<String, AppError> {
    let presented = json!({
        "participant_information_version": config.direct.participant_information_version,
        "participant_information_url": config.direct.participant_information_url,
        "consents": expanded_consent_items(config, game_settings),
    });
    let canonical = serde_json::to_string(&presented)
        .map_err(|error| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    Ok(format!("sha256:{}", hex_sha256(&canonical)))
}

/// Builds an exact-origin CORS policy from the public URL and configured additions.
fn configured_cors(config: &ExperimentConfig) -> Result<CorsLayer> {
    let origins = allowed_origin_values(config)?;
    Ok(CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_methods([Method::GET, Method::POST])
        .allow_headers([
            AUTHORIZATION,
            CONTENT_TYPE,
            HeaderName::from_static("x-csrf-token"),
        ]))
}

/// Returns canonical configured origins, always including the public base origin.
fn allowed_origin_values(config: &ExperimentConfig) -> Result<Vec<HeaderValue>> {
    let mut origins = config.server.allowed_origins.clone();
    origins.push(canonical_origin(&config.server.public_base_url)?);
    origins.sort();
    origins.dedup();
    origins
        .into_iter()
        .map(|origin| {
            origin
                .parse::<HeaderValue>()
                .map_err(|error| anyhow!("invalid allowed origin {origin:?}: {error}"))
        })
        .collect()
}

/// Extracts a scheme-and-authority origin from an absolute HTTP(S) URL.
fn canonical_origin(url: &str) -> Result<String> {
    let uri = url
        .parse::<http::Uri>()
        .map_err(|error| anyhow!("invalid public URL {url:?}: {error}"))?;
    let scheme = uri
        .scheme_str()
        .filter(|scheme| matches!(*scheme, "http" | "https"))
        .ok_or_else(|| anyhow!("public URL must use http or https"))?;
    let authority = uri
        .authority()
        .ok_or_else(|| anyhow!("public URL must contain an authority"))?;
    Ok(format!("{scheme}://{authority}"))
}

/// Enforces the configured browser origin policy on WebSocket upgrades.
fn validate_websocket_origin(
    config: &ExperimentConfig,
    headers: &HeaderMap,
) -> Result<(), AppError> {
    let origin = headers.get(ORIGIN).and_then(|value| value.to_str().ok());
    if origin.is_some_and(|origin| origin_matches_request_host(origin, headers)) {
        return Ok(());
    }
    let public_origin = canonical_origin(&config.server.public_base_url)
        .map_err(|error| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    if origin.is_none() && is_loopback_origin(&public_origin) {
        return Ok(());
    }
    let Some(origin) = origin else {
        return Err(AppError::forbidden("WebSocket Origin header is required"));
    };
    let mut allowed = config.server.allowed_origins.clone();
    allowed.push(public_origin);
    if allowed.iter().any(|candidate| candidate == origin) {
        Ok(())
    } else {
        Err(AppError::forbidden("WebSocket origin is not allowed"))
    }
}

/// Rejects cross-site administrator mutations using Origin and Fetch Metadata signals.
fn validate_admin_request_origin(
    config: &ExperimentConfig,
    headers: &HeaderMap,
) -> Result<(), AppError> {
    if headers
        .get("sec-fetch-site")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == "cross-site")
    {
        return Err(AppError::forbidden(
            "Cross-site administrator request rejected",
        ));
    }
    let Some(origin) = headers.get(ORIGIN).and_then(|value| value.to_str().ok()) else {
        let public_origin = canonical_origin(&config.server.public_base_url)
            .map_err(|error| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
        if is_loopback_origin(&public_origin) {
            return Ok(());
        }
        return Err(AppError::forbidden(
            "Administrator Origin header is required",
        ));
    };
    if origin_matches_request_host(origin, headers) {
        return Ok(());
    }
    let mut allowed = config.server.allowed_origins.clone();
    allowed.push(
        canonical_origin(&config.server.public_base_url)
            .map_err(|error| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?,
    );
    if allowed.iter().any(|candidate| candidate == origin) {
        Ok(())
    } else {
        Err(AppError::forbidden("Administrator origin is not allowed"))
    }
}

/// Returns whether an Origin represents the same authority as the HTTP Host header.
fn origin_matches_request_host(origin: &str, headers: &HeaderMap) -> bool {
    let Some(host) = headers.get(HOST).and_then(|value| value.to_str().ok()) else {
        return false;
    };
    origin
        .parse::<http::Uri>()
        .ok()
        .and_then(|uri| uri.authority().map(ToString::to_string))
        .is_some_and(|authority| authority.eq_ignore_ascii_case(host))
}

/// Returns whether a canonical HTTP origin names a local loopback host.
fn is_loopback_origin(origin: &str) -> bool {
    origin.starts_with("http://localhost")
        || origin.starts_with("http://127.0.0.1")
        || origin.starts_with("http://[::1]")
}

/// Builds an origin-independent WebSocket path under the experiment route prefix.
fn websocket_path(config: &ExperimentConfig, suffix: &str) -> String {
    let route_prefix = config
        .server
        .public_base_url
        .parse::<http::Uri>()
        .ok()
        .map(|uri| uri.path().trim_end_matches('/').to_string())
        .unwrap_or_default();
    format!("{route_prefix}/{}", suffix.trim_start_matches('/'))
}

/// Adds browser hardening headers to every normal HTTP response.
#[derive(Clone)]
struct CspNonce(String);

/// Adds browser hardening headers and a request-specific script nonce.
async fn security_headers<A: Game>(
    State(_state): State<Arc<AppState<A>>>,
    mut request: Request,
    next: Next,
) -> Response {
    let no_store =
        request.uri().path().starts_with("/api/") || request.uri().path().starts_with("/admin");
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    request.extensions_mut().insert(CspNonce(nonce.clone()));
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    if no_store {
        headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    headers.insert(
        HeaderName::from_static("content-security-policy"),
        HeaderValue::from_str(&format!(
            "default-src 'self'; connect-src 'self' ws: wss:; img-src 'self' data:; style-src 'self' 'unsafe-inline'; script-src 'self' 'nonce-{nonce}'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'"
        ))
        .expect("generated CSP nonce is valid header text"),
    );
    headers.insert(
        HeaderName::from_static("strict-transport-security"),
        HeaderValue::from_static("max-age=31536000; includeSubDomains"),
    );
    headers.insert(
        HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        HeaderName::from_static("x-frame-options"),
        HeaderValue::from_static("DENY"),
    );
    headers.insert(
        HeaderName::from_static("referrer-policy"),
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        HeaderName::from_static("permissions-policy"),
        HeaderValue::from_static("camera=(), geolocation=(), payment=(), usb=()"),
    );
    response
}

/// Runs bounded periodic cleanup for transient credentials, sessions, and tickets.
fn spawn_security_cleanup<A: Game>(state: Arc<AppState<A>>, clean_admin_sessions: bool) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        loop {
            interval.tick().await;
            let expired_participants = state.participant_auth.cleanup().await;
            state.upgrade_tickets.cleanup().await;
            if clean_admin_sessions {
                if let Err(error) = state.admin_auth.cleanup().await {
                    tracing::warn!(%error, "failed to clean expired administrator sessions");
                }
            }
            cleanup_transient_rooms(&state).await;
            let participant_ids_in_rooms = state
                .memory
                .read()
                .await
                .rooms
                .values()
                .flat_map(|room| room.participants.keys().cloned())
                .collect::<HashSet<_>>();
            state
                .memory
                .write()
                .await
                .participants
                .retain(|participant_id, _| {
                    !expired_participants.contains(participant_id)
                        || participant_ids_in_rooms.contains(participant_id)
                });
            let unattached_timeout = state.config.session.waiting_room_timeout_seconds.max(1);
            let stale_unattached = {
                let now = chrono::Utc::now();
                let mut memory = state.memory.write().await;
                let stale = memory
                    .participants
                    .iter()
                    .filter(|(participant_id, participant)| {
                        !participant_ids_in_rooms.contains(*participant_id)
                            && chrono::DateTime::parse_from_rfc3339(&participant.updated_at)
                                .ok()
                                .is_some_and(|updated| {
                                    updated.with_timezone(&chrono::Utc)
                                        < now - chrono::Duration::seconds(unattached_timeout)
                                })
                    })
                    .map(|(participant_id, _)| participant_id.clone())
                    .collect::<Vec<_>>();
                for participant_id in &stale {
                    memory.participants.remove(participant_id);
                }
                stale
            };
            for participant_id in &stale_unattached {
                state
                    .participant_auth
                    .revoke_participant_session(participant_id)
                    .await;
            }
            if !stale_unattached.is_empty() {
                let stale = stale_unattached.iter().collect::<HashSet<_>>();
                state
                    .chat_submission_budgets
                    .write()
                    .await
                    .retain(|participant_id, _| !stale.contains(participant_id));
                state.rejection_windows.write().await.retain(|key, _| {
                    !stale_unattached
                        .iter()
                        .any(|participant_id| key.contains(participant_id))
                });
            }
        }
    });
}

/// Returns whether a runtime room has reached a durable terminal lifecycle state.
fn room_status_is_terminal(status: &str) -> bool {
    matches!(status, "completed" | "abandoned")
}

/// Removes or expires transient rooms after configured waiting, idle, and lifetime bounds.
async fn cleanup_transient_rooms<A: Game>(state: &Arc<AppState<A>>) {
    let now = chrono::Utc::now();
    let waiting_timeout = state.config.session.waiting_room_timeout_seconds.max(1);
    let reconnect_grace = state.config.session.reconnect_grace_seconds.max(1);
    let idle_timeout = state.config.session.session_idle_timeout_seconds.max(1);
    let max_lifetime = state.config.session.session_max_lifetime_seconds.max(1);
    let candidates = {
        let memory = state.memory.read().await;
        memory
            .rooms
            .iter()
            .filter_map(|(room_id, room)| {
                let created = chrono::DateTime::parse_from_rfc3339(&room.created_at)
                    .ok()?
                    .with_timezone(&chrono::Utc);
                let updated = chrono::DateTime::parse_from_rfc3339(&room.updated_at)
                    .ok()?
                    .with_timezone(&chrono::Utc);
                let has_connection = room
                    .participants
                    .values()
                    .any(|participant| participant.connected);
                let disconnected_since = room
                    .participants
                    .values()
                    .filter_map(|participant| {
                        chrono::DateTime::parse_from_rfc3339(&participant.updated_at)
                            .ok()
                            .map(|timestamp| timestamp.with_timezone(&chrono::Utc))
                    })
                    .max()
                    .unwrap_or(updated);
                let reason = if !room_status_is_terminal(&room.status)
                    && created < now - chrono::Duration::seconds(max_lifetime)
                {
                    Some("maximum_lifetime")
                } else if room.status == "waiting"
                    && updated < now - chrono::Duration::seconds(waiting_timeout)
                {
                    Some("waiting_timeout")
                } else if room.status == "running"
                    && updated < now - chrono::Duration::seconds(idle_timeout)
                {
                    Some("idle_timeout")
                } else if !room_status_is_terminal(&room.status)
                    && !has_connection
                    && disconnected_since < now - chrono::Duration::seconds(reconnect_grace)
                {
                    Some("reconnect_timeout")
                } else if room_status_is_terminal(&room.status)
                    && !has_connection
                    && disconnected_since < now - chrono::Duration::seconds(reconnect_grace)
                {
                    Some("terminal_cleanup")
                } else {
                    None
                }?;
                Some((
                    room_id.clone(),
                    room.experiment_id.clone(),
                    room.session_id,
                    room.updated_at.clone(),
                    reason,
                ))
            })
            .collect::<Vec<_>>()
    };
    let mut removed = Vec::new();
    for (room_id, experiment_id, session_id, observed_updated_at, reason) in candidates {
        let removed_current_room = {
            let mut memory = state.memory.write().await;
            if memory
                .rooms
                .get(&room_id)
                .is_some_and(|room| room.updated_at == observed_updated_at)
            {
                memory.rooms.remove(&room_id);
                true
            } else {
                false
            }
        };
        if !removed_current_room {
            continue;
        }
        if reason != "terminal_cleanup" && session_id > 0 {
            persist_event(
                "session_expired",
                state
                    .store
                    .expire_session(&experiment_id, session_id, reason),
            )
            .await;
        }
        removed.push(room_id);
    }
    if removed.is_empty() {
        return;
    }
    let removed_set = removed.iter().collect::<HashSet<_>>();
    state
        .room_buses
        .write()
        .await
        .retain(|room_id, _| !removed_set.contains(room_id));
    state
        .room_transition_locks
        .write()
        .await
        .retain(|room_id, _| !removed_set.contains(room_id));
    state.committed_transcripts.write().await.retain(|key| {
        !removed
            .iter()
            .any(|room_id| key.starts_with(&format!("{room_id}:")))
    });
    state.agent_inboxes.write().await.retain(|key, _| {
        !removed
            .iter()
            .any(|room_id| key.starts_with(&format!("{room_id}:")))
    });
    let pending_agents = {
        let mut pending = state.pending_agents.lock().await;
        let keys = pending
            .keys()
            .filter(|key| {
                removed
                    .iter()
                    .any(|room_id| key.starts_with(&format!("{room_id}:")))
            })
            .cloned()
            .collect::<Vec<_>>();
        keys.into_iter()
            .filter_map(|key| pending.remove(&key))
            .collect::<Vec<_>>()
    };
    for mut agent in pending_agents {
        let _ = tokio::time::timeout(Duration::from_secs(5), agent.shutdown()).await;
    }
    state.game_connections.write().await.retain(|key, _| {
        !removed
            .iter()
            .any(|room_id| key.starts_with(&format!("{room_id}:")))
    });
    state.audio_connections.write().await.retain(|key, _| {
        !removed
            .iter()
            .any(|room_id| key.starts_with(&format!("{room_id}:")))
    });
    state.rejection_windows.write().await.retain(|key, _| {
        !removed
            .iter()
            .any(|room_id| key.starts_with(&format!("{room_id}\0")))
    });
}

/// Applies a process-wide safety ceiling to unauthenticated participant creation bursts.
async fn enforce_creation_rate<A: Game>(state: &Arc<AppState<A>>) -> Result<(), AppError> {
    let now = chrono::Utc::now().timestamp();
    let mut attempts = state.participant_creation_window.write().await;
    attempts.record(now).then_some(()).ok_or_else(|| {
        AppError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "Participant creation rate limit exceeded",
        )
    })
}

/// Authenticates participant API requests and attaches the resolved principal.
async fn require_participant_auth<A: Game>(
    State(state): State<Arc<AppState<A>>>,
    mut request: Request,
    next: Next,
) -> Result<Response, AppError> {
    let credential = bearer_token(request.headers())
        .ok_or_else(|| AppError::new(StatusCode::UNAUTHORIZED, "Authentication required"))?;
    let principal = state
        .participant_auth
        .authenticate(credential)
        .await
        .ok_or_else(|| AppError::new(StatusCode::UNAUTHORIZED, "Invalid participant credential"))?;
    request.extensions_mut().insert(principal);
    Ok(next.run(request).await)
}

/// Restricts all administrator surfaces to configured direct-peer CIDR ranges.
async fn require_admin_network<A: Game>(
    State(state): State<Arc<AppState<A>>>,
    request: Request,
    next: Next,
) -> Result<Response, AppError> {
    let ranges = state
        .game_settings
        .read()
        .await
        .admin_allowed_ip_ranges
        .clone();
    if ranges.is_empty() {
        return Ok(next.run(request).await);
    }
    let client_ip = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|peer| peer.0.ip());
    let allowed = admin_network_allows(&ranges, client_ip);
    if !allowed {
        tracing::warn!(?client_ip, "administrator network policy rejected request");
        return Err(AppError::new(
            StatusCode::FORBIDDEN,
            "Administrator access is not available from this network",
        ));
    }
    Ok(next.run(request).await)
}

/// Applies a validated CIDR list to one direct network peer.
fn admin_network_allows(ranges: &[String], client_ip: Option<IpAddr>) -> bool {
    ranges.is_empty()
        || client_ip.is_some_and(|client_ip| {
            ranges.iter().any(|range| {
                range
                    .parse::<ipnet::IpNet>()
                    .is_ok_and(|network| network.contains(&client_ip))
            })
        })
}

/// Authenticates every administrator route and enforces role and CSRF on mutations.
async fn require_admin_auth<A: Game>(
    State(state): State<Arc<AppState<A>>>,
    mut request: Request,
    next: Next,
) -> Result<Response, AppError> {
    let browser_page =
        request.method() == Method::GET && request.uri().path().starts_with("/admin/");
    let Some(token) = cookie_value(request.headers(), "parlando_admin") else {
        if browser_page {
            return Ok(Redirect::temporary("/admin/login").into_response());
        }
        return Err(AppError::new(
            StatusCode::UNAUTHORIZED,
            "Authentication required",
        ));
    };
    if !state.admin_auth.is_configured().await {
        if browser_page {
            return Ok(Redirect::temporary("/admin/login").into_response());
        }
        return Err(AppError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "Administrator authentication is not configured",
        ));
    }
    let Some(session) = state.admin_auth.authenticate(token).await? else {
        if browser_page {
            return Ok(Redirect::temporary("/admin/login").into_response());
        }
        return Err(AppError::new(
            StatusCode::UNAUTHORIZED,
            "Invalid administrator session",
        ));
    };
    if request.method() != Method::GET {
        validate_admin_request_origin(&state.config, request.headers())?;
        let csrf = request
            .headers()
            .get("x-csrf-token")
            .and_then(|value| value.to_str().ok());
        if csrf != Some(session.csrf_token.as_str()) {
            return Err(AppError::forbidden("Invalid CSRF token"));
        }
        if request.uri().path() != "/api/admin/logout" && !session.role.may_mutate_experiments() {
            return Err(AppError::forbidden("Administrator role required"));
        }
    }
    request.extensions_mut().insert(session);
    Ok(next.run(request).await)
}

/// Returns the bearer value from an RFC 6750 Authorization header.
fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .filter(|token| !token.is_empty())
}

/// Returns one named cookie value without logging the surrounding cookie header.
fn cookie_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get(COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .find_map(|part| {
            let (candidate, value) = part.trim().split_once('=')?;
            (candidate == name).then_some(value)
        })
}

/// Returns the participant identity established by bearer authentication.
fn authenticated_participant_id(
    principal: Option<Extension<ParticipantPrincipal>>,
) -> Result<String, AppError> {
    if let Some(Extension(principal)) = principal {
        return Ok(principal.participant_session_id);
    }
    Err(AppError::new(
        StatusCode::UNAUTHORIZED,
        "Authentication required",
    ))
}

/// Redirects the short administrator URL away from the participant SPA fallback.
async fn admin_entry() -> Redirect {
    Redirect::temporary("/admin/login")
}

/// Renders first-run administrator setup or the normal login form.
async fn admin_login_page<A: Game>(
    State(state): State<Arc<AppState<A>>>,
    Extension(nonce): Extension<CspNonce>,
) -> Html<String> {
    let setup = !state.admin_auth.is_configured().await;
    let (
        title,
        eyebrow,
        description,
        button,
        endpoint,
        confirmation,
        autocomplete,
        password_minimum,
        setup_note,
    ) = if setup {
        (
            "Create your administrator account",
            "First-time setup",
            "Set the credentials you will use to manage experiments and inspect sessions.",
            "Create administrator",
            "/api/admin/setup",
            r#"<label class="field"><span>Confirm password</span><input name="password_confirmation" type="password" autocomplete="new-password" required minlength="12"><small>Enter the same password again.</small></label>"#,
            "new-password",
            r#" minlength="12""#,
            r#"<div class="setup-note"><span class="setup-note-icon">✓</span><div><strong>Stored securely</strong><span>Only an Argon2id password hash is saved in this database.</span></div></div>"#,
        )
    } else {
        (
            "Welcome back",
            "Administrator access",
            "Sign in to manage experiments and review sessions.",
            "Sign in",
            "/api/admin/login",
            "",
            "current-password",
            "",
            "",
        )
    };
    Html(format!(
        r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>{title} · Parlando</title>
  <style>
    :root {{ font-family: Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; background: #f6f7f9; color: #182026; }}
    * {{ box-sizing: border-box; }}
    body {{ margin: 0; min-height: 100vh; }}
    button, input {{ font: inherit; }}
    .app-header {{ align-items: center; background: #fff; border-bottom: 1px solid #dde2e7; display: flex; min-height: 64px; padding: 12px 24px; }}
    .brand {{ display: grid; gap: 3px; }}
    .brand-name {{ font-size: 20px; font-weight: 800; line-height: 1.15; }}
    .brand-section {{ color: #65717c; font-size: 12px; font-weight: 700; text-transform: uppercase; }}
    .page {{ align-items: center; display: flex; justify-content: center; min-height: calc(100vh - 64px); padding: 48px 20px; }}
    .auth-shell {{ display: grid; grid-template-columns: minmax(0, 42%) minmax(0, 58%); max-width: 880px; overflow: hidden; width: 100%; background: #fff; border: 1px solid #dce2e8; border-radius: 12px; box-shadow: 0 18px 60px rgba(20, 34, 45, .12); }}
    .context {{ background: #185f8f; color: #fff; display: flex; flex-direction: column; justify-content: space-between; min-height: 500px; padding: 42px; position: relative; }}
    .context::after {{ background: rgba(255,255,255,.08); border-radius: 999px; content: ""; height: 240px; position: absolute; right: -110px; top: -90px; width: 240px; }}
    .mark {{ align-items: center; background: rgba(255,255,255,.14); border: 1px solid rgba(255,255,255,.22); border-radius: 10px; display: inline-flex; font-size: 24px; font-weight: 850; height: 48px; justify-content: center; letter-spacing: -.04em; width: 48px; }}
    .context-copy {{ position: relative; z-index: 1; }}
    .context h2 {{ font-size: 29px; letter-spacing: -.02em; line-height: 1.15; margin: 24px 0 12px; max-width: 300px; }}
    .context p {{ color: #dbeaf4; line-height: 1.6; margin: 0; max-width: 310px; }}
    .context-footer {{ color: #c9dfed; font-size: 12px; font-weight: 650; position: relative; z-index: 1; }}
    .auth-panel {{ display: flex; flex-direction: column; justify-content: center; padding: 48px; }}
    .eyebrow {{ color: #185f8f; font-size: 12px; font-weight: 800; letter-spacing: .08em; margin-bottom: 10px; text-transform: uppercase; }}
    h1 {{ font-size: 28px; letter-spacing: -.02em; line-height: 1.2; margin: 0; }}
    .intro {{ color: #65717c; line-height: 1.55; margin: 10px 0 28px; }}
    form {{ display: grid; gap: 18px; }}
    .field {{ display: grid; gap: 7px; }}
    .field > span {{ color: #31404c; font-size: 13px; font-weight: 750; }}
    .field small {{ color: #76838e; font-size: 12px; }}
    input {{ appearance: none; background: #fff; border: 1px solid #c9d2da; border-radius: 7px; color: #182026; outline: none; padding: 11px 12px; transition: border-color 120ms ease, box-shadow 120ms ease; width: 100%; }}
    input:hover {{ border-color: #9fadb8; }}
    input:focus {{ border-color: #185f8f; box-shadow: 0 0 0 3px rgba(24,95,143,.14); }}
    .primary {{ background: #185f8f; border: 1px solid #185f8f; border-radius: 7px; color: #fff; cursor: pointer; font-weight: 750; margin-top: 4px; min-height: 44px; padding: 10px 16px; transition: background 120ms ease, transform 80ms ease; }}
    .primary:hover {{ background: #124f79; }}
    .primary:active {{ transform: translateY(1px); }}
    .primary:disabled {{ cursor: wait; opacity: .7; }}
    .setup-note {{ align-items: flex-start; background: #f1f7fb; border: 1px solid #cfe2ef; border-radius: 8px; color: #31404c; display: flex; gap: 10px; margin-bottom: 22px; padding: 12px; }}
    .setup-note-icon {{ align-items: center; background: #dceef8; border-radius: 999px; color: #135276; display: inline-flex; flex: 0 0 auto; font-size: 12px; font-weight: 900; height: 23px; justify-content: center; width: 23px; }}
    .setup-note div {{ display: grid; gap: 2px; }}
    .setup-note strong {{ font-size: 13px; }}
    .setup-note div span {{ color: #5b6873; font-size: 12px; line-height: 1.4; }}
    .error {{ background: #fff8f7; border: 1px solid #efc3bf; border-radius: 7px; color: #9f1d16; display: none; font-size: 13px; line-height: 1.4; margin: 16px 0 0; padding: 10px 12px; }}
    .error.visible {{ display: block; }}
    @media (max-width: 720px) {{
      .app-header {{ padding: 10px 16px; }}
      .brand-name {{ font-size: 18px; }}
      .page {{ align-items: flex-start; padding: 24px 14px; }}
      .auth-shell {{ display: block; }}
      .context {{ min-height: auto; padding: 26px; }}
      .context h2 {{ font-size: 23px; margin-top: 18px; }}
      .context-footer {{ display: none; }}
      .auth-panel {{ padding: 30px 26px 34px; }}
      h1 {{ font-size: 24px; }}
    }}
  </style>
</head>
<body>
  <header class="app-header"><div class="brand"><span class="brand-name">Parlando</span><span class="brand-section">Experimenter dashboard</span></div></header>
  <main class="page">
    <section class="auth-shell" aria-labelledby="auth-title">
      <aside class="context"><div class="context-copy"><div class="mark" aria-hidden="true">P</div><h2>Dialogue experiments, clearly managed.</h2><p>Configure experiments, follow live sessions, and export research data from one focused workspace.</p></div><div class="context-footer">Secure administrator area</div></aside>
      <div class="auth-panel">
        <div class="eyebrow">{eyebrow}</div>
        <h1 id="auth-title">{title}</h1>
        <p class="intro">{description}</p>
        {setup_note}
        <form id="admin-form">
          <label class="field"><span>Username</span><input name="username" autocomplete="username" required maxlength="128" autofocus></label>
          <label class="field"><span>Password</span><input name="password" type="password" autocomplete="{autocomplete}" required{password_minimum}><small>{password_help}</small></label>
          {confirmation}
          <button class="primary" type="submit">{button}</button>
        </form>
        <p class="error" id="error" role="alert"></p>
      </div>
    </section>
  </main>
  <script nonce="{nonce}">const form=document.getElementById('admin-form');const error=document.getElementById('error');const button=form.querySelector('button');const showError=message=>{{error.textContent=message;error.classList.add('visible');}};form.addEventListener('submit',async e=>{{e.preventDefault();error.classList.remove('visible');const f=new FormData(form);const password=f.get('password');const confirmation=f.get('password_confirmation');if(confirmation!==null&&password!==confirmation){{showError('Passwords do not match.');return;}}button.disabled=true;button.textContent='{pending_label}';try{{const r=await fetch('{endpoint}',{{method:'POST',headers:{{'content-type':'application/json'}},body:JSON.stringify({{username:f.get('username'),password}})}});if(r.ok){{location.href='/admin/experiments';return;}}showError(await r.text()||'Request failed.');}}catch(_){{showError('Could not reach the server. Please try again.');}}finally{{button.disabled=false;button.textContent='{button}';}}}});</script>
</body>
</html>"#,
        password_help = if setup {
            "Use at least 12 characters."
        } else {
            "Enter the password for this administrator account."
        },
        pending_label = if setup {
            "Creating…"
        } else {
            "Signing in…"
        },
        nonce = nonce.0,
    ))
}

#[derive(Deserialize)]
struct AdminLoginRequest {
    username: String,
    password: String,
}

/// Persists the first administrator credential and signs that administrator in.
async fn admin_setup<A: Game>(
    State(state): State<Arc<AppState<A>>>,
    headers: HeaderMap,
    Json(request): Json<AdminLoginRequest>,
) -> Result<Response, AppError> {
    validate_admin_request_origin(&state.config, &headers)?;
    if !state
        .admin_auth
        .setup(&request.username, &request.password)
        .await
        .map_err(|error| AppError::bad_request(error.to_string()))?
    {
        return Err(AppError::new(
            StatusCode::CONFLICT,
            "Administrator setup is already complete",
        ));
    }
    tracing::info!(username = %request.username, "first administrator created");
    admin_login_response(&state, &request.username, &request.password).await
}

/// Verifies the built-in administrator credential and issues a secure session cookie.
async fn admin_login<A: Game>(
    State(state): State<Arc<AppState<A>>>,
    headers: HeaderMap,
    Json(request): Json<AdminLoginRequest>,
) -> Result<Response, AppError> {
    validate_admin_request_origin(&state.config, &headers)?;
    admin_login_response(&state, &request.username, &request.password).await
}

/// Verifies a credential and builds the secure browser-session response.
async fn admin_login_response<A: Game>(
    state: &Arc<AppState<A>>,
    username: &str,
    password: &str,
) -> Result<Response, AppError> {
    let login = state
        .admin_auth
        .login(username, password)
        .await
        .map_err(AppError::from)?;
    let (token, session) = match login {
        AdminLoginResult::Authenticated(token, session) => (token, session),
        AdminLoginResult::Invalid => {
            tracing::warn!(username, "administrator login failed");
            return Err(AppError::new(
                StatusCode::UNAUTHORIZED,
                "Invalid administrator credentials",
            ));
        }
        AdminLoginResult::Busy => {
            tracing::warn!("administrator login concurrency limit reached");
            return Err(AppError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "Administrator login is busy; try again shortly",
            ));
        }
    };
    tracing::info!(username, role = ?session.role, "administrator login succeeded");
    let mut response = Json(json!({"ok": true, "csrf_token": session.csrf_token})).into_response();
    response.headers_mut().insert(
        SET_COOKIE,
        HeaderValue::from_str(&administrator_cookie(
            &token,
            ADMIN_ABSOLUTE_SECONDS,
            &state.config,
        ))
        .map_err(|error| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?,
    );
    Ok(response)
}

/// Revokes the current administrator session and expires its cookie.
async fn admin_logout<A: Game>(
    State(state): State<Arc<AppState<A>>>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    if let Some(token) = cookie_value(&headers, "parlando_admin") {
        state.admin_auth.logout(token).await?;
    }
    tracing::info!("administrator logged out");
    let mut response = Json(json!({"ok": true})).into_response();
    response.headers_mut().insert(
        SET_COOKIE,
        HeaderValue::from_str(&administrator_cookie("", 0, &state.config))
            .map_err(|error| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?,
    );
    Ok(response)
}

/// Builds one host-only first-party administrator cookie for HTTPS or loopback HTTP.
fn administrator_cookie(token: &str, max_age: i64, config: &ExperimentConfig) -> String {
    let secure = config.server.public_base_url.starts_with("https://");
    format!(
        "parlando_admin={token}; Path=/; Max-Age={max_age}; {}HttpOnly; SameSite=Strict",
        if secure { "Secure; " } else { "" }
    )
}

/// Shared server state used by HTTP handlers, WebSocket tasks, and background agents.
pub struct AppState<A: Game> {
    pub adapter: Arc<A>,
    pub config: ExperimentConfig,
    /// Typed game-owned configuration validated when this experiment runtime is built.
    pub game_config: A::Config,
    pub experiment_id: String,
    /// Identity of the one compiled game which owns this experiment.
    pub game_descriptor: GameMetadata,
    /// Settings shared by every experiment of this compiled game.
    pub game_settings: Arc<RwLock<StoredGameSettings>>,
    /// Process bootstrap credentials used only when an experiment has no override.
    bootstrap_secrets: Arc<HashMap<String, String>>,
    /// Immutable configuration revision used for newly created sessions.
    pub config_revision: i64,
    experiment_lifecycle: RwLock<ExperimentLifecycle>,
    pub memory: RwLock<MemoryState<A::State>>,
    pub store: SharedExperimentStore,
    pub room_buses: RwLock<HashMap<String, broadcast::Sender<ServerMessage>>>,
    pub agent_factory: Option<SharedAgentFactory<A>>,
    /// Agent choices registered by the compiled server and rendered by the dashboard.
    pub agent_definitions: Vec<AgentDefinition>,
    pub started_agents: RwLock<HashSet<String>>,
    pending_agents: Mutex<HashMap<String, Box<dyn Agent<A> + Send>>>,
    agent_inboxes: RwLock<HashMap<String, mpsc::Sender<AgentObservation<A>>>>,
    pub tts_provider: Option<Arc<dyn StreamingTtsProvider>>,
    /// Whether the embedding application supplied TTS independently of dashboard credentials.
    tts_provider_is_override: bool,
    pub audio_publisher: Option<Arc<dyn AgentAudioPublisher>>,
    pub audio_rooms: SharedAudioRooms,
    pub transcription_provider: Option<Arc<dyn TranscriptionProvider>>,
    /// Whether the embedding application supplied transcription independently of dashboard credentials.
    transcription_provider_is_override: bool,
    committed_transcripts: RwLock<HashSet<String>>,
    participant_auth: ParticipantAuthenticator,
    upgrade_tickets: UpgradeTicketStore,
    admin_auth: Arc<AdminAuthenticator>,
    participant_creation_window: RwLock<ParticipantCreationRate>,
    chat_submission_budgets: RwLock<HashMap<String, TokenBucket>>,
    rejection_windows: RwLock<HashMap<String, RejectionWindow>>,
    telemetry: Arc<RuntimeTelemetry>,
    /// Weak references to every experiment runtime hosted by this game process.
    runtime_registry: Arc<RwLock<HashMap<String, Weak<AppState<A>>>>>,
    room_transition_locks: RwLock<HashMap<String, Arc<Mutex<()>>>>,
    game_connections: RwLock<HashMap<String, ConnectionControl>>,
    audio_connections: RwLock<HashMap<String, ConnectionControl>>,
    pub version_manifest: Value,
}

/// Selects a historical experiment for storage-only administrator handlers.
#[derive(Clone)]
struct AdminExperimentScope(String);

/// Returns the explicitly routed experiment or the runtime's own experiment.
fn admin_experiment_id<A: Game>(
    state: &AppState<A>,
    scope: Option<&Extension<AdminExperimentScope>>,
) -> String {
    scope
        .map(|Extension(scope)| scope.0.clone())
        .unwrap_or_else(|| state.experiment_id.clone())
}

/// Sliding-window participant-creation counters bounded by cleanup on each request.
#[derive(Default)]
struct ParticipantCreationRate {
    /// Process-wide creation timestamps used as a final safety ceiling.
    global: Vec<i64>,
}

impl ParticipantCreationRate {
    /// Records one allowed attempt after pruning the bounded sixty-second window.
    fn record(&mut self, now: i64) -> bool {
        self.global.retain(|timestamp| *timestamp > now - 60);
        if self.global.len() >= 300 {
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

mod lifecycle;
use lifecycle::{require_open_experiment, ExperimentLifecycle};
mod telemetry;
use telemetry::{
    CapacitySample, ConnectionLiveness, ConnectionLivenessSnapshot, ConnectionSample, LoadSample,
    ParticipantLiveness, RuntimeTelemetry, SessionLiveness,
};

/// Event delivered to one started agent instance.
enum AgentObservation<A: Game> {
    /// Accepted action plus role-specific state snapshot after the action.
    Action {
        actor: PlayerRole,
        action: A::Action,
        resulting_observation: A::Observation,
        completion: Option<A::Completion>,
    },
    /// Conversation message with its sender role.
    Message { speaker: PlayerRole, text: String },
}

/// Cancellation handle for the single live connection that owns a participant room role.
struct ConnectionControl {
    generation: String,
    shutdown: mpsc::Sender<()>,
    liveness: Arc<ConnectionLiveness>,
}

impl<A: Game> AppState<A> {
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
async fn persist_session_event<A: Game>(
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

/// Appends a session event and propagates any persistence failure to the caller.
async fn persist_session_event_required<A: Game>(
    state: &Arc<AppState<A>>,
    room_id: &str,
    participant_session_id: Option<&str>,
    event_type: &'static str,
    payload: Value,
    game_state: Option<Value>,
) -> Result<()> {
    let record = session_event_record(
        state,
        room_id,
        participant_session_id,
        event_type,
        payload,
        game_state,
    )
    .await?;
    state.store.append_session_event(record).await?;
    Ok(())
}

/// Resolves one session event record without writing it, for atomic transition batches.
async fn session_event_record<A: Game>(
    state: &Arc<AppState<A>>,
    room_id: &str,
    participant_session_id: Option<&str>,
    event_type: &'static str,
    payload: Value,
    game_state: Option<Value>,
) -> Result<SessionEventRecord> {
    let (experiment_id, session_id, actor) = {
        let memory = state.memory.read().await;
        let room = memory
            .rooms
            .get(room_id)
            .ok_or_else(|| anyhow!("Room not found."))?;
        let actor = participant_session_id.and_then(|id| {
            room.participants.get(id).map(|participant| {
                (
                    participant.participant_id,
                    participant.role.as_str().to_string(),
                )
            })
        });
        (room.experiment_id.clone(), room.session_id, actor)
    };
    if session_id <= 0 {
        return Err(anyhow!("Room does not have a durable session."));
    }
    let (actor_participant_id, actor_role) = actor
        .map(|(participant_id, role)| (Some(participant_id), Some(role)))
        .unwrap_or((None, None));
    Ok(SessionEventRecord {
        experiment_id,
        session_id,
        event_type: event_type.to_string(),
        actor_participant_id,
        actor_role,
        payload,
        game_state,
    })
}

/// Returns the serialization lock that orders durable transitions for one room.
async fn room_transition_lock<A: Game>(state: &Arc<AppState<A>>, room_id: &str) -> Arc<Mutex<()>> {
    let mut locks = state.room_transition_locks.write().await;
    locks
        .entry(room_id.to_string())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

/// Refreshes a room's meaningful-activity timestamp without treating heartbeats as activity.
async fn touch_room_activity<A: Game>(state: &Arc<AppState<A>>, room_id: &str) {
    if let Some(room) = state.memory.write().await.rooms.get_mut(room_id) {
        room.updated_at = now_iso();
    }
}

/// Registers one role-owned connection and immediately cancels the previous owner.
async fn register_connection(
    connections: &RwLock<HashMap<String, ConnectionControl>>,
    key: String,
    liveness: Arc<ConnectionLiveness>,
) -> (String, mpsc::Receiver<()>, bool) {
    let generation = new_id("connection");
    let (shutdown, receiver) = mpsc::channel(1);
    if let Some(previous) = connections.write().await.insert(
        key,
        ConnectionControl {
            generation: generation.clone(),
            shutdown,
            liveness,
        },
    ) {
        let _ = previous.shutdown.try_send(());
        return (generation, receiver, true);
    }
    (generation, receiver, false)
}

/// Removes a connection control only when the finishing socket still owns its generation.
async fn unregister_connection(
    connections: &RwLock<HashMap<String, ConnectionControl>>,
    key: &str,
    generation: &str,
) -> bool {
    let mut connections = connections.write().await;
    if connections
        .get(key)
        .is_some_and(|control| control.generation == generation)
    {
        connections.remove(key);
        true
    } else {
        false
    }
}

/// Classifies one current game transport from its most recent heartbeat or message.
fn game_connection_health(snapshot: &ConnectionLivenessSnapshot, now_ms: i64) -> &'static str {
    let observed_at = snapshot
        .last_heartbeat_at_ms
        .unwrap_or(snapshot.last_message_at_ms);
    match now_ms.saturating_sub(observed_at) {
        0..=5_000 => "live",
        5_001..=15_000 => "delayed",
        _ => "stale",
    }
}

/// Takes a short-lock snapshot of current runtime capacity and operational counters.
async fn build_load_sample<A: Game>(state: &Arc<AppState<A>>) -> LoadSample {
    let now = chrono::Utc::now();
    let now_ms = now.timestamp_millis();
    let (waiting, active, completed, unattached, transcription) = {
        let memory = state.memory.read().await;
        let waiting = memory
            .rooms
            .values()
            .filter(|room| room.status == "waiting" && room.participants.len() < 2)
            .count();
        let completed = memory
            .rooms
            .values()
            .filter(|room| room.status == "completed")
            .count();
        let active = memory.rooms.len().saturating_sub(waiting);
        let attached = memory
            .rooms
            .values()
            .flat_map(|room| room.participants.keys().cloned())
            .collect::<HashSet<_>>();
        let unattached = memory
            .participants
            .keys()
            .filter(|participant_id| !attached.contains(*participant_id))
            .count();
        let transcription = if speechmatics_readiness_required(&state.config) {
            memory
                .rooms
                .values()
                .flat_map(|room| room.participants.values())
                .filter(|participant| participant.source != "agent")
                .count()
        } else {
            0
        };
        (waiting, active, completed, unattached, transcription)
    };
    let game_snapshots = state
        .game_connections
        .read()
        .await
        .values()
        .map(|control| control.liveness.snapshot())
        .collect::<Vec<_>>();
    let game_live = game_snapshots
        .iter()
        .filter(|snapshot| game_connection_health(snapshot, now_ms) == "live")
        .count();
    let game_delayed = game_snapshots
        .iter()
        .filter(|snapshot| game_connection_health(snapshot, now_ms) == "delayed")
        .count();
    let game_stale = game_snapshots.len() - game_live - game_delayed;
    let audio_connections = state.audio_connections.read().await.len();
    let pending_rejections = state
        .rejection_windows
        .read()
        .await
        .values()
        .map(|window| window.occurrences)
        .sum();
    let storage = match state.store.storage_capacity().await {
        Ok(storage) => storage,
        Err(error) => {
            tracing::warn!(%error, "could not sample storage capacity for load telemetry");
            None
        }
    };
    LoadSample {
        sampled_at: now.to_rfc3339(),
        sampled_at_ms: now_ms,
        counters: state.telemetry.counters(),
        capacity: CapacitySample {
            active_reserved_sessions: active,
            active_session_limit: state.config.capacity.max_active_sessions,
            waiting_sessions: waiting,
            waiting_session_limit: state.config.capacity.max_waiting_sessions,
            completed_retained_sessions: completed,
            unattached_participants: unattached,
            unattached_participant_limit: state.config.capacity.max_unattached_participants,
            transcription_streams_reserved: transcription,
            transcription_stream_limit: state.config.capacity.max_transcription_streams,
            active_agents: state.started_agents.read().await.len(),
            storage_reserve_bytes: state
                .config
                .capacity
                .storage_reserve_megabytes
                .saturating_mul(1024 * 1024),
        },
        connections: ConnectionSample {
            game_connections: game_snapshots.len(),
            game_live,
            game_delayed,
            game_stale,
            audio_connections,
        },
        pending_rejections,
        storage,
    }
}

/// Adds one candidate timeout and keeps the earliest deadline affecting a room.
fn retain_earliest_deadline(
    current: &mut Option<(chrono::DateTime<chrono::Utc>, String)>,
    timestamp: &str,
    seconds: i64,
    reason: &str,
) {
    let Some(base) = chrono::DateTime::parse_from_rfc3339(timestamp)
        .ok()
        .map(|value| value.with_timezone(&chrono::Utc))
    else {
        return;
    };
    let candidate = (
        base + chrono::Duration::seconds(seconds.max(1)),
        reason.to_string(),
    );
    if current
        .as_ref()
        .is_none_or(|(deadline, _)| candidate.0 < *deadline)
    {
        *current = Some(candidate);
    }
}

/// Minimal participant projection copied while taking a liveness snapshot.
struct ParticipantOperationalSnapshot {
    role: Seat,
    source: String,
    connected: bool,
    audio_ready: bool,
    updated_at: String,
}

/// Minimal room projection that intentionally excludes potentially large game state.
struct RoomOperationalSnapshot {
    session_id: i64,
    room_id: String,
    status: String,
    created_at: String,
    updated_at: String,
    participants: Vec<ParticipantOperationalSnapshot>,
}

/// Builds participant and lifecycle liveness without writing heartbeat data to storage.
async fn build_session_liveness<A: Game>(state: &Arc<AppState<A>>) -> Vec<SessionLiveness> {
    let rooms = {
        let memory = state.memory.read().await;
        memory
            .rooms
            .values()
            .map(|room| RoomOperationalSnapshot {
                session_id: room.session_id,
                room_id: room.id.clone(),
                status: room.status.clone(),
                created_at: room.created_at.clone(),
                updated_at: room.updated_at.clone(),
                participants: room
                    .participants
                    .values()
                    .map(|participant| ParticipantOperationalSnapshot {
                        role: participant.role,
                        source: participant.source.clone(),
                        connected: participant.connected,
                        audio_ready: participant.audio_ready,
                        updated_at: participant.updated_at.clone(),
                    })
                    .collect(),
            })
            .collect::<Vec<_>>()
    };
    let game = state
        .game_connections
        .read()
        .await
        .iter()
        .map(|(key, control)| (key.clone(), control.liveness.snapshot()))
        .collect::<HashMap<_, _>>();
    let audio = state
        .audio_connections
        .read()
        .await
        .iter()
        .map(|(key, control)| (key.clone(), control.liveness.snapshot()))
        .collect::<HashMap<_, _>>();
    let now_ms = chrono::Utc::now().timestamp_millis();
    let mut sessions = rooms
        .into_iter()
        .map(|room| {
            let mut deadline = None;
            if room.status == "waiting" {
                retain_earliest_deadline(
                    &mut deadline,
                    &room.updated_at,
                    state.config.session.waiting_room_timeout_seconds,
                    "waiting timeout",
                );
            } else if room.status == "running" {
                retain_earliest_deadline(
                    &mut deadline,
                    &room.updated_at,
                    state.config.session.session_idle_timeout_seconds,
                    "idle timeout",
                );
            }
            if !room_status_is_terminal(&room.status) {
                retain_earliest_deadline(
                    &mut deadline,
                    &room.created_at,
                    state.config.session.session_max_lifetime_seconds,
                    "maximum lifetime",
                );
            }
            if !room_status_is_terminal(&room.status)
                && room.participants.iter().all(|row| !row.connected)
            {
                if let Some(latest) = room
                    .participants
                    .iter()
                    .max_by(|left, right| left.updated_at.cmp(&right.updated_at))
                {
                    retain_earliest_deadline(
                        &mut deadline,
                        &latest.updated_at,
                        state.config.session.reconnect_grace_seconds,
                        "reconnect timeout",
                    );
                }
            }
            let participants = room
                .participants
                .iter()
                .map(|participant| {
                    let key = format!("{}:{}", room.room_id, participant.role.as_str());
                    let game_transport = game.get(&key).cloned();
                    let game_health = if participant.source == "agent" {
                        "server"
                    } else {
                        game_transport
                            .as_ref()
                            .map(|snapshot| game_connection_health(snapshot, now_ms))
                            .unwrap_or(if room.status == "waiting" {
                                "waiting"
                            } else {
                                "disconnected"
                            })
                    };
                    ParticipantLiveness {
                        role: participant.role.as_str().to_string(),
                        source: participant.source.clone(),
                        game_health: game_health.to_string(),
                        game: game_transport,
                        audio: audio.get(&key).cloned(),
                        audio_ready: participant.audio_ready,
                    }
                })
                .collect::<Vec<_>>();
            let browser_participants = participants
                .iter()
                .filter(|participant| participant.source != "agent")
                .collect::<Vec<_>>();
            let health = if room_status_is_terminal(&room.status) {
                if room.status == "completed" {
                    "completed"
                } else {
                    "abandoned"
                }
            } else if browser_participants
                .iter()
                .any(|participant| participant.game_health == "stale")
            {
                "stale"
            } else if browser_participants
                .iter()
                .any(|participant| participant.game_health == "delayed")
            {
                "delayed"
            } else if room.status == "running"
                && browser_participants
                    .iter()
                    .any(|participant| participant.game_health == "disconnected")
            {
                "disconnected"
            } else if browser_participants
                .iter()
                .any(|participant| participant.game_health == "live")
            {
                "live"
            } else if room.status == "waiting" {
                "waiting"
            } else {
                "disconnected"
            };
            SessionLiveness {
                experiment_id: state.experiment_id.clone(),
                session_id: room.session_id,
                room_id: room.room_id,
                status: room.status,
                health: health.to_string(),
                meaningful_activity_at: room.updated_at,
                lifecycle_deadline_at: deadline.as_ref().map(|(value, _)| value.to_rfc3339()),
                deadline_reason: deadline.map(|(_, reason)| reason),
                participants,
            }
        })
        .collect::<Vec<_>>();
    sessions.sort_by_key(|session| session.session_id);
    sessions
}

/// Returns strong references to every currently hosted experiment runtime.
async fn hosted_runtime_states<A: Game>(state: &Arc<AppState<A>>) -> Vec<Arc<AppState<A>>> {
    let mut runtimes = state
        .runtime_registry
        .read()
        .await
        .values()
        .filter_map(Weak::upgrade)
        .collect::<Vec<_>>();
    runtimes.sort_by(|left, right| left.experiment_id.cmp(&right.experiment_id));
    runtimes
}

/// Aggregates current capacity and transport use across every hosted experiment runtime.
async fn build_game_load_sample<A: Game>(state: &Arc<AppState<A>>) -> LoadSample {
    let runtimes = hosted_runtime_states(state).await;
    let mut samples = Vec::with_capacity(runtimes.len().max(1));
    if runtimes.is_empty() {
        samples.push(build_load_sample(state).await);
    } else {
        for runtime in runtimes {
            samples.push(build_load_sample(&runtime).await);
        }
    }
    let mut aggregate = samples.remove(0);
    for sample in samples {
        aggregate.capacity.active_reserved_sessions += sample.capacity.active_reserved_sessions;
        aggregate.capacity.active_session_limit += sample.capacity.active_session_limit;
        aggregate.capacity.waiting_sessions += sample.capacity.waiting_sessions;
        aggregate.capacity.waiting_session_limit += sample.capacity.waiting_session_limit;
        aggregate.capacity.completed_retained_sessions +=
            sample.capacity.completed_retained_sessions;
        aggregate.capacity.unattached_participants += sample.capacity.unattached_participants;
        aggregate.capacity.unattached_participant_limit +=
            sample.capacity.unattached_participant_limit;
        aggregate.capacity.transcription_streams_reserved +=
            sample.capacity.transcription_streams_reserved;
        aggregate.capacity.transcription_stream_limit += sample.capacity.transcription_stream_limit;
        aggregate.capacity.active_agents += sample.capacity.active_agents;
        aggregate.capacity.storage_reserve_bytes = aggregate
            .capacity
            .storage_reserve_bytes
            .max(sample.capacity.storage_reserve_bytes);
        aggregate.connections.game_connections += sample.connections.game_connections;
        aggregate.connections.game_live += sample.connections.game_live;
        aggregate.connections.game_delayed += sample.connections.game_delayed;
        aggregate.connections.game_stale += sample.connections.game_stale;
        aggregate.connections.audio_connections += sample.connections.audio_connections;
        aggregate.pending_rejections += sample.pending_rejections;
    }
    aggregate.counters = state.telemetry.counters();
    aggregate
}

/// Returns liveness for every in-memory session across the hosted game process.
async fn build_game_session_liveness<A: Game>(state: &Arc<AppState<A>>) -> Vec<SessionLiveness> {
    let mut sessions = Vec::new();
    for runtime in hosted_runtime_states(state).await {
        sessions.extend(build_session_liveness(&runtime).await);
    }
    sessions.sort_by(|left, right| {
        left.experiment_id
            .cmp(&right.experiment_id)
            .then(left.session_id.cmp(&right.session_id))
    });
    sessions
}

/// Samples bounded game-process operational history every five seconds.
fn spawn_load_sampler<A: Game>(state: Arc<AppState<A>>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(5));
        loop {
            interval.tick().await;
            let sample = build_game_load_sample(&state).await;
            state.telemetry.push_sample(sample).await;
        }
    });
}

/// Returns game-wide current load, one-hour history, and runtime session liveness.
async fn admin_load<A: Game>(
    State(state): State<Arc<AppState<A>>>,
) -> Result<Json<Value>, AppError> {
    let current = build_game_load_sample(&state).await;
    let history = state.telemetry.history().await;
    let sessions = build_game_session_liveness(&state).await;
    Ok(Json(json!({
        "current": current,
        "history": history,
        "sessions": sessions,
        "sampling": { "interval_seconds": 5, "retention_seconds": 3600 },
        "liveness_thresholds_ms": { "live": 5000, "delayed": 15000, "socket_timeout": 90000 },
    })))
}

/// Measures HTTP concurrency, response classes, and latency for the load dashboard.
async fn track_request_load<A: Game>(
    State(state): State<Arc<AppState<A>>>,
    request: Request,
    next: Next,
) -> Response {
    let guard = state.telemetry.begin_request();
    let response = next.run(request).await;
    guard.finish(response.status());
    response
}

/// Persists one participant's session-local role and connection status.
async fn persist_session_participant<A: Game>(
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
        tracing::error!(error = %value, "internal request failure");
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error")
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (self.status, self.message).into_response()
    }
}

mod routing;
pub use routing::serve_game;
#[cfg(any(test, feature = "internal-tools"))]
pub use routing::{build_game_router, build_router};

async fn health<A: Game>(State(state): State<Arc<AppState<A>>>) -> Result<Json<Value>, AppError> {
    tokio::time::timeout(Duration::from_secs(2), state.store.health_check())
        .await
        .map_err(|_| AppError::new(StatusCode::SERVICE_UNAVAILABLE, "storage check timed out"))?
        .map_err(|error| {
            tracing::error!(%error, "health check could not acquire SQLite write transaction");
            AppError::new(StatusCode::SERVICE_UNAVAILABLE, "storage unavailable")
        })?;
    Ok(Json(json!({"status": "ok", "storage": "read_write"})))
}

async fn public_config<A: Game>(
    State(state): State<Arc<AppState<A>>>,
) -> Json<PublicConfigResponse> {
    let config = &state.config;
    let game_settings = state.game_settings.read().await.clone();
    Json(PublicConfigResponse {
        game_name: state.game_descriptor.name.clone(),
        experiment_status: state.experiment_lifecycle.read().await.as_str().to_string(),
        institution: nonempty_string(&game_settings.institution),
        participant_information_version: nonempty_string(
            &config.direct.participant_information_version,
        ),
        participant_information_url: nonempty_string(&config.direct.participant_information_url),
        consents: expanded_consent_items(config, &game_settings)
            .iter()
            .map(|item| ConsentItemResponse {
                id: item.id.clone(),
                title: item.title.clone(),
                body: item.body.clone(),
                required: item.required,
            })
            .collect(),
        voice: json!({
            "enabled": config.voice.enabled,
        }),
    })
}

async fn create_participant<A: Game>(
    State(state): State<Arc<AppState<A>>>,
    Json(request): Json<ParticipantCreateRequest>,
) -> Result<Json<ParticipantCreateResponse>, AppError> {
    create_participant_inner(state, request).await.map(Json)
}

async fn create_participant_inner<A: Game>(
    state: Arc<AppState<A>>,
    _request: ParticipantCreateRequest,
) -> Result<ParticipantCreateResponse, AppError> {
    let _intake_guard = require_open_experiment(&state).await?;
    enforce_creation_rate(&state).await?;
    if !state.config.direct.enabled {
        return Err(AppError::not_found("Direct mode is disabled."));
    }
    let mut memory = state.memory.write().await;
    let attached_participants = memory
        .rooms
        .values()
        .flat_map(|room| room.participants.keys())
        .collect::<HashSet<_>>();
    let unattached_participants = memory
        .participants
        .keys()
        .filter(|participant_id| !attached_participants.contains(participant_id))
        .count();
    if unattached_participants >= state.config.capacity.max_unattached_participants {
        return Err(AppError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "Participant intake is temporarily full",
        ));
    }
    let participant_id = state
        .store
        .upsert_participant(ParticipantRecord {
            experiment_id: state.experiment_id.clone(),
            participant_kind: "human".to_string(),
            identity_provider: "direct".to_string(),
            external_id: None,
            metadata: Value::Null,
        })
        .await?;
    let research_id = state
        .store
        .participant_research_id(participant_id)
        .await?
        .ok_or_else(|| {
            AppError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Participant identifier is missing",
            )
        })?;
    let participant = memory.create_participant(
        participant_id,
        research_id,
        "direct".to_string(),
        _intake_guard.data_purpose().to_string(),
    );
    let participant_credential = state.participant_auth.issue(participant.id.clone()).await;
    Ok(ParticipantCreateResponse {
        participant_credential,
        participant_id: participant.research_id,
    })
}

async fn consent<A: Game>(
    State(state): State<Arc<AppState<A>>>,
    principal: Option<Extension<ParticipantPrincipal>>,
    Json(request): Json<ConsentRequest>,
) -> Result<Json<Value>, AppError> {
    let participant_session_id = authenticated_participant_id(principal)?;
    let configured_items = state
        .config
        .direct
        .consents
        .iter()
        .map(|item| (item.id.as_str(), item))
        .collect::<HashMap<_, _>>();
    if request
        .decisions
        .keys()
        .any(|item_id| !configured_items.contains_key(item_id.as_str()))
    {
        return Err(AppError::bad_request("Unknown consent item"));
    }
    let (participant_id, purpose, current_decisions) = {
        let memory = state.memory.read().await;
        let participant = memory
            .participants
            .get(&participant_session_id)
            .ok_or_else(|| AppError::not_found("Participant session not found."))?;
        (
            participant.participant_id,
            participant.purpose.clone(),
            participant.consent_decisions.clone(),
        )
    };
    let changed_decisions = request
        .decisions
        .iter()
        .filter(|(item_id, accepted)| current_decisions.get(*item_id) != Some(*accepted))
        .map(|(item_id, accepted)| (item_id.clone(), *accepted))
        .collect::<HashMap<_, _>>();
    if changed_decisions.is_empty() {
        return Ok(Json(json!({"ok": true})));
    }
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
    let game_settings = state.game_settings.read().await.clone();
    let consent_text_hash = Some(consent_configuration_hash(&state.config, &game_settings)?);
    let consent_metadata = json!({
        "participant_information_version": nonempty_string(&state.config.direct.participant_information_version),
        "participant_information_url": nonempty_string(&state.config.direct.participant_information_url),
    });
    for (consent_item_id, accepted) in &changed_decisions {
        state
            .store
            .record_consent_declaration(ConsentDeclarationRecord {
                experiment_id: state.experiment_id.clone(),
                session_id,
                participant_id,
                purpose: purpose.clone(),
                consent_item_id: consent_item_id.clone(),
                accepted: *accepted,
                consent_text_hash: consent_text_hash.clone(),
                metadata: consent_metadata.clone(),
            })
            .await?;
    }
    let mut memory = state.memory.write().await;
    let participant = memory
        .participants
        .get_mut(&participant_session_id)
        .ok_or_else(|| AppError::not_found("Participant session not found."))?;
    participant
        .consent_decisions
        .extend(changed_decisions.clone());
    participant.updated_at = now_iso();
    for room in memory.rooms.values_mut() {
        if request
            .room_id
            .as_ref()
            .is_some_and(|room_id| room_id != &room.id)
        {
            continue;
        }
        if let Some(room_participant) = room.participants.get_mut(&participant_session_id) {
            room_participant
                .consent_decisions
                .extend(changed_decisions.clone());
            room_participant.updated_at = now_iso();
        }
    }
    Ok(Json(json!({"ok": true})))
}

async fn create_room<A: Game>(
    State(state): State<Arc<AppState<A>>>,
    principal: Option<Extension<ParticipantPrincipal>>,
    Json(_request): Json<CreateRoomRequest>,
) -> Result<Json<CreateRoomResponse>, AppError>
where
    A::State: Serialize,
{
    let _intake_guard = require_open_experiment(&state).await?;
    let participant_session_id = authenticated_participant_id(principal)?;
    require_consent(&state, &participant_session_id).await?;
    require_session_storage_reserve(&state).await?;
    let requested_mode = "direct".to_string();
    let mut prepared_agent = if state.config.agents.mode == AgentsMode::HumanVsAgent {
        let room_seed = rand::random::<u64>();
        let agent_seed = state
            .config
            .agents
            .human_vs_agent
            .as_ref()
            .and_then(|config| config.seed)
            .unwrap_or(room_seed);
        let raw_settings = state
            .config
            .agents
            .human_vs_agent
            .as_ref()
            .map(|config| config.config.clone())
            .unwrap_or_else(|| json!({}));
        let factory = state.agent_factory.clone().ok_or_else(|| {
            AppError::new(StatusCode::SERVICE_UNAVAILABLE, "No agent is registered")
        })?;
        let timeout = state
            .config
            .agents
            .human_vs_agent
            .as_ref()
            .map(|config| config.act_timeout_seconds)
            .unwrap_or(10.0);
        let definition = factory.definition();
        let settings = definition.normalize_settings(&raw_settings).map_err(|_| {
            AppError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "The configured agent settings are invalid",
            )
        })?;
        let (factory_secrets, agent_instance_secrets) = definition
            .resolve_secrets(&settings, &state.config.game_secrets)
            .map_err(|_| {
                AppError::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "The configured agent references an unavailable secret",
                )
            })?;
        let context = AgentContext {
            role: PlayerRole::B,
            seed: agent_seed,
            settings,
            factory_secrets,
            agent_instance_secrets,
        };
        let agent =
            match tokio::time::timeout(Duration::from_secs_f64(timeout), factory.create(context))
                .await
            {
                Ok(Ok(agent)) => agent,
                Ok(Err(_)) | Err(_) => {
                    return Err(AppError::new(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "The configured agent could not be initialized",
                    ));
                }
            };
        Some((room_seed, agent))
    } else {
        None
    };
    let paired_or_created: Result<(String, Seat, bool), AppError> = {
        let mut memory = state.memory.write().await;
        if state.config.agents.mode == AgentsMode::HumanVsHuman {
            if let Some(room_id) = open_human_room_for_pairing(
                &memory,
                &requested_mode,
                &memory
                    .participants
                    .get(&participant_session_id)
                    .ok_or_else(|| AppError::not_found("Participant session not found."))?
                    .purpose,
            ) {
                ensure_session_capacity(&state.config, &memory, SessionAdmission::Active, 1)?;
                let role = add_human_participant_to_room_locked(
                    &state,
                    &mut memory,
                    &room_id,
                    &participant_session_id,
                )?;
                Ok((room_id, role, false))
            } else {
                ensure_session_capacity(&state.config, &memory, SessionAdmission::Waiting, 1)?;
                let (room_id, role) = create_room_locked(
                    &state,
                    &mut memory,
                    participant_session_id.clone(),
                    requested_mode.clone(),
                    Seat::A,
                    rand::random::<u64>(),
                )?;
                let session_id = match state
                    .store
                    .create_session(SessionRecord {
                        experiment_id: state.experiment_id.clone(),
                        config_revision: state.config_revision,
                        game_version: state.game_descriptor.version.to_string(),
                        room_id: room_id.clone(),
                        mode: requested_mode.clone(),
                        status: "waiting".to_string(),
                        purpose: memory
                            .rooms
                            .get(&room_id)
                            .map(|room| room.purpose.clone())
                            .ok_or_else(|| AppError::not_found("Room not found."))?,
                    })
                    .await
                {
                    Ok(session_id) => session_id,
                    Err(error) => {
                        memory.rooms.remove(&room_id);
                        return Err(AppError::from(error));
                    }
                };
                if let Some(room) = memory.rooms.get_mut(&room_id) {
                    room.session_id = session_id;
                }
                Ok((room_id, role, true))
            }
        } else {
            ensure_session_capacity(&state.config, &memory, SessionAdmission::Active, 1)?;
            let (room_id, role) = create_room_locked(
                &state,
                &mut memory,
                participant_session_id.clone(),
                requested_mode.clone(),
                Seat::A,
                prepared_agent.as_ref().expect("agent was prepared").0,
            )?;
            let session_id = match state
                .store
                .create_session(SessionRecord {
                    experiment_id: state.experiment_id.clone(),
                    config_revision: state.config_revision,
                    game_version: state.game_descriptor.version.to_string(),
                    room_id: room_id.clone(),
                    mode: requested_mode.clone(),
                    status: "waiting".to_string(),
                    purpose: memory
                        .rooms
                        .get(&room_id)
                        .map(|room| room.purpose.clone())
                        .ok_or_else(|| AppError::not_found("Room not found."))?,
                })
                .await
            {
                Ok(session_id) => session_id,
                Err(error) => {
                    memory.rooms.remove(&room_id);
                    return Err(AppError::from(error));
                }
            };
            if let Some(room) = memory.rooms.get_mut(&room_id) {
                room.session_id = session_id;
            }
            Ok((room_id, role, true))
        }
    };
    let (room_id, role, created_room) = paired_or_created?;
    if created_room {
        let seed = state
            .memory
            .read()
            .await
            .rooms
            .get(&room_id)
            .map(|room| room.seed);
        persist_session_event(
            &state,
            &room_id,
            None,
            "session_created",
            json!({"room_id": room_id, "seed": seed}),
            None,
        )
        .await;
    }
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
    if state.config.agents.mode == AgentsMode::HumanVsAgent {
        let agent_id = add_agent_to_room(&state, &room_id).await?;
        let (_, agent) = prepared_agent.take().expect("agent was prepared");
        state
            .pending_agents
            .lock()
            .await
            .insert(agent_key(&room_id, &agent_id), agent);
    }
    let response = room_response(&state, &room_id, role).await?;
    Ok(Json(response))
}

/// Pauses only new session admission before SQLite exhausts its filesystem.
async fn require_session_storage_reserve<A: Game>(
    state: &Arc<AppState<A>>,
) -> Result<(), AppError> {
    let Some(capacity) = state.store.storage_capacity().await? else {
        return Ok(());
    };
    let reserve = state
        .config
        .capacity
        .storage_reserve_megabytes
        .saturating_mul(1024 * 1024);
    if capacity.available_bytes <= reserve {
        return Err(AppError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "New sessions are paused because storage reserve has been reached",
        ));
    }
    Ok(())
}

/// Admission class used by the single session-capacity policy.
#[derive(Clone, Copy)]
enum SessionAdmission {
    /// A human-human room still waiting for its second participant.
    Waiting,
    /// A fully allocated human-human or human-agent research session.
    Active,
}

/// Reserves room and ASR capacity before any durable session is created.
fn ensure_session_capacity<S>(
    config: &ExperimentConfig,
    memory: &MemoryState<S>,
    admission: SessionAdmission,
    transcription_streams_to_add: usize,
) -> Result<(), AppError> {
    let (waiting, active) = memory
        .rooms
        .values()
        .fold((0_usize, 0_usize), |counts, room| {
            if room.status == "waiting" && room.participants.len() < 2 {
                (counts.0 + 1, counts.1)
            } else {
                (counts.0, counts.1 + 1)
            }
        });
    match admission {
        SessionAdmission::Waiting if waiting >= config.capacity.max_waiting_sessions => {
            return Err(AppError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "Waiting-room capacity is temporarily full",
            ));
        }
        SessionAdmission::Active if active >= config.capacity.max_active_sessions => {
            return Err(AppError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "Active-session capacity is temporarily full",
            ));
        }
        _ => {}
    }
    if speechmatics_readiness_required(config) {
        let reserved_streams = memory
            .rooms
            .values()
            .flat_map(|room| room.participants.values())
            .filter(|participant| participant.source != "agent")
            .count();
        if reserved_streams + transcription_streams_to_add
            > config.capacity.max_transcription_streams
        {
            return Err(AppError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "Transcription capacity is temporarily full",
            ));
        }
    }
    Ok(())
}

async fn add_agent_to_room<A: Game>(
    state: &Arc<AppState<A>>,
    room_id: &str,
) -> Result<String, AppError> {
    let purpose = state
        .memory
        .read()
        .await
        .rooms
        .get(room_id)
        .map(|room| room.purpose.clone())
        .ok_or_else(|| AppError::not_found("Room not found."))?;
    let configured_agent = state.config.agents.human_vs_agent.as_ref();
    let raw_agent_settings = configured_agent
        .map(|config| config.config.clone())
        .unwrap_or_else(|| json!({}));
    let factory_definition = state
        .agent_factory
        .as_ref()
        .map(|factory| factory.definition())
        .ok_or_else(|| AppError::new(StatusCode::SERVICE_UNAVAILABLE, "No agent is registered"))?;
    let agent_settings = factory_definition
        .normalize_settings(&raw_agent_settings)
        .map_err(AppError::from)?;
    let fingerprint = configuration_fingerprint(&factory_definition.id, &agent_settings)
        .map_err(AppError::from)?;
    let factory_identity = state
        .agent_factory
        .as_ref()
        .map(|factory| factory.identity(&agent_settings))
        .transpose()
        .map_err(AppError::from)?
        .ok_or_else(|| AppError::new(StatusCode::SERVICE_UNAVAILABLE, "No agent is registered"))?;
    factory_identity.validate().map_err(AppError::from)?;
    let external_id = Some(format!(
        "{}@{}#{fingerprint}",
        factory_identity.name, factory_identity.version
    ));
    let metadata = json!({
        "agent_name": factory_identity.name,
        "agent_version": factory_identity.version,
        "factory": factory_definition.id,
        "configuration_fingerprint": fingerprint,
    });
    let agent_metadata = metadata.clone();
    let agent_participant_db_id = state
        .store
        .upsert_participant(ParticipantRecord {
            experiment_id: state.experiment_id.clone(),
            participant_kind: "agent".to_string(),
            identity_provider: "agent".to_string(),
            external_id,
            metadata,
        })
        .await?;
    let agent_research_id = state
        .store
        .participant_research_id(agent_participant_db_id)
        .await?
        .ok_or_else(|| {
            AppError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Agent identifier is missing",
            )
        })?;
    let agent_participant_id = {
        let mut memory = state.memory.write().await;
        if let Some(existing_agent) = memory
            .rooms
            .get(room_id)
            .and_then(|room| {
                room.participants
                    .values()
                    .find(|participant| participant.source == "agent")
            })
            .map(|participant| participant.participant_session_id.clone())
        {
            return Ok(existing_agent);
        }
        let agent = memory.create_participant(
            agent_participant_db_id,
            agent_research_id,
            "agent".to_string(),
            purpose,
        );
        let room = memory
            .rooms
            .get_mut(room_id)
            .ok_or_else(|| AppError::not_found("Room not found."))?;
        room.participants.insert(
            agent.id.clone(),
            RoomParticipant {
                participant_session_id: agent.id.clone(),
                participant_id: agent.participant_id,
                source: "agent".to_string(),
                role: Seat::B,
                connected: true,
                ready_declared: true,
                audio_ready: true,
                consent_decisions: HashMap::new(),
                joined_at: now_iso(),
                updated_at: now_iso(),
            },
        );
        agent.id
    };
    persist_session_participant(state, room_id, &agent_participant_id).await?;
    persist_session_event(
        state,
        room_id,
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
    Ok(agent_participant_id)
}

/// Finds an existing human-human waiting room that can accept one more human.
fn open_human_room_for_pairing<S>(
    memory: &MemoryState<S>,
    mode: &str,
    purpose: &str,
) -> Option<String> {
    memory
        .rooms
        .iter()
        .find(|(_, room)| {
            room.status == "waiting"
                && room.mode == mode
                && room.purpose == purpose
                && next_role(room) == Some(Seat::B)
                && room
                    .participants
                    .values()
                    .all(|participant| participant.source != "agent")
        })
        .map(|(room_id, _)| room_id.clone())
}

/// Adds a direct human participant to an existing room and returns its assigned seat.
fn add_human_participant_to_room_locked<A: Game>(
    state: &AppState<A>,
    memory: &mut MemoryState<A::State>,
    room_id: &str,
    participant_session_id: &str,
) -> Result<Seat, AppError> {
    let participant = memory
        .participants
        .get(participant_session_id)
        .ok_or_else(|| AppError::not_found("Participant session not found."))?
        .clone();
    let room = memory
        .rooms
        .get_mut(room_id)
        .ok_or_else(|| AppError::not_found("Room not found."))?;
    if room.purpose != participant.purpose {
        return Err(AppError::new(
            StatusCode::CONFLICT,
            "Testing and research participants cannot share a session",
        ));
    }
    if let Some(existing) = room.participants.get(participant_session_id) {
        return Ok(existing.role);
    }
    let role =
        next_role(room).ok_or_else(|| AppError::forbidden("Room already has two players."))?;
    room.participants.insert(
        participant_session_id.to_string(),
        RoomParticipant {
            participant_session_id: participant_session_id.to_string(),
            participant_id: participant.participant_id,
            source: "direct".to_string(),
            role,
            connected: false,
            ready_declared: false,
            audio_ready: !speechmatics_readiness_required(&state.config),
            consent_decisions: participant.consent_decisions,
            joined_at: now_iso(),
            updated_at: now_iso(),
        },
    );
    Ok(role)
}

fn create_room_locked<A: Game>(
    state: &AppState<A>,
    memory: &mut MemoryState<A::State>,
    participant_session_id: String,
    mode: String,
    role: Seat,
    seed: u64,
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
            ready_declared: false,
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
            purpose: participant.purpose,
            seed,
            state: state
                .adapter
                .initial_state(GameInitializationContext {
                    config: &state.game_config,
                    seed,
                    secrets: &SecretValues::new(
                        state.config.game_secrets.clone().into_iter().collect(),
                    ),
                })
                .map_err(|error| AppError::bad_request(error.to_string()))?,
            status: "waiting".to_string(),
            participants,
            created_at: now_iso(),
            updated_at: now_iso(),
        },
    );
    Ok((room_id, role))
}

async fn room_response<A: Game>(
    state: &Arc<AppState<A>>,
    room_id: &str,
    role: Seat,
) -> Result<RoomResponse, AppError>
where
    A::State: Serialize,
{
    let (presence, observation, available_actions) = {
        let memory = state.memory.read().await;
        let room = memory
            .rooms
            .get(room_id)
            .ok_or_else(|| AppError::not_found("Room not found."))?;
        let presence = room_presence(room);
        if room.status == "waiting" {
            return Ok(RoomResponse {
                room_id: room_id.to_string(),
                role: role.as_str().to_string(),
                presence: Some(presence),
                observation: None,
                available_actions: None,
            });
        }
        let player = role.player_role();
        (
            Some(presence),
            Some(protocol_json(
                &state.adapter.observation(&room.state, player),
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
        )
    };
    Ok(RoomResponse {
        room_id: room_id.to_string(),
        role: role.as_str().to_string(),
        presence,
        observation,
        available_actions,
    })
}

async fn audio_session<A: Game>(
    State(state): State<Arc<AppState<A>>>,
    Path(room_id): Path<String>,
    principal: Option<Extension<ParticipantPrincipal>>,
    Json(_request): Json<AudioSessionRequest>,
) -> Result<Json<AudioSessionPlanResponse>, AppError> {
    let participant_session_id = authenticated_participant_id(principal.clone())?;
    let role = participant_role(&state, &room_id, &participant_session_id).await?;
    if !state.config.voice.enabled {
        return Ok(Json(AudioSessionPlanResponse::disabled()));
    }
    let claims = UpgradeTicketClaims {
        room_id: room_id.clone(),
        participant_session_id,
        role: role.as_str().to_string(),
        generation: principal.map_or(1, |Extension(principal)| principal.generation),
        purpose: UpgradePurpose::Audio,
        expires_at: 0,
    };
    let token = state.upgrade_tickets.issue(claims).await;
    Ok(Json(AudioSessionPlanResponse {
        enabled: true,
        websocket_url: Some(websocket_path(
            &state.config,
            &format!("ws/audio/{room_id}"),
        )),
        token: Some(token),
        protocol_version: AUDIO_PROTOCOL_VERSION,
        sample_rate_hz: AUDIO_SAMPLE_RATE,
        channels: AUDIO_CHANNELS,
        frame_duration_ms: AUDIO_FRAME_DURATION_MS,
        jitter_buffer_ms: state.config.voice.jitter_buffer_ms,
    }))
}

/// Mints a purpose-bound, one-use game WebSocket upgrade ticket.
async fn game_session<A: Game>(
    State(state): State<Arc<AppState<A>>>,
    Path(room_id): Path<String>,
    Extension(principal): Extension<ParticipantPrincipal>,
) -> Result<Json<GameSessionPlanResponse>, AppError> {
    let role = participant_role(&state, &room_id, &principal.participant_session_id).await?;
    let token = state
        .upgrade_tickets
        .issue(UpgradeTicketClaims {
            room_id: room_id.clone(),
            participant_session_id: principal.participant_session_id,
            role: role.as_str().to_string(),
            generation: principal.generation,
            purpose: UpgradePurpose::Game,
            expires_at: 0,
        })
        .await;
    Ok(Json(GameSessionPlanResponse {
        websocket_url: websocket_path(&state.config, &format!("ws/game/{room_id}")),
        token,
    }))
}

/// Commits one final provider utterance to storage, conversation, and agents.
async fn commit_final_transcript<A: Game>(
    state: &Arc<AppState<A>>,
    room_id: &str,
    participant_session_id: &str,
    utterance: FinalTranscriptUtterance,
) -> Result<Option<TranscriptSegment>, AppError> {
    let role = participant_role(state, room_id, participant_session_id).await?;
    ensure_room_accepts_game_input(state, room_id).await?;
    let provider_identity = if utterance.result_ids.is_empty() {
        format!(
            "{}:{}:{}",
            utterance.start_time_ms, utterance.end_time_ms, utterance.text
        )
    } else {
        utterance.result_ids.join(",")
    };
    let idempotency_key = format!("{room_id}:{participant_session_id}:{provider_identity}");
    if !state
        .committed_transcripts
        .write()
        .await
        .insert(idempotency_key.clone())
    {
        return Ok(None);
    }
    let stored = TranscriptSegment {
        id: new_id("tr"),
        room_id: room_id.to_string(),
        participant_session_id: participant_session_id.to_string(),
        player: role.as_str().to_string(),
        start_time_ms: utterance.start_time_ms,
        end_time_ms: utterance.end_time_ms,
        text: utterance.text,
        metadata: json!({"provider":state.config.transcription.provider,"result_ids":utterance.result_ids}),
        created_at: now_iso(),
    };
    let message = ConversationMessageResponse {
        id: new_id("msg"),
        room_id: room_id.to_string(),
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
    let persist_result = async {
        persist_session_event_required(
            state,
            room_id,
            message.sender_participant_session_id.as_deref(),
            "conversation_message",
            serde_json::to_value(&message).unwrap(),
            None,
        )
        .await?;
        Result::<()>::Ok(())
    }
    .await;
    if let Err(error) = persist_result {
        state
            .committed_transcripts
            .write()
            .await
            .remove(&idempotency_key);
        return Err(AppError::from(error));
    }
    touch_room_activity(state, room_id).await;
    if let Some(player_message) = message.player_message() {
        let _ =
            state
                .room_bus(room_id)
                .await
                .send(ServerMessage::broadcast(ServerPayload::Message {
                    room_id: room_id.to_string(),
                    message: player_message,
                }));
    }
    if let Some(speaker) = player_role_from_str(&stored.player) {
        notify_agents_of_message(state, room_id, speaker, stored.text.clone()).await;
    }
    Ok(Some(stored))
}

/// Accepts a bounded operational voice metric without retaining it in research storage.
async fn add_voice_diagnostic<A: Game>(
    State(state): State<Arc<AppState<A>>>,
    Path(room_id): Path<String>,
    principal: Option<Extension<ParticipantPrincipal>>,
    Json(diagnostic): Json<VoiceDiagnosticIn>,
) -> Result<Json<Value>, AppError> {
    let participant_session_id = authenticated_participant_id(principal)?;
    participant_role(&state, &room_id, &participant_session_id).await?;
    if diagnostic.event.is_empty()
        || diagnostic.event.len() > 64
        || !diagnostic
            .event
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(AppError::bad_request("Invalid voice diagnostic event name"));
    }
    let _ = minimized_voice_diagnostic_metadata(&diagnostic.metadata);
    Ok(Json(json!({"stored": false})))
}

/// Keeps only non-identifying scalar metrics used to diagnose voice transport quality.
fn minimized_voice_diagnostic_metadata(metadata: &Value) -> Value {
    const ALLOWED: &[&str] = &[
        "buffered_samples",
        "channels",
        "count",
        "enabled",
        "frame_duration_ms",
        "jitter_buffer_ms",
        "protocol_version",
        "sample_rate_hz",
    ];
    let mut minimized = serde_json::Map::new();
    if let Some(values) = metadata.as_object() {
        for key in ALLOWED {
            if let Some(value) = values
                .get(*key)
                .filter(|value| value.is_boolean() || value.is_number() || value.is_null())
            {
                minimized.insert((*key).to_string(), value.clone());
            }
        }
    }
    Value::Object(minimized)
}

/// Adds a conversation message while honoring the participant-message storage switch.
async fn add_conversation<A: Game>(
    State(state): State<Arc<AppState<A>>>,
    Path(room_id): Path<String>,
    Json(input): Json<ConversationMessageIn>,
) -> Result<Json<ConversationMessageResponse>, AppError> {
    require_room(&state, &room_id).await?;
    ensure_room_accepts_game_input(&state, &room_id).await?;
    if input.text.chars().count() > 4_000 {
        return Err(AppError::bad_request("Conversation message is too long"));
    }
    if serde_json::to_vec(&input.metadata)
        .map_err(|error| AppError::bad_request(error.to_string()))?
        .len()
        > 8 * 1024
    {
        return Err(AppError::bad_request("Conversation metadata is too large"));
    }
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
    persist_session_event_required(
        &state,
        &room_id,
        message.sender_participant_session_id.as_deref(),
        "conversation_message",
        serde_json::to_value(&message).unwrap(),
        None,
    )
    .await?;
    touch_room_activity(&state, &room_id).await;
    if let Some(player_message) = message.player_message() {
        let _ = state
            .room_bus(&room_id)
            .await
            .send(ServerMessage::broadcast(ServerPayload::Message {
                room_id: room_id.clone(),
                message: player_message,
            }));
    }
    if message.origin == "typed" {
        state.telemetry.record_chat_accepted();
    }
    if let Some(speaker) = message
        .sender_role
        .as_deref()
        .and_then(player_role_from_str)
    {
        notify_agents_of_message(&state, &room_id, speaker, message.text.clone()).await;
    }
    Ok(Json(message))
}

/// Rejects participant game-channel input after a room has completed.
async fn ensure_room_accepts_game_input<A: Game>(
    state: &Arc<AppState<A>>,
    room_id: &str,
) -> Result<(), AppError> {
    let completed = {
        let memory = state.memory.read().await;
        memory
            .rooms
            .get(room_id)
            .ok_or_else(|| AppError::not_found("Room not found."))?
            .status
            == "completed"
    };
    if completed {
        return Err(AppError::forbidden(
            "Room is completed and no longer accepts game messages.",
        ));
    }
    Ok(())
}

#[derive(Clone, Debug, Deserialize)]
struct AdminSessionsQuery {
    limit: Option<i64>,
    status: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct AdminEventsQuery {
    after: Option<i64>,
}

#[derive(Clone, Debug, Deserialize)]
struct AdminUpdateExperimentStatusRequest {
    status: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AdminCreateExperimentRequest {
    experiment_id: String,
    config: Option<Value>,
    notes: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AdminCloneExperimentRequest {
    experiment_id: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AdminSaveExperimentConfigRequest {
    expected_revision: i64,
    config: Value,
    #[serde(default)]
    game_yaml: Option<String>,
    #[serde(default)]
    secret_updates: HashMap<String, String>,
    #[serde(default)]
    secret_deletions: Vec<String>,
    change_summary: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AdminRevealExperimentSecretRequest {
    key: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AdminValidateGameConfigRequest {
    game_yaml: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AdminCatalogueRequest {
    pinned: bool,
    notes: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AdminGameSettingsRequest {
    expected_revision: i64,
    institution: String,
    admin_allowed_ip_ranges: Vec<String>,
    speechmatics_realtime_url: Option<String>,
    #[serde(default)]
    secret_updates: HashMap<String, String>,
    #[serde(default)]
    secret_deletions: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AdminRevealGameSecretRequest {
    key: String,
}

#[derive(Clone, Debug, Deserialize)]
struct AdminExportQuery {
    format: Option<String>,
    variant: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
struct PrivacyStatus {
    generated_at: String,
    experiment_id: Option<String>,
    privacy_contract_version: String,
    overview: PrivacyOverviewStatus,
    configuration: Vec<PrivacyConfigurationStatus>,
    storage: Vec<PrivacyStorageStatus>,
    not_retained: Vec<PrivacyNonRetentionStatus>,
    raw_audio_stored_by_parlando: bool,
    external_services: Vec<PrivacyServiceStatus>,
    exports: PrivacyExportStatus,
    participant_deletion: PrivacyFeatureStatus,
    consent_evidence: PrivacyFeatureStatus,
}

/// One effective experiment setting that determines the report's processing claims.
#[derive(Clone, Debug, Serialize)]
struct PrivacyConfigurationStatus {
    setting: String,
    status: String,
    detail: String,
}

#[derive(Clone, Debug, Serialize)]
struct PrivacyOverviewStatus {
    purpose: String,
    primary_storage: String,
    access: String,
    retention: String,
}

#[derive(Clone, Debug, Serialize)]
struct PrivacyStorageStatus {
    category: String,
    persisted_when_produced: bool,
    purpose: String,
    detail: String,
}

#[derive(Clone, Debug, Serialize)]
struct PrivacyNonRetentionStatus {
    category: String,
    behavior: String,
    boundary: String,
}

#[derive(Clone, Debug, Serialize)]
struct PrivacyServiceStatus {
    service: String,
    enabled: bool,
    purpose: String,
    data_sent: String,
}

#[derive(Clone, Debug, Serialize)]
struct PrivacyExportStatus {
    available: bool,
    variant: String,
    schema_id: String,
    schema_sha256: String,
    scope: String,
    release_status: String,
    content_review_required: bool,
    formats: Vec<String>,
    identifiers: String,
    structure: String,
    timing: String,
    selection_rule: String,
    included_fields: Vec<PrivacyExportFieldStatus>,
    not_written: Vec<String>,
    detail: String,
}

#[derive(Clone, Debug, Serialize)]
struct PrivacyExportFieldStatus {
    section: String,
    description: String,
    fields: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
struct PrivacyFeatureStatus {
    available: bool,
    detail: String,
}

/// Serves the database-backed experiment/session dashboard.
async fn admin_experiments_page(Extension(nonce): Extension<CspNonce>) -> Html<String> {
    Html(ADMIN_EXPERIMENT_HTML.replacen("<script>", &format!("<script nonce=\"{}\">", nonce.0), 1))
}

/// Serves the installation-wide privacy status inside the protected administrator area.
async fn admin_privacy_page<A: Game>(
    State(state): State<Arc<AppState<A>>>,
) -> Result<Html<String>, AppError> {
    Ok(Html(render_privacy_status_html(
        &privacy_status(&state, None).await?,
    )))
}

/// Returns the installation-wide privacy status as structured JSON for administrative tooling.
async fn admin_privacy_json<A: Game>(
    State(state): State<Arc<AppState<A>>>,
) -> Result<Json<PrivacyStatus>, AppError> {
    Ok(Json(privacy_status(&state, None).await?))
}

/// Downloads the installation-wide privacy status as a pretty-printed JSON attachment.
async fn admin_privacy_json_download<A: Game>(
    State(state): State<Arc<AppState<A>>>,
) -> Result<Response, AppError> {
    let body = serde_json::to_string_pretty(&privacy_status(&state, None).await?)
        .map_err(|error| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    Ok(privacy_download_response(
        body,
        "application/json; charset=utf-8",
        "parlando-privacy-status.json",
    ))
}

/// Downloads the installation-wide privacy status as a Markdown attachment for review records.
async fn admin_privacy_markdown_download<A: Game>(
    State(state): State<Arc<AppState<A>>>,
) -> Result<Response, AppError> {
    Ok(privacy_download_response(
        render_privacy_status_markdown(&privacy_status(&state, None).await?),
        "text/markdown; charset=utf-8",
        "parlando-privacy-status.md",
    ))
}

/// Downloads the exact JSON Schema enforced by the corpus export projector.
async fn admin_corpus_export_schema() -> Response {
    privacy_download_response(
        CORPUS_EXPORT_SCHEMA_V1.to_string(),
        "application/schema+json; charset=utf-8",
        "parlando.corpus.v1.schema.json",
    )
}

/// Returns the privacy status for one experiment selected in the dashboard workspace.
async fn admin_experiment_privacy_json<A: Game>(
    State(state): State<Arc<AppState<A>>>,
    Path(experiment_id): Path<String>,
) -> Result<Json<PrivacyStatus>, AppError> {
    Ok(Json(privacy_status(&state, Some(&experiment_id)).await?))
}

/// Downloads one experiment's privacy status as a pretty-printed JSON attachment.
async fn admin_experiment_privacy_json_download<A: Game>(
    State(state): State<Arc<AppState<A>>>,
    Path(experiment_id): Path<String>,
) -> Result<Response, AppError> {
    let body =
        serde_json::to_string_pretty(&privacy_status(&state, Some(&experiment_id)).await?)
            .map_err(|error| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    Ok(privacy_download_response(
        body,
        "application/json; charset=utf-8",
        &format!("{experiment_id}-privacy-status.json"),
    ))
}

/// Downloads one experiment's privacy status as Markdown for review records.
async fn admin_experiment_privacy_markdown_download<A: Game>(
    State(state): State<Arc<AppState<A>>>,
    Path(experiment_id): Path<String>,
) -> Result<Response, AppError> {
    Ok(privacy_download_response(
        render_privacy_status_markdown(&privacy_status(&state, Some(&experiment_id)).await?),
        "text/markdown; charset=utf-8",
        &format!("{experiment_id}-privacy-status.md"),
    ))
}

/// Privacy-relevant projection which remains readable across configuration schema versions.
struct PrivacyConfigFacts {
    contract_version: String,
    transcription_enabled: bool,
    transcription_provider: String,
    tts_enabled: bool,
    tts_provider: String,
    voice_enabled: bool,
    human_vs_agent: bool,
    consent_items: usize,
    participant_information_version: String,
    participant_information_url: String,
}

/// Reads one boolean from a nested normalized experiment configuration.
fn privacy_config_bool(config: &Value, section: &str, key: &str) -> bool {
    config
        .get(section)
        .and_then(|value| value.get(key))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// Reads one string from a nested normalized experiment configuration.
fn privacy_config_string(config: &Value, section: &str, key: &str) -> String {
    config
        .get(section)
        .and_then(|value| value.get(key))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Projects privacy facts without requiring an old configuration to validate under today's schema.
fn privacy_config_facts(config: &Value) -> PrivacyConfigFacts {
    let information_version =
        privacy_config_string(config, "direct", "participant_information_version");
    let information_url = privacy_config_string(config, "direct", "participant_information_url");
    PrivacyConfigFacts {
        contract_version: privacy_config_string(config, "privacy", "contract_version"),
        transcription_enabled: privacy_config_bool(config, "transcription", "enabled"),
        transcription_provider: privacy_config_string(config, "transcription", "provider"),
        tts_enabled: privacy_config_bool(config, "tts", "enabled"),
        tts_provider: privacy_config_string(config, "tts", "provider"),
        voice_enabled: privacy_config_bool(config, "voice", "enabled"),
        human_vs_agent: privacy_config_string(config, "agents", "mode") == "human_vs_agent",
        consent_items: config
            .get("direct")
            .and_then(|value| value.get("consents"))
            .and_then(Value::as_array)
            .map_or(0, Vec::len),
        participant_information_version: information_version,
        participant_information_url: information_url,
    }
}

/// Converts a static corpus-schema field list into the serialized privacy descriptor.
fn privacy_field_names(fields: &[&str]) -> Vec<String> {
    fields.iter().map(|field| (*field).to_string()).collect()
}

/// Builds a privacy report for one experiment or, for tooling, the complete game process.
async fn privacy_status<A: Game>(
    state: &Arc<AppState<A>>,
    experiment_id: Option<&str>,
) -> Result<PrivacyStatus, AppError> {
    let mut facts = Vec::new();
    if let Some(experiment_id) = experiment_id {
        let definition = state
            .store
            .experiment_definition(experiment_id)
            .await?
            .ok_or_else(|| AppError::not_found("Experiment not found."))?;
        facts.push(privacy_config_facts(&definition.config));
    } else {
        let summaries = state.store.list_experiments(1_000).await?;
        facts.reserve(summaries.len());
        for summary in &summaries {
            if let Some(definition) = state
                .store
                .experiment_definition(&summary.experiment_id)
                .await?
            {
                facts.push(privacy_config_facts(&definition.config));
            }
        }
    }
    if facts.is_empty() {
        facts.push(privacy_config_facts(&persistable_config_json(
            &state.config,
        )?));
    }
    let total = facts.len();
    let mut contract_versions = facts
        .iter()
        .map(|fact| fact.contract_version.as_str())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    contract_versions.sort_unstable();
    contract_versions.dedup();
    let mut external_services = Vec::new();
    for fact in &facts {
        if fact.transcription_enabled
            && !external_services
                .iter()
                .any(|service: &PrivacyServiceStatus| {
                    service.service == fact.transcription_provider
                        && service.data_sent.starts_with("Live microphone audio")
                })
        {
            external_services.push(PrivacyServiceStatus {
                service: fact.transcription_provider.clone(),
                enabled: true,
                purpose: "Convert participant speech to text during the live session."
                    .to_string(),
                data_sent: "Live microphone audio. The provider returns transcript text and timing information to Parlando.".to_string(),
            });
        }
        if fact.tts_enabled
            && !external_services.iter().any(|service| {
                service.service == fact.tts_provider
                    && service
                        .data_sent
                        .starts_with("Software-agent-generated text")
            })
        {
            external_services.push(PrivacyServiceStatus {
                service: fact.tts_provider.clone(),
                enabled: true,
                purpose: "Convert a software agent's message to speech for participants."
                    .to_string(),
                data_sent: "The software agent's message text. The request does not contain participant audio, participant identifiers, or game state as separate fields. Because agent-generated text can repeat information from the conversation, the message itself must still be treated as session content.".to_string(),
            });
        }
    }
    let transcription = facts
        .iter()
        .filter(|fact| fact.transcription_enabled)
        .count();
    let voice = facts.iter().filter(|fact| fact.voice_enabled).count();
    let consent_items = facts.iter().map(|fact| fact.consent_items).sum::<usize>();
    let information_references = facts
        .iter()
        .filter(|fact| {
            !fact.participant_information_version.trim().is_empty()
                && !fact.participant_information_url.trim().is_empty()
        })
        .count();
    let consent_storage_detail = if total == 1 && consent_items == 0 {
        "No consent items are configured for this experiment, so Parlando does not create consent declaration records."
            .to_string()
    } else {
        "For each declaration, Parlando stores the consent item, whether it was accepted, declaration time, research or testing purpose, a cryptographic fingerprint of the complete text shown to the participant, and consent metadata. A fingerprint can verify unchanged text but does not contain the text itself; the configured consent text is retained in the versioned experiment configuration described above."
            .to_string()
    };
    let mut transcription_providers = facts
        .iter()
        .filter(|fact| fact.transcription_enabled)
        .map(|fact| fact.transcription_provider.as_str())
        .filter(|provider| !provider.is_empty())
        .collect::<Vec<_>>();
    transcription_providers.sort_unstable();
    transcription_providers.dedup();
    let provider_description = if transcription_providers.is_empty() {
        "the configured transcription provider".to_string()
    } else {
        transcription_providers.join(", ")
    };
    let transcript_storage_detail = if total == 1 && transcription == 0 {
        "Speech transcription is disabled. Parlando does not generate or store voice-transcript events for this experiment."
            .to_string()
    } else if total == 1 {
        format!(
            "Speech transcription through {provider_description} is enabled. Parlando stores event order and wall-clock time, speaking participant and role, final transcript text, and utterance timing returned by the service. It does not store interim recognition hypotheses."
        )
    } else {
        format!(
            "Speech transcription is enabled in {transcription} of {total} covered experiments. Those experiments retain event order and wall-clock time, speaking participant and role, final transcript text, and returned utterance timing; interim recognition hypotheses are not stored."
        )
    };
    let (audio_behavior, audio_boundary, interim_behavior, interim_boundary) = if total > 1 {
        (
            format!(
                "Across the {total} covered experiments, voice communication is enabled in {voice} and speech transcription is enabled in {transcription}. When voice is enabled, Parlando relays microphone audio live but does not store it."
            ),
            if transcription == 0 {
                "None of the covered experiments sends microphone audio to a transcription provider."
                    .to_string()
            } else {
                format!(
                    "Experiments with transcription enabled stream audio to {provider_description}; provider processing is governed by the institution's provider agreement and retention settings."
                )
            },
            if transcription == 0 {
                "None of the covered experiments generates or receives speech-recognition text."
                    .to_string()
            } else {
                "Experiments with transcription enabled store final transcript text but not changing interim recognition hypotheses."
                    .to_string()
            },
            "Final transcript text and timing are retained only for experiments with transcription enabled."
                .to_string(),
        )
    } else if voice == 0 {
        (
                "Voice communication is disabled. Parlando does not receive, relay, or store participant microphone audio for this experiment."
                    .to_string(),
                "No transcription provider receives microphone audio from Parlando."
                    .to_string(),
                "Speech transcription is disabled. Parlando does not generate or receive interim or final speech-recognition text for this experiment."
                    .to_string(),
                "Typed participant messages are unaffected and are stored as described above."
                    .to_string(),
            )
    } else if transcription == 0 {
        (
                "Voice communication is enabled. Parlando relays microphone audio live to the other participant but does not store it."
                    .to_string(),
                "Speech transcription is disabled, so Parlando does not send microphone audio to a transcription provider."
                    .to_string(),
                "Speech transcription is disabled. Parlando does not generate or receive speech-recognition text."
                    .to_string(),
                "Spoken audio is relayed live but is not converted to stored text by Parlando."
                    .to_string(),
            )
    } else {
        (
                format!(
                    "Voice communication and speech transcription are enabled. Parlando relays microphone audio live to the other participant and streams it to {provider_description}; Parlando does not store the audio."
                ),
                format!(
                    "{provider_description} processes the live stream under the institution's provider agreement and retention settings."
                ),
                "Parlando stores the final transcript returned for an utterance, not changing interim recognition hypotheses."
                    .to_string(),
                "Final transcript text and its timing are stored as described above."
                    .to_string(),
            )
    };
    let mut operational_sources = vec!["session lifecycle", "connection changes"];
    if facts.iter().any(|fact| fact.human_vs_agent) {
        operational_sources.push("software-agent status");
    }
    if !external_services.is_empty() {
        operational_sources.push("enabled external speech-service status");
    }
    let operational_event_detail = format!(
        "Events for {} are retained with event order and wall-clock time. For rejected action input, Parlando stores a reason, byte count, and cryptographic fingerprint rather than the rejected content. Operational events have no output field in the corpus unless the event is an accepted action or participant message.",
        operational_sources.join(", ")
    );
    let has_external_services = !external_services.is_empty();
    let (browser_diagnostic_behavior, browser_diagnostic_boundary) = if total == 1 && voice == 0 {
        (
            "Voice communication is disabled, so the browser does not start a Parlando voice session. Parlando does not store browser voice-diagnostic reports."
                .to_string(),
            "No browser voice-diagnostic records are expected for this experiment."
                .to_string(),
        )
    } else if total == 1 {
        (
            "During an enabled voice session, the browser may report temporary audio problems to the diagnostics endpoint. The endpoint does not write those reports to SQLite."
                .to_string(),
            "This guarantee concerns browser diagnostic reports; status events produced by enabled server-side speech services are retained."
                .to_string(),
        )
    } else {
        (
            format!(
                "Voice communication is enabled in {voice} of {total} covered experiments. Parlando does not write browser voice-diagnostic reports to SQLite."
            ),
            "This guarantee concerns browser diagnostic reports; status events produced by enabled server-side speech services are retained."
                .to_string(),
            )
    };
    let selected = (total == 1).then(|| &facts[0]);
    let configuration = if let Some(fact) = selected {
        let participant_information = if fact.participant_information_url.is_empty()
            && fact.participant_information_version.is_empty()
        {
            PrivacyConfigurationStatus {
                setting: "Participant information".to_string(),
                status: "Not configured".to_string(),
                detail: "This experiment has no participant-information document URL or version in its Parlando configuration."
                    .to_string(),
            }
        } else {
            PrivacyConfigurationStatus {
                setting: "Participant information".to_string(),
                status: "Configured".to_string(),
                detail: format!(
                    "Version: {}; document: {}.",
                    if fact.participant_information_version.is_empty() {
                        "not specified"
                    } else {
                        &fact.participant_information_version
                    },
                    if fact.participant_information_url.is_empty() {
                        "URL not specified"
                    } else {
                        &fact.participant_information_url
                    }
                ),
            }
        };
        vec![
            PrivacyConfigurationStatus {
                setting: "Participant arrangement".to_string(),
                status: if fact.human_vs_agent {
                    "One human and one software agent"
                } else {
                    "Two human participants"
                }
                .to_string(),
                detail: if fact.human_vs_agent {
                    "Each session assigns one human participant and one configured software agent."
                } else {
                    "Each session assigns two human participants; this experiment does not create software-agent participant records."
                }
                .to_string(),
            },
            PrivacyConfigurationStatus {
                setting: "Voice communication".to_string(),
                status: if fact.voice_enabled { "Enabled" } else { "Disabled" }.to_string(),
                detail: if fact.voice_enabled {
                    "The browser sends microphone audio to Parlando for live voice communication."
                } else {
                    "The browser does not start Parlando voice capture; Parlando therefore does not receive, relay, or store participant microphone audio for this experiment."
                }
                .to_string(),
            },
            PrivacyConfigurationStatus {
                setting: "Speech transcription".to_string(),
                status: if fact.transcription_enabled { "Enabled" } else { "Disabled" }.to_string(),
                detail: if fact.transcription_enabled {
                    format!("Participant microphone audio is sent to {} for live speech-to-text processing.", fact.transcription_provider)
                } else {
                    "No participant audio is sent to a transcription provider and no speech transcripts are produced."
                        .to_string()
                },
            },
            PrivacyConfigurationStatus {
                setting: "Agent speech synthesis".to_string(),
                status: if fact.tts_enabled { "Enabled" } else { "Disabled" }.to_string(),
                detail: if fact.tts_enabled {
                    format!("Software-agent message text is sent to {} to generate speech.", fact.tts_provider)
                } else {
                    "No text is sent to a speech-synthesis provider."
                        .to_string()
                },
            },
            PrivacyConfigurationStatus {
                setting: "Consent items".to_string(),
                status: if fact.consent_items == 0 {
                    "None configured".to_string()
                } else {
                    format!("{} configured", fact.consent_items)
                },
                detail: if fact.consent_items == 0 {
                    "Parlando does not collect consent declarations for this experiment."
                        .to_string()
                } else {
                    "The configured declarations are presented and recorded as described below."
                        .to_string()
                },
            },
            participant_information,
        ]
    } else {
        vec![PrivacyConfigurationStatus {
            setting: "Experiments covered".to_string(),
            status: total.to_string(),
            detail: "This installation-wide endpoint summarizes multiple experiment configurations; use an experiment Privacy tab for an experiment-specific record."
                .to_string(),
        }]
    };
    Ok(PrivacyStatus {
        generated_at: now_iso(),
        experiment_id: experiment_id.map(str::to_string),
        privacy_contract_version: if contract_versions.len() == 1 {
            contract_versions[0].to_string()
        } else if contract_versions.is_empty() {
            "Unspecified".to_string()
        } else {
            format!("Mixed ({})", contract_versions.join(", "))
        },
        overview: PrivacyOverviewStatus {
            purpose: if consent_items > 0 {
                "Run the experiment, reconnect participants to their sessions, let authorized experimenters monitor and analyze sessions, preserve configured consent evidence, and prepare the documented corpus export."
            } else {
                "Run the experiment, reconnect participants to their sessions, let authorized experimenters monitor and analyze sessions, and prepare the documented corpus export."
            }
            .to_string(),
            primary_storage: "The SQLite database file configured for this Parlando server. Parlando does not encrypt individual database fields itself; encryption of the disk and protection of database backups depend on the deployment."
                .to_string(),
            access: "Participants can use their own live session. Authenticated Parlando administrators can inspect experiment records, download the corpus, and delete participant data. A web server, hosting service, or other infrastructure in front of Parlando may have separate access and logging rules."
                .to_string(),
            retention: "Parlando does not automatically delete completed research sessions after a fixed time. Records remain in SQLite until an administrator deletes a participant and the participant's shared sessions, or the operator removes the database data. Copies in backups or external exports must be managed separately by the institution."
                .to_string(),
        },
        configuration,
        storage: vec![
            PrivacyStorageStatus {
                category: "Participant records".to_string(),
                persisted_when_produced: true,
                purpose: if selected.is_some_and(|fact| fact.human_vs_agent) {
                    "Distinguish the human participant and software agent and link both to their session."
                } else {
                    "Distinguish the two human participants and link them to their session."
                }
                .to_string(),
                detail: if selected.is_some_and(|fact| fact.human_vs_agent) {
                    "When the human participant record is created, Parlando randomly generates a three-word pseudonym in the form adverb-adjective-animal, retries if it collides with an existing pseudonym, and stores it with this experiment. The pseudonym is not derived from a name, contact detail, participant IP address, or other external identifier, and Parlando does not use it to match the person across experiments. Parlando also stores that this is a human record and its creation time; the session links it to the software-agent participant."
                } else {
                    "When each participant record is created, Parlando randomly generates a three-word pseudonym in the form adverb-adjective-animal, retries if it collides with an existing pseudonym, and stores it with this experiment. The pseudonym is not derived from a name, contact detail, participant IP address, or other external identifier, and Parlando does not use it to match the person across experiments. Parlando also stores that this is a human record and its creation time."
                }
                .to_string(),
            },
            PrivacyStorageStatus {
                category: "Sessions and assignments".to_string(),
                persisted_when_produced: true,
                purpose: "Operate a two-participant session, support reconnection, and preserve its experimental context and outcome."
                    .to_string(),
                detail: "For each session, Parlando randomly generates a three-word pseudonym in the form adverb-adjective-place-or-object. It also stores internal session and room identifiers; temporary participant-session identifiers used for access and reconnection; research or testing purpose; participant roles; connection status; creation, start, completion, join, and leave times; final status; and the game-supplied completion record."
                    .to_string(),
            },
            PrivacyStorageStatus {
                category: "Consent evidence".to_string(),
                persisted_when_produced: consent_items > 0,
                purpose: "Show which version of the participant information and consent text was presented and accepted."
                    .to_string(),
                detail: consent_storage_detail,
            },
            PrivacyStorageStatus {
                category: "Accepted actions and game state".to_string(),
                persisted_when_produced: true,
                purpose: "Reconstruct the experimental interaction and analyze game behavior and outcomes."
                    .to_string(),
                detail: "Event order and wall-clock time; acting participant and role; accepted action; game-supplied information about how the action changed the game; and the complete game state after the action."
                    .to_string(),
            },
            PrivacyStorageStatus {
                category: "Typed participant messages".to_string(),
                persisted_when_produced: true,
                purpose: "Preserve the dialogue as experimental data."
                    .to_string(),
                detail: "Event order and wall-clock time, sending participant and role, message text, an indication that it was typed, and message metadata."
                    .to_string(),
            },
            PrivacyStorageStatus {
                category: "Final voice transcripts".to_string(),
                persisted_when_produced: transcription > 0,
                purpose: if transcription == 0 {
                    "Not applicable because speech transcription is disabled.".to_string()
                } else {
                    "Preserve spoken dialogue as text for experiments with speech transcription enabled."
                        .to_string()
                },
                detail: transcript_storage_detail,
            },
            PrivacyStorageStatus {
                category: "Operational session events".to_string(),
                persisted_when_produced: true,
                purpose: "Diagnose session failures and establish what happened during collection."
                    .to_string(),
                detail: operational_event_detail,
            },
        ]
        .into_iter()
        .filter(|item| total > 1 || item.persisted_when_produced)
        .collect(),
        not_retained: vec![
            PrivacyNonRetentionStatus {
                category: "Participant IP addresses".to_string(),
                behavior: "Parlando does not write participant network addresses to SQLite or the corpus export."
                    .to_string(),
                boundary: "A web server, hosting service, firewall, or other infrastructure outside Parlando may keep its own access logs; the institution must document and manage those separately."
                    .to_string(),
            },
            PrivacyNonRetentionStatus {
                category: "Raw microphone audio".to_string(),
                behavior: audio_behavior,
                boundary: audio_boundary,
            },
            PrivacyNonRetentionStatus {
                category: "Interim speech-recognition text".to_string(),
                behavior: interim_behavior,
                boundary: interim_boundary,
            },
            PrivacyNonRetentionStatus {
                category: "Browser voice diagnostics".to_string(),
                behavior: browser_diagnostic_behavior,
                boundary: browser_diagnostic_boundary,
            },
        ]
        .into_iter()
        .filter(|item| {
            total > 1
                || item.category == "Participant IP addresses"
                || (voice > 0
                    && (item.category == "Raw microphone audio"
                        || item.category == "Browser voice diagnostics"))
                || (transcription > 0 && item.category == "Interim speech-recognition text")
        })
        .collect(),
        raw_audio_stored_by_parlando: false,
        external_services,
        exports: PrivacyExportStatus {
            available: true,
            variant: "corpus".to_string(),
            schema_id: "parlando.corpus.v1".to_string(),
            schema_sha256: format!("{:x}", Sha256::digest(CORPUS_EXPORT_SCHEMA_V1.as_bytes())),
            scope: "Every session in the selected experiment that is not marked as a testing session. Testing sessions are omitted before the corpus document is built.".to_string(),
            release_status: "corpus_candidate".to_string(),
            content_review_required: true,
            formats: vec!["JSON".to_string(), "YAML".to_string(), "CSV".to_string()],
            identifiers: "Human participants are represented by Parlando's randomly generated three-word pseudonyms, and sessions by separate randomly generated three-word pseudonyms. These identifiers are not derived from names, contact details, IP addresses, or external participant identifiers. Software agents use an identifier derived from their agent implementation rather than a human identity. Internal numeric keys, room identifiers, and temporary connection identifiers are not written to the corpus. The corpus is pseudonymized, not anonymous: participant-authored text can still contain identifying information and must be reviewed before sharing.".to_string(),
            structure: "The corpus contains one experiment record with game information and configuration, a list of participants, and a list of sessions. Each session contains its own context, both participant-role assignments, the game-supplied completion record, and ordered accepted-action and participant-message events.".to_string(),
            timing: "Parlando replaces its stored wall-clock event and session times with milliseconds relative to session start, waiting duration, and session duration. An invalid or missing start time blocks export instead of producing a misleading value. Game-defined configuration, actions, transition information, state, and completion are included unchanged.".to_string(),
            selection_rule: "Parlando builds a new corpus document and writes only the categories listed below. It does not copy a database-shaped document and then remove columns. A database field that has no listed destination is never written to the corpus.".to_string(),
            included_fields: vec![
                PrivacyExportFieldStatus {
                    section: "Manifest".to_string(),
                    description: "The export format and Privacy Contract versions, the reminder to review participant utterances for explicit identifying information, and summary totals for participants, sessions, and exported events."
                        .to_string(),
                    fields: privacy_field_names(&[
                        "export_schema_version",
                        "export_variant",
                        "release_status",
                        "content_review_required",
                        "privacy_contract_version",
                        "data_inventory.{participants,sessions,events,actions,messages}",
                    ]),
                },
                PrivacyExportFieldStatus {
                    section: "Experiment".to_string(),
                    description: "The experiment label shown in the dashboard, game version, complete game-specific configuration, participant catalogue, and sessions."
                        .to_string(),
                    fields: privacy_field_names(&[
                        "experiment_id",
                        "game.version",
                        "configuration (complete game-owned value)",
                        "participants",
                        "sessions",
                    ]),
                },
                PrivacyExportFieldStatus {
                    section: "Participant".to_string(),
                    description: if selected.is_some_and(|fact| fact.human_vs_agent) {
                        "The human's randomly generated three-word pseudonym and the software agent's implementation-derived identifier. The agent entry also identifies its implementation and configuration."
                    } else {
                        "The two humans' randomly generated three-word pseudonyms."
                    }
                    .to_string(),
                    fields: if selected.is_some_and(|fact| fact.human_vs_agent) {
                        privacy_field_names(&[
                            "participant_id (dashboard ID)",
                            "kind",
                            "agent_identity.{factory_id,agent_name,agent_version,configuration_fingerprint}",
                        ])
                    } else {
                        privacy_field_names(&["participant_id (dashboard ID)", "kind=human"])
                    },
                },
                PrivacyExportFieldStatus {
                    section: "Session".to_string(),
                    description: "The randomly generated three-word session pseudonym, game and configuration versions, how the session was created, final status, relative waiting time and duration, both participant-role assignments, game-supplied completion record, and exported events."
                        .to_string(),
                    fields: privacy_field_names(&[
                        "session_id (dashboard ID)",
                        "metadata.{game_version,config_revision,mode,status,time_to_start_ms,duration_ms}",
                        "participants[].{participant_id,role}",
                        "completion (complete game-owned value)",
                        "events",
                    ]),
                },
                PrivacyExportFieldStatus {
                    section: "Action event".to_string(),
                    description: "Order and time relative to session start, participant and role, complete accepted action, game-supplied transition information, and complete resulting game state."
                        .to_string(),
                    fields: privacy_field_names(&[
                        "index",
                        "time_from_session_start_ms",
                        "kind=action",
                        "participant_id",
                        "role",
                        "action (complete game-owned value)",
                        "transition_metadata (complete game-owned value)",
                        "state (complete game-owned value)",
                    ]),
                },
                PrivacyExportFieldStatus {
                    section: "Message event".to_string(),
                    description: if transcription > 0 {
                        "Order and time relative to session start, participant and role, whether the message was typed or transcribed from speech, message text, and optional utterance timing."
                    } else {
                        "Order and time relative to session start, participant and role, typed message text, and its typed origin."
                    }
                    .to_string(),
                    fields: privacy_field_names(&[
                        "index",
                        "time_from_session_start_ms",
                        "kind=message",
                        "participant_id",
                        "role",
                        "origin",
                        "text",
                        "utterance_timing?.{origin,start_ms,end_ms}",
                    ]),
                },
            ],
            not_written: vec![
                "Testing sessions and participants that occur only in testing sessions."
                    .to_string(),
                "Consent declarations, participant-information references, and the configured consent text."
                    .to_string(),
                "Database-only participant creation metadata.".to_string(),
                "Internal numeric participant and session keys, room identifiers, temporary participant-session identifiers, connection status, and join or leave times."
                    .to_string(),
                "Parlando wall-clock timestamps; only the documented relative durations and event offsets are written."
                    .to_string(),
                if selected.is_some_and(|fact| fact.human_vs_agent) || has_external_services {
                    "Operational lifecycle, connection, rejected-input, and any enabled software-agent or speech-service status events."
                } else {
                    "Operational lifecycle, connection, and rejected-input events."
                }
                .to_string(),
                "Administrator accounts and sessions, server access settings, provider credentials, and other administrator metadata."
                    .to_string(),
            ],
            detail: if transcription > 0 {
                "Before sharing the corpus, review participant-authored typed messages and final voice transcripts and remove explicit identifying information. Parlando does not perform this text review automatically."
            } else {
                "Before sharing the corpus, review participant-authored typed messages and remove explicit identifying information. Parlando does not perform this text review automatically."
            }
            .to_string(),
        },
        participant_deletion: PrivacyFeatureStatus {
            available: true,
            detail: "Deleting a human participant physically removes that participant and every session in which the participant appeared, including all messages, transcripts, actions, game states, operational events, role assignments, and consent records attached to those sessions. Because the complete shared session is removed, the other participant's contribution to that session is removed as well. Deletion is blocked while an affected session is still live or otherwise unfinished. Database backups and corpus files already copied elsewhere are not changed by this operation."
                .to_string(),
        },
        consent_evidence: PrivacyFeatureStatus {
            available: consent_items > 0,
            detail: if total == 1 {
                if consent_items == 0 {
                    format!(
                        "No consent items are configured for this experiment. A link and version for the participant-information document are {}.",
                        if information_references == 1 { "configured" } else { "not configured" }
                    )
                } else {
                    format!(
                        "For each configured consent item, Parlando records whether it was accepted, when it was declared, and a cryptographic fingerprint of the complete text shown. A link and version for the participant-information document are {}.",
                        if information_references == 1 { "configured" } else { "not configured" }
                    )
                }
            } else {
                format!(
                    "Across these {total} experiments, Parlando records whether each configured consent item was accepted, when it was declared, and a cryptographic fingerprint of the complete text shown. A link and version for the participant-information document are configured in {information_references} experiments."
                )
            },
        },
    })
}

/// Renders the privacy status as a self-contained administrator page without client-side scripts.
fn render_privacy_status_html(status: &PrivacyStatus) -> String {
    let services = if status.external_services.is_empty() {
        "<tr><td colspan=\"3\" class=\"empty\">No external speech services are enabled.</td></tr>"
            .to_string()
    } else {
        status
            .external_services
            .iter()
            .map(|service| {
                format!(
                    "<tr><td>{}</td><td>{}</td><td>{}</td></tr>",
                    escape_html_text(&service.service),
                    yes_no(service.enabled),
                    escape_html_text(&service.data_sent)
                )
            })
            .collect::<Vec<_>>()
            .join("")
    };
    let storage = status
        .storage
        .iter()
        .map(|item| {
            format!(
                "<tr><td>{}</td><td>{}</td><td>{}</td></tr>",
                escape_html_text(&item.category),
                yes_no(item.persisted_when_produced),
                escape_html_text(&item.detail)
            )
        })
        .collect::<Vec<_>>()
        .join("");
    let export_fields = status
        .exports
        .included_fields
        .iter()
        .map(|item| {
            format!(
                "<tr><td>{}</td><td>{}</td></tr>",
                escape_html_text(&item.section),
                escape_html_text(&item.fields.join(", "))
            )
        })
        .collect::<Vec<_>>()
        .join("");
    format!(
        r##"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Parlando Privacy Status</title>
  <style>
    :root {{ font-family: Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; background: #f6f7f9; color: #182026; }}
    * {{ box-sizing: border-box; }}
    body {{ margin: 0; min-height: 100vh; }}
    .app-header {{ align-items: center; background: #fff; border-bottom: 1px solid #dde2e7; display: flex; justify-content: space-between; min-height: 64px; padding: 12px 24px; }}
    .brand {{ display: grid; gap: 3px; }}
    .brand strong {{ font-size: 20px; }}
    .brand span {{ color: #65717c; font-size: 12px; font-weight: 700; text-transform: uppercase; }}
    .header-actions {{ display: flex; flex-wrap: wrap; gap: 8px; }}
    .button {{ background: #fff; border: 1px solid #c9d2da; border-radius: 6px; color: #31404c; font-size: 13px; font-weight: 750; padding: 8px 11px; text-decoration: none; }}
    .button.primary {{ background: #185f8f; border-color: #185f8f; color: #fff; }}
    main {{ margin: 0 auto; max-width: 1100px; padding: 28px 24px 48px; }}
    h1 {{ font-size: 28px; margin: 0 0 8px; }}
    h2 {{ font-size: 18px; margin: 0 0 12px; }}
    .intro {{ color: #53616c; margin: 0 0 20px; }}
    .grid {{ display: grid; gap: 14px; grid-template-columns: repeat(2, minmax(0, 1fr)); }}
    .panel {{ background: #fff; border: 1px solid #dce2e8; border-radius: 8px; margin-bottom: 14px; padding: 16px; }}
    .facts {{ display: grid; gap: 10px; grid-template-columns: repeat(2, minmax(0, 1fr)); }}
    .fact span {{ color: #65717c; display: block; font-size: 12px; margin-bottom: 3px; }}
    .fact strong {{ overflow-wrap: anywhere; }}
    table {{ border-collapse: collapse; font-size: 13px; width: 100%; }}
    th, td {{ border-bottom: 1px solid #e2e7eb; padding: 9px 8px; text-align: left; vertical-align: top; }}
    th {{ color: #52606b; font-size: 12px; }}
    .state {{ font-weight: 800; }}
    .empty {{ color: #65717c; font-size: 13px; }}
    @media (max-width: 760px) {{ .app-header {{ align-items: flex-start; padding: 10px 14px; }} .header-actions {{ justify-content: flex-end; }} main {{ padding: 20px 14px 36px; }} .grid, .facts {{ grid-template-columns: 1fr; }} table {{ display: block; overflow-x: auto; }} }}
  </style>
</head>
<body>
  <header class="app-header">
    <div class="brand"><strong>Parlando</strong><span>Privacy status</span></div>
    <nav class="header-actions"><a class="button" href="/admin/experiments">Dashboard</a><a class="button" download href="/api/admin/privacy.md">Markdown</a><a class="button primary" download href="/api/admin/privacy.json">JSON</a></nav>
  </header>
  <main>
    <h1>Privacy status</h1>
    <p class="intro">Installation-wide privacy behavior derived from the effective experiment configurations. This page does not infer controller identity, legal basis, retention decisions, hosting logs, or provider contracts.</p>
    <section class="panel">
      <h2>Scope</h2>
      <div class="facts"><div class="fact"><span>Generated</span><strong>{generated_at}</strong></div><div class="fact"><span>Privacy contract</span><strong>{contract_version}</strong></div></div>
    </section>
    <section class="panel">
      <h2>Storage behavior</h2>
      <table><thead><tr><th>Category</th><th>Stored by Parlando</th><th>Data retained</th></tr></thead><tbody>{storage}</tbody></table>
      <p class="state">Raw microphone audio stored by Parlando: {raw_audio}</p>
    </section>
    <section class="panel">
      <h2>External services</h2>
      <table><thead><tr><th>Service</th><th>Enabled</th><th>Data sent</th></tr></thead><tbody>{services}</tbody></table>
    </section>
    <div class="grid">
      <section class="panel"><h2>Corpus export</h2><p>Available: <strong>{export_available}</strong><br>Variant: <strong>{export_variant}</strong><br>Schema: <strong>{export_schema}</strong><br>Release status: <strong>{release_status}</strong><br>Formats: <strong>{export_formats}</strong></p><p>{export_scope}</p><p>{export_structure}</p><p>{export_timing}</p><p><strong>Field-selection rule:</strong> {selection_rule}</p><table><thead><tr><th>Output section</th><th>Fields written</th></tr></thead><tbody>{export_fields}</tbody></table><p class="empty">{export_detail}</p></section>
      <section class="panel"><h2>Participant administration</h2><p>Manual deletion available: <strong>{deletion}</strong></p><p class="empty">{deletion_detail}</p><p>Versioned information evidence available: <strong>{consent}</strong></p><p class="empty">{consent_detail}</p></section>
    </div>
  </main>
</body>
</html>"##,
        contract_version = escape_html_text(&status.privacy_contract_version),
        generated_at = escape_html_text(&status.generated_at),
        storage = storage,
        raw_audio = yes_no(status.raw_audio_stored_by_parlando),
        services = services,
        export_available = yes_no(status.exports.available),
        export_variant = escape_html_text(&status.exports.variant),
        export_schema = escape_html_text(&status.exports.schema_id),
        release_status = escape_html_text(&status.exports.release_status),
        export_formats = escape_html_text(&status.exports.formats.join(", ")),
        export_scope = escape_html_text(&status.exports.scope),
        export_structure = escape_html_text(&status.exports.structure),
        export_timing = escape_html_text(&status.exports.timing),
        selection_rule = escape_html_text(&status.exports.selection_rule),
        export_fields = export_fields,
        export_detail = escape_html_text(&status.exports.detail),
        deletion = yes_no(status.participant_deletion.available),
        deletion_detail = escape_html_text(&status.participant_deletion.detail),
        consent = yes_no(status.consent_evidence.available),
        consent_detail = escape_html_text(&status.consent_evidence.detail),
    )
}

/// Renders a portable experiment data-processing record for archival with collected data.
fn render_privacy_status_markdown(status: &PrivacyStatus) -> String {
    let subject = status.experiment_id.as_deref().map_or_else(
        || "all experiments".to_string(),
        |id| format!("experiment {id}"),
    );
    let mut output = format!(
        "# Data processing record — {}\n\nThis record describes how Parlando stored, processed, exported, and deleted participant data for {}. It was generated for retention with the experiment data. The data-handling contract version identifies the Parlando storage, export, and deletion behavior documented here.\n\n| Record | Value |\n| --- | --- |\n| Generated | {} |\n| Parlando data-handling contract version | {} |\n\n## Scope and responsibility\n\n| Topic | Description |\n| --- | --- |\n| Purpose | {} |\n| Primary storage | {} |\n| Access through Parlando | {} |\n| Retention | {} |\n\n## Privacy-relevant experiment settings\n\n| Setting | Status | Effect on data processing |\n| --- | --- | --- |\n",
        markdown_cell(&subject),
        markdown_cell(&subject),
        markdown_cell(&status.generated_at),
        markdown_cell(&status.privacy_contract_version),
        markdown_cell(&status.overview.purpose),
        markdown_cell(&status.overview.primary_storage),
        markdown_cell(&status.overview.access),
        markdown_cell(&status.overview.retention),
    );
    for item in &status.configuration {
        output.push_str(&format!(
            "| {} | {} | {} |\n",
            markdown_cell(&item.setting),
            markdown_cell(&item.status),
            markdown_cell(&item.detail)
        ));
    }
    output.push_str("\n## Data retained in Parlando's SQLite database\n\n| Information | Why it is processed | What is retained |\n| --- | --- | --- |\n");
    for item in &status.storage {
        output.push_str(&format!(
            "| {} | {} | {} |\n",
            markdown_cell(&item.category),
            markdown_cell(&item.purpose),
            markdown_cell(&item.detail)
        ));
    }
    output.push_str("\n## Data not retained by Parlando\n\n| Information | Parlando guarantee | Boundary of the guarantee |\n| --- | --- | --- |\n");
    for item in &status.not_retained {
        output.push_str(&format!(
            "| {} | {} | {} |\n",
            markdown_cell(&item.category),
            markdown_cell(&item.behavior),
            markdown_cell(&item.boundary)
        ));
    }
    output.push_str(
        "\n## External speech services\n\n| Service | Purpose | Data sent |\n| --- | --- | --- |\n",
    );
    if status.external_services.is_empty() {
        output.push_str("| None | Not applicable | No external speech service is enabled for this experiment. |\n");
    } else {
        for service in &status.external_services {
            output.push_str(&format!(
                "| {} | {} | {} |\n",
                markdown_cell(&service.service),
                markdown_cell(&service.purpose),
                markdown_cell(&service.data_sent)
            ));
        }
    }
    output.push_str(&format!(
        "\n## Corpus export\n\n{} {}\n\n{} {}\n\n**How fields are selected:** {}\n\n| Part of the corpus | What it contains |\n| --- | --- |\n",
        markdown_cell(&status.exports.structure),
        markdown_cell(&status.exports.scope),
        markdown_cell(&status.exports.identifiers),
        markdown_cell(&status.exports.timing),
        markdown_cell(&status.exports.selection_rule),
    ));
    for item in &status.exports.included_fields {
        output.push_str(&format!(
            "| {} | {} |\n",
            markdown_cell(&item.section),
            markdown_cell(&item.description)
        ));
    }
    output.push_str("\n**Stored or used by Parlando but not written to the corpus:**\n\n");
    for item in &status.exports.not_written {
        output.push_str(&format!("- {}\n", markdown_cell(item)));
    }
    let institution_provider_addition = if status.external_services.is_empty() {
        String::new()
    } else {
        ", and agreements and retention settings for the external speech services listed above"
            .to_string()
    };
    output.push_str(&format!(
        "\n**Before sharing:** {}\n\nThe export uses schema `{}` and is available as {}.\n\n## Consent evidence and deletion\n\n| Area | Behavior |\n| --- | --- |\n| Consent evidence | {} |\n| Participant deletion | {} |\n\n## What the institution must add\n\nThis record covers behavior enforced by Parlando. The institution must keep it together with its own record of the data controller, legal basis, retention schedule, server location, transport and disk encryption, backup policy, administrator access, and reverse-proxy or hosting logs{}.\n",
        markdown_cell(&status.exports.detail),
        markdown_cell(&status.exports.schema_id),
        status.exports.formats.iter().map(|value| markdown_cell(value)).collect::<Vec<_>>().join(", "),
        markdown_cell(&status.consent_evidence.detail),
        markdown_cell(&status.participant_deletion.detail),
        institution_provider_addition,
    ));
    output
}

/// Creates a download response with an experiment-id-safe filename and explicit media type.
fn privacy_download_response(body: String, content_type: &'static str, filename: &str) -> Response {
    let mut response = body.into_response();
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
    response.headers_mut().insert(
        CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("attachment; filename=\"{filename}\""))
            .expect("validated experiment ids produce a valid privacy-status filename"),
    );
    response
}

/// Escapes untrusted configuration text before inserting it into server-rendered HTML.
fn escape_html_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// Escapes one value for safe use inside a compact Markdown table cell or list item.
fn markdown_cell(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('|', "\\|")
        .replace(['\r', '\n'], " ")
}

/// Formats a boolean fact consistently in HTML and Markdown reports.
fn yes_no(value: bool) -> &'static str {
    if value {
        "Yes"
    } else {
        "No"
    }
}

/// Returns the catalogue of experiments owned by this compiled game process.
async fn admin_experiments<A: Game>(
    State(state): State<Arc<AppState<A>>>,
    Extension(admin_session): Extension<AdminSession>,
) -> Result<Json<Value>, AppError> {
    let experiments = state.store.list_experiments(1_000).await?;
    let mut catalogue = Vec::with_capacity(experiments.len());
    for experiment in experiments {
        let (configuration_valid, configuration_error, runnable, runnable_issues) =
            experiment_catalogue_readiness(
                &state,
                &experiment.experiment_id,
                &experiment.game_version,
            )
            .await;
        let mut value = serde_json::to_value(experiment)
            .map_err(|error| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
        let object = value
            .as_object_mut()
            .expect("serialized experiment summaries are JSON objects");
        object.insert(
            "configuration_valid".to_string(),
            json!(configuration_valid),
        );
        object.insert(
            "configuration_error".to_string(),
            json!(configuration_error),
        );
        object.insert("runnable".to_string(), json!(runnable));
        object.insert("runnable_issues".to_string(), json!(runnable_issues));
        catalogue.push(value);
    }
    let game_settings = state.game_settings.read().await.clone();
    let game_secrets = state.store.game_secrets().await?;
    Ok(Json(json!({
        "game": state.game_descriptor,
        "version_manifest": state.version_manifest,
        "game_settings": game_settings,
        "game_provider_secrets": configured_provider_secret_statuses(&state.bootstrap_secrets, &game_secrets),
        "experiments": catalogue,
        "csrf_token": admin_session.csrf_token,
    })))
}

/// Assesses structural validity and intake readiness without constructing a runtime router.
async fn experiment_catalogue_readiness<A: Game>(
    state: &Arc<AppState<A>>,
    experiment_id: &str,
    game_version: &str,
) -> (bool, Option<String>, bool, Vec<String>) {
    let mut config = match hydrated_experiment_config(state, experiment_id).await {
        Ok(config) => config,
        Err(error) => return (false, Some(error.message), false, Vec::new()),
    };
    if let Err(error) = config.validate() {
        return (false, Some(error.to_string()), false, Vec::new());
    }
    if let Err(error) = parse_game_config(state.adapter.as_ref(), &config.game) {
        return (false, Some(error.to_string()), false, Vec::new());
    }
    let mut issues = Vec::new();
    if game_version != state.game_descriptor.version.to_string() {
        issues.push(
            "This experiment belongs to another game version; clone it before activation."
                .to_string(),
        );
    }
    issues.extend(activation_issues_for_config(state, &mut config));
    (true, None, issues.is_empty(), issues)
}

/// Creates one inactive experiment for the exact game version compiled into this process.
async fn admin_create_experiment<A: Game>(
    State(state): State<Arc<AppState<A>>>,
    Json(request): Json<AdminCreateExperimentRequest>,
) -> Result<Json<Value>, AppError> {
    validate_new_experiment_id(&request.experiment_id)?;
    let mut config = if let Some(value) = request.config {
        experiment_config_from_json(value, &state.config, &request.experiment_id)
            .map_err(|error| AppError::bad_request(error.to_string()))?
    } else {
        state.config.clone()
    };
    config.experiment.id = Some(request.experiment_id.clone());
    config
        .validate()
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    validate_agent_configuration(&state.agent_definitions, &config, false)
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    let persisted = persistable_config_json(&config)?;
    state
        .store
        .create_experiment(ExperimentRecord {
            experiment_id: request.experiment_id.clone(),
            game_version: state.game_descriptor.version.to_string(),
            config: persisted,
            server_version: Some(env!("CARGO_PKG_VERSION").to_string()),
            version_manifest: Some(state.version_manifest.clone()),
            status: "inactive".to_string(),
            notes: request.notes,
        })
        .await
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    Ok(Json(json!({
        "experiment_id": request.experiment_id,
        "game_version": state.game_descriptor.version,
        "status": "inactive",
    })))
}

/// Clones a historical configuration into a new current-version inactive experiment.
async fn admin_clone_experiment<A: Game>(
    State(state): State<Arc<AppState<A>>>,
    Path(source_experiment_id): Path<String>,
    Json(request): Json<AdminCloneExperimentRequest>,
) -> Result<Json<Value>, AppError> {
    validate_new_experiment_id(&request.experiment_id)?;
    let source = state
        .store
        .experiment_definition(&source_experiment_id)
        .await?
        .ok_or_else(|| AppError::not_found("Source experiment not found."))?;
    let mut config =
        experiment_config_from_json(source.config, &state.config, &request.experiment_id).map_err(
            |error| {
                AppError::bad_request(format!(
                    "Source configuration is not valid for the current game version: {error}"
                ))
            },
        )?;
    config.experiment.id = Some(request.experiment_id.clone());
    config
        .validate()
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    state
        .store
        .create_experiment(ExperimentRecord {
            experiment_id: request.experiment_id.clone(),
            game_version: state.game_descriptor.version.to_string(),
            config: persistable_config_json(&config)?,
            server_version: Some(env!("CARGO_PKG_VERSION").to_string()),
            version_manifest: Some(state.version_manifest.clone()),
            status: "inactive".to_string(),
            notes: None,
        })
        .await
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    Ok(Json(json!({
        "experiment_id": request.experiment_id,
        "source_experiment_id": source_experiment_id,
        "game_version": state.game_descriptor.version,
        "status": "inactive",
    })))
}

/// Returns the current normalized configuration for one catalogue experiment.
async fn admin_experiment_config<A: Game>(
    State(state): State<Arc<AppState<A>>>,
    Path(experiment_id): Path<String>,
) -> Result<Json<Value>, AppError> {
    let mut experiment = state
        .store
        .experiment_definition(&experiment_id)
        .await?
        .ok_or_else(|| AppError::not_found("Experiment not found."))?;
    let stored_secrets = state.store.experiment_secrets(&experiment_id).await?;
    let empty_game = Value::Object(Default::default());
    let game_yaml = serde_yaml::to_string(
        experiment
            .config
            .get("game")
            .filter(|game| !game.is_null())
            .unwrap_or(&empty_game),
    )
    .map_err(|error| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    let configured_secrets = configured_secret_statuses(&stored_secrets);
    let activation_issues = if experiment.game_version == state.game_descriptor.version.to_string()
    {
        let mut normalized = hydrated_experiment_config(&state, &experiment_id).await?;
        let game_settings = state.game_settings.read().await.clone();
        normalized.direct.consents = expanded_consent_items(&normalized, &game_settings);
        experiment.config = persistable_config_json(&normalized)
            .map_err(|error| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
        experiment_activation_issues(&state, &experiment_id).await?
    } else {
        Vec::new()
    };
    Ok(Json(json!({
        "experiment": experiment,
        "game_yaml": game_yaml,
        "agent_factories": state.agent_definitions,
        "configured_secrets": configured_secrets,
        "activation_issues": activation_issues,
    })))
}

/// Describes secret availability without returning credential values.
fn configured_provider_secret_statuses(
    bootstrap: &HashMap<String, String>,
    stored: &HashMap<String, String>,
) -> Vec<Value> {
    vec![
        json!({
            "key": "speechmatics.api_key",
            "configured": stored.contains_key("speechmatics.api_key") || bootstrap.contains_key("speechmatics.api_key"),
            "source": if stored.contains_key("speechmatics.api_key") { "game" } else if bootstrap.contains_key("speechmatics.api_key") { "server" } else { "missing" },
        }),
        json!({
            "key": "tts.api_key",
            "configured": stored.contains_key("tts.api_key") || bootstrap.contains_key("tts.api_key"),
            "source": if stored.contains_key("tts.api_key") { "game" } else if bootstrap.contains_key("tts.api_key") { "server" } else { "missing" },
        }),
    ]
}

/// Describes configured experiment-owned game secrets without revealing values.
fn configured_secret_statuses(stored: &HashMap<String, String>) -> Vec<Value> {
    let mut game_keys = stored
        .keys()
        .filter(|key| key.starts_with("game."))
        .cloned()
        .collect::<Vec<_>>();
    game_keys.sort();
    game_keys
        .drain(..)
        .map(|key| json!({"key": key, "configured": true, "source": "experiment"}))
        .collect()
}

/// Validates one write-only secret name and value before opening a transaction.
fn validate_secret_update(key: &str, value: Option<&str>) -> Result<(), AppError> {
    let game = key
        .strip_prefix("game.")
        .is_some_and(|name| !name.is_empty());
    if key.chars().count() > 128
        || !game
        || !key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
    {
        return Err(AppError::bad_request(
            "Experiment secret keys must use game.<name> with letters, digits, dots, dashes, and underscores",
        ));
    }
    if value.is_some_and(|value| value.is_empty() || value.len() > 65_536) {
        return Err(AppError::bad_request(
            "Secret values must contain 1 to 65536 bytes",
        ));
    }
    Ok(())
}

/// Reveals one explicitly requested secret to an authenticated administrator.
async fn admin_reveal_experiment_secret<A: Game>(
    State(state): State<Arc<AppState<A>>>,
    Path(experiment_id): Path<String>,
    Json(request): Json<AdminRevealExperimentSecretRequest>,
) -> Result<Response, AppError> {
    validate_secret_update(&request.key, None)?;
    state
        .store
        .experiment_definition(&experiment_id)
        .await?
        .ok_or_else(|| AppError::not_found("Experiment not found."))?;
    let stored = state.store.experiment_secrets(&experiment_id).await?;
    let value = stored
        .get(&request.key)
        .cloned()
        .or_else(|| state.bootstrap_secrets.get(&request.key).cloned());
    let value = value.ok_or_else(|| AppError::not_found("Secret is not configured."))?;
    tracing::warn!(
        experiment_id,
        secret_key = request.key,
        "administrator revealed experiment secret"
    );
    let mut response = Json(json!({"key": request.key, "value": value})).into_response();
    response.headers_mut().insert(
        CACHE_CONTROL,
        HeaderValue::from_static("no-store, max-age=0"),
    );
    Ok(response)
}

/// Parses and validates game-owned YAML without saving a configuration revision.
async fn admin_validate_game_config<A: Game>(
    State(state): State<Arc<AppState<A>>>,
    Path(experiment_id): Path<String>,
    Json(request): Json<AdminValidateGameConfigRequest>,
) -> Result<Json<Value>, AppError> {
    state
        .store
        .experiment_definition(&experiment_id)
        .await?
        .ok_or_else(|| AppError::not_found("Experiment not found."))?;
    let game = serde_yaml::from_str::<Value>(&request.game_yaml)
        .map_err(|error| AppError::bad_request(format!("Invalid game YAML: {error}")))?;
    validate_game_config_contains_no_secrets(&game)
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    parse_game_config(state.adapter.as_ref(), &game)
        .map_err(|error| AppError::bad_request(format!("Invalid game configuration: {error}")))?;
    Ok(Json(json!({"valid": true})))
}

/// Validates and saves a new immutable configuration revision for an inactive experiment.
async fn admin_save_experiment_config<A: Game>(
    State(state): State<Arc<AppState<A>>>,
    Path(experiment_id): Path<String>,
    Json(request): Json<AdminSaveExperimentConfigRequest>,
) -> Result<Json<Value>, AppError> {
    let experiment = state
        .store
        .experiment_definition(&experiment_id)
        .await?
        .ok_or_else(|| AppError::not_found("Experiment not found."))?;
    if experiment.game_version != state.game_descriptor.version.to_string() {
        return Err(AppError::new(
            StatusCode::CONFLICT,
            "Clone this experiment before editing it under the current game version",
        ));
    }
    for (key, value) in &request.secret_updates {
        validate_secret_update(key, Some(value))?;
    }
    for key in &request.secret_deletions {
        validate_secret_update(key, None)?;
    }
    let mut config_value = request.config;
    if let Some(game_yaml) = request.game_yaml.as_deref() {
        let game = serde_yaml::from_str::<Value>(game_yaml)
            .map_err(|error| AppError::bad_request(format!("Invalid game YAML: {error}")))?;
        validate_game_config_contains_no_secrets(&game)
            .map_err(|error| AppError::bad_request(error.to_string()))?;
        config_value
            .as_object_mut()
            .ok_or_else(|| AppError::bad_request("configuration must be an object"))?
            .insert("game".to_string(), game);
    }
    let mut config = experiment_config_from_json(config_value, &state.config, &experiment_id)
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    let mut secrets = state.store.experiment_secrets(&experiment_id).await?;
    for key in &request.secret_deletions {
        secrets.remove(key);
    }
    secrets.extend(request.secret_updates.clone());
    apply_experiment_secrets(&mut config, &secrets);
    let game_secrets = state.store.game_secrets().await?;
    let game_settings = state.game_settings.read().await.clone();
    apply_game_provider_settings(
        &mut config,
        &game_settings,
        &state.bootstrap_secrets,
        &game_secrets,
    );
    config.experiment.id = Some(experiment_id.clone());
    config
        .validate()
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    validate_game_config_contains_no_secrets(&config.game)
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    parse_game_config(state.adapter.as_ref(), &config.game)
        .map_err(|error| AppError::bad_request(format!("Invalid game configuration: {error}")))?;
    let revision = state
        .store
        .save_experiment_configuration(
            &experiment_id,
            request.expected_revision,
            persistable_config_json(&config)?,
            request.change_summary,
            request.secret_updates,
            request.secret_deletions,
        )
        .await
        .map_err(|error| AppError::new(StatusCode::CONFLICT, error.to_string()))?;
    Ok(Json(
        json!({ "experiment_id": experiment_id, "revision": revision }),
    ))
}

/// Lists immutable configuration revisions for one experiment.
async fn admin_experiment_revisions<A: Game>(
    State(state): State<Arc<AppState<A>>>,
    Path(experiment_id): Path<String>,
) -> Result<Json<Value>, AppError> {
    Ok(Json(json!({
        "experiment_id": experiment_id.clone(),
        "revisions": state.store.experiment_revisions(&experiment_id).await?,
    })))
}

/// Updates pinning, obsolescence, and notes without changing runtime configuration.
async fn admin_update_experiment_catalogue<A: Game>(
    State(state): State<Arc<AppState<A>>>,
    Path(experiment_id): Path<String>,
    Json(request): Json<AdminCatalogueRequest>,
) -> Result<Json<Value>, AppError> {
    state
        .store
        .update_experiment_catalogue(&experiment_id, request.pinned, request.notes)
        .await
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    Ok(Json(json!({ "experiment_id": experiment_id, "ok": true })))
}

/// Archives catalogue state without constructing the selected experiment's runtime.
async fn admin_archive_experiment<A: Game>(
    State(state): State<Arc<AppState<A>>>,
    Path(experiment_id): Path<String>,
) -> Result<Json<Value>, AppError> {
    state
        .store
        .archive_experiment(&experiment_id)
        .await
        .map_err(|error| {
            let message = error.to_string();
            if message == "Experiment not found." {
                AppError::not_found(message)
            } else {
                AppError::new(StatusCode::CONFLICT, message)
            }
        })?;
    Ok(Json(
        json!({ "experiment_id": experiment_id, "status": "archived" }),
    ))
}

/// Returns settings shared by every experiment of the compiled game.
async fn admin_game_settings<A: Game>(
    State(state): State<Arc<AppState<A>>>,
) -> Json<StoredGameSettings> {
    Json(state.game_settings.read().await.clone())
}

/// Updates the shared institution with optimistic concurrency.
async fn admin_update_game_settings<A: Game>(
    State(state): State<Arc<AppState<A>>>,
    Json(request): Json<AdminGameSettingsRequest>,
) -> Result<Json<Value>, AppError> {
    let institution = request.institution.trim().to_string();
    let admin_allowed_ip_ranges = request
        .admin_allowed_ip_ranges
        .into_iter()
        .map(|range| range.trim().to_string())
        .filter(|range| !range.is_empty())
        .collect::<Vec<_>>();
    for range in &admin_allowed_ip_ranges {
        range.parse::<ipnet::IpNet>().map_err(|error| {
            AppError::bad_request(format!("Invalid administrator IP range {range:?}: {error}"))
        })?;
    }
    let current_realtime_url = state
        .game_settings
        .read()
        .await
        .speechmatics_realtime_url
        .clone();
    let speechmatics_realtime_url = request
        .speechmatics_realtime_url
        .unwrap_or(current_realtime_url)
        .trim()
        .to_string();
    if !(speechmatics_realtime_url.starts_with("wss://")
        || speechmatics_realtime_url.starts_with("ws://"))
    {
        return Err(AppError::bad_request(
            "Speechmatics realtime URL must use ws:// or wss://",
        ));
    }
    for (key, value) in &request.secret_updates {
        validate_game_provider_secret(key, Some(value))?;
    }
    for key in &request.secret_deletions {
        validate_game_provider_secret(key, None)?;
    }
    let revision = state
        .store
        .update_game_settings(
            request.expected_revision,
            institution.clone(),
            admin_allowed_ip_ranges.clone(),
            speechmatics_realtime_url.clone(),
            request.secret_updates,
            request.secret_deletions,
        )
        .await
        .map_err(|error| AppError::new(StatusCode::CONFLICT, error.to_string()))?;
    *state.game_settings.write().await = StoredGameSettings {
        institution,
        admin_allowed_ip_ranges,
        speechmatics_realtime_url,
        revision,
    };
    Ok(Json(json!({ "revision": revision })))
}

/// Validates one game-wide hosted-provider credential change.
fn validate_game_provider_secret(key: &str, value: Option<&str>) -> Result<(), AppError> {
    if !matches!(key, "speechmatics.api_key" | "tts.api_key") {
        return Err(AppError::bad_request(
            "Unknown game-wide provider credential",
        ));
    }
    if value.is_some_and(|value| value.is_empty() || value.len() > 65_536) {
        return Err(AppError::bad_request(
            "Provider credentials must contain 1 to 65536 bytes",
        ));
    }
    Ok(())
}

/// Reveals one game-wide provider credential after an explicit administrator action.
async fn admin_reveal_game_secret<A: Game>(
    State(state): State<Arc<AppState<A>>>,
    Json(request): Json<AdminRevealGameSecretRequest>,
) -> Result<Response, AppError> {
    validate_game_provider_secret(&request.key, None)?;
    let stored = state.store.game_secrets().await?;
    let value = stored
        .get(&request.key)
        .cloned()
        .or_else(|| state.bootstrap_secrets.get(&request.key).cloned())
        .ok_or_else(|| AppError::not_found("Provider credential is not configured."))?;
    tracing::warn!(
        secret_key = request.key,
        "administrator revealed game provider credential"
    );
    let mut response = Json(json!({"key": request.key, "value": value})).into_response();
    response.headers_mut().insert(
        CACHE_CONTROL,
        HeaderValue::from_static("no-store, max-age=0"),
    );
    Ok(response)
}

/// Validates a dashboard-supplied experiment id using the public configuration contract.
fn validate_new_experiment_id(experiment_id: &str) -> Result<(), AppError> {
    if experiment_id.is_empty()
        || experiment_id.chars().count() > 128
        || !experiment_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(AppError::bad_request(
            "experiment_id must contain 1 to 128 letters, digits, dots, dashes, or underscores",
        ));
    }
    Ok(())
}

/// Returns the process-owned experiment with dashboard aggregates.
async fn admin_experiment<A: Game>(
    State(state): State<Arc<AppState<A>>>,
    Extension(admin_session): Extension<AdminSession>,
) -> Result<Json<Value>, AppError> {
    let experiment = state
        .store
        .experiment_summary(&state.experiment_id)
        .await?
        .ok_or_else(|| AppError::not_found("Configured experiment not found."))?;
    Ok(Json(json!({
        "experiment": experiment,
        "game": state.game_descriptor,
        "game_settings": state.game_settings.read().await.clone(),
        "version_manifest": state.version_manifest,
        "csrf_token": admin_session.csrf_token,
    })))
}

/// Updates a durable experiment lifecycle status.
async fn admin_update_experiment_status<A: Game>(
    State(state): State<Arc<AppState<A>>>,
    Json(request): Json<AdminUpdateExperimentStatusRequest>,
) -> Result<Json<Value>, AppError> {
    let lifecycle = ExperimentLifecycle::parse(&request.status)?;
    let stored = state
        .store
        .experiment_definition(&state.experiment_id)
        .await?
        .ok_or_else(|| AppError::not_found("Configured experiment not found."))?;
    if lifecycle.allows_intake() && stored.game_version != state.game_descriptor.version.to_string()
    {
        return Err(AppError::new(
            StatusCode::CONFLICT,
            "This experiment belongs to another game version; clone it before activation",
        ));
    }
    if lifecycle.allows_intake() {
        let issues = experiment_activation_issues(&state, &state.experiment_id).await?;
        if !issues.is_empty() {
            return Err(AppError::new(
                StatusCode::CONFLICT,
                format!("Experiment cannot start: {}", issues.join(" ")),
            ));
        }
    }
    let mut current_lifecycle = state.experiment_lifecycle.write().await;
    if !current_lifecycle.can_transition_to(lifecycle) {
        return Err(AppError::new(
            StatusCode::CONFLICT,
            format!(
                "Experiment cannot transition directly from {} to {}",
                current_lifecycle.as_str(),
                lifecycle.as_str()
            ),
        ));
    }
    state
        .store
        .update_experiment_status(&state.experiment_id, lifecycle.as_str())
        .await?;
    *current_lifecycle = lifecycle;
    tracing::info!(experiment_id = %state.experiment_id, status = lifecycle.as_str(), "administrator updated experiment status");
    Ok(Json(json!({
        "experiment_id": state.experiment_id,
        "status": lifecycle.as_str(),
    })))
}

/// Returns recent database sessions for the experiment dashboard.
async fn admin_sessions<A: Game>(
    State(state): State<Arc<AppState<A>>>,
    scope: Option<Extension<AdminExperimentScope>>,
    Query(query): Query<AdminSessionsQuery>,
) -> Result<Json<Value>, AppError> {
    let experiment_id = admin_experiment_id(&state, scope.as_ref());
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
        "sessions": sessions,
    })))
}

/// Returns one database session's metadata and important event timeline.
async fn admin_session_detail<A: Game>(
    State(state): State<Arc<AppState<A>>>,
    scope: Option<Extension<AdminExperimentScope>>,
    Path(session_id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let experiment_id = admin_experiment_id(&state, scope.as_ref());
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
    let participants = participants
        .into_iter()
        .map(|participant| {
            let mut value = serde_json::to_value(&participant).unwrap_or(Value::Null);
            value["research_id"] = json!(participant.research_id);
            value
        })
        .collect::<Vec<_>>();
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
async fn admin_session_events<A: Game>(
    State(state): State<Arc<AppState<A>>>,
    scope: Option<Extension<AdminExperimentScope>>,
    Path(session_id): Path<i64>,
    Query(query): Query<AdminEventsQuery>,
) -> Result<Json<Value>, AppError> {
    let after = query.after.unwrap_or(0);
    let experiment_id = admin_experiment_id(&state, scope.as_ref());
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
        "session_completed" | "session_abandoned" | "session_expired" | "participant_disconnected"
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
        "session_created" | "session_completed" | "session_abandoned" | "session_expired" => {
            "Session".to_string()
        }
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
            | "session_abandoned"
            | "session_completed"
            | "session_created"
            | "session_expired"
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
        "session_abandoned" => "Session abandoned",
        "session_completed" => "Session completed",
        "session_created" => "Session created",
        "session_expired" => "Session expired",
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
        "session_abandoned" | "session_expired" => payload
            .get("reason")
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

async fn admin_export<A: Game>(
    State(state): State<Arc<AppState<A>>>,
    scope: Option<Extension<AdminExperimentScope>>,
    Query(query): Query<AdminExportQuery>,
) -> Result<Response, AppError>
where
    A::State: Serialize,
{
    let experiment_id = admin_experiment_id(&state, scope.as_ref());
    let raw = filtered_export(&state, &experiment_id).await?;
    let mut value = match query.variant.as_deref().unwrap_or("corpus") {
        "corpus" => corpus_experiment_export(raw.clone(), &state.config.privacy.contract_version)?,
        #[cfg(test)]
        "full" => raw,
        other => {
            return Err(AppError::bad_request(format!(
                "Unsupported export variant {other:?}."
            )))
        }
    };
    tracing::info!(experiment_id, "administrator requested corpus export");
    redact_secret_fields(&mut value);
    if value
        .get("data_inventory")
        .and_then(|inventory| inventory.get("events"))
        .and_then(Value::as_u64)
        .is_some_and(|events| events > 100_000)
    {
        return Err(AppError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "Export exceeds the 100,000 event limit.",
        ));
    }
    let (content_type, body) = match query.format.as_deref().unwrap_or("json") {
        "json" => (
            "application/json; charset=utf-8",
            serde_json::to_string(&value).map_err(|error| {
                AppError::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
            })?,
        ),
        "yaml" | "yml" => (
            "application/yaml; charset=utf-8",
            serde_yaml::to_string(&value).map_err(|error| {
                AppError::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
            })?,
        ),
        "csv" => ("text/csv; charset=utf-8", export_csv(&value)),
        other => {
            return Err(AppError::bad_request(format!(
                "Unsupported export format {other:?}."
            )))
        }
    };
    if body.len() > 32 * 1024 * 1024 {
        return Err(AppError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "Export exceeds the 32 MiB response limit.",
        ));
    }
    let extension = match query.format.as_deref().unwrap_or("json") {
        "yaml" | "yml" => "yaml",
        "csv" => "csv",
        _ => "json",
    };
    Ok((
        [
            ("content-type", content_type),
            ("cache-control", "no-store"),
            ("x-content-type-options", "nosniff"),
            (
                "content-disposition",
                &format!(
                    "attachment; filename=\"{}-corpus-v1.{extension}\"",
                    experiment_id
                ),
            ),
        ],
        body,
    )
        .into_response())
}

/// Resolves an experiment-specific participant identifier without exposing database ids.
async fn participant_id_for_research_id<A: Game>(
    state: &Arc<AppState<A>>,
    experiment_id: &str,
    requested: &str,
) -> Result<Option<i64>, AppError> {
    let exported = state.store.export_experiment(experiment_id).await?;
    Ok(exported
        .get("participants")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|row| row.get("research_id").and_then(Value::as_str) == Some(requested))
        .and_then(|row| row.get("participant_id").and_then(Value::as_i64)))
}

/// Previews the bounded records affected by manual participant-data deletion.
async fn admin_participant_deletion_preview<A: Game>(
    State(state): State<Arc<AppState<A>>>,
    scope: Option<Extension<AdminExperimentScope>>,
    Path(requested): Path<String>,
) -> Result<Json<Value>, AppError> {
    let experiment_id = admin_experiment_id(&state, scope.as_ref());
    let participant_id = participant_id_for_research_id(&state, &experiment_id, &requested)
        .await?
        .ok_or_else(|| AppError::not_found("Participant not found."))?;
    let preview = state
        .store
        .participant_data_preview(&experiment_id, participant_id)
        .await?;
    Ok(Json(json!({"research_id": requested, "preview": preview})))
}

/// Executes confirmed participant-data deletion and returns its audit counts.
async fn admin_delete_participant_data<A: Game>(
    State(state): State<Arc<AppState<A>>>,
    scope: Option<Extension<AdminExperimentScope>>,
    Path(requested): Path<String>,
) -> Result<Json<Value>, AppError> {
    let experiment_id = admin_experiment_id(&state, scope.as_ref());
    let Some(participant_id) =
        participant_id_for_research_id(&state, &experiment_id, &requested).await?
    else {
        return Ok(Json(json!({
            "research_id": requested,
            "deleted": true,
            "already_absent": true,
        })));
    };
    let preview = state
        .store
        .participant_data_preview(&experiment_id, participant_id)
        .await?;
    if preview.has_non_terminal_session {
        return Err(AppError::new(
            StatusCode::CONFLICT,
            "Participant deletion is blocked while an affected session is live or non-terminal.",
        ));
    }
    let deleted = state
        .store
        .delete_participant_data(&experiment_id, participant_id)
        .await?;
    tracing::info!(
        experiment_id,
        research_id = requested,
        "administrator deleted participant data"
    );
    Ok(Json(json!({
        "research_id": requested,
        "deleted": true,
        "affected": deleted,
    })))
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

async fn filtered_export<A: Game>(
    state: &Arc<AppState<A>>,
    experiment_id: &str,
) -> Result<Value, AppError>
where
    A::State: Serialize,
{
    let experiment = state
        .store
        .experiment_definition(experiment_id)
        .await?
        .ok_or_else(|| AppError::not_found("Configured experiment not found."))?;
    if experiment.status == "archived" {
        return Err(AppError::new(
            StatusCode::CONFLICT,
            "Archived experiments must be restored before export",
        ));
    }
    let mut exported = state.store.export_experiment(experiment_id).await?;
    exclude_testing_sessions(&mut exported);
    Ok(exported)
}

/// Removes test sessions and every session-scoped record linked only to them.
fn exclude_testing_sessions(exported: &mut Value) {
    filter_array_by_string_not_equal(exported, "sessions", "purpose", "testing");
    filter_array_by_string_not_equal(exported, "consent_declarations", "purpose", "testing");
    filter_scoped_tables_to_sessions(exported);
    let participant_ids = exported
        .get("session_participants")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|row| row.get("participant_id").and_then(Value::as_i64))
        .collect::<HashSet<_>>();
    if let Some(rows) = exported
        .get_mut("participants")
        .and_then(Value::as_array_mut)
    {
        rows.retain(|row| {
            row.get("participant_id")
                .and_then(Value::as_i64)
                .is_some_and(|id| participant_ids.contains(&id))
        });
    }
}

/// Retains rows whose string field does not equal the excluded value.
fn filter_array_by_string_not_equal(
    exported: &mut Value,
    table: &str,
    field: &str,
    excluded: &str,
) {
    if let Some(rows) = exported.get_mut(table).and_then(Value::as_array_mut) {
        rows.retain(|row| row.get(field).and_then(Value::as_str) != Some(excluded));
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
    for table in ["session_participants", "session_events"] {
        if let Some(rows) = exported.get_mut(table).and_then(Value::as_array_mut) {
            rows.retain(|row| {
                row.get("session_id")
                    .and_then(Value::as_i64)
                    .is_some_and(|id| session_ids.contains(&id))
            });
        }
    }
    let participant_ids = exported
        .get("session_participants")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|row| row.get("participant_id").and_then(Value::as_i64))
        .collect::<HashSet<_>>();
    if let Some(rows) = exported
        .get_mut("consent_declarations")
        .and_then(Value::as_array_mut)
    {
        rows.retain(|row| {
            row.get("session_id").and_then(Value::as_i64).map_or_else(
                || {
                    row.get("participant_id")
                        .and_then(Value::as_i64)
                        .is_some_and(|id| participant_ids.contains(&id))
                },
                |id| session_ids.contains(&id),
            )
        });
    }
}

/// Derives a publication-oriented dialogue corpus with consistent readable identifiers.
fn corpus_experiment_export(
    exported: Value,
    privacy_contract_version: &str,
) -> Result<Value, AppError> {
    let experiment = exported
        .get("experiment")
        .and_then(Value::as_object)
        .ok_or_else(|| export_integrity_error("experiment metadata is missing"))?;
    let participant_rows = exported
        .get("participants")
        .and_then(Value::as_array)
        .ok_or_else(|| export_integrity_error("participant catalogue is missing"))?;
    let mut participant_ids = HashMap::<i64, String>::new();
    let participants = participant_rows
        .iter()
        .map(|row| {
            let internal_id = row
                .get("participant_id")
                .and_then(Value::as_i64)
                .ok_or_else(|| export_integrity_error("a participant lacks its database key"))?;
            let public_id = row
                .get("research_id")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| export_integrity_error("a participant lacks its dashboard ID"))?
                .to_string();
            participant_ids.insert(internal_id, public_id.clone());
            let kind = row.get("participant_kind").cloned().unwrap_or(Value::Null);
            let agent_identity = (kind.as_str() == Some("agent")).then(|| {
                let metadata = row.get("metadata").unwrap_or(&Value::Null);
                json!({
                    "factory_id": metadata.get("factory"),
                    "agent_name": metadata.get("agent_name"),
                    "agent_version": metadata.get("agent_version"),
                    "configuration_fingerprint": metadata.get("configuration_fingerprint"),
                })
            });
            Ok(json!({
                "participant_id": public_id,
                "kind": kind,
                "agent_identity": agent_identity,
            }))
        })
        .collect::<Result<Vec<_>, AppError>>()?;

    let membership_rows = exported
        .get("session_participants")
        .and_then(Value::as_array)
        .ok_or_else(|| export_integrity_error("session assignments are missing"))?;
    let event_rows = exported
        .get("session_events")
        .and_then(Value::as_array)
        .ok_or_else(|| export_integrity_error("session events are missing"))?;
    let session_rows = exported
        .get("sessions")
        .and_then(Value::as_array)
        .ok_or_else(|| export_integrity_error("sessions are missing"))?;
    let mut semantic_event_count = 0_usize;
    let mut message_count = 0_usize;
    let mut action_count = 0_usize;
    let sessions = session_rows
        .iter()
        .map(|session| {
            let internal_session_id = session
                .get("session_id")
                .and_then(Value::as_i64)
                .ok_or_else(|| export_integrity_error("a session lacks its database key"))?;
            let session_id = session
                .get("dialogue_id")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| export_integrity_error("a session lacks its dashboard ID"))?;
            let assignments = membership_rows
                .iter()
                .filter(|row| {
                    row.get("session_id").and_then(Value::as_i64) == Some(internal_session_id)
                })
                .map(|row| {
                    let participant = row
                        .get("participant_id")
                        .and_then(Value::as_i64)
                        .and_then(|id| participant_ids.get(&id))
                        .ok_or_else(|| {
                            export_integrity_error(
                                "a session assignment has no exported participant",
                            )
                        })?;
                    Ok(json!({
                        "participant_id": participant,
                        "role": row.get("role"),
                    }))
                })
                .collect::<Result<Vec<_>, AppError>>()?;
            if assignments.len() != 2 {
                return Err(export_integrity_error(format!(
                    "session {session_id} has {} participant assignments; expected exactly two",
                    assignments.len()
                )));
            }
            let started_at = optional_timestamp_ms(session.get("started_at"), "session start")?;
            let created_at = required_timestamp_ms(session.get("created_at"), "session creation")?;
            let completed_at =
                optional_timestamp_ms(session.get("completed_at"), "session completion")?;
            let session_events = event_rows
                .iter()
                .filter(|row| {
                    row.get("session_id").and_then(Value::as_i64) == Some(internal_session_id)
                })
                .filter(|row| {
                    matches!(
                        row.get("event_type").and_then(Value::as_str),
                        Some("game_action_accepted" | "conversation_message")
                    )
                })
                .map(|row| {
                    let start = started_at.ok_or_else(|| {
                        export_integrity_error(format!(
                            "session {session_id} has semantic events but no valid start time"
                        ))
                    })?;
                    let event_time = required_timestamp_ms(row.get("created_at"), "event")?;
                    let relative = event_time
                        .checked_sub(start)
                        .filter(|value| *value >= 0)
                        .ok_or_else(|| {
                            export_integrity_error(format!(
                                "session {session_id} contains an event before its start"
                            ))
                        })?;
                    let index = row.get("event_index").cloned().unwrap_or(Value::Null);
                    let participant = row
                        .get("actor_participant_id")
                        .and_then(Value::as_i64)
                        .and_then(|id| participant_ids.get(&id))
                        .ok_or_else(|| {
                            export_integrity_error("a semantic event lacks an exported actor")
                        })?;
                    let payload = row.get("payload").unwrap_or(&Value::Null);
                    semantic_event_count += 1;
                    match row.get("event_type").and_then(Value::as_str) {
                        Some("game_action_accepted") => {
                            action_count += 1;
                            Ok(json!({
                                "index": index,
                                "time_from_session_start_ms": relative,
                                "kind": "action",
                                "participant_id": participant,
                                "role": row.get("actor_role"),
                                "action": payload.get("action"),
                                "transition_metadata": payload.get("metadata"),
                                "state": row.get("game_state"),
                            }))
                        }
                        Some("conversation_message") => {
                            message_count += 1;
                            let mut event = json!({
                                "index": index,
                                "time_from_session_start_ms": relative,
                                "kind": "message",
                                "participant_id": participant,
                                "role": row.get("actor_role"),
                                "origin": payload.get("origin"),
                                "text": payload.get("text"),
                            });
                            let start_time = payload
                                .get("metadata")
                                .and_then(|value| value.get("start_time_ms"));
                            let end_time = payload
                                .get("metadata")
                                .and_then(|value| value.get("end_time_ms"));
                            if start_time.is_some() || end_time.is_some() {
                                event["utterance_timing"] = json!({
                                    "origin": "speaker_audio_stream",
                                    "start_ms": start_time,
                                    "end_ms": end_time,
                                });
                            }
                            Ok(event)
                        }
                        _ => unreachable!(),
                    }
                })
                .collect::<Result<Vec<_>, AppError>>()?;
            let time_to_start_ms = started_at
                .map(|start| checked_interval(start, created_at, session_id, "time to start"))
                .transpose()?;
            let duration_ms = completed_at
                .zip(started_at)
                .map(|(completed, start)| {
                    checked_interval(completed, start, session_id, "duration")
                })
                .transpose()?;
            Ok(json!({
                "session_id": session_id,
                "metadata": {
                    "game_version": session.get("game_version"),
                    "config_revision": session.get("config_revision"),
                    "mode": session.get("mode"),
                    "status": session.get("status"),
                    "time_to_start_ms": time_to_start_ms,
                    "duration_ms": duration_ms,
                },
                "participants": assignments,
                "completion": session.get("completion"),
                "events": session_events,
            }))
        })
        .collect::<Result<Vec<_>, AppError>>()?;
    let config = experiment.get("config").unwrap_or(&Value::Null);
    Ok(json!({
        "export_schema_version": "parlando.corpus.v1",
        "export_variant": "corpus",
        "release_status": "corpus_candidate",
        "content_review_required": true,
        "privacy_contract_version": privacy_contract_version,
        "data_inventory": {
            "participants": participants.len(),
            "sessions": sessions.len(),
            "events": semantic_event_count,
            "actions": action_count,
            "messages": message_count,
        },
        "experiment": {
            "experiment_id": experiment.get("experiment_id"),
            "game": {
                "version": experiment.get("game_version"),
            },
            "configuration": config.get("game").cloned().unwrap_or_else(|| json!({})),
            "participants": participants,
            "sessions": sessions,
        },
    }))
}

/// Creates a visible conflict for corrupt timing or identity data at the export boundary.
fn export_integrity_error(message: impl Into<String>) -> AppError {
    AppError::new(
        StatusCode::CONFLICT,
        format!("Corpus export integrity error: {}", message.into()),
    )
}

/// Parses a required RFC3339 timestamp used only to derive a relative interval.
fn required_timestamp_ms(value: Option<&Value>, label: &str) -> Result<i64, AppError> {
    value
        .and_then(Value::as_str)
        .and_then(rfc3339_millis)
        .ok_or_else(|| export_integrity_error(format!("{label} timestamp is missing or invalid")))
}

/// Parses an optional RFC3339 timestamp while rejecting a present malformed value.
fn optional_timestamp_ms(value: Option<&Value>, label: &str) -> Result<Option<i64>, AppError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(value) => required_timestamp_ms(Some(value), label).map(Some),
    }
}

/// Computes a checked non-negative session interval.
fn checked_interval(end: i64, start: i64, session_id: &str, label: &str) -> Result<i64, AppError> {
    end.checked_sub(start)
        .filter(|value| *value >= 0)
        .ok_or_else(|| {
            export_integrity_error(format!("session {session_id} has a negative {label}"))
        })
}

/// Parses an RFC3339 timestamp into milliseconds for export-relative timing.
fn rfc3339_millis(value: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|timestamp| timestamp.timestamp_millis())
}

/// Serializes all tables in an export variant into a two-column CSV stream.
fn export_csv(exported: &Value) -> String {
    let mut output = String::from("table,row_json\n");
    if exported.get("export_schema_version").is_some() {
        push_csv_row(
            &mut output,
            "manifest",
            &json!({
                "export_schema_version": exported.get("export_schema_version"),
                "export_variant": exported.get("export_variant"),
                "release_status": exported.get("release_status"),
                "content_review_required": exported.get("content_review_required"),
                "privacy_contract_version": exported.get("privacy_contract_version"),
                "data_inventory": exported.get("data_inventory"),
            }),
        );
        if let Some(experiment) = exported.get("experiment") {
            let mut metadata = experiment.clone();
            let sessions = metadata
                .as_object_mut()
                .and_then(|object| object.remove("sessions"))
                .unwrap_or_else(|| json!([]));
            push_csv_row(&mut output, "experiment", &metadata);
            if let Some(rows) = sessions.as_array() {
                for row in rows {
                    push_csv_row(&mut output, "session", row);
                }
            }
        }
        return output;
    }
    for table in [
        "experiment",
        "participants",
        "sessions",
        "session_participants",
        "consent_declarations",
        "session_events",
        "events",
        "messages",
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

const ADMIN_EXPERIMENT_HTML: &str = include_str!("app/admin_dashboard.html");
const CORPUS_EXPORT_SCHEMA_V1: &str = include_str!("app/corpus-export-schema-v1.json");

async fn game_socket<A: Game>(
    State(state): State<Arc<AppState<A>>>,
    Path(room_id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Result<Response, AppError>
where
    A::State: Serialize,
{
    validate_websocket_origin(&state.config, &headers)?;
    let ticket = query
        .get("token")
        .ok_or_else(|| AppError::bad_request("token query parameter is required"))?;
    let claims = state
        .upgrade_tickets
        .consume(ticket, UpgradePurpose::Game, &room_id)
        .await
        .ok_or_else(|| AppError::forbidden("Invalid or expired game ticket"))?;
    if !state
        .participant_auth
        .generation_is_active(&claims.participant_session_id, claims.generation)
        .await
    {
        return Err(AppError::forbidden("Game ticket credential was revoked"));
    }
    let participant_session_id = claims.participant_session_id;
    let claimed_role = Some(claims.role);
    let role = participant_role(&state, &room_id, &participant_session_id).await?;
    if claimed_role
        .as_deref()
        .is_some_and(|claimed| claimed != role.as_str())
    {
        return Err(AppError::forbidden(
            "Game ticket role no longer matches participant",
        ));
    }
    Ok(ws
        .max_frame_size(32 * 1024)
        .max_message_size(64 * 1024)
        .on_upgrade(move |socket| async move {
            websocket_loop(state, socket, room_id, participant_session_id, role).await;
        }))
}

#[derive(Deserialize)]
struct AudioSocketQuery {
    token: String,
}

/// Authenticates and upgrades one participant-owned audio transport.
async fn audio_socket<A: Game>(
    State(state): State<Arc<AppState<A>>>,
    Path(room_id): Path<String>,
    Query(query): Query<AudioSocketQuery>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Result<Response, AppError>
where
    A::State: Serialize,
{
    validate_websocket_origin(&state.config, &headers)?;
    let claims = state
        .upgrade_tickets
        .consume(&query.token, UpgradePurpose::Audio, &room_id)
        .await
        .ok_or_else(|| AppError::forbidden("Invalid or expired audio token"))?;
    if !state
        .participant_auth
        .generation_is_active(&claims.participant_session_id, claims.generation)
        .await
    {
        #[cfg(not(test))]
        return Err(AppError::forbidden("Audio ticket credential was revoked"));
    }
    let role = participant_role(&state, &room_id, &claims.participant_session_id).await?;
    if role.as_str() != claims.role {
        return Err(AppError::forbidden(
            "Audio token role no longer matches participant",
        ));
    }
    Ok(ws
        .max_frame_size(8 * 1024)
        .max_message_size(8 * 1024)
        .on_upgrade(move |socket| async move {
            audio_websocket_loop(state, socket, claims).await;
        }))
}

/// Relays browser PCM and consumes normalized server-side transcription events.
async fn audio_websocket_loop<A: Game>(
    state: Arc<AppState<A>>,
    socket: WebSocket,
    claims: UpgradeTicketClaims,
) where
    A::State: Serialize,
{
    let room_id = claims.room_id.clone();
    let role = claims.role.clone();
    let participant_session_id = claims.participant_session_id.clone();
    let connection_key = format!("{room_id}:{role}");
    let liveness = Arc::new(ConnectionLiveness::new());
    let (connection_generation, mut shutdown, replaced) = register_connection(
        &state.audio_connections,
        connection_key.clone(),
        liveness.clone(),
    )
    .await;
    if replaced {
        state.telemetry.record_reconnection();
    }
    let (generation, mut outbound) = state.audio_rooms.connect(&room_id, &role).await;
    let (mut sender, mut incoming) = socket.split();
    let send_task = tokio::spawn(async move {
        while let Some(outbound) = outbound.recv().await {
            let message = match outbound {
                AudioOutbound::Binary(bytes) => Message::Binary(bytes),
                AudioOutbound::Text(text) => Message::Text(text),
            };
            if sender.send(message).await.is_err() {
                break;
            }
        }
    });

    let transcription = if let Some(provider) = state.transcription_provider.clone() {
        match tokio::time::timeout(
            Duration::from_secs(10),
            provider.start_session(TranscriptionSessionContext {
                role: role.clone(),
                language: state.config.transcription.language.clone(),
                model: state.config.transcription.model.clone(),
            }),
        )
        .await
        {
            Ok(Ok(session)) => Some(session),
            Ok(Err(error)) => {
                tracing::warn!(%error, %room_id, "could not start transcription session");
                state
                    .audio_rooms
                    .send_control(
                        &room_id,
                        &role,
                        json!({"type":"transcriptionStatus","ready":false,"message":"ASR unavailable"}).to_string(),
                    )
                    .await;
                None
            }
            Err(_) => {
                tracing::warn!(%room_id, "timed out starting transcription session");
                state.audio_rooms.send_control(
                    &room_id,
                    &role,
                    json!({"type":"transcriptionStatus","ready":false,"message":"ASR startup timed out"}).to_string(),
                ).await;
                None
            }
        }
    } else {
        None
    };
    let transcription_input = transcription.as_ref().map(|session| session.input.clone());
    let event_task = transcription.map(|mut session| {
        let state = state.clone();
        let room_id = room_id.clone();
        let role = role.clone();
        let participant_session_id = participant_session_id.clone();
        tokio::spawn(async move {
            while let Some(event) = session.events.recv().await {
                match event {
                    TranscriptionEvent::Ready => {
                        state.audio_rooms.send_control(&room_id, &role, json!({"type":"transcriptionStatus","ready":true,"message":"ASR listening"}).to_string()).await;
                        mark_participant_audio_ready(&state, &room_id, &participant_session_id).await;
                        let _ = state.room_bus(&room_id).await.send(voice_message(&state, &room_id).await);
                        maybe_start_game(state.clone(), &room_id).await;
                    }
                    TranscriptionEvent::FinalUtterance(utterance) => {
                        if let Err(error) = commit_final_transcript(&state, &room_id, &participant_session_id, utterance).await {
                            tracing::warn!(error = %error.message, %room_id, "failed to commit final transcript");
                        }
                    }
                    TranscriptionEvent::Failed(error) => {
                        state.audio_rooms.send_control(&room_id, &role, json!({"type":"transcriptionStatus","ready":false,"message":"ASR error"}).to_string()).await;
                        tracing::warn!(%error, %room_id, "transcription session failed");
                    }
                }
            }
        })
    });
    if transcription_input.is_none() && !state.config.transcription.enabled {
        state
            .audio_rooms
            .send_control(
                &room_id,
                &role,
                json!({"type":"transcriptionStatus","ready":true,"message":"ASR idle"}).to_string(),
            )
            .await;
        mark_participant_audio_ready(&state, &room_id, &participant_session_id).await;
        let _ = state
            .room_bus(&room_id)
            .await
            .send(voice_message(&state, &room_id).await);
        maybe_start_game(state.clone(), &room_id).await;
    }

    let mut last_activity_touch = Instant::now();
    let media_started = Instant::now();
    let mut first_timestamp_ms = None;
    let mut last_sequence = None;
    let mut last_timestamp_ms = None;
    let mut dropped_audio_frames = 0_u64;
    loop {
        let next_message = tokio::select! {
            _ = shutdown.recv() => break,
            message = incoming.next() => message,
        };
        let Some(Ok(message)) = next_message else {
            break;
        };
        match message {
            Message::Binary(bytes) => match AudioFrame::decode(&bytes) {
                Ok(frame) => {
                    liveness.touch_message(true);
                    let stream_origin = *first_timestamp_ms.get_or_insert(frame.timestamp_ms);
                    let media_elapsed_ms = frame.timestamp_ms.saturating_sub(stream_origin);
                    let wall_elapsed_ms = media_started.elapsed().as_millis() as u64;
                    let timing_valid = last_sequence
                        .is_none_or(|sequence| frame.sequence > sequence)
                        && last_timestamp_ms
                            .is_none_or(|timestamp| frame.timestamp_ms >= timestamp)
                        && media_elapsed_ms
                            <= wall_elapsed_ms.saturating_mul(2).saturating_add(5_000);
                    if !timing_valid {
                        dropped_audio_frames += 1;
                        state.telemetry.record_audio_frame_dropped();
                        continue;
                    }
                    last_sequence = Some(frame.sequence);
                    last_timestamp_ms = Some(frame.timestamp_ms);
                    if last_activity_touch.elapsed() >= Duration::from_secs(30) {
                        touch_room_activity(&state, &room_id).await;
                        last_activity_touch = Instant::now();
                    }
                    if !state
                        .audio_rooms
                        .is_current(&room_id, &role, &generation)
                        .await
                    {
                        break;
                    }
                    state
                        .audio_rooms
                        .relay_partner(&room_id, &role, bytes)
                        .await;
                    state.telemetry.record_audio_frame();
                    if let Some(input) = &transcription_input {
                        if input.try_send(TranscriptionInput::Audio(frame)).is_err() {
                            state.telemetry.record_asr_backpressure();
                            state.audio_rooms.send_control(&room_id, &role, json!({"type":"transcriptionStatus","ready":false,"message":"ASR is falling behind"}).to_string()).await;
                        }
                    }
                }
                Err(error) => {
                    dropped_audio_frames += 1;
                    state.telemetry.record_audio_frame_dropped();
                    tracing::debug!(%error, %room_id, "dropped invalid browser audio frame")
                }
            },
            Message::Close(_) => break,
            _ => {}
        }
    }
    if let Some(input) = transcription_input {
        let _ = tokio::time::timeout(
            Duration::from_secs(2),
            input.send(TranscriptionInput::Finish),
        )
        .await;
    }
    if dropped_audio_frames > 0 {
        tracing::debug!(dropped_audio_frames, %room_id, "browser audio frames were dropped");
    }
    state
        .audio_rooms
        .disconnect(&room_id, &role, &generation)
        .await;
    unregister_connection(
        &state.audio_connections,
        &connection_key,
        &connection_generation,
    )
    .await;
    send_task.abort();
    if let Some(task) = event_task {
        let mut task = task;
        if tokio::time::timeout(Duration::from_secs(5), &mut task)
            .await
            .is_err()
        {
            task.abort();
        }
    }
}

async fn websocket_loop<A: Game>(
    state: Arc<AppState<A>>,
    socket: WebSocket,
    room_id: String,
    participant_session_id: String,
    role: Seat,
) where
    A::State: Serialize,
{
    let connection_key = format!("{room_id}:{}", role.as_str());
    let liveness = Arc::new(ConnectionLiveness::new());
    let (connection_generation, mut shutdown, replaced) = register_connection(
        &state.game_connections,
        connection_key.clone(),
        liveness.clone(),
    )
    .await;
    if replaced {
        state.telemetry.record_reconnection();
    }
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
                .recipient()
                .is_some_and(|target| target != outbound_participant_session_id)
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
    let mut transport_budget = TokenBucket::new(200.0, 100.0);
    loop {
        let next_message = tokio::select! {
            _ = shutdown.recv() => break,
            _ = tokio::time::sleep(Duration::from_secs(90)) => break,
            message = FuturesStreamExt::next(&mut incoming) => message,
        };
        let Some(Ok(message)) = next_message else {
            break;
        };
        let Message::Text(text) = message else {
            continue;
        };
        state.telemetry.record_game_message();
        if text.len() > 64 * 1024 {
            let _ = bus.send(error_message(
                &participant_session_id,
                &room_id,
                "message_too_large",
                true,
            ));
            break;
        }
        if !transport_budget.consume() {
            state.telemetry.record_transport_rejected();
            persist_rejected_input(
                &state,
                &room_id,
                &participant_session_id,
                "client_message_rejected",
                "transport_pacing",
                "Game channel exceeded 100 messages per second after a burst of 200",
                Some(text.as_bytes()),
            )
            .await;
            continue;
        }
        let Ok(client_message) = serde_json::from_str::<ClientMessage>(&text) else {
            let _ = bus.send(error_message(
                &participant_session_id,
                &room_id,
                "invalid_message",
                false,
            ));
            continue;
        };
        if matches!(&client_message, ClientMessage::Heartbeat) {
            liveness.touch_heartbeat();
            state.telemetry.record_heartbeat();
        } else {
            liveness.touch_message(true);
        }
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
    let still_owner = unregister_connection(
        &state.game_connections,
        &connection_key,
        &connection_generation,
    )
    .await;
    if !still_owner {
        return;
    }
    flush_rejected_input_aggregates(&state, &room_id, &participant_session_id).await;
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
    let _ = bus.send(presence_message(&state, &room_id).await.unwrap_or_else(|| {
        ServerMessage::broadcast(ServerPayload::Presence {
            room_id: room_id.clone(),
            presence: Value::Null,
        })
    }));
}

/// Atomically records an intentional departure and closes the room to further game input.
async fn abandon_room<A: Game>(
    state: &Arc<AppState<A>>,
    room_id: &str,
    participant_session_id: &str,
) -> Result<bool> {
    let transition_lock = room_transition_lock(state, room_id).await;
    let _transition_guard = transition_lock.lock().await;
    let status = state
        .memory
        .read()
        .await
        .rooms
        .get(room_id)
        .map(|room| room.status.clone())
        .ok_or_else(|| anyhow!("Room not found."))?;
    if room_status_is_terminal(&status) {
        return Ok(false);
    }
    let event = session_event_record(
        state,
        room_id,
        Some(participant_session_id),
        "session_abandoned",
        json!({"reason": "participant_left"}),
        None,
    )
    .await?;
    state.store.abandon_session(event).await?;
    if let Some(room) = state.memory.write().await.rooms.get_mut(room_id) {
        room.status = "abandoned".to_string();
        room.updated_at = now_iso();
    }
    Ok(true)
}

async fn handle_client_message<A: Game>(
    state: Arc<AppState<A>>,
    bus: &broadcast::Sender<ServerMessage>,
    room_id: &str,
    participant_session_id: &str,
    role: Seat,
    message: ClientMessage,
) where
    A::State: Serialize,
{
    match message {
        ClientMessage::Heartbeat => {
            // Transport liveness only: heartbeats intentionally cause no database write,
            // room-activity refresh, presence broadcast, or research event.
        }
        ClientMessage::Ready => {
            let first_declaration = {
                let mut memory = state.memory.write().await;
                memory
                    .rooms
                    .get_mut(room_id)
                    .and_then(|room| room.participants.get_mut(participant_session_id))
                    .is_some_and(|participant| {
                        if participant.ready_declared {
                            return false;
                        }
                        participant.ready_declared = true;
                        participant.updated_at = now_iso();
                        true
                    })
            };
            if !first_declaration {
                return;
            }
            if let Err(error) = persist_session_event_required(
                &state,
                room_id,
                Some(participant_session_id),
                "ready",
                Value::Null,
                None,
            )
            .await
            {
                if let Some(participant) = state
                    .memory
                    .write()
                    .await
                    .rooms
                    .get_mut(room_id)
                    .and_then(|room| room.participants.get_mut(participant_session_id))
                {
                    participant.ready_declared = false;
                }
                tracing::error!(%error, room_id, "could not durably record readiness");
                let _ = bus.send(error_message(
                    participant_session_id,
                    room_id,
                    "readiness_failed",
                    true,
                ));
                return;
            }
            touch_room_activity(&state, room_id).await;
            if let Some(message) = presence_message(&state, room_id).await {
                let _ = bus.send(message);
            }
            let _ = bus.send(voice_message(&state, room_id).await);
        }
        ClientMessage::Leave => match abandon_room(&state, room_id, participant_session_id).await {
            Ok(true) => {
                let _ = bus.send(ServerMessage::broadcast(ServerPayload::Abandoned {
                    room_id: room_id.to_string(),
                    code: "participant_left".to_string(),
                }));
            }
            Ok(false) => {}
            Err(error) => {
                tracing::error!(%error, room_id, "could not abandon session");
                let _ = bus.send(error_message(
                    participant_session_id,
                    room_id,
                    "session_end_failed",
                    false,
                ));
            }
        },
        ClientMessage::Message { text } => {
            if text.chars().count() > 4_000 {
                persist_rejected_input(
                    &state,
                    room_id,
                    participant_session_id,
                    "chat_message_rejected",
                    "message_too_large",
                    "Chat message exceeds 4000 characters",
                    Some(text.as_bytes()),
                )
                .await;
                let _ = bus.send(error_message(
                    participant_session_id,
                    room_id,
                    "message_too_large",
                    false,
                ));
                return;
            }
            let chat_allowed = state
                .chat_submission_budgets
                .write()
                .await
                .entry(participant_session_id.to_string())
                .or_insert_with(|| TokenBucket::new(20.0, 1.0))
                .consume();
            if !chat_allowed {
                persist_rejected_input(
                    &state,
                    room_id,
                    participant_session_id,
                    "chat_message_rejected",
                    "human_pacing",
                    "Chat burst exceeded twenty messages; retry after one second",
                    Some(text.as_bytes()),
                )
                .await;
                let _ = bus.send(error_message(
                    participant_session_id,
                    room_id,
                    "message_rate_limited",
                    false,
                ));
                return;
            }
            let input = ConversationMessageIn {
                text,
                origin: "typed".to_string(),
                source_message_id: None,
                metadata: json!({"sender_participant_session_id": participant_session_id}),
            };
            if let Err(error) =
                add_conversation(State(state.clone()), Path(room_id.to_string()), Json(input)).await
            {
                tracing::warn!(message = %error.message, room_id, "player message was rejected");
                let _ = bus.send(error_message(
                    participant_session_id,
                    room_id,
                    "message_rejected",
                    false,
                ));
            }
        }
        ClientMessage::Action { action: raw_action } => {
            let raw_action_bytes = match serde_json::to_vec(&raw_action) {
                Ok(bytes) => bytes,
                Err(error) => {
                    persist_rejected_action(
                        &state,
                        room_id,
                        participant_session_id,
                        "invalid_action_json",
                        &error.to_string(),
                        None,
                    )
                    .await;
                    let _ = bus.send(error_message(
                        participant_session_id,
                        room_id,
                        "invalid_action",
                        false,
                    ));
                    return;
                }
            };
            if raw_action_bytes.len() > 4 * 1024 {
                persist_rejected_action(
                    &state,
                    room_id,
                    participant_session_id,
                    "action_too_large",
                    "Action payload exceeds 4096 bytes",
                    Some(&raw_action_bytes),
                )
                .await;
                let _ = bus.send(error_message(
                    participant_session_id,
                    room_id,
                    "action_too_large",
                    false,
                ));
                return;
            }
            let action = match serde_json::from_value(raw_action) {
                Ok(action) => action,
                Err(error) => {
                    persist_rejected_action(
                        &state,
                        room_id,
                        participant_session_id,
                        "invalid_action",
                        &error.to_string(),
                        Some(&raw_action_bytes),
                    )
                    .await;
                    let _ = bus.send(error_message(
                        participant_session_id,
                        room_id,
                        "invalid_action",
                        false,
                    ));
                    return;
                }
            };
            let observed_action = match protocol_json(&action) {
                Ok(action) => action,
                Err(error) => {
                    tracing::error!(%error, room_id, "could not serialize accepted action");
                    let _ = bus.send(internal_error_message(participant_session_id, room_id));
                    return;
                }
            };
            match submit_action(state.clone(), room_id, participant_session_id, role, action).await
            {
                Ok((completed, completion)) => {
                    broadcast_player_views(
                        state.clone(),
                        room_id,
                        role.player_role(),
                        observed_action,
                    )
                    .await;
                    if completed {
                        let _ = bus.send(ServerMessage::broadcast(ServerPayload::Completed {
                            room_id: room_id.to_string(),
                            completion: completion.unwrap_or(Value::Null),
                        }));
                    }
                }
                Err(error) => {
                    let rejection_code = error
                        .downcast_ref::<ActionRejection>()
                        .map(|rejection| rejection.code.as_str())
                        .or_else(|| {
                            error
                                .downcast_ref::<SubmissionRejection>()
                                .map(|rejection| rejection.0)
                        });
                    let Some(rejection_code) = rejection_code else {
                        tracing::error!(%error, room_id, "action submission failed internally");
                        let _ = bus.send(internal_error_message(participant_session_id, room_id));
                        return;
                    };
                    persist_rejected_action(
                        &state,
                        room_id,
                        participant_session_id,
                        rejection_code,
                        rejection_code,
                        Some(&raw_action_bytes),
                    )
                    .await;
                    let _ = bus.send(action_rejected_message(
                        participant_session_id,
                        room_id,
                        rejection_code,
                    ));
                }
            }
        }
    }
}

/// Persists one analyzable, size-bounded rejected-action event.
async fn persist_rejected_action<A: Game>(
    state: &Arc<AppState<A>>,
    room_id: &str,
    participant_session_id: &str,
    reason_code: &str,
    error: &str,
    raw_action: Option<&[u8]>,
) {
    persist_rejected_input(
        state,
        room_id,
        participant_session_id,
        "game_action_rejected",
        reason_code,
        error,
        raw_action,
    )
    .await;
}

/// Persists the first and then one periodic aggregate for repeated rejected inputs.
async fn persist_rejected_input<A: Game>(
    state: &Arc<AppState<A>>,
    room_id: &str,
    participant_session_id: &str,
    event_type: &'static str,
    reason_code: &str,
    error: &str,
    raw_input: Option<&[u8]>,
) {
    match event_type {
        "game_action_rejected" => state.telemetry.record_action_rejected(),
        "chat_message_rejected" => state.telemetry.record_chat_rejected(),
        _ => {}
    }
    let now = chrono::Utc::now().timestamp();
    let key = format!("{room_id}\0{participant_session_id}\0{event_type}\0{reason_code}");
    let aggregate = {
        let mut windows = state.rejection_windows.write().await;
        let window = windows.entry(key).or_insert(RejectionWindow {
            first_seen: now,
            last_seen: now,
            last_persisted: now - 61,
            occurrences: 0,
        });
        window.occurrences += 1;
        window.last_seen = now;
        if window.last_persisted > now - 60 {
            return;
        }
        let aggregate = (window.first_seen, window.last_seen, window.occurrences);
        window.first_seen = now;
        window.last_persisted = now;
        window.occurrences = 0;
        aggregate
    };
    let mut payload = json!({
        "reason_code": reason_code,
        "error": error.chars().take(512).collect::<String>(),
        "first_seen_at": aggregate.0,
        "last_seen_at": aggregate.1,
        "occurrences": aggregate.2,
    });
    if let Some(raw_input) = raw_input {
        if let Some(object) = payload.as_object_mut() {
            object.insert("submitted_bytes".to_string(), json!(raw_input.len()));
            object.insert(
                if event_type == "game_action_rejected" {
                    "action_sha256"
                } else {
                    "input_sha256"
                }
                .to_string(),
                json!(format!("{:x}", Sha256::digest(raw_input))),
            );
        }
    }
    persist_session_event(
        state,
        room_id,
        Some(participant_session_id),
        event_type,
        payload,
        None,
    )
    .await;
}

/// Flushes suppressed rejection counts when the participant's current game transport ends.
async fn flush_rejected_input_aggregates<A: Game>(
    state: &Arc<AppState<A>>,
    room_id: &str,
    participant_session_id: &str,
) {
    let prefix = format!("{room_id}\0{participant_session_id}\0");
    let aggregates = {
        let mut windows = state.rejection_windows.write().await;
        let keys = windows
            .keys()
            .filter(|key| key.starts_with(&prefix))
            .cloned()
            .collect::<Vec<_>>();
        keys.into_iter()
            .filter_map(|key| {
                let window = windows.remove(&key)?;
                let mut parts = key.split('\0');
                let _room = parts.next()?;
                let _participant = parts.next()?;
                let event_type = match parts.next()? {
                    "game_action_rejected" => "game_action_rejected",
                    "chat_message_rejected" => "chat_message_rejected",
                    "client_message_rejected" => "client_message_rejected",
                    _ => return None,
                };
                let reason_code = parts.next()?.to_string();
                (window.occurrences > 0).then_some((event_type, reason_code, window))
            })
            .collect::<Vec<_>>()
    };
    for (event_type, reason_code, window) in aggregates {
        persist_session_event(
            state,
            room_id,
            Some(participant_session_id),
            event_type,
            json!({
                "reason_code": reason_code,
                "first_seen_at": window.first_seen,
                "last_seen_at": window.last_seen,
                "occurrences": window.occurrences,
                "coalesced": true,
            }),
            None,
        )
        .await;
    }
}

async fn submit_action<A: Game>(
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
    let transition_lock = room_transition_lock(&state, room_id).await;
    let _transition_guard = transition_lock.lock().await;
    let (after, completion, transition_metadata) = {
        let memory = state.memory.read().await;
        let room = memory
            .rooms
            .get(room_id)
            .ok_or_else(|| anyhow!("Room not found."))?;
        if room_status_is_terminal(&room.status) {
            return Err(anyhow!(SubmissionRejection("session_complete")));
        }
        if !room_ready_for_game::<A>(&state.config, room) {
            if speechmatics_readiness_required(&state.config) {
                return Err(anyhow!(SubmissionRejection("transcription_not_ready")));
            }
            return Err(anyhow!(SubmissionRejection("players_not_ready")));
        }
        let after = state
            .adapter
            .apply_action(&room.state, &action, player)
            .map_err(|rejection| anyhow!(rejection))?;
        let completion = state.adapter.completion(&after);
        let transition_metadata =
            state
                .adapter
                .transition_metadata(&room.state, &after, &action, player);
        (after, completion, transition_metadata)
    };
    let completed = completion.is_some();
    let after_json = protocol_json(&after)?;
    let stored_game_state = Some(after_json.clone());
    let completion_json = completion.as_ref().map(protocol_json).transpose()?;
    let accepted_payload = json!({
        "action": protocol_json(&action)?,
        "metadata": transition_metadata,
    });
    let mut durable_events = vec![
        session_event_record(
            &state,
            room_id,
            Some(participant_session_id),
            "game_action_accepted",
            accepted_payload,
            stored_game_state,
        )
        .await?,
    ];
    if completed {
        durable_events.push(
            session_event_record(
                &state,
                room_id,
                Some(participant_session_id),
                "session_completed",
                completion_json.clone().unwrap_or(Value::Null),
                None,
            )
            .await?,
        );
    }
    if let Err(error) = state
        .store
        .commit_session_transition(
            durable_events,
            completed.then(|| completion_json.clone().unwrap_or(Value::Null)),
        )
        .await
    {
        tracing::error!(%error, room_id, "could not commit game transition");
        return Err(anyhow!("Action could not be committed."));
    }
    state.telemetry.record_action_accepted();
    {
        let mut memory = state.memory.write().await;
        let room = memory
            .rooms
            .get_mut(room_id)
            .ok_or_else(|| anyhow!("Room not found."))?;
        room.state = after;
        room.updated_at = now_iso();
        if completed {
            room.status = "completed".to_string();
        }
    }
    notify_agents_of_action(&state, room_id, player, action.clone(), completion).await;
    Ok((completed, completion_json))
}

async fn broadcast_player_views<A: Game>(
    state: Arc<AppState<A>>,
    room_id: &str,
    actor: PlayerRole,
    action: Value,
) where
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
        if let Ok(response) = room_response(&state, room_id, role).await {
            if let Some(observation) = response.observation {
                let _ = bus.send(ServerMessage::targeted(
                    participant_session_id,
                    ServerPayload::Transition {
                        room_id: room_id.to_string(),
                        actor: actor.as_str().to_string(),
                        action: action.clone(),
                        observation,
                        available_actions: response.available_actions,
                    },
                ));
            }
        }
    }
}

async fn maybe_start_room_agents<A: Game>(state: Arc<AppState<A>>, room_id: &str)
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
        let key = agent_key(room_id, &participant_session_id);
        let agent = state.pending_agents.lock().await.remove(&key);
        if let Some(agent) = agent {
            maybe_start_agent(
                state.clone(),
                room_id.to_string(),
                participant_session_id,
                role,
                agent,
            )
            .await;
        }
    }
}

/// Builds the stable map key used for one room-local agent instance.
fn agent_key(room_id: &str, participant_session_id: &str) -> String {
    format!("{room_id}:{participant_session_id}")
}

/// Converts a wire-format role string into a player role.
fn player_role_from_str(value: &str) -> Option<PlayerRole> {
    match value {
        "A" => Some(PlayerRole::A),
        "B" => Some(PlayerRole::B),
        _ => None,
    }
}

/// Returns the currently available actions for an agent role in a room.
async fn agent_available_actions<A: Game>(
    state: &Arc<AppState<A>>,
    room_id: &str,
    role: Seat,
) -> Result<Option<Vec<A::Action>>> {
    let memory = state.memory.read().await;
    let room = memory
        .rooms
        .get(room_id)
        .ok_or_else(|| anyhow!("Room not found."))?;
    Ok(state
        .adapter
        .available_actions(&room.state, role.player_role()))
}

/// Sends an accepted-action observation to every started agent in the room.
async fn notify_agents_of_action<A: Game>(
    state: &Arc<AppState<A>>,
    room_id: &str,
    actor: PlayerRole,
    action: A::Action,
    completion: Option<A::Completion>,
) where
    A::State: Serialize,
{
    let observations = {
        let memory = state.memory.read().await;
        let Some(room) = memory.rooms.get(room_id) else {
            return;
        };
        room.participants
            .values()
            .filter(|participant| participant.source == "agent")
            .map(|participant| {
                (
                    agent_key(room_id, &participant.participant_session_id),
                    AgentObservation::Action {
                        actor,
                        action: action.clone(),
                        resulting_observation: state
                            .adapter
                            .observation(&room.state, participant.role.player_role()),
                        completion: completion.clone(),
                    },
                )
            })
            .collect::<Vec<_>>()
    };
    let inboxes = state.agent_inboxes.read().await;
    for (key, observation) in observations {
        if let Some(sender) = inboxes.get(&key) {
            let _ = sender.send(observation).await;
        }
    }
}

/// Sends a player message only to agents controlling the other role.
async fn notify_agents_of_message<A: Game>(
    state: &Arc<AppState<A>>,
    room_id: &str,
    speaker: PlayerRole,
    text: String,
) {
    let keys = {
        let memory = state.memory.read().await;
        let Some(room) = memory.rooms.get(room_id) else {
            return;
        };
        room.participants
            .values()
            .filter(|participant| {
                participant.source == "agent" && participant.role.player_role() != speaker
            })
            .map(|participant| agent_key(room_id, &participant.participant_session_id))
            .collect::<Vec<_>>()
    };
    let inboxes = state.agent_inboxes.read().await;
    for key in keys {
        if let Some(sender) = inboxes.get(&key) {
            let _ = sender
                .send(AgentObservation::Message {
                    speaker,
                    text: text.clone(),
                })
                .await;
        }
    }
}

/// Converts an agent timeout result into a normal anyhow result.
fn flatten_agent_timeout<T>(
    result: std::result::Result<Result<T>, tokio::time::error::Elapsed>,
    timeout_message: &str,
) -> Result<T> {
    match result {
        Ok(result) => result,
        Err(_) => anyhow::bail!("{timeout_message}"),
    }
}

/// Persists and applies one validated agent response.
async fn handle_agent_response<A: Game>(
    state: Arc<AppState<A>>,
    room_id: &str,
    participant_session_id: &str,
    role: Seat,
    response: AgentResponse<A::Action>,
) -> Result<(bool, Option<Value>)>
where
    A::State: Serialize,
{
    let (action, message) = response.into_parts();
    let mut outcome = (false, None);
    if let Some(action) = action {
        let observed_action = protocol_json(&action)?;
        persist_session_event(
            &state,
            room_id,
            Some(participant_session_id),
            "agent_action",
            json!({"action": protocol_json(&action).unwrap_or(Value::Null)}),
            None,
        )
        .await;
        outcome =
            submit_action(state.clone(), room_id, participant_session_id, role, action).await?;
        broadcast_player_views(state.clone(), room_id, role.player_role(), observed_action).await;
    }
    if let Some(text) = message {
        if let Ok(Json(message)) = add_conversation(
            State(state.clone()),
            Path(room_id.to_string()),
            Json(ConversationMessageIn {
                text,
                origin: "agent".to_string(),
                source_message_id: None,
                metadata: json!({"sender_participant_session_id": participant_session_id}),
            }),
        )
        .await
        {
            speak_agent_message(&state, room_id, &message).await;
        }
    }
    Ok(outcome)
}

/// Asks one agent for an optional response and applies it when present.
async fn request_agent_decision<A: Game>(
    state: Arc<AppState<A>>,
    agent: &mut Box<dyn Agent<A> + Send>,
    room_id: &str,
    participant_session_id: &str,
    role: Seat,
    timeout: f64,
) -> Result<Option<(bool, Option<Value>)>>
where
    A::State: Serialize,
{
    let available_actions = agent_available_actions(&state, room_id, role).await?;
    let result = tokio::time::timeout(
        Duration::from_secs_f64(timeout),
        agent.respond(available_actions),
    )
    .await;
    let Some(response) = flatten_agent_timeout(result, "agent respond timeout")? else {
        return Ok(None);
    };
    let outcome = handle_agent_response(
        state.clone(),
        room_id,
        participant_session_id,
        role,
        response,
    )
    .await?;
    Ok(Some(outcome))
}

/// Converts one trusted agent message to speech with a network timeout but no application quota.
async fn speak_agent_message<A: Game>(
    state: &Arc<AppState<A>>,
    room_id: &str,
    message: &ConversationMessageResponse,
) {
    let Some(provider) = state.tts_provider.clone() else {
        return;
    };
    state.telemetry.begin_tts();
    let mut failed = false;
    persist_tts_diagnostic(
        state,
        room_id,
        "tts_message_started",
        json!({"message_id": message.id}),
    )
    .await;
    match tokio::time::timeout(
        Duration::from_secs(60),
        provider.synthesize(&message.text, &message.id),
    )
    .await
    {
        Ok(Ok(chunks)) => {
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
            }
            persist_tts_diagnostic(
                state,
                room_id,
                "tts_audio_summary",
                json!({
                    "message_id": message.id,
                    "chunks": chunks.len(),
                    "bytes": chunks.iter().map(|chunk| chunk.data.len()).sum::<usize>(),
                    "sample_rate": chunks.first().map(|chunk| chunk.sample_rate),
                    "channels": chunks.first().map(|chunk| chunk.channels),
                }),
            )
            .await;
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
                        failed = true;
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
        Ok(Err(error)) => {
            failed = true;
            persist_tts_diagnostic(
                state,
                room_id,
                "tts_message_failed",
                json!({"message_id": message.id, "error": error.to_string()}),
            )
            .await;
        }
        Err(_) => {
            failed = true;
            persist_tts_diagnostic(
                state,
                room_id,
                "tts_message_failed",
                json!({"message_id": message.id, "error": "TTS network timeout"}),
            )
            .await;
        }
    }
    state.telemetry.finish_tts(failed);
}

// Persists one TTS diagnostic event into the evaluation event stream.
async fn persist_tts_diagnostic<A: Game>(
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
        }),
        None,
    )
    .await;
}

async fn maybe_start_agent<A: Game>(
    state: Arc<AppState<A>>,
    room_id: String,
    participant_session_id: String,
    role: Seat,
    mut agent: Box<dyn Agent<A> + Send>,
) where
    A::State: Serialize,
{
    let Some(factory) = state.agent_factory.clone() else {
        return;
    };
    let key = format!("{room_id}:{participant_session_id}");
    {
        let mut started = state.started_agents.write().await;
        if !started.insert(key.clone()) {
            return;
        }
    }
    tokio::spawn(async move {
        let role_name = role.as_str().to_string();
        let agent_settings = state
            .config
            .agents
            .human_vs_agent
            .as_ref()
            .map(|config| config.config.clone())
            .unwrap_or_else(|| json!({}));
        let definition = factory.definition();
        let Ok(agent_settings) = definition.normalize_settings(&agent_settings) else {
            return;
        };
        let Ok(agent_identity) = factory.identity(&agent_settings) else {
            return;
        };
        if agent_identity.validate().is_err() {
            return;
        }
        let Ok(fingerprint) = configuration_fingerprint(&definition.id, &agent_settings) else {
            return;
        };
        let agent_metadata = json!({
            "agent_name": agent_identity.name,
            "agent_version": agent_identity.version,
            "factory": definition.id,
            "configuration_fingerprint": fingerprint,
        });
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
        let (sender, mut receiver) = mpsc::channel(64);
        state
            .agent_inboxes
            .write()
            .await
            .insert(agent_key(&room_id, &participant_session_id), sender);
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
        let timeout = state
            .config
            .agents
            .human_vs_agent
            .as_ref()
            .map(|c| c.act_timeout_seconds)
            .unwrap_or(10.0);
        let initial_observation = {
            let memory = state.memory.read().await;
            memory
                .rooms
                .get(&room_id)
                .map(|room| state.adapter.observation(&room.state, role.player_role()))
        };
        let Some(initial_observation) = initial_observation else {
            return;
        };
        let started = tokio::time::timeout(
            Duration::from_secs_f64(timeout),
            agent.start(initial_observation),
        )
        .await;
        if let Err(error) = flatten_agent_timeout(started, "agent start timeout") {
            persist_session_event(
                &state,
                &room_id,
                Some(&participant_session_id),
                "agent_error",
                json!({"last_error": error.to_string(), "phase": "start"}),
                None,
            )
            .await;
            return;
        }
        let mut awaiting_completion = false;
        'agent_loop: loop {
            let limit = state
                .config
                .agents
                .human_vs_agent
                .as_ref()
                .map(|c| c.invalid_action_limit)
                .unwrap_or(3);
            if !awaiting_completion {
                match request_agent_decision(
                    state.clone(),
                    &mut agent,
                    &room_id,
                    &participant_session_id,
                    role,
                    timeout,
                )
                .await
                {
                    Ok(Some((completed, completion))) => {
                        if completed {
                            awaiting_completion = true;
                            let _ = state
                                .room_bus(&room_id)
                                .await
                                .send(ServerMessage::broadcast(ServerPayload::Completed {
                                    room_id: room_id.clone(),
                                    completion: completion.unwrap_or(Value::Null),
                                }));
                        }
                    }
                    Ok(None) => {}
                    Err(error) => {
                        last_error = Some(error.to_string());
                        if error.downcast_ref::<ActionRejection>().is_some() {
                            invalid_actions += 1;
                            if invalid_actions < limit {
                                continue;
                            }
                        } else {
                            persist_session_event(
                                &state,
                                &room_id,
                                Some(&participant_session_id),
                                "agent_error",
                                json!({"last_error": last_error, "phase": "runtime"}),
                                None,
                            )
                            .await;
                            break;
                        }
                    }
                }
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

            let Some(observation) = receiver.recv().await else {
                break;
            };
            let (observed, completion) = match observation {
                AgentObservation::Action {
                    actor,
                    action,
                    resulting_observation,
                    completion,
                } => (
                    tokio::time::timeout(
                        Duration::from_secs_f64(timeout),
                        agent.observe_transition(actor, action, resulting_observation),
                    )
                    .await,
                    completion,
                ),
                AgentObservation::Message { speaker, text } => (
                    tokio::time::timeout(
                        Duration::from_secs_f64(timeout),
                        agent.observe_message(speaker, text),
                    )
                    .await,
                    None,
                ),
            };
            if let Err(error) = flatten_agent_timeout(observed, "agent observe timeout") {
                last_error = Some(error.to_string());
                invalid_actions += 1;
            }
            if let Some(completion) = completion {
                let finished = tokio::time::timeout(
                    Duration::from_secs_f64(timeout),
                    agent.finish(completion),
                )
                .await;
                if let Err(error) = flatten_agent_timeout(finished, "agent finish timeout") {
                    persist_session_event(
                        &state,
                        &room_id,
                        Some(&participant_session_id),
                        "agent_error",
                        json!({"last_error": error.to_string(), "phase": "finish"}),
                        None,
                    )
                    .await;
                }
                break;
            }
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
            continue 'agent_loop;
        }
        state
            .agent_inboxes
            .write()
            .await
            .remove(&agent_key(&room_id, &participant_session_id));
        match tokio::time::timeout(Duration::from_secs(5), agent.shutdown()).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => tracing::warn!(%error, room_id, "agent shutdown failed"),
            Err(error) => tracing::warn!(%error, room_id, "agent shutdown timed out"),
        }
        state.started_agents.write().await.remove(&key);
    });
}

async fn require_consent<A: Game>(
    state: &Arc<AppState<A>>,
    participant_session_id: &str,
) -> Result<(), AppError> {
    let memory = state.memory.read().await;
    let participant = memory
        .participants
        .get(participant_session_id)
        .ok_or_else(|| AppError::not_found("Participant session not found."))?;
    if participant.source == "direct" {
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

async fn require_room<A: Game>(state: &Arc<AppState<A>>, room_id: &str) -> Result<(), AppError> {
    if state.memory.read().await.rooms.contains_key(room_id) {
        Ok(())
    } else {
        Err(AppError::not_found("Room not found."))
    }
}

async fn participant_role<A: Game>(
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
fn participant_ready_for_game(config: &ExperimentConfig, participant: &RoomParticipant) -> bool {
    participant.connected
        && (participant.source == "agent"
            || participant.source == "worker"
            || !speechmatics_readiness_required(config)
            || participant.audio_ready)
}

/// Returns whether both player roles are connected and past required audio setup.
fn room_ready_for_game<A: Game>(config: &ExperimentConfig, room: &GameRoom<A::State>) -> bool {
    let ready_roles = room
        .participants
        .values()
        .filter(|participant| participant_ready_for_game(config, participant))
        .map(|participant| participant.role.player_role())
        .collect::<HashSet<_>>();
    ready_roles == HashSet::from([PlayerRole::A, PlayerRole::B])
}

/// Marks one participant's room-local audio/STT setup as complete.
async fn mark_participant_audio_ready<A: Game>(
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
        tracing::debug!(%room_id, %participant_session_id, "participant transcription initialized");
    }
}

/// Starts a room once all humans/agents and required audio setup are ready.
async fn maybe_start_game<A: Game>(state: Arc<AppState<A>>, room_id: &str)
where
    A::State: Serialize,
{
    let transition_lock = room_transition_lock(&state, room_id).await;
    let _transition_guard = transition_lock.lock().await;
    let durable_session = {
        let memory = state.memory.read().await;
        memory.rooms.get(room_id).and_then(|room| {
            (room.status == "waiting" && room_ready_for_game::<A>(&state.config, room))
                .then(|| (room.experiment_id.clone(), room.session_id))
        })
    };
    let Some((experiment_id, session_id)) = durable_session else {
        return;
    };
    match state.store.start_session(&experiment_id, session_id).await {
        Ok(true) => {}
        Ok(false) => {
            tracing::warn!(
                room_id,
                session_id,
                "waiting session was not durably startable"
            );
            return;
        }
        Err(error) => {
            tracing::error!(%error, room_id, session_id, "could not durably start session");
            return;
        }
    }
    {
        let mut memory = state.memory.write().await;
        let Some(room) = memory.rooms.get_mut(room_id) else {
            return;
        };
        room.status = "running".to_string();
        room.updated_at = now_iso();
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
async fn room_has_started<A: Game>(state: &Arc<AppState<A>>, room_id: &str) -> bool {
    let memory = state.memory.read().await;
    memory
        .rooms
        .get(room_id)
        .is_some_and(|room| room.status != "waiting")
}

/// Sends the targeted game-start payload for one participant.
async fn send_role_assignment<A: Game>(
    state: &Arc<AppState<A>>,
    room_id: &str,
    participant_session_id: &str,
    role: Seat,
) where
    A::State: Serialize,
{
    if let Ok(response) = room_response(state, room_id, role).await {
        let bus = state.room_bus(room_id).await;
        if let Some(observation) = response.observation {
            let _ = bus.send(ServerMessage::targeted(
                participant_session_id,
                ServerPayload::SessionStarted {
                    room_id: room_id.to_string(),
                    role: role.as_str().to_string(),
                    observation,
                    available_actions: response.available_actions,
                },
            ));
        }
    }
}

async fn presence_message<A: Game>(
    state: &Arc<AppState<A>>,
    room_id: &str,
) -> Option<ServerMessage> {
    let memory = state.memory.read().await;
    let room = memory.rooms.get(room_id)?;
    Some(ServerMessage::broadcast(ServerPayload::Presence {
        room_id: room_id.to_string(),
        presence: room_presence(room),
    }))
}

fn room_presence<S>(room: &GameRoom<S>) -> Value {
    json!(room
        .participants
        .values()
        .map(|participant| {
            (
                participant.role.as_str().to_string(),
                json!({
                    "connected": participant.connected,
                    "audioReady": participant.audio_ready,
                }),
            )
        })
        .collect::<serde_json::Map<_, _>>())
}

async fn voice_message<A: Game>(state: &Arc<AppState<A>>, room_id: &str) -> ServerMessage {
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
    ServerMessage::broadcast(ServerPayload::VoiceStatus {
        room_id: room_id.to_string(),
        voice: json!({
            "audioReady": audio_ready,
            "transcriptionReady": transcription_ready,
            "transcriptionStatus": if !state.config.transcription.enabled {
                "Disabled"
            } else if transcription_ready {
                "Ready"
            } else {
                "Initializing"
            },
        }),
    })
}

/// Builds one presentation-neutral runtime or transport failure.
fn error_message(recipient: &str, room_id: &str, code: &str, fatal: bool) -> ServerMessage {
    ServerMessage::targeted(
        recipient,
        ServerPayload::Error {
            room_id: room_id.to_string(),
            code: code.to_string(),
            fatal,
        },
    )
}

/// Builds a machine-readable expected game-rule rejection.
fn action_rejected_message(recipient: &str, room_id: &str, code: &str) -> ServerMessage {
    ServerMessage::targeted(
        recipient,
        ServerPayload::ActionRejected {
            room_id: room_id.to_string(),
            code: code.to_string(),
        },
    )
}

/// Builds a presentation-neutral internal failure without exposing diagnostics.
fn internal_error_message(recipient: &str, room_id: &str) -> ServerMessage {
    error_message(recipient, room_id, "internal_error", true)
}

fn protocol_json<T: Serialize>(value: &T) -> Result<Value> {
    Ok(serde_json::to_value(value)?)
}

#[cfg(test)]
mod tests;
