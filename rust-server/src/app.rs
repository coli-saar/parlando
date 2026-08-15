use std::{
    collections::{HashMap, HashSet},
    future::Future,
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{anyhow, Context, Result};
use axum::{
    extract::{
        ws::{Message, WebSocket},
        ConnectInfo, Extension, Path, Query, Request, State, WebSocketUpgrade,
    },
    http::{
        header::{
            AUTHORIZATION, CONTENT_DISPOSITION, CONTENT_TYPE, COOKIE, HOST, ORIGIN, SET_COOKIE,
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
use tokio::sync::{broadcast, mpsc, Mutex, RwLock, Semaphore};
use tower::limit::ConcurrencyLimitLayer;
use tower_http::{
    cors::{AllowOrigin, CorsLayer},
    limit::RequestBodyLimitLayer,
    services::{ServeDir, ServeFile},
    timeout::TimeoutLayer,
};

use crate::{
    agents::{
        AgentFactory, AgentInitContext, AgentResponse, AgentUtteranceKind, GameAgent,
        SharedAgentFactory,
    },
    audio::{
        AudioFrame, AudioOutbound, AudioRoomRegistry, SharedAudioRooms, AUDIO_CHANNELS,
        AUDIO_FRAME_DURATION_MS, AUDIO_PROTOCOL_VERSION, AUDIO_SAMPLE_RATE,
    },
    audio_publisher::{AgentAudioPublisher, RoomAgentAudioPublisher},
    auth::{
        AdminAuthenticator, AdminSession, ParticipantAuthenticator, ParticipantPrincipal,
        UpgradePurpose, UpgradeTicketClaims, UpgradeTicketStore,
    },
    config::{AgentsMode, ExperimentConfig},
    game::{GameAdapter, GameDescriptor, PlayerRole, Seat},
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
pub struct ServeOptions<A: GameAdapter> {
    /// Identity of the one game implementation compiled into this server process.
    pub game_descriptor: Option<GameDescriptor>,
    /// Factory used to create one fresh agent per agent participant.
    pub agent_factory: Option<Arc<dyn AgentFactory<A>>>,
    /// Streaming TTS provider used for agent-origin conversation messages.
    pub tts_provider: Option<Arc<dyn StreamingTtsProvider>>,
    /// Optional publisher used to send synthesized agent audio into the room relay.
    pub audio_publisher: Option<Arc<dyn AgentAudioPublisher>>,
    /// Optional server-side STT provider override used by tests or local deployments.
    pub transcription_provider: Option<Arc<dyn TranscriptionProvider>>,
    /// Game/client version metadata supplied by the game-specific binary.
    pub game_version_manifest: Option<Value>,
}

impl<A: GameAdapter> Default for ServeOptions<A> {
    /// Creates serve options with no agent factory configured.
    fn default() -> Self {
        Self {
            game_descriptor: None,
            agent_factory: None,
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
    if let Some(study) = value.get_mut("study").and_then(Value::as_object_mut) {
        study.remove("institution");
    }
    for provider in ["speechmatics", "tts"] {
        if let Some(settings) = value.get_mut(provider).and_then(Value::as_object_mut) {
            settings.remove("api_key");
        }
    }
    redact_secret_fields(&mut value);
    Ok(value)
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
                    *child = Value::String(String::new());
                } else {
                    redact_secret_fields(child);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(redact_secret_fields),
        _ => {}
    }
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

/// Hashes the complete participant-information reference and ordered consent presentation.
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
async fn security_headers<A: GameAdapter>(
    State(state): State<Arc<AppState<A>>>,
    mut request: Request,
    next: Next,
) -> Response {
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    request.extensions_mut().insert(CspNonce(nonce.clone()));
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        HeaderName::from_static("content-security-policy"),
        HeaderValue::from_str(&format!(
            "default-src 'self'; connect-src 'self' ws: wss:; img-src 'self' data:; style-src 'self' 'unsafe-inline'; script-src 'self' 'nonce-{nonce}'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'"
        ))
        .expect("generated CSP nonce is valid header text"),
    );
    if state.config.server.public_base_url.starts_with("https://") {
        headers.insert(
            HeaderName::from_static("strict-transport-security"),
            HeaderValue::from_static("max-age=31536000; includeSubDomains"),
        );
    }
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
fn spawn_security_cleanup<A: GameAdapter>(state: Arc<AppState<A>>, clean_admin_sessions: bool) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        loop {
            interval.tick().await;
            let expired_participants = state.participant_auth.cleanup().await;
            state.upgrade_tickets.cleanup().await;
            if clean_admin_sessions {
                state.admin_auth.cleanup().await;
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
        }
    });
}

/// Removes abandoned transient room registries after configured waiting/reconnect bounds.
async fn cleanup_transient_rooms<A: GameAdapter>(state: &Arc<AppState<A>>) {
    let now = chrono::Utc::now();
    let waiting_timeout = state.config.study.waiting_room_timeout_seconds.max(1);
    let reconnect_grace = state.config.study.reconnect_grace_seconds.max(1);
    let removed = {
        let mut memory = state.memory.write().await;
        let removed = memory
            .rooms
            .iter()
            .filter_map(|(room_id, room)| {
                let updated = chrono::DateTime::parse_from_rfc3339(&room.updated_at)
                    .ok()?
                    .with_timezone(&chrono::Utc);
                let timeout = if room.status == "waiting" {
                    waiting_timeout
                } else {
                    reconnect_grace
                };
                let has_connection = room
                    .participants
                    .values()
                    .any(|participant| participant.connected);
                (!has_connection && updated < now - chrono::Duration::seconds(timeout))
                    .then(|| room_id.clone())
            })
            .collect::<Vec<_>>();
        for room_id in &removed {
            memory.rooms.remove(room_id);
        }
        removed
    };
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
}

/// Applies a process-wide safety ceiling to unauthenticated participant creation bursts.
async fn enforce_creation_rate<A: GameAdapter>(
    state: &Arc<AppState<A>>,
    client_ip: Option<IpAddr>,
) -> Result<(), AppError> {
    let now = chrono::Utc::now().timestamp();
    let mut attempts = state.participant_creation_window.write().await;
    attempts
        .record(client_ip, now)
        .then_some(())
        .ok_or_else(|| {
            AppError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "Participant creation rate limit exceeded",
            )
        })
}

/// Authenticates participant API requests and attaches the resolved principal.
async fn require_participant_auth<A: GameAdapter>(
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

/// Authenticates every administrator route and enforces role and CSRF on mutations.
async fn require_admin_auth<A: GameAdapter>(
    State(state): State<Arc<AppState<A>>>,
    mut request: Request,
    next: Next,
) -> Result<Response, AppError> {
    let token = cookie_value(request.headers(), "parlando_admin")
        .ok_or_else(|| AppError::new(StatusCode::UNAUTHORIZED, "Authentication required"))?;
    if !state.admin_auth.is_configured().await {
        return Err(AppError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "Administrator authentication is not configured",
        ));
    }
    let session =
        state.admin_auth.authenticate(token).await.ok_or_else(|| {
            AppError::new(StatusCode::UNAUTHORIZED, "Invalid administrator session")
        })?;
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
async fn admin_login_page<A: GameAdapter>(
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
            "Sign in to manage experiments and review study sessions.",
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
      <aside class="context"><div class="context-copy"><div class="mark" aria-hidden="true">P</div><h2>Dialogue experiments, clearly managed.</h2><p>Configure studies, follow live sessions, and export research data from one focused workspace.</p></div><div class="context-footer">Secure administrator area</div></aside>
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
async fn admin_setup<A: GameAdapter>(
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
async fn admin_login<A: GameAdapter>(
    State(state): State<Arc<AppState<A>>>,
    headers: HeaderMap,
    Json(request): Json<AdminLoginRequest>,
) -> Result<Response, AppError> {
    validate_admin_request_origin(&state.config, &headers)?;
    admin_login_response(&state, &request.username, &request.password).await
}

/// Verifies a credential and builds the secure browser-session response.
async fn admin_login_response<A: GameAdapter>(
    state: &Arc<AppState<A>>,
    username: &str,
    password: &str,
) -> Result<Response, AppError> {
    let Some((token, session)) = state
        .admin_auth
        .login(username, password)
        .await
        .map_err(AppError::from)?
    else {
        tracing::warn!(username, "administrator login failed");
        return Err(AppError::new(
            StatusCode::UNAUTHORIZED,
            "Invalid administrator credentials",
        ));
    };
    tracing::info!(username, role = ?session.role, "administrator login succeeded");
    let mut response = Json(json!({"ok": true, "csrf_token": session.csrf_token})).into_response();
    response.headers_mut().insert(
        SET_COOKIE,
        HeaderValue::from_str(&format!(
            "parlando_admin={token}; Path=/; Max-Age=28800; Secure; HttpOnly; SameSite=Strict"
        ))
        .map_err(|error| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?,
    );
    Ok(response)
}

/// Revokes the current administrator session and expires its cookie.
async fn admin_logout<A: GameAdapter>(
    State(state): State<Arc<AppState<A>>>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    if let Some(token) = cookie_value(&headers, "parlando_admin") {
        state.admin_auth.logout(token).await;
    }
    tracing::info!("administrator logged out");
    let mut response = Json(json!({"ok": true})).into_response();
    response.headers_mut().insert(
        SET_COOKIE,
        HeaderValue::from_static(
            "parlando_admin=; Path=/; Max-Age=0; Secure; HttpOnly; SameSite=Strict",
        ),
    );
    Ok(response)
}

/// Shared server state used by HTTP handlers, WebSocket tasks, and background agents.
pub struct AppState<A: GameAdapter> {
    pub adapter: A,
    pub config: ExperimentConfig,
    pub experiment_id: String,
    /// Identity of the one compiled game which owns this experiment.
    pub game_descriptor: GameDescriptor,
    /// Settings shared by every experiment of this compiled game.
    pub game_settings: Arc<RwLock<StoredGameSettings>>,
    /// Immutable configuration revision used for newly created sessions.
    pub config_revision: i64,
    experiment_lifecycle: RwLock<ExperimentLifecycle>,
    pub memory: RwLock<MemoryState<A::State>>,
    pub store: SharedExperimentStore,
    pub room_buses: RwLock<HashMap<String, broadcast::Sender<ServerMessage>>>,
    pub agent_factory: Option<SharedAgentFactory<A>>,
    pub started_agents: RwLock<HashSet<String>>,
    agent_inboxes: RwLock<HashMap<String, mpsc::Sender<AgentObservation<A>>>>,
    pub tts_provider: Option<Arc<dyn StreamingTtsProvider>>,
    pub audio_publisher: Option<Arc<dyn AgentAudioPublisher>>,
    pub audio_rooms: SharedAudioRooms,
    pub transcription_provider: Option<Arc<dyn TranscriptionProvider>>,
    committed_transcripts: RwLock<HashSet<String>>,
    participant_auth: ParticipantAuthenticator,
    upgrade_tickets: UpgradeTicketStore,
    admin_auth: Arc<AdminAuthenticator>,
    participant_creation_window: RwLock<ParticipantCreationRate>,
    game_connection_limit: Arc<Semaphore>,
    audio_connection_limit: Arc<Semaphore>,
    provider_connection_limit: Arc<Semaphore>,
    room_transition_locks: RwLock<HashMap<String, Arc<Mutex<()>>>>,
    game_connections: RwLock<HashMap<String, ConnectionControl>>,
    audio_connections: RwLock<HashMap<String, ConnectionControl>>,
    pub version_manifest: Value,
}

/// Sliding-window participant-creation counters bounded by cleanup on each request.
#[derive(Default)]
struct ParticipantCreationRate {
    /// Process-wide creation timestamps used as a final safety ceiling.
    global: Vec<i64>,
    /// Creation timestamps for each trusted transport peer address.
    by_ip: HashMap<IpAddr, Vec<i64>>,
}

impl ParticipantCreationRate {
    /// Records one allowed attempt after pruning the bounded sixty-second window.
    fn record(&mut self, client_ip: Option<IpAddr>, now: i64) -> bool {
        self.global.retain(|timestamp| *timestamp > now - 60);
        self.by_ip.retain(|_, timestamps| {
            timestamps.retain(|timestamp| *timestamp > now - 60);
            !timestamps.is_empty()
        });
        if self.global.len() >= 300 {
            return false;
        }
        if let Some(client_ip) = client_ip {
            let client_attempts = self.by_ip.entry(client_ip).or_default();
            if client_attempts.len() >= 30 {
                return false;
            }
            client_attempts.push(now);
        }
        self.global.push(now);
        true
    }
}

mod lifecycle;
use lifecycle::{require_active_experiment, ExperimentLifecycle};

/// Event delivered to one started agent instance.
enum AgentObservation<A: GameAdapter> {
    /// Accepted action plus role-specific state snapshot after the action.
    Action {
        actor: PlayerRole,
        action: A::Action,
        resulting_observation: A::Observation,
    },
    /// Conversation utterance with known speaker role and modality.
    Message {
        speaker: PlayerRole,
        kind: AgentUtteranceKind,
        text: String,
    },
}

/// Cancellation handle for the single live connection that owns a participant room role.
struct ConnectionControl {
    generation: String,
    shutdown: mpsc::Sender<()>,
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

/// Appends a session event and propagates any persistence failure to the caller.
async fn persist_session_event_required<A: GameAdapter>(
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
async fn session_event_record<A: GameAdapter>(
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
async fn room_transition_lock<A: GameAdapter>(
    state: &Arc<AppState<A>>,
    room_id: &str,
) -> Arc<Mutex<()>> {
    let mut locks = state.room_transition_locks.write().await;
    locks
        .entry(room_id.to_string())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

/// Registers one role-owned connection and immediately cancels the previous owner.
async fn register_connection(
    connections: &RwLock<HashMap<String, ConnectionControl>>,
    key: String,
) -> (String, mpsc::Receiver<()>) {
    let generation = new_id("connection");
    let (shutdown, receiver) = mpsc::channel(1);
    if let Some(previous) = connections.write().await.insert(
        key,
        ConnectionControl {
            generation: generation.clone(),
            shutdown,
        },
    ) {
        let _ = previous.shutdown.try_send(());
    }
    (generation, receiver)
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
pub use routing::{build_game_router, build_router, serve, serve_game};

async fn health() -> Json<Value> {
    Json(json!({"status": "ok"}))
}

async fn public_config<A: GameAdapter>(
    State(state): State<Arc<AppState<A>>>,
) -> Json<PublicConfigResponse> {
    let config = &state.config;
    Json(PublicConfigResponse {
        study_name: config.study.name.clone(),
        experiment_status: state.experiment_lifecycle.read().await.as_str().to_string(),
        institution: nonempty_string(&state.game_settings.read().await.institution),
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
        voice: json!({
            "enabled": config.voice.enabled,
            "transport": "websocket",
            "sample_rate_hz": config.voice.sample_rate_hz,
            "frame_duration_ms": config.voice.frame_duration_ms,
            "jitter_buffer_ms": config.voice.jitter_buffer_ms,
        }),
        transcription: json!({
            "enabled": config.transcription.enabled,
            "language": config.transcription.language,
        }),
        tts: json!({
            "enabled": config.tts.enabled,
            "voice_name": if config.tts.voice_name.is_empty() { None } else { Some(config.tts.voice_name.clone()) },
        }),
        agents: json!({
            "mode": config.agents.mode,
            "human_vs_agent": config.agents.human_vs_agent.is_some(),
        }),
        privacy: json!({
            "contract_version": config.privacy.contract_version,
            "store_full_game_state": config.privacy.store_full_game_state,
            "store_typed_messages": config.privacy.store_typed_messages,
            "store_final_transcripts": config.privacy.store_final_transcripts,
            "store_voice_diagnostics": config.privacy.store_voice_diagnostics,
        }),
    })
}

async fn create_participant<A: GameAdapter>(
    State(state): State<Arc<AppState<A>>>,
    client: Option<ConnectInfo<SocketAddr>>,
    Json(request): Json<ParticipantCreateRequest>,
) -> Result<Json<ParticipantCreateResponse>, AppError> {
    create_participant_inner(state, client.map(|peer| peer.0.ip()), request)
        .await
        .map(Json)
}

async fn create_participant_inner<A: GameAdapter>(
    state: Arc<AppState<A>>,
    client_ip: Option<IpAddr>,
    _request: ParticipantCreateRequest,
) -> Result<ParticipantCreateResponse, AppError> {
    let _intake_guard = require_active_experiment(&state).await?;
    enforce_creation_rate(&state, client_ip).await?;
    if !state.config.direct.enabled {
        return Err(AppError::not_found("Direct mode is disabled."));
    }
    let mut memory = state.memory.write().await;
    if memory.participants.len() >= 10_000 {
        return Err(AppError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "Participant capacity reached",
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
        Some(state.config.study.name.clone()),
    );
    let participant_credential = state.participant_auth.issue(participant.id.clone()).await;
    Ok(ParticipantCreateResponse {
        participant_session_id: participant.id,
        participant_credential,
        source: participant.source,
        participant_id: participant.research_id,
    })
}

async fn consent<A: GameAdapter>(
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
    let participant_id = state
        .memory
        .read()
        .await
        .participants
        .get(&participant_session_id)
        .ok_or_else(|| AppError::not_found("Participant session not found."))?
        .participant_id;
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
    let consent_text_hash = Some(consent_configuration_hash(&state.config)?);
    let consent_metadata = json!({
        "participant_information_version": nonempty_string(&state.config.direct.participant_information_version),
        "participant_information_url": nonempty_string(&state.config.direct.participant_information_url),
    });
    for (consent_item_id, accepted) in &request.decisions {
        state
            .store
            .record_consent_declaration(ConsentDeclarationRecord {
                experiment_id: state.experiment_id.clone(),
                session_id,
                participant_id,
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
        if let Some(room_participant) = room.participants.get_mut(&participant_session_id) {
            room_participant
                .consent_decisions
                .extend(request.decisions.clone());
            room_participant.updated_at = now_iso();
        }
    }
    Ok(Json(json!({"ok": true})))
}

async fn create_room<A: GameAdapter>(
    State(state): State<Arc<AppState<A>>>,
    principal: Option<Extension<ParticipantPrincipal>>,
    Json(_request): Json<CreateRoomRequest>,
) -> Result<Json<CreateRoomResponse>, AppError>
where
    A::State: Serialize,
{
    let _intake_guard = require_active_experiment(&state).await?;
    let participant_session_id = authenticated_participant_id(principal)?;
    require_consent(&state, &participant_session_id).await?;
    let requested_mode = "direct".to_string();
    let paired_or_created: Result<(String, Seat, bool), AppError> = {
        let mut memory = state.memory.write().await;
        if memory.rooms.len() >= 1_000 {
            return Err(AppError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "Room capacity reached",
            ));
        }
        if state.config.agents.mode == AgentsMode::HumanVsHuman {
            if let Some(room_id) = open_human_room_for_pairing(
                &memory,
                &requested_mode,
                Some(state.config.study.name.as_str()),
            ) {
                let role = add_human_participant_to_room_locked(
                    &state,
                    &mut memory,
                    &room_id,
                    &participant_session_id,
                )?;
                Ok((room_id, role, false))
            } else {
                let (room_id, role) = create_room_locked(
                    &state,
                    &mut memory,
                    participant_session_id.clone(),
                    requested_mode.clone(),
                    Seat::A,
                    Some(state.config.study.name.clone()),
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
            let (room_id, role) = create_room_locked(
                &state,
                &mut memory,
                participant_session_id.clone(),
                requested_mode.clone(),
                Seat::A,
                Some(state.config.study.name.clone()),
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
        persist_session_event(
            &state,
            &room_id,
            None,
            "session_created",
            json!({"room_id": room_id}),
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
        add_agent_to_room(&state, &room_id).await?;
        maybe_start_room_agents(state.clone(), &room_id).await;
    }
    let response = room_response(&state, &room_id, &participant_session_id, role, vec![]).await?;
    Ok(Json(response))
}

async fn add_agent_to_room<A: GameAdapter>(
    state: &Arc<AppState<A>>,
    room_id: &str,
) -> Result<String, AppError> {
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
            experiment_id: state.experiment_id.clone(),
            participant_kind: "agent".to_string(),
            identity_provider: factory_identity.identity_provider,
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
            Some(state.config.study.name.clone()),
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
    study_id: Option<&str>,
) -> Option<String> {
    memory
        .rooms
        .iter()
        .find(|(_, room)| {
            room.status == "waiting"
                && room.mode == mode
                && room.study_id.as_deref() == study_id
                && next_role(room) == Some(Seat::B)
                && room
                    .participants
                    .values()
                    .all(|participant| participant.source != "agent")
        })
        .map(|(room_id, _)| room_id.clone())
}

/// Adds a direct human participant to an existing room and returns its assigned seat.
fn add_human_participant_to_room_locked<A: GameAdapter>(
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
            audio_ready: !speechmatics_readiness_required(&state.config),
            consent_decisions: participant.consent_decisions,
            joined_at: now_iso(),
            updated_at: now_iso(),
        },
    );
    Ok(role)
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
    let (presence, observation, available_actions, events) = {
        let memory = state.memory.read().await;
        let room = memory
            .rooms
            .get(room_id)
            .ok_or_else(|| AppError::not_found("Room not found."))?;
        let presence = room_presence(room);
        if room.status == "waiting" {
            return Ok(RoomResponse {
                room_id: room_id.to_string(),
                participant_session_id: participant_session_id.to_string(),
                role: role.as_str().to_string(),
                presence: Some(presence),
                state: None,
                observation: None,
                available_actions: None,
                events: vec![],
                conversation: vec![],
            });
        }
        let player = role.player_role();
        (
            Some(presence),
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
        presence,
        state: None,
        observation,
        available_actions,
        events,
        conversation: vec![],
    })
}

async fn audio_session<A: GameAdapter>(
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
async fn game_session<A: GameAdapter>(
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
async fn commit_final_transcript<A: GameAdapter>(
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
        if !state.config.privacy.store_final_transcripts {
            return Result::<()>::Ok(());
        }
        persist_session_event_required(
            state,
            room_id,
            Some(&stored.participant_session_id),
            "transcript_segment",
            serde_json::to_value(&stored).unwrap(),
            None,
        )
        .await?;
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
    let _ = state.room_bus(room_id).await.send(ServerMessage {
        conversation_message: Some(message),
        room_id: Some(room_id.to_string()),
        ..ServerMessage::new("conversationMessageAdded")
    });
    if let Some(speaker) = player_role_from_str(&stored.player) {
        notify_agents_of_message(
            state,
            room_id,
            speaker,
            AgentUtteranceKind::Spoken,
            stored.text.clone(),
        )
        .await;
    }
    Ok(Some(stored))
}

/// Accepts a bounded operational voice metric and persists it only when enabled.
async fn add_voice_diagnostic<A: GameAdapter>(
    State(state): State<Arc<AppState<A>>>,
    Path(room_id): Path<String>,
    principal: Option<Extension<ParticipantPrincipal>>,
    Json(diagnostic): Json<VoiceDiagnosticIn>,
) -> Result<Json<Value>, AppError> {
    let participant_session_id = authenticated_participant_id(principal)?;
    participant_role(&state, &room_id, &participant_session_id).await?;
    if !state.config.privacy.store_voice_diagnostics {
        return Ok(Json(json!({"stored": false})));
    }
    if diagnostic.event.is_empty()
        || diagnostic.event.len() > 64
        || !diagnostic
            .event
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(AppError::bad_request("Invalid voice diagnostic event name"));
    }
    let stored = json!({
        "id": new_id("vdiag"),
        "room_id": room_id,
        "participant_session_id": participant_session_id,
        "event": diagnostic.event,
        "metadata": minimized_voice_diagnostic_metadata(&diagnostic.metadata),
        "created_at": now_iso(),
    });
    persist_session_event_required(
        &state,
        stored["room_id"].as_str().unwrap_or(""),
        stored["participant_session_id"].as_str(),
        "voice_diagnostic",
        stored.clone(),
        None,
    )
    .await?;
    Ok(Json(stored))
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
async fn add_conversation<A: GameAdapter>(
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
    if message.origin != "typed" || state.config.privacy.store_typed_messages {
        persist_session_event_required(
            &state,
            &room_id,
            message.sender_participant_session_id.as_deref(),
            "conversation_message",
            serde_json::to_value(&message).unwrap(),
            None,
        )
        .await?;
    }
    let _ = state.room_bus(&room_id).await.send(ServerMessage {
        room_id: Some(room_id.clone()),
        conversation_message: Some(message.clone()),
        ..ServerMessage::new("conversationMessageAdded")
    });
    if let Some(speaker) = message
        .sender_role
        .as_deref()
        .and_then(player_role_from_str)
    {
        notify_agents_of_message(
            &state,
            &room_id,
            speaker,
            utterance_kind_from_origin(&message.origin),
            message.text.clone(),
        )
        .await;
    }
    Ok(Json(message))
}

/// Rejects participant game-channel input after a room has completed.
async fn ensure_room_accepts_game_input<A: GameAdapter>(
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
    study_name: Option<String>,
    config: Option<Value>,
    notes: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AdminCloneExperimentRequest {
    experiment_id: String,
    notes: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AdminSaveExperimentConfigRequest {
    expected_revision: i64,
    config: Value,
    change_summary: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AdminCatalogueRequest {
    pinned: bool,
    obsolete: bool,
    notes: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AdminGameSettingsRequest {
    expected_revision: i64,
    institution: String,
}

#[derive(Clone, Debug, Deserialize)]
struct AdminExportQuery {
    session_id: Option<i64>,
    status: Option<String>,
    event_type: Option<String>,
    format: Option<String>,
    variant: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
struct PrivacyStatus {
    generated_at: String,
    privacy_contract_version: String,
    review_state: String,
    software: PrivacySoftwareStatus,
    storage: Vec<PrivacyStorageStatus>,
    raw_audio_stored_by_parlando: bool,
    external_services: Vec<PrivacyServiceStatus>,
    exports: PrivacyExportStatus,
    participant_deletion: PrivacyFeatureStatus,
    consent_evidence: PrivacyFeatureStatus,
}

#[derive(Clone, Debug, Serialize)]
struct PrivacySoftwareStatus {
    server_name: String,
    server_version: String,
    git_sha: Option<String>,
    git_dirty: String,
}

#[derive(Clone, Debug, Serialize)]
struct PrivacyStorageStatus {
    category: String,
    persisted_when_produced: bool,
    configurable: bool,
    detail: String,
}

#[derive(Clone, Debug, Serialize)]
struct PrivacyServiceStatus {
    service: String,
    enabled: bool,
    data_sent: String,
}

#[derive(Clone, Debug, Serialize)]
struct PrivacyExportStatus {
    full_internal_export: bool,
    research_export: bool,
    corpus_export: bool,
    formats: Vec<String>,
    detail: String,
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
async fn admin_privacy_page<A: GameAdapter>(State(state): State<Arc<AppState<A>>>) -> Html<String> {
    Html(render_privacy_status_html(&privacy_status(&state)))
}

/// Returns the installation-wide privacy status as structured JSON for administrative tooling.
async fn admin_privacy_json<A: GameAdapter>(
    State(state): State<Arc<AppState<A>>>,
) -> Json<PrivacyStatus> {
    Json(privacy_status(&state))
}

/// Downloads the installation-wide privacy status as a pretty-printed JSON attachment.
async fn admin_privacy_json_download<A: GameAdapter>(
    State(state): State<Arc<AppState<A>>>,
) -> Result<Response, AppError> {
    let body = serde_json::to_string_pretty(&privacy_status(&state))
        .map_err(|error| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    Ok(privacy_download_response(
        body,
        "application/json; charset=utf-8",
        "parlando-privacy-status.json",
    ))
}

/// Downloads the installation-wide privacy status as a Markdown attachment for review records.
async fn admin_privacy_markdown_download<A: GameAdapter>(
    State(state): State<Arc<AppState<A>>>,
) -> Response {
    privacy_download_response(
        render_privacy_status_markdown(&privacy_status(&state)),
        "text/markdown; charset=utf-8",
        "parlando-privacy-status.md",
    )
}

/// Builds a privacy report from executable server facts and the effective runtime configuration.
fn privacy_status<A: GameAdapter>(state: &AppState<A>) -> PrivacyStatus {
    let server_manifest = state.version_manifest.get("server");
    let mut external_services = Vec::new();
    if state.config.transcription.enabled {
        external_services.push(PrivacyServiceStatus {
            service: state.config.transcription.provider.clone(),
            enabled: true,
            data_sent: "Live microphone audio for real-time transcription; Parlando receives text and timing information.".to_string(),
        });
    }
    if state.config.tts.enabled {
        external_services.push(PrivacyServiceStatus {
            service: state.config.tts.provider.clone(),
            enabled: true,
            data_sent: "Software-agent-generated text and technical voice/model parameters; no participant audio, transcript, identifier, or game state.".to_string(),
        });
    }
    PrivacyStatus {
        generated_at: now_iso(),
        privacy_contract_version: state.config.privacy.contract_version.clone(),
        review_state: "Not yet bound to a completed DPO platform assessment.".to_string(),
        software: PrivacySoftwareStatus {
            server_name: server_manifest
                .and_then(|value| value.get("name"))
                .and_then(Value::as_str)
                .unwrap_or(env!("CARGO_PKG_NAME"))
                .to_string(),
            server_version: server_manifest
                .and_then(|value| value.get("version"))
                .and_then(Value::as_str)
                .unwrap_or(env!("CARGO_PKG_VERSION"))
                .to_string(),
            git_sha: server_manifest
                .and_then(|value| value.get("git_sha"))
                .and_then(Value::as_str)
                .map(str::to_string),
            git_dirty: server_manifest
                .and_then(|value| value.get("git_dirty"))
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string(),
        },
        storage: vec![
            PrivacyStorageStatus {
                category: "Full game state".to_string(),
                persisted_when_produced: state.config.privacy.store_full_game_state,
                configurable: true,
                detail: "Controlled by privacy.store_full_game_state.".to_string(),
            },
            PrivacyStorageStatus {
                category: "Typed participant messages".to_string(),
                persisted_when_produced: state.config.privacy.store_typed_messages,
                configurable: true,
                detail: "Controlled by privacy.store_typed_messages.".to_string(),
            },
            PrivacyStorageStatus {
                category: "Final voice transcripts".to_string(),
                persisted_when_produced: state.config.privacy.store_final_transcripts,
                configurable: true,
                detail: format!(
                    "Controlled by privacy.store_final_transcripts; transcription is configured {}.",
                    enabled_label(state.config.transcription.enabled)
                ),
            },
            PrivacyStorageStatus {
                category: "Voice diagnostics".to_string(),
                persisted_when_produced: state.config.privacy.store_voice_diagnostics,
                configurable: true,
                detail: format!(
                    "Controlled by privacy.store_voice_diagnostics; stored metadata is restricted to scalar transport metrics. Voice is configured {}.",
                    enabled_label(state.config.voice.enabled)
                ),
            },
        ],
        raw_audio_stored_by_parlando: false,
        external_services,
        exports: PrivacyExportStatus {
            full_internal_export: true,
            research_export: true,
            corpus_export: true,
            formats: vec!["JSON".to_string(), "YAML".to_string(), "CSV".to_string()],
            detail: "Research is the dashboard default. Research and corpus exports retain readable participant and dialogue identifiers across repeated exports of one experiment. Human participant identifiers are random and are not reused across experiments; agent identifiers expose agent type and version. Corpus candidates require content review before publication; full is restricted to internal administration.".to_string(),
        },
        participant_deletion: PrivacyFeatureStatus {
            available: true,
            detail: "Human participant cards provide a counted preview and irreversible manual deletion of that experiment's recruitment mapping, participant identifier, consent evidence, and authored communication; no automatic retention deletion is performed.".to_string(),
        },
        consent_evidence: PrivacyFeatureStatus {
            available: !state.config.direct.consents.is_empty(),
            detail: format!(
                "Consent is configured through {} consent item(s). Accepted declarations record a server-computed hash of the complete presentation. A participant-information reference is {}.",
                state.config.direct.consents.len(),
                enabled_label(
                    nonempty_string(&state.config.direct.participant_information_version).is_some()
                        && nonempty_string(&state.config.direct.participant_information_url).is_some()
                )
            ),
        },
    }
}

/// Returns a stable human-readable label for an effective boolean configuration value.
fn enabled_label(enabled: bool) -> &'static str {
    if enabled {
        "enabled"
    } else {
        "disabled"
    }
}

/// Renders the privacy status as a self-contained administrator page without client-side scripts.
fn render_privacy_status_html(status: &PrivacyStatus) -> String {
    let git_sha = status.software.git_sha.as_deref().unwrap_or("not embedded");
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
                "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                escape_html_text(&item.category),
                yes_no(item.persisted_when_produced),
                yes_no(item.configurable),
                escape_html_text(&item.detail)
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
    .warning {{ background: #fff8e8; border: 1px solid #f2c36b; border-radius: 8px; color: #694500; margin-bottom: 18px; padding: 12px 14px; }}
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
    <p class="intro">Installation-wide facts derived from the running Parlando version and effective configuration. This page does not infer controller identity, legal basis, self-hosting, retention decisions, or provider contracts.</p>
    <div class="warning"><strong>Review state:</strong> {review_state} Privacy Contract: <strong>{contract_version}</strong>.</div>
    <section class="panel">
      <h2>Software</h2>
      <div class="facts"><div class="fact"><span>Server</span><strong>{server_name} {server_version}</strong></div><div class="fact"><span>Generated</span><strong>{generated_at}</strong></div><div class="fact"><span>Git revision</span><strong>{git_sha}</strong></div><div class="fact"><span>Working tree at build</span><strong>{git_dirty}</strong></div></div>
    </section>
    <section class="panel">
      <h2>Storage behavior</h2>
      <table><thead><tr><th>Category</th><th>Persisted when produced</th><th>Configurable</th><th>Detail</th></tr></thead><tbody>{storage}</tbody></table>
      <p class="state">Raw microphone audio stored by Parlando: {raw_audio}</p>
    </section>
    <section class="panel">
      <h2>External services</h2>
      <table><thead><tr><th>Service</th><th>Enabled</th><th>Data sent</th></tr></thead><tbody>{services}</tbody></table>
    </section>
    <div class="grid">
      <section class="panel"><h2>Exports</h2><p>Full internal export: <strong>{full_export}</strong><br>Research export: <strong>{research_export}</strong><br>Corpus export: <strong>{corpus_export}</strong></p><p class="empty">{export_detail}</p></section>
      <section class="panel"><h2>Participant administration</h2><p>Manual deletion available: <strong>{deletion}</strong></p><p class="empty">{deletion_detail}</p><p>Versioned information evidence available: <strong>{consent}</strong></p><p class="empty">{consent_detail}</p></section>
    </div>
  </main>
</body>
</html>"##,
        review_state = escape_html_text(&status.review_state),
        contract_version = escape_html_text(&status.privacy_contract_version),
        server_name = escape_html_text(&status.software.server_name),
        server_version = escape_html_text(&status.software.server_version),
        generated_at = escape_html_text(&status.generated_at),
        git_sha = escape_html_text(git_sha),
        git_dirty = escape_html_text(&status.software.git_dirty),
        storage = storage,
        raw_audio = yes_no(status.raw_audio_stored_by_parlando),
        services = services,
        full_export = yes_no(status.exports.full_internal_export),
        research_export = yes_no(status.exports.research_export),
        corpus_export = yes_no(status.exports.corpus_export),
        export_detail = escape_html_text(&status.exports.detail),
        deletion = yes_no(status.participant_deletion.available),
        deletion_detail = escape_html_text(&status.participant_deletion.detail),
        consent = yes_no(status.consent_evidence.available),
        consent_detail = escape_html_text(&status.consent_evidence.detail),
    )
}

/// Renders the privacy status as a portable Markdown record for DPO review material.
fn render_privacy_status_markdown(status: &PrivacyStatus) -> String {
    let mut output = format!(
        "# Parlando privacy status\n\nGenerated: {}  \nPrivacy Contract: {}  \nReview state: {}\n\n## Software\n\n- Server: {} {}\n- Git revision: {}\n- Working tree at build: {}\n\n## Storage behavior\n\n| Category | Persisted when produced | Configurable | Detail |\n| --- | --- | --- | --- |\n",
        markdown_cell(&status.generated_at),
        markdown_cell(&status.privacy_contract_version),
        markdown_cell(&status.review_state),
        markdown_cell(&status.software.server_name),
        markdown_cell(&status.software.server_version),
        markdown_cell(status.software.git_sha.as_deref().unwrap_or("not embedded")),
        markdown_cell(&status.software.git_dirty),
    );
    for item in &status.storage {
        output.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            markdown_cell(&item.category),
            yes_no(item.persisted_when_produced),
            yes_no(item.configurable),
            markdown_cell(&item.detail)
        ));
    }
    output.push_str(&format!(
        "\nRaw microphone audio stored by Parlando: **{}**\n\n## External services\n\n",
        yes_no(status.raw_audio_stored_by_parlando)
    ));
    if status.external_services.is_empty() {
        output.push_str("No external speech services are enabled.\n");
    } else {
        output.push_str("| Service | Enabled | Data sent |\n| --- | --- | --- |\n");
        for service in &status.external_services {
            output.push_str(&format!(
                "| {} | {} | {} |\n",
                markdown_cell(&service.service),
                yes_no(service.enabled),
                markdown_cell(&service.data_sent)
            ));
        }
    }
    output.push_str(&format!(
        "\n## Exports and participant administration\n\n- Full internal export: **{}**\n- Research export: **{}**\n- Corpus export: **{}**\n- Export formats: {}\n- Export detail: {}\n- Manual participant deletion available: **{}**\n- Deletion detail: {}\n- Versioned participant-information evidence available: **{}**\n- Evidence detail: {}\n\nThis report describes technical facts only. It does not infer controller identity, legal basis, self-hosting, retention decisions, or provider contracts.\n",
        yes_no(status.exports.full_internal_export),
        yes_no(status.exports.research_export),
        yes_no(status.exports.corpus_export),
        status.exports.formats.iter().map(|value| markdown_cell(value)).collect::<Vec<_>>().join(", "),
        markdown_cell(&status.exports.detail),
        yes_no(status.participant_deletion.available),
        markdown_cell(&status.participant_deletion.detail),
        yes_no(status.consent_evidence.available),
        markdown_cell(&status.consent_evidence.detail),
    ));
    output
}

/// Creates a download response with a fixed safe filename and explicit media type.
fn privacy_download_response(
    body: String,
    content_type: &'static str,
    filename: &'static str,
) -> Response {
    let mut response = body.into_response();
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
    response.headers_mut().insert(
        CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("attachment; filename=\"{filename}\""))
            .expect("fixed privacy-status filename is a valid header value"),
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
async fn admin_experiments<A: GameAdapter>(
    State(state): State<Arc<AppState<A>>>,
    Extension(admin_session): Extension<AdminSession>,
) -> Result<Json<Value>, AppError> {
    let experiments = state.store.list_experiments(1_000).await?;
    let game_settings = state.game_settings.read().await.clone();
    Ok(Json(json!({
        "game": state.game_descriptor,
        "game_settings": game_settings,
        "experiments": experiments,
        "running_experiment_id": state.experiment_id,
        "csrf_token": admin_session.csrf_token,
    })))
}

/// Creates one inactive experiment for the exact game version compiled into this process.
async fn admin_create_experiment<A: GameAdapter>(
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
    if let Some(study_name) = request.study_name {
        config.study.name = study_name;
    }
    config
        .validate()
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
async fn admin_clone_experiment<A: GameAdapter>(
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
            notes: request.notes,
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
async fn admin_experiment_config<A: GameAdapter>(
    State(state): State<Arc<AppState<A>>>,
    Path(experiment_id): Path<String>,
) -> Result<Json<Value>, AppError> {
    let experiment = state
        .store
        .experiment_definition(&experiment_id)
        .await?
        .ok_or_else(|| AppError::not_found("Experiment not found."))?;
    Ok(Json(json!({ "experiment": experiment })))
}

/// Validates and saves a new immutable configuration revision for an inactive experiment.
async fn admin_save_experiment_config<A: GameAdapter>(
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
    let mut config = experiment_config_from_json(request.config, &state.config, &experiment_id)
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    config.experiment.id = Some(experiment_id.clone());
    config
        .validate()
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    let revision = state
        .store
        .save_experiment_revision(
            &experiment_id,
            request.expected_revision,
            persistable_config_json(&config)?,
            request.change_summary,
        )
        .await
        .map_err(|error| AppError::new(StatusCode::CONFLICT, error.to_string()))?;
    Ok(Json(
        json!({ "experiment_id": experiment_id, "revision": revision }),
    ))
}

/// Lists immutable configuration revisions for one experiment.
async fn admin_experiment_revisions<A: GameAdapter>(
    State(state): State<Arc<AppState<A>>>,
    Path(experiment_id): Path<String>,
) -> Result<Json<Value>, AppError> {
    Ok(Json(json!({
        "experiment_id": experiment_id.clone(),
        "revisions": state.store.experiment_revisions(&experiment_id).await?,
    })))
}

/// Updates pinning, obsolescence, and notes without changing runtime configuration.
async fn admin_update_experiment_catalogue<A: GameAdapter>(
    State(state): State<Arc<AppState<A>>>,
    Path(experiment_id): Path<String>,
    Json(request): Json<AdminCatalogueRequest>,
) -> Result<Json<Value>, AppError> {
    state
        .store
        .update_experiment_catalogue(
            &experiment_id,
            request.pinned,
            request.obsolete,
            request.notes,
        )
        .await
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    Ok(Json(json!({ "experiment_id": experiment_id, "ok": true })))
}

/// Returns settings shared by every experiment of the compiled game.
async fn admin_game_settings<A: GameAdapter>(
    State(state): State<Arc<AppState<A>>>,
) -> Json<StoredGameSettings> {
    Json(state.game_settings.read().await.clone())
}

/// Updates the shared institution with optimistic concurrency.
async fn admin_update_game_settings<A: GameAdapter>(
    State(state): State<Arc<AppState<A>>>,
    Json(request): Json<AdminGameSettingsRequest>,
) -> Result<Json<Value>, AppError> {
    let institution = request.institution.trim().to_string();
    let revision = state
        .store
        .update_game_settings(request.expected_revision, institution.clone())
        .await
        .map_err(|error| AppError::new(StatusCode::CONFLICT, error.to_string()))?;
    *state.game_settings.write().await = StoredGameSettings {
        institution,
        revision,
    };
    Ok(Json(json!({ "revision": revision })))
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
async fn admin_experiment<A: GameAdapter>(
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
async fn admin_update_experiment_status<A: GameAdapter>(
    State(state): State<Arc<AppState<A>>>,
    Json(request): Json<AdminUpdateExperimentStatusRequest>,
) -> Result<Json<Value>, AppError> {
    let lifecycle = ExperimentLifecycle::parse(&request.status)?;
    let stored = state
        .store
        .experiment_definition(&state.experiment_id)
        .await?
        .ok_or_else(|| AppError::not_found("Configured experiment not found."))?;
    if lifecycle == ExperimentLifecycle::Active
        && stored.game_version != state.game_descriptor.version.to_string()
    {
        return Err(AppError::new(
            StatusCode::CONFLICT,
            "This experiment belongs to another game version; clone it before activation",
        ));
    }
    let mut current_lifecycle = state.experiment_lifecycle.write().await;
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
async fn admin_sessions<A: GameAdapter>(
    State(state): State<Arc<AppState<A>>>,
    Query(query): Query<AdminSessionsQuery>,
) -> Result<Json<Value>, AppError> {
    let experiment_id = state.experiment_id.clone();
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
async fn admin_session_detail<A: GameAdapter>(
    State(state): State<Arc<AppState<A>>>,
    Path(session_id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let experiment_id = state.experiment_id.clone();
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
async fn admin_session_events<A: GameAdapter>(
    State(state): State<Arc<AppState<A>>>,
    Path(session_id): Path<i64>,
    Query(query): Query<AdminEventsQuery>,
) -> Result<Json<Value>, AppError> {
    let after = query.after.unwrap_or(0);
    let experiment_id = state.experiment_id.clone();
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
    let mut value = filtered_export(&state, &query).await?;
    value = match query.variant.as_deref().unwrap_or("research") {
        "full" => value,
        "research" => research_export(value, &state.config.privacy.contract_version),
        "corpus" => corpus_export(research_export(
            value,
            &state.config.privacy.contract_version,
        )),
        other => {
            return Err(AppError::bad_request(format!(
                "Unsupported export variant {other:?}."
            )))
        }
    };
    tracing::info!(experiment_id = %state.experiment_id, session_id = ?query.session_id, "administrator requested export");
    redact_secret_fields(&mut value);
    if value
        .get("session_events")
        .and_then(Value::as_array)
        .is_some_and(|events| events.len() > 100_000)
    {
        return Err(AppError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "Export exceeds the 100,000 event limit; select one session",
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
            "Export exceeds the 32 MiB response limit; narrow the scope",
        ));
    }
    Ok(([("content-type", content_type)], body).into_response())
}

/// Resolves an experiment-specific participant identifier without exposing database ids.
async fn participant_id_for_research_id<A: GameAdapter>(
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
async fn admin_participant_deletion_preview<A: GameAdapter>(
    State(state): State<Arc<AppState<A>>>,
    Path(requested): Path<String>,
) -> Result<Json<Value>, AppError> {
    let experiment_id = state.experiment_id.clone();
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
async fn admin_delete_participant_data<A: GameAdapter>(
    State(state): State<Arc<AppState<A>>>,
    Path(requested): Path<String>,
) -> Result<Json<Value>, AppError> {
    let experiment_id = state.experiment_id.clone();
    let Some(participant_id) =
        participant_id_for_research_id(&state, &experiment_id, &requested).await?
    else {
        return Ok(Json(json!({
            "research_id": requested,
            "deleted": true,
            "already_absent": true,
        })));
    };
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

async fn filtered_export<A: GameAdapter>(
    state: &Arc<AppState<A>>,
    query: &AdminExportQuery,
) -> Result<Value, AppError>
where
    A::State: Serialize,
{
    let mut exported = if let Some(session_id) = query.session_id {
        state
            .store
            .export_session(&state.experiment_id, session_id)
            .await?
    } else {
        state.store.export_experiment(&state.experiment_id).await?
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

/// Converts the internal export into a fixed pseudonymous research allowlist.
fn research_export(exported: Value, privacy_contract_version: &str) -> Value {
    let participant_details = exported
        .get("participants")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|row| {
            Some((
                row.get("participant_id")?.as_i64()?,
                (
                    row.get("research_id")?.as_str()?.to_string(),
                    row.get("participant_kind")?.as_str()?.to_string(),
                ),
            ))
        })
        .collect::<HashMap<_, _>>();
    let mut participant_ids = HashMap::<i64, String>::new();
    let mut session_participant_ids = HashMap::<String, String>::new();
    let session_participants = exported
        .get("session_participants")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|row| {
            let participant_id = row.get("participant_id")?.as_i64()?;
            let pseudonym = participant_details.get(&participant_id)?.0.clone();
            participant_ids.insert(participant_id, pseudonym.clone());
            if let Some(session_identity) = row
                .get("participant_session_id")
                .and_then(Value::as_str)
            {
                session_participant_ids.insert(session_identity.to_string(), pseudonym.clone());
            }
            Some(json!({
                "dialogue_id": exported.get("sessions").and_then(Value::as_array).and_then(|sessions| sessions.iter().find(|session| session.get("session_id") == row.get("session_id"))).and_then(|session| session.get("dialogue_id")),
                "participant_id": pseudonym,
                "role": row.get("role"),
                "participant_kind": participant_details.get(&participant_id).map(|details| &details.1),
            }))
        })
        .collect::<Vec<_>>();
    let participants = participant_ids
        .iter()
        .map(|(participant_id, pseudonym)| {
            json!({
                "participant_id": pseudonym,
                "participant_kind": participant_details.get(participant_id).map(|details| &details.1),
            })
        })
        .collect::<Vec<_>>();
    let sessions = exported
        .get("sessions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|row| {
            json!({
                "dialogue_id": row.get("dialogue_id"),
                "config_revision": row.get("config_revision"),
                "game_version": row.get("game_version"),
                "mode": row.get("mode"),
                "status": row.get("status"),
                "created_at": row.get("created_at"),
                "started_at": row.get("started_at"),
                "completed_at": row.get("completed_at"),
                "completion": row.get("completion"),
            })
        })
        .collect::<Vec<_>>();
    let session_events = exported
        .get("session_events")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|row| {
            let dialogue_id = exported
                .get("sessions")
                .and_then(Value::as_array)
                .and_then(|sessions| sessions.iter().find(|session| session.get("session_id") == row.get("session_id")))
                .and_then(|session| session.get("dialogue_id"));
            let mut payload = row.get("payload").cloned().unwrap_or(Value::Null);
            sanitize_research_value(&mut payload, &session_participant_ids);
            let mut game_state = row.get("game_state").cloned().unwrap_or(Value::Null);
            sanitize_research_value(&mut game_state, &session_participant_ids);
            json!({
                "dialogue_id": dialogue_id,
                "event_index": row.get("event_index"),
                "event_type": row.get("event_type"),
                "actor_participant_id": row.get("actor_participant_id").and_then(Value::as_i64).and_then(|id| participant_ids.get(&id)),
                "actor_role": row.get("actor_role"),
                "payload": payload,
                "game_state": game_state,
                "created_at": row.get("created_at"),
            })
        })
        .collect::<Vec<_>>();
    json!({
        "export_variant": "research",
        "generated_at": now_iso(),
        "privacy_contract_version": privacy_contract_version,
        "experiment": {
            "experiment_id": exported.get("experiment").and_then(|value| value.get("experiment_id")),
            "game_version": exported.get("experiment").and_then(|value| value.get("game_version")),
            "config_revision": exported.get("experiment").and_then(|value| value.get("config_revision")),
            "server_version": exported.get("experiment").and_then(|value| value.get("server_version")),
            "version_manifest": exported.get("experiment").and_then(|value| value.get("version_manifest")),
        },
        "participants": participants,
        "sessions": sessions,
        "session_participants": session_participants,
        "session_events": session_events,
    })
}

/// Removes direct-identity fields and replaces runtime handles with participant identifiers.
fn sanitize_research_value(value: &mut Value, replacements: &HashMap<String, String>) {
    match value {
        Value::String(text) => {
            if let Some(replacement) = replacements.get(text) {
                *text = replacement.clone();
            }
        }
        Value::Array(items) => items
            .iter_mut()
            .for_each(|item| sanitize_research_value(item, replacements)),
        Value::Object(fields) => {
            for key in [
                "external_id",
                "selected_audio_input_id",
                "selected_audio_input_label",
            ] {
                fields.remove(key);
            }
            fields
                .values_mut()
                .for_each(|item| sanitize_research_value(item, replacements));
        }
        _ => {}
    }
}

/// Derives a publication-oriented dialogue corpus with consistent readable identifiers.
fn corpus_export(research: Value) -> Value {
    let participant_labels = research
        .get("participants")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|row| {
            let participant_id = row.get("participant_id")?.as_str()?.to_string();
            Some((participant_id.clone(), participant_id))
        })
        .collect::<HashMap<_, _>>();
    let session_labels = research
        .get("sessions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|row| {
            let dialogue_id = row.get("dialogue_id")?.as_str()?.to_string();
            Some((dialogue_id.clone(), dialogue_id))
        })
        .collect::<HashMap<_, _>>();
    let session_start_ms = research
        .get("sessions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|row| {
            Some((
                row.get("dialogue_id")?.as_str()?.to_string(),
                rfc3339_millis(row.get("created_at")?.as_str()?)?,
            ))
        })
        .collect::<HashMap<_, _>>();
    let participants = research
        .get("participants")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|row| {
            let source = row.get("participant_id")?.as_str()?;
            Some(json!({
                "participant_id": participant_labels.get(source),
                "participant_kind": row.get("participant_kind"),
            }))
        })
        .collect::<Vec<_>>();
    let events = research
        .get("session_events")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|row| {
            let dialogue_id = row.get("dialogue_id")?.as_str()?;
            let mut payload = row.get("payload").cloned().unwrap_or(Value::Null);
            sanitize_corpus_value(&mut payload, &participant_labels);
            let mut game_state = row.get("game_state").cloned().unwrap_or(Value::Null);
            sanitize_corpus_value(&mut game_state, &participant_labels);
            let actor = row
                .get("actor_participant_id")
                .and_then(Value::as_str)
                .and_then(|id| participant_labels.get(id));
            let relative_time_ms = corpus_relative_time_ms(row, dialogue_id, &session_start_ms);
            Some(json!({
                "dialogue_id": session_labels.get(dialogue_id),
                "turn": row.get("event_index"),
                "event_type": row.get("event_type"),
                "participant_id": actor,
                "role": row.get("actor_role"),
                "payload": payload,
                "game_state": game_state,
                "time_from_session_start_ms": relative_time_ms,
            }))
        })
        .collect::<Vec<_>>();
    let messages = research
        .get("session_events")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|row| row.get("event_type").and_then(Value::as_str) == Some("conversation_message"))
        .filter_map(|row| {
            let dialogue_id = row.get("dialogue_id")?.as_str()?;
            let payload = row.get("payload")?;
            let actor = row
                .get("actor_participant_id")
                .and_then(Value::as_str)
                .and_then(|id| participant_labels.get(id));
            let relative_time_ms = corpus_relative_time_ms(row, dialogue_id, &session_start_ms);
            Some(json!({
                "dialogue_id": session_labels.get(dialogue_id),
                "turn": row.get("event_index"),
                "participant_id": actor,
                "role": row.get("actor_role"),
                "origin": payload.get("origin"),
                "text": payload.get("text"),
                "time_from_session_start_ms": relative_time_ms,
                "start_time_ms": payload.get("metadata").and_then(|metadata| metadata.get("start_time_ms")),
                "end_time_ms": payload.get("metadata").and_then(|metadata| metadata.get("end_time_ms")),
            }))
        })
        .collect::<Vec<_>>();
    json!({
        "export_variant": "corpus",
        "release_status": "corpus_candidate",
        "content_review_required": true,
        "experiment": research.get("experiment"),
        "sessions": research.get("sessions").and_then(Value::as_array).into_iter().flatten().map(|row| json!({
            "dialogue_id": row.get("dialogue_id"),
            "config_revision": row.get("config_revision"),
            "game_version": row.get("game_version"),
        })).collect::<Vec<_>>(),
        "participants": participants,
        "events": events,
        "messages": messages,
    })
}

/// Computes one event's non-negative time offset from its session start.
fn corpus_relative_time_ms(
    event: &Value,
    dialogue_id: &str,
    session_start_ms: &HashMap<String, i64>,
) -> Option<i64> {
    event
        .get("created_at")
        .and_then(Value::as_str)
        .and_then(rfc3339_millis)
        .zip(session_start_ms.get(dialogue_id).copied())
        .map(|(event, start)| event.saturating_sub(start))
}

/// Removes live-system identifiers while retaining experiment-specific corpus labels.
fn sanitize_corpus_value(value: &mut Value, replacements: &HashMap<String, String>) {
    match value {
        Value::String(text) => {
            if let Some(replacement) = replacements.get(text) {
                *text = replacement.clone();
            }
        }
        Value::Array(items) => items
            .iter_mut()
            .for_each(|item| sanitize_corpus_value(item, replacements)),
        Value::Object(fields) => {
            for key in [
                "id",
                "room_id",
                "source_message_id",
                "participant_session_id",
                "sender_participant_session_id",
            ] {
                fields.remove(key);
            }
            fields
                .values_mut()
                .for_each(|item| sanitize_corpus_value(item, replacements));
        }
        _ => {}
    }
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

async fn game_socket<A: GameAdapter>(
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
    let permit = state
        .game_connection_limit
        .clone()
        .try_acquire_owned()
        .map_err(|_| {
            AppError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "Game connection capacity reached",
            )
        })?;
    Ok(ws
        .max_frame_size(32 * 1024)
        .max_message_size(64 * 1024)
        .on_upgrade(move |socket| async move {
            let _permit = permit;
            websocket_loop(state, socket, room_id, participant_session_id, role).await;
        }))
}

#[derive(Deserialize)]
struct AudioSocketQuery {
    token: String,
}

/// Authenticates and upgrades one participant-owned audio transport.
async fn audio_socket<A: GameAdapter>(
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
    let permit = state
        .audio_connection_limit
        .clone()
        .try_acquire_owned()
        .map_err(|_| {
            AppError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "Audio connection capacity reached",
            )
        })?;
    Ok(ws
        .max_frame_size(8 * 1024)
        .max_message_size(8 * 1024)
        .on_upgrade(move |socket| async move {
            let _permit = permit;
            audio_websocket_loop(state, socket, claims).await;
        }))
}

/// Relays browser PCM and consumes normalized server-side transcription events.
async fn audio_websocket_loop<A: GameAdapter>(
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
    let (connection_generation, mut shutdown) =
        register_connection(&state.audio_connections, connection_key.clone()).await;
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

    let provider_permit = if state.transcription_provider.is_some() {
        state
            .provider_connection_limit
            .clone()
            .try_acquire_owned()
            .ok()
    } else {
        None
    };
    let transcription = if let (Some(provider), Some(_permit)) = (
        state.transcription_provider.clone(),
        provider_permit.as_ref(),
    ) {
        match provider
            .start_session(TranscriptionSessionContext {
                room_id: room_id.clone(),
                participant_session_id: participant_session_id.clone(),
                role: role.clone(),
                language: state.config.transcription.language.clone(),
                model: state.config.transcription.model.clone(),
            })
            .await
        {
            Ok(session) => Some(session),
            Err(error) => {
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
                    TranscriptionEvent::Partial(_) => {}
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

    let mut rate_window_started = Instant::now();
    let mut messages_in_window = 0_u32;
    loop {
        let next_message = tokio::select! {
            _ = shutdown.recv() => break,
            _ = tokio::time::sleep(Duration::from_secs(30)) => break,
            message = incoming.next() => message,
        };
        let Some(Ok(message)) = next_message else {
            break;
        };
        if rate_window_started.elapsed() >= Duration::from_secs(10) {
            rate_window_started = Instant::now();
            messages_in_window = 0;
        }
        messages_in_window += 1;
        if messages_in_window > 750 {
            break;
        }
        match message {
            Message::Binary(bytes) => match AudioFrame::decode(&bytes) {
                Ok(frame) => {
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
                    if let Some(input) = &transcription_input {
                        if input.try_send(TranscriptionInput::Audio(frame)).is_err() {
                            state.audio_rooms.send_control(&room_id, &role, json!({"type":"transcriptionStatus","ready":false,"message":"ASR is falling behind"}).to_string()).await;
                        }
                    }
                }
                Err(error) => {
                    tracing::warn!(%error, %room_id, "rejected invalid browser audio frame")
                }
            },
            Message::Close(_) => break,
            _ => {}
        }
    }
    if let Some(input) = transcription_input {
        let _ = input.send(TranscriptionInput::Finish).await;
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
        let _ = task.await;
    }
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
    let connection_key = format!("{room_id}:{}", role.as_str());
    let (connection_generation, mut shutdown) =
        register_connection(&state.game_connections, connection_key.clone()).await;
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
    let mut rate_window_started = Instant::now();
    let mut messages_in_window = 0_u32;
    loop {
        let next_message = tokio::select! {
            _ = shutdown.recv() => break,
            _ = tokio::time::sleep(Duration::from_secs(120)) => break,
            message = FuturesStreamExt::next(&mut incoming) => message,
        };
        let Some(Ok(message)) = next_message else {
            break;
        };
        if rate_window_started.elapsed() >= Duration::from_secs(10) {
            rate_window_started = Instant::now();
            messages_in_window = 0;
        }
        messages_in_window += 1;
        if messages_in_window > 120 {
            let _ = bus.send(error_message(&room_id, "Game message rate limit exceeded"));
            break;
        }
        let Message::Text(text) = message else {
            continue;
        };
        if text.len() > 64 * 1024 {
            let _ = bus.send(error_message(&room_id, "Client message is too large"));
            break;
        }
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
    let still_owner = unregister_connection(
        &state.game_connections,
        &connection_key,
        &connection_generation,
    )
    .await;
    if !still_owner {
        return;
    }
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
                tracing::error!(%error, room_id, "could not durably record readiness");
                let _ = bus.send(error_message(room_id, "Could not durably record readiness"));
                return;
            }
            if let Some(message) = presence_message(&state, room_id).await {
                let _ = bus.send(message);
            }
            let _ = bus.send(voice_message(&state, room_id).await);
        }
        "consentUpdated" => {
            if let Some(consent_request) = message.consent {
                let _ = consent(
                    State(state.clone()),
                    Some(Extension(ParticipantPrincipal {
                        participant_session_id: participant_session_id.to_string(),
                        generation: 1,
                    })),
                    Json(consent_request),
                )
                .await;
            }
        }
        "sendChatMessage" => {
            let text = message.text.unwrap_or_default();
            if text.chars().count() > 4_000 {
                let _ = bus.send(error_message(room_id, "Chat message is too long"));
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
                let _ = bus.send(error_message(room_id, &error.message));
            }
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
            if let Err(error) = persist_session_event_required(
                &state,
                room_id,
                Some(participant_session_id),
                "game_action_submitted",
                json!({"action": raw_action.clone()}),
                None,
            )
            .await
            {
                tracing::error!(%error, room_id, "could not durably record submitted action");
                let _ = bus.send(error_message(room_id, "Could not durably record action"));
                return;
            }
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
    let transition_lock = room_transition_lock(&state, room_id).await;
    let _transition_guard = transition_lock.lock().await;
    let (before, after, events, completed, summary) = {
        let memory = state.memory.read().await;
        let room = memory
            .rooms
            .get(room_id)
            .ok_or_else(|| anyhow!("Room not found."))?;
        if room.status == "completed" {
            return Err(anyhow!(
                "Room is completed and no longer accepts game messages."
            ));
        }
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
        let completed = state.adapter.is_complete(&after);
        let summary = completed.then(|| state.adapter.completion_summary(&after));
        (before, after, events, completed, summary)
    };
    let after_json = protocol_json(&after)?;
    let stored_game_state = state
        .config
        .privacy
        .store_full_game_state
        .then(|| after_json.clone());
    let summary = summary.map(|summary| protocol_json(&summary)).transpose()?;
    let mut durable_events = vec![
        session_event_record(
            &state,
            room_id,
            Some(participant_session_id),
            "game_action_accepted",
            json!({
                "action": protocol_json(&action)?,
                "before": protocol_json(&before)?,
                "events": protocol_json(&events)?,
            }),
            stored_game_state.clone(),
        )
        .await?,
        session_event_record(
            &state,
            room_id,
            Some(participant_session_id),
            "state_changed",
            json!({"events": protocol_json(&events)?}),
            stored_game_state.clone(),
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
                summary.clone().unwrap_or(Value::Null),
                stored_game_state,
            )
            .await?,
        );
    }
    if let Err(error) = state
        .store
        .commit_session_transition(
            durable_events,
            completed.then(|| summary.clone().unwrap_or(Value::Null)),
        )
        .await
    {
        tracing::error!(%error, room_id, "could not commit game transition");
        return Err(anyhow!("Action could not be committed."));
    }
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
    notify_agents_of_action(&state, room_id, player, action.clone()).await;
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

/// Converts a persisted conversation origin into the agent utterance kind.
fn utterance_kind_from_origin(origin: &str) -> AgentUtteranceKind {
    match origin {
        "voice_transcript" => AgentUtteranceKind::Spoken,
        "agent" => AgentUtteranceKind::Agent,
        _ => AgentUtteranceKind::Typed,
    }
}

/// Returns the currently available actions for an agent role in a room.
async fn agent_available_actions<A: GameAdapter>(
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
async fn notify_agents_of_action<A: GameAdapter>(
    state: &Arc<AppState<A>>,
    room_id: &str,
    actor: PlayerRole,
    action: A::Action,
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
                            .observe_state(&room.state, participant.role.player_role()),
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

/// Sends a conversation-message observation to every started agent in the room.
async fn notify_agents_of_message<A: GameAdapter>(
    state: &Arc<AppState<A>>,
    room_id: &str,
    speaker: PlayerRole,
    kind: AgentUtteranceKind,
    text: String,
) {
    let keys = {
        let memory = state.memory.read().await;
        let Some(room) = memory.rooms.get(room_id) else {
            return;
        };
        room.participants
            .values()
            .filter(|participant| participant.source == "agent")
            .map(|participant| agent_key(room_id, &participant.participant_session_id))
            .collect::<Vec<_>>()
    };
    let inboxes = state.agent_inboxes.read().await;
    for key in keys {
        if let Some(sender) = inboxes.get(&key) {
            let _ = sender
                .send(AgentObservation::Message {
                    speaker,
                    kind: kind.clone(),
                    text: text.clone(),
                })
                .await;
        }
    }
}

/// Returns an error if an agent response contains no visible effect.
fn validate_agent_response<Action>(response: &AgentResponse<Action>) -> Result<()> {
    if response.is_empty() {
        anyhow::bail!("agent returned an empty response");
    }
    Ok(())
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
async fn handle_agent_response<A: GameAdapter>(
    state: Arc<AppState<A>>,
    room_id: &str,
    participant_session_id: &str,
    role: Seat,
    response: AgentResponse<A::Action>,
) -> Result<(bool, Option<Value>)>
where
    A::State: Serialize,
{
    validate_agent_response(&response)?;
    let AgentResponse { message, action } = response;
    let mut outcome = (false, None);
    if let Some(action) = action {
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
        broadcast_player_views(state.clone(), room_id).await;
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
async fn request_agent_decision<A: GameAdapter>(
    state: Arc<AppState<A>>,
    agent: &mut Box<dyn GameAgent<A> + Send>,
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
        agent.maybe_act(available_actions),
    )
    .await;
    let Some(response) = flatten_agent_timeout(result, "agent maybe_act timeout")? else {
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

// Converts one agent message to speech immediately after the agent emits it.
async fn speak_agent_message<A: GameAdapter>(
    state: &Arc<AppState<A>>,
    room_id: &str,
    message: &ConversationMessageResponse,
) {
    let Some(provider) = state.tts_provider.clone() else {
        return;
    };
    let Ok(_provider_permit) = state.provider_connection_limit.try_acquire() else {
        persist_tts_diagnostic(
            state,
            room_id,
            "tts_capacity_rejected",
            json!({"message_id": message.id}),
        )
        .await;
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
    {
        let mut started = state.started_agents.write().await;
        let maximum = state
            .config
            .agents
            .human_vs_agent
            .as_ref()
            .map(|config| config.max_concurrent_games)
            .unwrap_or(20);
        if started.len() >= maximum || !started.insert(key.clone()) {
            return;
        }
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
                .map(|room| state.adapter.observe_state(&room.state, role.player_role()))
        };
        if let Some(initial_observation) = initial_observation {
            let observed = tokio::time::timeout(
                Duration::from_secs_f64(timeout),
                agent.observe_state(initial_observation),
            )
            .await;
            if let Err(error) = flatten_agent_timeout(observed, "agent observe_state timeout") {
                last_error = Some(error.to_string());
                invalid_actions += 1;
            }
        }
        'agent_loop: loop {
            let limit = state
                .config
                .agents
                .human_vs_agent
                .as_ref()
                .map(|c| c.invalid_action_limit)
                .unwrap_or(3);
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
                Ok(Some((completed, summary))) => {
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
                Ok(None) => {}
                Err(error) => {
                    last_error = Some(error.to_string());
                    invalid_actions += 1;
                    if invalid_actions < limit {
                        continue;
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

            let Some(observation) = receiver.recv().await else {
                break;
            };
            let observed = match observation {
                AgentObservation::Action {
                    actor,
                    action,
                    resulting_observation,
                } => {
                    tokio::time::timeout(
                        Duration::from_secs_f64(timeout),
                        agent.observe_action(actor, action, resulting_observation),
                    )
                    .await
                }
                AgentObservation::Message {
                    speaker,
                    kind,
                    text,
                } => {
                    tokio::time::timeout(
                        Duration::from_secs_f64(timeout),
                        agent.observe_message(speaker, kind, text),
                    )
                    .await
                }
            };
            if let Err(error) = flatten_agent_timeout(observed, "agent observe timeout") {
                last_error = Some(error.to_string());
                invalid_actions += 1;
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
            let completed = {
                let memory = state.memory.read().await;
                memory
                    .rooms
                    .get(&room_id)
                    .is_none_or(|room| state.adapter.is_complete(&room.state))
            };
            if completed {
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

async fn require_consent<A: GameAdapter>(
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
fn participant_ready_for_game(config: &ExperimentConfig, participant: &RoomParticipant) -> bool {
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
        .filter(|participant| participant_ready_for_game(config, participant))
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
    if changed && state.config.privacy.store_voice_diagnostics {
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
        presence: Some(room_presence(room)),
        ..ServerMessage::new("presenceChanged")
    })
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
mod tests;
