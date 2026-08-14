use std::{
    collections::{HashMap, HashSet},
    future::Future,
    net::SocketAddr,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{anyhow, Result};
use axum::{
    extract::{
        ws::{Message, WebSocket},
        Extension, Path, Query, Request, State, WebSocketUpgrade,
    },
    http::{
        header::{AUTHORIZATION, CONTENT_DISPOSITION, CONTENT_TYPE, COOKIE, ORIGIN, SET_COOKIE},
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
    config::{AgentOptionConfig, AgentsMode, ExperimentConfig},
    game::{GameAdapter, PlayerRole, Seat},
    identity::{new_id, room_code},
    protocol::*,
    storage::{
        experiment_store_from_url, generated_experiment_id, now_iso, ConsentDeclarationRecord,
        ExperimentRecord, GameRoom, MemoryState, ParticipantRecord, RoomParticipant,
        SessionEventRecord, SessionParticipantRecord, SessionRecord, SharedExperimentStore,
        TranscriptSegment,
    },
    transcription::{
        FinalTranscriptUtterance, SpeechmaticsTranscriptionProvider, TranscriptionEvent,
        TranscriptionInput, TranscriptionProvider, TranscriptionSessionContext,
    },
    tts::{ElevenLabsStreamingTtsProvider, StreamingTtsProvider},
};

#[cfg(test)]
use crate::auth::AdminRole;

/// Optional runtime components supplied by the game-specific binary.
#[derive(Clone)]
pub struct ServeOptions<A: GameAdapter> {
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
    /// Agent factory selectors the game binary knows how to instantiate.
    pub admin_agent_options: Vec<AgentOptionConfig>,
}

impl<A: GameAdapter> Default for ServeOptions<A> {
    /// Creates serve options with no agent factory configured.
    fn default() -> Self {
        Self {
            agent_factory: None,
            tts_provider: None,
            audio_publisher: None,
            transcription_provider: None,
            game_version_manifest: None,
            admin_agent_options: vec![],
        }
    }
}

/// Produces the only configuration representation allowed to enter durable storage.
fn persistable_config_json(config: &ExperimentConfig) -> Result<Value> {
    let mut value = serde_json::to_value(config)?;
    redact_secret_fields(&mut value);
    Ok(value)
}

/// Recursively removes credential-shaped fields as a defense-in-depth serialization boundary.
fn redact_secret_fields(value: &mut Value) {
    match value {
        Value::Object(object) => {
            for (key, child) in object.iter_mut() {
                let normalized = key.to_ascii_lowercase();
                if matches!(
                    normalized.as_str(),
                    "api_key" | "apikey" | "password" | "password_hash" | "token" | "secret"
                ) {
                    *child = Value::Null;
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
    let public_origin = canonical_origin(&config.server.public_base_url)
        .map_err(|error| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    let origin = headers.get(ORIGIN).and_then(|value| value.to_str().ok());
    if origin.is_none()
        && (public_origin.starts_with("http://localhost")
            || public_origin.starts_with("http://127.0.0.1")
            || public_origin.starts_with("http://[::1]"))
    {
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
        if public_origin.starts_with("http://localhost")
            || public_origin.starts_with("http://127.0.0.1")
            || public_origin.starts_with("http://[::1]")
        {
            return Ok(());
        }
        return Err(AppError::forbidden(
            "Administrator Origin header is required",
        ));
    };
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
fn spawn_security_cleanup<A: GameAdapter>(state: Arc<AppState<A>>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        loop {
            interval.tick().await;
            state.participant_auth.cleanup().await;
            state.upgrade_tickets.cleanup().await;
            state.admin_auth.cleanup().await;
            cleanup_transient_rooms(&state).await;
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
async fn enforce_creation_rate<A: GameAdapter>(state: &Arc<AppState<A>>) -> Result<(), AppError> {
    let now = chrono::Utc::now().timestamp();
    let mut attempts = state.participant_creation_window.write().await;
    attempts.retain(|timestamp| *timestamp > now - 60);
    if attempts.len() >= 300 {
        return Err(AppError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "Participant creation rate limit exceeded",
        ));
    }
    attempts.push(now);
    Ok(())
}

/// Authenticates participant API requests and attaches the resolved principal.
async fn require_participant_auth<A: GameAdapter>(
    State(state): State<Arc<AppState<A>>>,
    mut request: Request,
    next: Next,
) -> Result<Response, AppError> {
    #[cfg(test)]
    if request.headers().get(AUTHORIZATION).is_none() {
        return Ok(next.run(request).await);
    }
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
    #[cfg(test)]
    if cookie_value(request.headers(), "parlando_admin").is_none() {
        request.extensions_mut().insert(AdminSession {
            role: AdminRole::Administrator,
            csrf_token: "unit-test".to_string(),
            created_at: 0,
            last_seen_at: 0,
        });
        return Ok(next.run(request).await);
    }
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

/// Ensures an authenticated principal matches any transitional request-body identifier.
fn authenticated_participant_id(
    principal: Option<Extension<ParticipantPrincipal>>,
    claimed_id: &str,
) -> Result<String, AppError> {
    if let Some(Extension(principal)) = principal {
        if principal.participant_session_id != claimed_id {
            return Err(AppError::forbidden(
                "Participant identifier does not match bearer credential",
            ));
        }
        return Ok(principal.participant_session_id);
    }
    #[cfg(test)]
    return Ok(claimed_id.to_string());
    #[cfg(not(test))]
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
  <script nonce="{nonce}">const form=document.getElementById('admin-form');const error=document.getElementById('error');const button=form.querySelector('button');const showError=message=>{{error.textContent=message;error.classList.add('visible');}};form.addEventListener('submit',async e=>{{e.preventDefault();error.classList.remove('visible');const f=new FormData(form);const password=f.get('password');const confirmation=f.get('password_confirmation');if(confirmation!==null&&password!==confirmation){{showError('Passwords do not match.');return;}}button.disabled=true;button.textContent='{pending_label}';try{{const r=await fetch('{endpoint}',{{method:'POST',headers:{{'content-type':'application/json'}},body:JSON.stringify({{username:f.get('username'),password}})}});if(r.ok){{location.href='/admin/games';return;}}showError(await r.text()||'Request failed.');}}catch(_){{showError('Could not reach the server. Please try again.');}}finally{{button.disabled=false;button.textContent='{button}';}}}});</script>
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
    admin_auth: AdminAuthenticator,
    participant_creation_window: RwLock<Vec<i64>>,
    game_connection_limit: Arc<Semaphore>,
    audio_connection_limit: Arc<Semaphore>,
    provider_connection_limit: Arc<Semaphore>,
    room_transition_locks: RwLock<HashMap<String, Arc<Mutex<()>>>>,
    game_connections: RwLock<HashMap<String, ConnectionControl>>,
    audio_connections: RwLock<HashMap<String, ConnectionControl>>,
    pub version_manifest: Value,
    pub admin_agent_options: Vec<AgentOptionConfig>,
}

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

/// Builds and runs a Parlando HTTP/WebSocket server on the provided socket address.
pub async fn serve<A: GameAdapter>(
    adapter: A,
    config: ExperimentConfig,
    bind_addr: SocketAddr,
    options: ServeOptions<A>,
) -> Result<()> {
    if !bind_addr.ip().is_loopback() && !config.server.public_base_url.starts_with("https://") {
        return Err(anyhow!("public binding requires an https public_base_url"));
    }
    if !bind_addr.ip().is_loopback()
        && config
            .server
            .allowed_origins
            .iter()
            .any(|origin| !origin.starts_with("https://"))
    {
        return Err(anyhow!("public binding allows only https browser origins"));
    }
    let router = build_router(adapter, config, options).await?;
    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    axum::serve(listener, router).await?;
    Ok(())
}

/// Builds an Axum router for tests or for embedding in a custom server runner.
pub async fn build_router<A: GameAdapter>(
    adapter: A,
    mut config: ExperimentConfig,
    options: ServeOptions<A>,
) -> Result<Router>
where
    A::State: Serialize,
{
    let speechmatics_api_key = std::mem::take(&mut config.speechmatics.api_key);
    let tts_api_key = std::mem::take(&mut config.tts.api_key);
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
            config: persistable_config_json(&config)?,
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
        let mut provider_config = config.tts.clone();
        provider_config.api_key = tts_api_key;
        Some(
            Arc::new(ElevenLabsStreamingTtsProvider::new(provider_config)?)
                as Arc<dyn StreamingTtsProvider>,
        )
    } else {
        None
    };
    let audio_rooms = Arc::new(AudioRoomRegistry::default());
    let transcription_provider = if options.transcription_provider.is_some() {
        options.transcription_provider
    } else if config.transcription.enabled && config.transcription.provider == "speechmatics" {
        let mut provider_config = config.speechmatics.clone();
        provider_config.api_key = speechmatics_api_key;
        Some(
            Arc::new(SpeechmaticsTranscriptionProvider::new(provider_config)?)
                as Arc<dyn TranscriptionProvider>,
        )
    } else {
        None
    };
    let audio_publisher = if options.audio_publisher.is_some() {
        options.audio_publisher
    } else if config.tts.enabled && config.voice.enabled {
        Some(Arc::new(RoomAgentAudioPublisher::new(
            audio_rooms.clone(),
            config.voice.jitter_buffer_ms,
        )) as Arc<dyn AgentAudioPublisher>)
    } else {
        None
    };
    let cors = configured_cors(&config)?;
    let admin_auth = AdminAuthenticator::load(store.clone()).await?;
    let state = Arc::new(AppState {
        adapter,
        config,
        experiment_id,
        memory: RwLock::new(MemoryState::default()),
        store,
        room_buses: RwLock::new(HashMap::new()),
        agent_factory: options.agent_factory,
        started_agents: RwLock::new(HashSet::new()),
        agent_inboxes: RwLock::new(HashMap::new()),
        tts_provider,
        audio_publisher,
        audio_rooms,
        transcription_provider,
        committed_transcripts: RwLock::new(HashSet::new()),
        participant_auth: ParticipantAuthenticator::default(),
        upgrade_tickets: UpgradeTicketStore::default(),
        admin_auth,
        participant_creation_window: RwLock::new(Vec::new()),
        game_connection_limit: Arc::new(Semaphore::new(1_000)),
        audio_connection_limit: Arc::new(Semaphore::new(200)),
        provider_connection_limit: Arc::new(Semaphore::new(32)),
        room_transition_locks: RwLock::new(HashMap::new()),
        game_connections: RwLock::new(HashMap::new()),
        audio_connections: RwLock::new(HashMap::new()),
        version_manifest,
        admin_agent_options: options.admin_agent_options,
    });

    let public_routes = Router::new()
        .route("/health", get(health))
        .route("/api/config", get(public_config::<A>))
        .route("/api/participants", post(create_participant::<A>))
        .route("/api/direct/start", post(direct_start::<A>))
        .route("/admin", get(admin_entry))
        .route("/admin/", get(admin_entry))
        .route("/admin/login", get(admin_login_page::<A>))
        .route("/api/admin/setup", post(admin_setup::<A>))
        .route("/api/admin/login", post(admin_login::<A>));
    let participant_routes = Router::new()
        .route("/api/consent", post(consent::<A>))
        .route("/api/rooms", post(create_room::<A>))
        .route("/api/rooms/:room_id/join", post(join_room::<A>))
        .route("/api/rooms/:room_id/game-session", post(game_session::<A>))
        .route(
            "/api/rooms/:room_id/audio-session",
            post(audio_session::<A>),
        )
        .route(
            "/api/rooms/:room_id/voice-diagnostics",
            post(add_voice_diagnostic::<A>),
        )
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_participant_auth::<A>,
        ));
    let admin_routes = Router::new()
        .route("/admin/games", get(admin_games_page))
        .route("/admin/experiments", get(admin_games_page))
        .route("/admin/privacy", get(admin_privacy_page::<A>))
        .route("/api/admin/privacy", get(admin_privacy_json::<A>))
        .route(
            "/api/admin/privacy.json",
            get(admin_privacy_json_download::<A>),
        )
        .route(
            "/api/admin/privacy.md",
            get(admin_privacy_markdown_download::<A>),
        )
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
        .route(
            "/api/admin/participants/:research_id/deletion",
            get(admin_participant_deletion_preview::<A>).post(admin_delete_participant_data::<A>),
        )
        .route("/api/admin/logout", post(admin_logout::<A>))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_admin_auth::<A>,
        ));
    let websocket_routes = Router::new()
        .route("/ws/game/:room_id", get(game_socket::<A>))
        .route("/ws/audio/:room_id", get(audio_socket::<A>));
    let api = Router::new()
        .merge(public_routes)
        .merge(participant_routes)
        .merge(admin_routes)
        .merge(websocket_routes)
        .layer(RequestBodyLimitLayer::new(64 * 1024))
        .layer(ConcurrencyLimitLayer::new(2_048))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(30),
        ))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            security_headers::<A>,
        ))
        .layer(cors)
        .with_state(state.clone());

    spawn_security_cleanup(state);

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
        institution: nonempty_string(&config.study.institution),
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
                body_html: item.body_html.clone(),
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
            "store_audio": config.transcription.store_audio,
        }),
        tts: json!({
            "enabled": config.tts.enabled,
            "voice_name": if config.tts.voice_name.is_empty() { None } else { Some(config.tts.voice_name.clone()) },
            "worker_autostart": config.tts.worker_autostart,
        }),
        conversation: json!({
            "enabled": config.conversation.enabled,
            "max_history_messages": config.conversation.max_history_messages,
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

async fn direct_start<A: GameAdapter>(
    State(state): State<Arc<AppState<A>>>,
    Json(_request): Json<DirectStartRequest>,
) -> Result<Json<DirectStartResponse>, AppError> {
    let request = ParticipantCreateRequest {
        source: "direct".to_string(),
        study_id: None,
        external_id: None,
        metadata: Value::Null,
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
    enforce_creation_rate(&state).await?;
    if request.source != "direct" {
        return Err(AppError::forbidden(
            "Public participant creation supports direct participants only",
        ));
    }
    if !state.config.direct.enabled
        || !state
            .config
            .study
            .enabled_sources
            .iter()
            .any(|source| source == "direct")
    {
        return Err(AppError::not_found("Direct mode is disabled."));
    }
    if request.external_id.is_some() || request.study_id.is_some() || !request.metadata.is_null() {
        return Err(AppError::bad_request(
            "External identity, study, and metadata are server-controlled",
        ));
    }
    let participant_id = state
        .store
        .upsert_participant(ParticipantRecord {
            experiment_id: state.experiment_id.clone(),
            participant_kind: "human".to_string(),
            identity_provider: request.source.clone(),
            external_id: request.external_id,
            metadata: request.metadata,
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
    let participant = {
        let mut memory = state.memory.write().await;
        if memory.participants.len() >= 10_000 {
            return Err(AppError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "Participant capacity reached",
            ));
        }
        memory.create_participant(
            participant_id,
            research_id,
            request.source,
            Some(
                request
                    .study_id
                    .unwrap_or_else(|| state.config.study.name.clone()),
            ),
        )
    };
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
    Json(mut request): Json<ConsentRequest>,
) -> Result<Json<Value>, AppError> {
    request.participant_session_id =
        authenticated_participant_id(principal, &request.participant_session_id)?;
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
        .get(&request.participant_session_id)
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
        .get_mut(&request.participant_session_id)
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
        if let Some(room_participant) = room.participants.get_mut(&request.participant_session_id) {
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
    Json(mut request): Json<CreateRoomRequest>,
) -> Result<Json<CreateRoomResponse>, AppError>
where
    A::State: Serialize,
{
    request.participant_session_id =
        authenticated_participant_id(principal, &request.participant_session_id)?;
    require_consent(&state, &request.participant_session_id).await?;
    let requested_mode = request.mode.clone();
    if requested_mode.chars().count() > 64 {
        return Err(AppError::bad_request("Room mode is too long"));
    }
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
                    &request.participant_session_id,
                )?;
                Ok((room_id, role, false))
            } else {
                let (room_id, role) = create_room_locked(
                    &state,
                    &mut memory,
                    request.participant_session_id.clone(),
                    requested_mode.clone(),
                    Seat::A,
                    Some(state.config.study.name.clone()),
                )?;
                let session_id = match state
                    .store
                    .create_session(SessionRecord {
                        experiment_id: state.experiment_id.clone(),
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
                request.participant_session_id.clone(),
                requested_mode.clone(),
                Seat::A,
                Some(state.config.study.name.clone()),
            )?;
            let session_id = match state
                .store
                .create_session(SessionRecord {
                    experiment_id: state.experiment_id.clone(),
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
    if state.config.agents.mode == AgentsMode::HumanVsAgent {
        add_agent_to_room(&state, &room_id).await?;
        maybe_start_room_agents(state.clone(), &room_id).await;
    }
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

async fn join_room<A: GameAdapter>(
    State(state): State<Arc<AppState<A>>>,
    Path(room_id): Path<String>,
    principal: Option<Extension<ParticipantPrincipal>>,
    Json(mut request): Json<JoinRoomRequest>,
) -> Result<Json<JoinRoomResponse>, AppError>
where
    A::State: Serialize,
{
    request.participant_session_id =
        authenticated_participant_id(principal, &request.participant_session_id)?;
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
    Json(mut request): Json<AudioSessionRequest>,
) -> Result<Json<AudioSessionPlanResponse>, AppError> {
    request.participant_session_id =
        authenticated_participant_id(principal.clone(), &request.participant_session_id)?;
    let role = participant_role(&state, &room_id, &request.participant_session_id).await?;
    if !state.config.voice.enabled {
        return Ok(Json(AudioSessionPlanResponse::disabled()));
    }
    let claims = UpgradeTicketClaims {
        room_id: room_id.clone(),
        participant_session_id: request.participant_session_id,
        role: role.as_str().to_string(),
        generation: principal.map_or(1, |Extension(principal)| principal.generation),
        purpose: UpgradePurpose::Audio,
        expires_at: 0,
    };
    let token = state.upgrade_tickets.issue(claims).await;
    let websocket_base = state
        .config
        .server
        .public_base_url
        .trim_end_matches('/')
        .replacen("https://", "wss://", 1)
        .replacen("http://", "ws://", 1);
    Ok(Json(AudioSessionPlanResponse {
        enabled: true,
        websocket_url: Some(format!("{websocket_base}/ws/audio/{room_id}")),
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
    let websocket_base = state
        .config
        .server
        .public_base_url
        .trim_end_matches('/')
        .replacen("https://", "wss://", 1)
        .replacen("http://", "ws://", 1);
    Ok(Json(GameSessionPlanResponse {
        websocket_url: format!("{websocket_base}/ws/game/{room_id}"),
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
    Json(mut diagnostic): Json<VoiceDiagnosticIn>,
) -> Result<Json<Value>, AppError> {
    diagnostic.participant_session_id =
        authenticated_participant_id(principal, &diagnostic.participant_session_id)?;
    participant_role(&state, &room_id, &diagnostic.participant_session_id).await?;
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
        "participant_session_id": diagnostic.participant_session_id,
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
    source_experiment_id: Option<String>,
    experiment_id: Option<String>,
    study_name: Option<String>,
    institution: Option<String>,
    status: Option<String>,
    agents_mode: Option<AgentsMode>,
    agent_factory: Option<String>,
    agent_seed: Option<u64>,
    agent_act_timeout_seconds: Option<f64>,
    agent_invalid_action_limit: Option<usize>,
    agent_config: Option<Value>,
    waiting_room_timeout_seconds: Option<i64>,
    reconnect_grace_seconds: Option<i64>,
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
async fn admin_games_page(Extension(nonce): Extension<CspNonce>) -> Html<String> {
    Html(ADMIN_GAMES_HTML.replacen("<script>", &format!("<script nonce=\"{}\">", nonce.0), 1))
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
                detail: format!(
                    "Controlled by privacy.store_typed_messages; conversation is configured {}.",
                    enabled_label(state.config.conversation.enabled)
                ),
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
    <nav class="header-actions"><a class="button" href="/admin/games">Dashboard</a><a class="button" download href="/api/admin/privacy.md">Markdown</a><a class="button primary" download href="/api/admin/privacy.json">JSON</a></nav>
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
        .replace('\r', " ")
        .replace('\n', " ")
}

/// Formats a boolean fact consistently in HTML and Markdown reports.
fn yes_no(value: bool) -> &'static str {
    if value {
        "Yes"
    } else {
        "No"
    }
}

/// Returns all known experiments with dashboard aggregates.
async fn admin_experiments<A: GameAdapter>(
    State(state): State<Arc<AppState<A>>>,
    Extension(admin_session): Extension<AdminSession>,
    Query(query): Query<AdminExperimentsQuery>,
) -> Result<Json<Value>, AppError> {
    let experiments = state
        .store
        .list_experiments(query.limit.unwrap_or(100))
        .await?;
    let agent_options = if state.config.agents.available.is_empty() {
        state.admin_agent_options.clone()
    } else {
        state.config.agents.available.clone()
    };
    Ok(Json(json!({
        "active_experiment_id": state.experiment_id,
        "version_manifest": state.version_manifest,
        "agent_options": agent_options,
        "default_agents": state.config.agents,
        "csrf_token": admin_session.csrf_token,
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
    let mut config = if let Some(source_experiment_id) = request
        .source_experiment_id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        let exported = state.store.export_experiment(source_experiment_id).await?;
        let source_config = exported
            .get("experiment")
            .and_then(|experiment| experiment.get("config"))
            .cloned()
            .ok_or_else(|| AppError::not_found("Source experiment not found."))?;
        serde_json::from_value::<ExperimentConfig>(source_config)
            .map_err(|error| AppError::bad_request(error.to_string()))?
    } else {
        state.config.clone()
    };
    config.experiment.id = Some(experiment_id.clone());
    if let Some(study_name) = request.study_name.filter(|value| !value.trim().is_empty()) {
        config.study.name = study_name.trim().to_string();
    }
    if let Some(institution) = request.institution {
        config.study.institution = institution.trim().to_string();
    }
    if let Some(timeout) = request.waiting_room_timeout_seconds {
        if timeout <= 0 {
            return Err(AppError::bad_request(
                "waiting_room_timeout_seconds must be positive",
            ));
        }
        config.study.waiting_room_timeout_seconds = timeout;
    }
    if let Some(grace) = request.reconnect_grace_seconds {
        if grace < 0 {
            return Err(AppError::bad_request(
                "reconnect_grace_seconds must be non-negative",
            ));
        }
        config.study.reconnect_grace_seconds = grace;
    }
    let agents_mode = request.agents_mode.unwrap_or(config.agents.mode);
    config.agents.mode = agents_mode.clone();
    if agents_mode == AgentsMode::HumanVsAgent {
        let mut human_vs_agent = config.agents.human_vs_agent.unwrap_or_default();
        if let Some(factory) = request
            .agent_factory
            .filter(|value| !value.trim().is_empty())
        {
            human_vs_agent.factory = Some(factory.trim().to_string());
        }
        if let Some(seed) = request.agent_seed {
            human_vs_agent.seed = Some(seed);
        }
        if let Some(timeout) = request.agent_act_timeout_seconds {
            if timeout <= 0.0 {
                return Err(AppError::bad_request(
                    "agent_act_timeout_seconds must be positive",
                ));
            }
            human_vs_agent.act_timeout_seconds = timeout;
        }
        if let Some(limit) = request.agent_invalid_action_limit {
            if limit == 0 {
                return Err(AppError::bad_request(
                    "agent_invalid_action_limit must be positive",
                ));
            }
            human_vs_agent.invalid_action_limit = limit;
        }
        if let Some(agent_config) = request.agent_config {
            human_vs_agent.config = agent_config;
        }
        config.agents.human_vs_agent = Some(human_vs_agent);
    } else {
        config.agents.human_vs_agent = None;
    }
    state
        .store
        .ensure_experiment(ExperimentRecord {
            experiment_id: experiment_id.clone(),
            config: serde_json::to_value(&config).map_err(|error| {
                AppError::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
            })?,
            server_version: Some(env!("CARGO_PKG_VERSION").to_string()),
            version_manifest: Some(state.version_manifest.clone()),
            status,
            notes: request.notes,
        })
        .await?;
    tracing::info!(%experiment_id, "administrator created experiment");
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
    tracing::info!(%experiment_id, status = %request.status, "administrator updated experiment status");
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
    tracing::info!(experiment_id = ?query.experiment_id, session_id = ?query.session_id, "administrator requested export");
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
    Query(query): Query<AdminGamesQuery>,
) -> Result<Json<Value>, AppError> {
    let experiment_id = query
        .experiment_id
        .unwrap_or_else(|| state.experiment_id.clone());
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
    Query(query): Query<AdminGamesQuery>,
) -> Result<Json<Value>, AppError> {
    let experiment_id = query
        .experiment_id
        .unwrap_or_else(|| state.experiment_id.clone());
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
    .app-header { align-items: center; background: #fff; border-bottom: 1px solid #dde2e7; display: flex; justify-content: space-between; min-height: 64px; padding: 12px 24px; }
    .header-link { background: #fff; border: 1px solid #c9d2da; border-radius: 6px; color: #31404c; font-size: 13px; font-weight: 750; padding: 8px 11px; text-decoration: none; }
    .header-link:hover { background: #f1f7fb; border-color: #185f8f; }
    .menu-button { align-items: center; background: #fff; border: 1px solid #c9d2da; border-radius: 6px; cursor: pointer; display: none; font-weight: 800; height: 36px; justify-content: center; width: 40px; }
    .shell { display: grid; grid-template-columns: minmax(280px, 380px) minmax(0, 1fr); min-height: calc(100vh - 64px); }
    .sidebar { border-right: 1px solid #dde2e7; background: #fff; padding: 18px; overflow: auto; }
    .main { padding: 24px; overflow: auto; }
    .brand { display: grid; gap: 3px; }
    .brand .game-name { font-size: 20px; font-weight: 800; line-height: 1.15; }
    .brand .dashboard-name { color: #65717c; font-size: 12px; font-weight: 700; text-transform: uppercase; }
    .workspace-header { align-items: end; display: flex; justify-content: space-between; gap: 16px; margin-bottom: 16px; }
    .workspace-title { display: grid; gap: 6px; min-width: 0; }
    .workspace-title h2 { font-size: 26px; margin: 0; overflow-wrap: anywhere; }
    .workspace-stats { display: flex; flex-wrap: wrap; gap: 8px; justify-content: flex-end; }
    .tabs { border-bottom: 1px solid #dce2e8; display: flex; gap: 4px; margin-bottom: 16px; }
    .tab { background: transparent; border: 0; border-bottom: 3px solid transparent; color: #5b6873; cursor: pointer; font-weight: 750; padding: 10px 12px; }
    .tab.active { border-bottom-color: #185f8f; color: #14202a; }
    .tab-panel[hidden] { display: none; }
    .sessions-layout { align-items: start; display: grid; gap: 16px; grid-template-columns: minmax(200px, 260px) minmax(0, 1fr); }
    .session-picker { background: #eef3f7; border-color: #cfd9e2; box-shadow: inset 0 1px 0 rgba(255,255,255,0.72); margin-bottom: 0; max-width: 100%; position: sticky; top: 16px; }
    .session-toolbar { align-items: center; display: flex; justify-content: space-between; gap: 12px; margin-bottom: 12px; }
    .session-detail { min-width: 0; }
    .topline { display: flex; align-items: center; justify-content: space-between; gap: 12px; margin-bottom: 18px; }
    .section-title { display: flex; align-items: center; justify-content: space-between; gap: 10px; margin: 18px 0 10px; }
    h1 { font-size: 22px; line-height: 1.2; margin: 0; }
    h2 { font-size: 18px; margin: 0 0 12px; }
    .muted { color: #65717c; }
    .small { font-size: 12px; }
    .refresh, .primary, .secondary, .danger, select, input, textarea { border: 1px solid #c9d2da; background: #fff; border-radius: 6px; padding: 7px 10px; }
    .refresh, .primary { cursor: pointer; }
    .primary { background: #185f8f; border-color: #185f8f; color: #fff; font-weight: 700; }
    .secondary { color: #31404c; cursor: pointer; font-weight: 650; }
    .danger { border-color: #b42318; color: #b42318; cursor: pointer; font-weight: 700; }
    textarea { min-height: 80px; resize: vertical; }
    .refresh:hover, .primary:hover, .secondary:hover { filter: brightness(0.97); }
    .control-grid { display: grid; gap: 8px; }
    .control-row { display: grid; grid-template-columns: minmax(0, 1fr) auto; gap: 8px; }
    .form-grid { display: grid; gap: 12px; }
    .form-grid label { display: grid; gap: 5px; }
    .form-actions { display: flex; flex-wrap: wrap; gap: 8px; justify-content: flex-end; }
    .modal-backdrop[hidden] { display: none; }
    .modal-backdrop { align-items: start; background: rgba(18, 29, 38, 0.38); bottom: 0; display: flex; justify-content: center; left: 0; overflow: auto; padding: 72px 16px 20px; position: fixed; right: 0; top: 0; z-index: 20; }
    .modal { background: #fff; border: 1px solid #dce2e8; border-radius: 8px; box-shadow: 0 18px 60px rgba(20, 34, 45, 0.24); max-width: 620px; padding: 18px; width: min(100%, 620px); }
    .modal .topline { margin-bottom: 12px; }
    .toggle-row { align-items: center; color: #4e5b66; display: inline-flex; font-size: 13px; gap: 8px; white-space: nowrap; }
    .toggle-row input { height: 16px; width: 16px; }
    .experiment-list { display: grid; gap: 8px; }
    .experiment { width: 100%; text-align: left; border: 1px solid #d9e0e6; border-radius: 8px; background: #fff; padding: 12px; cursor: pointer; }
    .experiment:hover, .experiment.active { border-color: #185f8f; background: #f1f7fb; }
    .warning { border: 1px solid #f2c36b; background: #fff8e8; color: #694500; border-radius: 8px; padding: 10px; margin-top: 10px; }
    .session-list { display: grid; gap: 6px; max-height: calc(100vh - 230px); overflow: auto; padding-right: 2px; }
    .session { width: 100%; text-align: left; border: 1px solid #d5dee6; border-radius: 6px; background: rgba(255,255,255,0.72); padding: 9px; cursor: pointer; }
    .session:hover, .session.active { border-color: #2773a8; background: #fff; }
    .session strong { display: block; color: #151b20; margin-bottom: 4px; overflow-wrap: anywhere; }
    .meta { display: flex; flex-wrap: wrap; gap: 8px; margin-top: 8px; }
    .pill { border: 1px solid #d7dee5; border-radius: 999px; padding: 3px 8px; background: #fff; color: #44515c; font-size: 12px; }
    .pill.warning-pill { border-color: #e5b35a; background: #fff8e8; color: #6f4700; }
    .panel { background: #fff; border: 1px solid #dce2e8; border-radius: 8px; padding: 16px; margin-bottom: 16px; }
    .grid { display: grid; grid-template-columns: repeat(4, minmax(0, 1fr)); gap: 12px; }
    .summary-line { display: none; }
    .summary-head { align-items: start; display: flex; justify-content: space-between; gap: 12px; margin-bottom: 12px; }
    .summary-head h2 { margin: 0; }
    .session-participants { border-top: 1px solid #e3e8ed; margin-top: 12px; padding-top: 12px; }
    .label { font-size: 12px; color: #66737f; margin-bottom: 4px; }
    .value { font-weight: 650; overflow-wrap: anywhere; }
    .participants { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 10px; }
    .participant { border: 1px solid #dde4ea; border-radius: 8px; padding: 12px; background: #fbfcfd; display: flex; align-items: center; gap: 12px; }
    .participant-body { min-width: 0; }
    .participant-collapse summary { align-items: center; cursor: pointer; display: flex; flex-wrap: wrap; gap: 8px 12px; list-style: none; }
    .participant-collapse summary::-webkit-details-marker { display: none; }
    .participant-collapse summary::after { color: #65717c; content: "Details"; font-size: 12px; font-weight: 700; margin-left: auto; }
    .participant-collapse[open] summary::after { content: "Hide details"; }
    .participant-summary-list { display: flex; flex-wrap: wrap; gap: 6px; min-width: 0; }
    .participant-details { margin-top: 12px; }
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
      .app-header { align-items: center; padding: 10px 14px; }
      .menu-button { display: inline-flex; }
      .brand .game-name { font-size: 17px; }
      .brand .dashboard-name { font-size: 11px; }
      .shell { grid-template-columns: 1fr; }
      .sidebar { border-right: 1px solid #dde2e7; border-bottom: 0; bottom: 0; box-shadow: 0 16px 50px rgba(20, 34, 45, 0.22); left: 0; max-height: none; max-width: 86vw; position: fixed; top: 57px; transform: translateX(-102%); transition: transform 160ms ease; width: 340px; z-index: 10; }
      body.sidebar-open .sidebar { transform: translateX(0); }
      .sessions-layout { display: flex; flex-direction: column; gap: 10px; }
      .session-picker { position: static; }
      .session-toolbar { margin-bottom: 8px; }
      .session-list { display: flex; gap: 8px; max-height: none; overflow-x: auto; padding: 0 0 4px; }
      .session { flex: 0 0 168px; padding: 8px; }
      .session strong { margin-bottom: 2px; }
      .session .meta { gap: 5px; margin-top: 6px; }
      .grid, .participants { grid-template-columns: 1fr; }
      .control-row { grid-template-columns: 1fr; }
      .modal-backdrop { align-items: stretch; padding: 64px 10px 12px; }
      .modal { padding: 14px; }
      .form-actions { justify-content: stretch; }
      .form-actions button { flex: 1 1 auto; }
      .main { padding: 10px 14px 14px; }
      .workspace-header { align-items: stretch; display: grid; }
      .workspace-title h2 { font-size: 19px; }
      .workspace-stats { justify-content: flex-start; }
      .tabs { margin-bottom: 10px; overflow-x: auto; }
      .tab { padding: 9px 10px; white-space: nowrap; }
      .panel { padding: 12px; margin-bottom: 10px; }
      .session-detail { margin-top: 0; }
      .summary-grid { display: none; }
      .summary-head { display: grid; gap: 8px; margin-bottom: 8px; }
      .summary-line { color: #4e5b66; display: flex; flex-wrap: wrap; gap: 6px; margin-top: 8px; }
      .summary-line .pill { background: #f8fafc; }
      .participants { gap: 8px; }
      .participant { align-items: center; padding: 10px; }
      .participant .meta { gap: 6px; margin-top: 4px; }
      .participant .pill { padding: 2px 7px; }
      .participant-collapse summary { align-items: flex-start; display: grid; gap: 8px; }
      .participant-collapse summary::after { margin-left: 0; }
      .events-header { align-items: center; display: grid; grid-template-columns: minmax(0, 1fr) auto auto; gap: 8px; }
      .events-header h2 { margin: 0; }
      .events-header .toggle-row span { display: none; }
      .events-header .small { white-space: nowrap; }
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
  <header class="app-header">
    <button id="menuButton" class="menu-button" type="button" aria-expanded="false" aria-controls="experimentSidebar" title="Experiments">☰</button>
    <div class="brand">
      <div id="gameName" class="game-name">Game</div>
      <div class="dashboard-name">Experimenter Dashboard</div>
    </div>
    <a class="header-link" href="/admin/privacy">Privacy status</a>
  </header>
  <div class="shell">
    <aside id="experimentSidebar" class="sidebar">
      <div class="topline">
        <h1>Experiments</h1>
        <button class="primary" id="createExperiment" title="Create draft experiment">New Experiment</button>
      </div>
      <section class="control-grid" aria-label="Experiments">
        <div id="experimentList" class="experiment-list"><div class="empty">Loading experiments...</div></div>
      </section>
    </aside>
    <main class="main">
      <section class="workspace-header">
        <div id="experimentHeader" class="workspace-title">
          <span class="muted small">No experiment selected.</span>
          <h2>Select an experiment</h2>
        </div>
        <div id="experimentStats" class="workspace-stats"></div>
      </section>
      <nav class="tabs" aria-label="Experiment workspace">
        <button class="tab active" type="button" data-tab="sessions">Sessions</button>
        <button class="tab" type="button" data-tab="export">Export</button>
        <button class="tab" type="button" data-tab="details">Experiment Details</button>
      </nav>
      <section id="sessionsPanel" class="tab-panel">
        <div class="sessions-layout">
          <aside class="panel session-picker">
            <div class="session-toolbar">
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
          <div id="sessionDetail" class="session-detail" hidden>
            <section class="panel" id="summary"><div class="empty">Select a session to inspect metadata and events.</div></section>
            <section>
              <div class="topline events-header">
                <h2>Important Events</h2>
                <label class="toggle-row" title="Show participant, session, and voice setup events">
                  <input id="showHousekeeping" type="checkbox">
                  <span>Show setup</span>
                </label>
                <span id="liveStatus" class="muted small">Idle</span>
              </div>
              <div id="timeline" class="timeline"></div>
            </section>
          </div>
        </div>
      </section>
      <section id="exportPanel" class="tab-panel" hidden>
        <section class="panel">
          <div class="topline">
            <h2>Export</h2>
            <span id="activeExperimentLabel" class="muted small">No experiment selected.</span>
          </div>
          <div class="control-grid">
            <div class="control-row">
              <select id="exportVariant" title="Export variant">
                <option value="research">Research (pseudonymized)</option>
                <option value="corpus">Corpus candidate</option>
                <option value="full">Full internal</option>
              </select>
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
              <button class="primary" id="downloadExport">Download export</button>
            </div>
          </div>
        </section>
      </section>
      <section id="detailsPanel" class="tab-panel" hidden>
        <section class="panel" id="reproducibility"><div class="empty">Loading experiment details...</div></section>
      </section>
    </main>
  </div>
  <div id="experimentFormBackdrop" class="modal-backdrop" hidden>
    <section class="modal" role="dialog" aria-modal="true" aria-labelledby="experimentFormTitle">
      <div class="topline">
        <div>
          <h2 id="experimentFormTitle">New Experiment</h2>
          <div class="muted small">Create a configured draft record for this game.</div>
        </div>
        <button class="secondary" id="cancelExperimentTop" type="button" title="Close">Close</button>
      </div>
      <form id="experimentForm" class="form-grid">
        <label>
          <span class="label">Study name</span>
          <input id="experimentStudyName" name="study_name" placeholder="e.g. Back-and-forth pilot 2" required>
        </label>
        <label>
          <span class="label">Institution</span>
          <input id="experimentInstitution" name="institution" placeholder="e.g. Saarland University">
        </label>
        <label>
          <span class="label">Experiment id</span>
          <input id="experimentId" name="experiment_id" placeholder="Generated if left blank">
        </label>
        <label>
          <span class="label">Copy settings from</span>
          <select id="experimentSource" name="source_experiment_id">
            <option value="">Current server config</option>
          </select>
        </label>
        <label>
          <span class="label">Status</span>
          <select id="experimentStatus" name="status">
            <option value="draft">Draft</option>
            <option value="active">Active</option>
            <option value="completed">Completed</option>
          </select>
        </label>
        <label>
          <span class="label">Participants</span>
          <select id="experimentAgentsMode" name="agents_mode">
            <option value="human_vs_human">Human-human</option>
            <option value="human_vs_agent">Human-agent</option>
          </select>
        </label>
        <section id="agentConfigFields" class="form-grid" hidden>
          <label>
            <span class="label">Agent</span>
            <select id="experimentAgentFactory" name="agent_factory"></select>
          </label>
          <div class="control-row">
            <label>
              <span class="label">Agent seed</span>
              <input id="experimentAgentSeed" name="agent_seed" type="number" min="0" step="1" placeholder="Optional">
            </label>
            <label>
              <span class="label">Action timeout, seconds</span>
              <input id="experimentAgentTimeout" name="agent_act_timeout_seconds" type="number" min="0.1" step="0.1" placeholder="Use current config">
            </label>
          </div>
          <label>
            <span class="label">Invalid action limit</span>
            <input id="experimentAgentInvalidLimit" name="agent_invalid_action_limit" type="number" min="1" step="1" placeholder="Use current config">
          </label>
          <label>
            <span class="label">Agent config JSON</span>
            <textarea id="experimentAgentConfig" name="agent_config" placeholder='{"endpoint":"http://127.0.0.1:50051","agent_name":"my-agent","agent_version":"v1"}'></textarea>
          </label>
        </section>
        <div class="control-row">
          <label>
            <span class="label">Waiting room timeout, seconds</span>
            <input id="experimentWaitingTimeout" name="waiting_room_timeout_seconds" type="number" min="1" step="1" placeholder="Use current config">
          </label>
          <label>
            <span class="label">Reconnect grace, seconds</span>
            <input id="experimentReconnectGrace" name="reconnect_grace_seconds" type="number" min="0" step="1" placeholder="Use current config">
          </label>
        </div>
        <label>
          <span class="label">Notes</span>
          <textarea id="experimentNotes" name="notes" placeholder="Protocol notes, recruitment batch, condition, exclusions..."></textarea>
        </label>
        <div id="experimentFormError" class="warning" hidden></div>
        <div class="form-actions">
          <button class="secondary" id="cancelExperiment" type="button">Cancel</button>
          <button class="primary" type="submit">Create Experiment</button>
        </div>
      </form>
    </section>
  </div>
  <script>
    const state = { experiments: [], activeExperimentId: null, sessions: [], selected: null, selectedSession: null, events: [], eventBundles: [], lastEventIndex: 0, timer: null, versionManifest: null, agentOptions: [], defaultAgents: null, csrfToken: null, activeTab: 'sessions' };
    const experimentList = document.getElementById('experimentList');
    const menuButton = document.getElementById('menuButton');
    const experimentHeader = document.getElementById('experimentHeader');
    const experimentStats = document.getElementById('experimentStats');
    const sessionList = document.getElementById('sessionList');
    const sessionDetail = document.getElementById('sessionDetail');
    const summary = document.getElementById('summary');
    const timeline = document.getElementById('timeline');
    const liveStatus = document.getElementById('liveStatus');
    const reproducibility = document.getElementById('reproducibility');
    const gameName = document.getElementById('gameName');
    const activeExperimentLabel = document.getElementById('activeExperimentLabel');
    const sessionStatusFilter = document.getElementById('sessionStatusFilter');
    const showHousekeeping = document.getElementById('showHousekeeping');
    const experimentFormBackdrop = document.getElementById('experimentFormBackdrop');
    const experimentForm = document.getElementById('experimentForm');
    const experimentFormError = document.getElementById('experimentFormError');
    const experimentSource = document.getElementById('experimentSource');
    const experimentAgentsMode = document.getElementById('experimentAgentsMode');
    const agentConfigFields = document.getElementById('agentConfigFields');
    const experimentAgentFactory = document.getElementById('experimentAgentFactory');
    const experimentAgentConfig = document.getElementById('experimentAgentConfig');
    const tabButtons = Array.from(document.querySelectorAll('.tab'));
    const tabPanels = {
      sessions: document.getElementById('sessionsPanel'),
      export: document.getElementById('exportPanel'),
      details: document.getElementById('detailsPanel')
    };

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
          state.selectedSession = null;
          sessionDetail.hidden = true;
          closeExperimentMenu();
          renderExperiments();
          renderExperimentHeader();
          renderExperimentDetails();
          loadSessions();
        });
      });
    }

    function renderExperimentHeader() {
      const experiment = state.experiments.find(item => item.experiment_id === state.activeExperimentId);
      if (!experiment) {
        experimentHeader.innerHTML = '<span class="muted small">No experiment selected.</span><h2>Select an experiment</h2>';
        experimentStats.innerHTML = '';
        return;
      }
      experimentHeader.innerHTML = `
        <span class="muted small">Experiment</span>
        <h2>${escapeHtml(experiment.study_name || experiment.experiment_id)}</h2>
        ${experiment.study_name ? `<span class="muted small">${escapeHtml(experiment.experiment_id)}</span>` : ''}
      `;
      experimentStats.innerHTML = `
        <span class="pill">${escapeHtml(experiment.status)}</span>
        <span class="pill">${experiment.session_count} sessions</span>
        <span class="pill">${experiment.completed_session_count} completed</span>
      `;
    }

    function renderExperimentDetails() {
      const experiment = state.experiments.find(item => item.experiment_id === state.activeExperimentId);
      const manifest = experiment?.version_manifest || state.versionManifest || {};
      const warnings = manifest.warnings || [];
      gameName.textContent = displayGameName(manifest.game);
      activeExperimentLabel.textContent = state.activeExperimentId ? `Experiment ${state.activeExperimentId}` : 'No experiment selected.';
      reproducibility.innerHTML = `
        <h2>Experiment Details</h2>
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

    function displayGameName(game) {
      const name = game?.display_name || game?.name || 'Game';
      return String(name).replace(/^parlando-/, '').replace(/-/g, ' ');
    }

    function renderSessions() {
      if (!state.sessions.length) {
        sessionList.innerHTML = '<div class="empty">No sessions in this experiment yet.</div>';
        return;
      }
      sessionList.innerHTML = state.sessions.map(session => `
        <button class="session ${state.selected === session.session_id ? 'active' : ''}" data-session="${session.session_id}">
          <strong>${escapeHtml(session.dialogue_id || `Session #${session.session_id}`)}</strong>
          <span class="muted small">Session #${session.session_id} · ${escapeHtml(fmtTime(session.created_at))}</span>
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
      if (state.selected && !state.sessions.some(session => session.session_id === state.selected)) {
        state.selected = null;
        state.selectedSession = null;
        sessionDetail.hidden = true;
      }
      renderSessions();
      if (!state.selected && state.sessions[0]) selectSession(state.sessions[0].session_id);
    }

    async function loadExperiments() {
      const response = await fetch('/api/admin/experiments?limit=100');
      const data = await response.json();
      state.experiments = data.experiments || [];
      state.versionManifest = data.version_manifest || null;
      state.agentOptions = data.agent_options || [];
      state.defaultAgents = data.default_agents || null;
      state.csrfToken = data.csrf_token || null;
      state.activeExperimentId = state.activeExperimentId || data.active_experiment_id || state.experiments[0]?.experiment_id || null;
      renderExperiments();
      renderExperimentHeader();
      renderExperimentDetails();
    }

    async function selectSession(sessionId) {
      state.selected = sessionId;
      state.lastEventIndex = 0;
      renderSessions();
      liveStatus.textContent = 'Loading';
      const detailParams = new URLSearchParams();
      if (state.activeExperimentId) detailParams.set('experiment_id', state.activeExperimentId);
      const response = await fetch(`/api/admin/sessions/${sessionId}?${detailParams}`);
      const data = await response.json();
      state.selectedSession = data.session;
      renderSummary(data.session, data.participants || []);
      sessionDetail.hidden = false;
      state.events = [];
      state.eventBundles = data.event_bundles || [];
      mergeEvents(data.events || []);
      renderEventBundles();
      liveStatus.textContent = 'Live';
    }

    function renderTabs() {
      tabButtons.forEach(button => button.classList.toggle('active', button.dataset.tab === state.activeTab));
      Object.entries(tabPanels).forEach(([name, panel]) => {
        panel.hidden = name !== state.activeTab;
      });
    }

    function closeExperimentMenu() {
      document.body.classList.remove('sidebar-open');
      menuButton.setAttribute('aria-expanded', 'false');
    }

    function toggleExperimentMenu() {
      const open = !document.body.classList.contains('sidebar-open');
      document.body.classList.toggle('sidebar-open', open);
      menuButton.setAttribute('aria-expanded', String(open));
    }

    function activeExperiment() {
      return state.experiments.find(item => item.experiment_id === state.activeExperimentId);
    }

    function openExperimentForm() {
      experimentForm.reset();
      experimentFormError.hidden = true;
      renderExperimentSourceOptions();
      applyExperimentConfigDefaults({
        study: {},
        agents: state.defaultAgents || {}
      }, null);
      experimentSource.value = '';
      document.getElementById('experimentStatus').value = 'draft';
      experimentFormBackdrop.hidden = false;
      document.getElementById('experimentStudyName').focus();
    }

    function closeExperimentForm() {
      experimentFormBackdrop.hidden = true;
      experimentFormError.hidden = true;
    }

    function optionalNumber(id) {
      const value = document.getElementById(id).value.trim();
      return value === '' ? null : Number(value);
    }

    function optionalInteger(id) {
      const value = document.getElementById(id).value.trim();
      return value === '' ? null : Number(value);
    }

    function renderExperimentSourceOptions() {
      experimentSource.innerHTML = '<option value="">Current server config</option>' + state.experiments.map(experiment => `
        <option value="${escapeHtml(experiment.experiment_id)}">${escapeHtml(experiment.study_name || experiment.experiment_id)}</option>
      `).join('');
    }

    function applyExperimentConfigDefaults(config, experiment) {
      const agents = config?.agents || state.defaultAgents || {};
      const humanVsAgent = agents.human_vs_agent || {};
      document.getElementById('experimentStudyName').value = config?.study?.name || experiment?.study_name || '';
      document.getElementById('experimentInstitution').value = config?.study?.institution || '';
      document.getElementById('experimentWaitingTimeout').value = config?.study?.waiting_room_timeout_seconds ?? '';
      document.getElementById('experimentReconnectGrace').value = config?.study?.reconnect_grace_seconds ?? '';
      document.getElementById('experimentNotes').value = experiment?.notes || '';
      experimentAgentsMode.value = agents.mode || 'human_vs_human';
      renderAgentOptions();
      if (humanVsAgent.factory && Array.from(experimentAgentFactory.options).some(option => option.value === humanVsAgent.factory)) {
        experimentAgentFactory.value = humanVsAgent.factory;
      }
      document.getElementById('experimentAgentSeed').value = humanVsAgent.seed ?? '';
      document.getElementById('experimentAgentTimeout').value = humanVsAgent.act_timeout_seconds ?? '';
      document.getElementById('experimentAgentInvalidLimit').value = humanVsAgent.invalid_action_limit ?? '';
      experimentAgentConfig.value = humanVsAgent.config && Object.keys(humanVsAgent.config).length ? JSON.stringify(humanVsAgent.config, null, 2) : '';
      updateAgentConfigVisibility(false);
    }

    async function loadExperimentSourceConfig() {
      experimentFormError.hidden = true;
      const sourceId = experimentSource.value;
      if (!sourceId) {
        applyExperimentConfigDefaults({ study: {}, agents: state.defaultAgents || {} }, null);
        return;
      }
      const response = await fetch(`/api/admin/export?experiment_id=${encodeURIComponent(sourceId)}`);
      if (!response.ok) {
        experimentFormError.textContent = await response.text();
        experimentFormError.hidden = false;
        return;
      }
      const exported = await response.json();
      const experiment = state.experiments.find(item => item.experiment_id === sourceId);
      applyExperimentConfigDefaults(exported.experiment?.config || {}, experiment);
    }

    function selectedAgentOption() {
      return state.agentOptions.find(option => option.selector === experimentAgentFactory.value);
    }

    function renderAgentOptions() {
      const options = state.agentOptions.length ? state.agentOptions : [{
        selector: 'remote_grpc',
        label: 'Remote gRPC agent',
        description: 'External agent service',
        default_config: { endpoint: 'http://127.0.0.1:50051', agent_name: 'space-game-remote-agent' }
      }];
      experimentAgentFactory.innerHTML = options.map(option => `
        <option value="${escapeHtml(option.selector)}">${escapeHtml(option.label || option.selector)}</option>
      `).join('');
    }

    function updateAgentConfigVisibility(resetConfig = true) {
      const enabled = experimentAgentsMode.value === 'human_vs_agent';
      agentConfigFields.hidden = !enabled;
      if (!enabled) return;
      const currentFactory = experimentAgentFactory.value;
      renderAgentOptions();
      if (currentFactory && Array.from(experimentAgentFactory.options).some(option => option.value === currentFactory)) {
        experimentAgentFactory.value = currentFactory;
      }
      const selected = selectedAgentOption();
      const config = selected?.default_config || {};
      if (resetConfig) {
        experimentAgentConfig.value = Object.keys(config).length ? JSON.stringify(config, null, 2) : '';
      }
    }

    function optionalJson(id) {
      const value = document.getElementById(id).value.trim();
      if (!value) return null;
      return JSON.parse(value);
    }

    async function submitExperimentForm(event) {
      event.preventDefault();
      experimentFormError.hidden = true;
      let body;
      try {
        body = {
          source_experiment_id: experimentSource.value || null,
          experiment_id: document.getElementById('experimentId').value.trim() || null,
          study_name: document.getElementById('experimentStudyName').value.trim() || null,
          institution: document.getElementById('experimentInstitution').value.trim(),
          status: document.getElementById('experimentStatus').value,
          agents_mode: experimentAgentsMode.value,
          agent_factory: experimentAgentsMode.value === 'human_vs_agent' ? experimentAgentFactory.value : null,
          agent_seed: optionalInteger('experimentAgentSeed'),
          agent_act_timeout_seconds: optionalNumber('experimentAgentTimeout'),
          agent_invalid_action_limit: optionalInteger('experimentAgentInvalidLimit'),
          agent_config: experimentAgentsMode.value === 'human_vs_agent' ? optionalJson('experimentAgentConfig') : null,
          waiting_room_timeout_seconds: optionalNumber('experimentWaitingTimeout'),
          reconnect_grace_seconds: optionalNumber('experimentReconnectGrace'),
          notes: document.getElementById('experimentNotes').value.trim() || null
        };
      } catch (error) {
        experimentFormError.textContent = `Agent config JSON is invalid: ${error.message}`;
        experimentFormError.hidden = false;
        return;
      }
      const response = await fetch('/api/admin/experiments', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', 'X-CSRF-Token': state.csrfToken || '' },
        body: JSON.stringify(body)
      });
      if (!response.ok) {
        experimentFormError.textContent = await response.text();
        experimentFormError.hidden = false;
        return;
      }
      const data = await response.json();
      state.activeExperimentId = data.experiment_id;
      state.selected = null;
      state.selectedSession = null;
      closeExperimentForm();
      await loadExperiments();
      await loadSessions();
    }

    function renderSummary(session, participantRows) {
      summary.innerHTML = `
        <div class="summary-head">
          <h2>${escapeHtml(session.dialogue_id || `Session #${session.session_id}`)}</h2>
          ${participantSummaryInline(participantRows)}
        </div>
        <div class="summary-line">
          <span class="pill">Session #${escapeHtml(session.session_id)}</span>
          <span class="pill">${escapeHtml(session.mode)}</span>
          <span class="pill">${escapeHtml(session.status)}</span>
          <span class="pill">${escapeHtml(fmtTime(session.created_at))}</span>
        </div>
        <div class="grid summary-grid">
          <div><div class="label">Dialogue ID</div><div class="value">${escapeHtml(session.dialogue_id)}</div></div>
          <div><div class="label">Mode</div><div class="value">${escapeHtml(session.mode)}</div></div>
          <div><div class="label">Status</div><div class="value">${escapeHtml(session.status)}</div></div>
          <div><div class="label">Created</div><div class="value">${escapeHtml(fmtTime(session.created_at))}</div></div>
        </div>
        ${participantDetailsInline(participantRows)}
      `;
    }

    function participantLabel(row) {
      return row.research_id || row.participant_kind || 'Participant';
    }

    function participantSummary(rows) {
      return rows.map(row => `<span class="pill">${escapeHtml(row.role || 'SYS')} ${escapeHtml(participantLabel(row))}</span>`).join('');
    }

    function participantSummaryInline(rows) {
      if (!rows.length) return '<span class="muted small">No players recorded.</span>';
      return `<span class="participant-summary-list">${participantSummary(rows)}</span>`;
    }

    function participantCards(rows) {
      return rows.map(row => `
        <div class="participant">
          ${roleBadge(row.role)}
          <div class="participant-body">
            <div class="label">Participant ID</div>
            <div class="value">${escapeHtml(participantLabel(row))}</div>
            <div class="meta">
              <span class="pill">${escapeHtml(row.connection_status)}</span>
              <span class="pill">${escapeHtml(row.participant_kind || 'participant')}</span>
              ${agentParticipantPills(row)}
            </div>
            ${row.participant_kind !== 'agent' && row.research_id ? `<button class="danger delete-participant" data-research-id="${escapeHtml(row.research_id)}" type="button">Delete participant data</button>` : ''}
          </div>
        </div>
      `).join('');
    }

    function participantDetailsInline(rows) {
      if (!rows.length) {
        return '';
      }
      const cards = participantCards(rows);
      return `
        <details class="participant-collapse session-participants">
          <summary>
            <span class="muted small">Player details</span>
          </summary>
          <div class="participants participant-details">${cards}</div>
        </details>
      `;
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

    // Previews and confirms deletion of one human participant's stored data.
    async function deleteParticipantData(researchId) {
      if (!state.activeExperimentId) return;
      const params = new URLSearchParams({ experiment_id: state.activeExperimentId });
      const path = `/api/admin/participants/${encodeURIComponent(researchId)}/deletion?${params}`;
      const previewResponse = await fetch(path);
      if (!previewResponse.ok) throw new Error(await previewResponse.text());
      const preview = (await previewResponse.json()).preview;
      const confirmed = window.confirm(
        `Delete ${researchId}? This removes ${preview.content_event_count} message/transcript events and ${preview.consent_count} consent declarations, and anonymizes ${preview.other_event_count} other event references. This cannot be undone.`
      );
      if (!confirmed) return;
      const response = await fetch(path, {
        method: 'POST',
        headers: { 'X-CSRF-Token': state.csrfToken || '' }
      });
      if (!response.ok) throw new Error(await response.text());
      await selectSession(state.selected);
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

    showHousekeeping.addEventListener('change', renderEventBundles);
    menuButton.addEventListener('click', toggleExperimentMenu);
    document.addEventListener('keydown', event => {
      if (event.key === 'Escape' && !experimentFormBackdrop.hidden) {
        closeExperimentForm();
        return;
      }
      if (event.key === 'Escape') closeExperimentMenu();
    });
    document.addEventListener('click', event => {
      if (!document.body.classList.contains('sidebar-open')) return;
      if (event.target.closest('#experimentSidebar') || event.target.closest('#menuButton')) return;
      closeExperimentMenu();
    });
    tabButtons.forEach(button => {
      button.addEventListener('click', () => {
        state.activeTab = button.dataset.tab;
        renderTabs();
      });
    });
    sessionStatusFilter.addEventListener('change', () => {
      state.selected = null;
      state.selectedSession = null;
      sessionDetail.hidden = true;
      loadSessions();
    });
    document.getElementById('createExperiment').addEventListener('click', openExperimentForm);
    document.getElementById('cancelExperiment').addEventListener('click', closeExperimentForm);
    document.getElementById('cancelExperimentTop').addEventListener('click', closeExperimentForm);
    experimentForm.addEventListener('submit', submitExperimentForm);
    experimentSource.addEventListener('change', loadExperimentSourceConfig);
    experimentAgentsMode.addEventListener('change', updateAgentConfigVisibility);
    experimentAgentFactory.addEventListener('change', () => updateAgentConfigVisibility(true));
    experimentFormBackdrop.addEventListener('click', event => {
      if (event.target === experimentFormBackdrop) closeExperimentForm();
    });
    document.getElementById('downloadExport').addEventListener('click', () => {
      if (!state.activeExperimentId) return;
      const params = new URLSearchParams({
        experiment_id: state.activeExperimentId,
        format: document.getElementById('exportFormat').value,
        variant: document.getElementById('exportVariant').value
      });
      if (document.getElementById('exportScope').value === 'session' && state.selected) {
        params.set('session_id', String(state.selected));
      }
      const eventType = document.getElementById('eventTypeFilter').value.trim();
      if (eventType) params.set('event_type', eventType);
      window.location.href = `/api/admin/export?${params}`;
    });
    summary.addEventListener('click', event => {
      const button = event.target.closest('.delete-participant');
      if (!button) return;
      button.disabled = true;
      deleteParticipantData(button.dataset.researchId).catch(error => {
        window.alert(error.message);
        button.disabled = false;
      });
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
    renderTabs();
    setInterval(() => {
      loadExperiments().then(loadSessions).catch(error => {
        liveStatus.textContent = error.message;
      });
    }, 5000);
    state.timer = setInterval(refreshEvents, 1500);
  </script>
</body>
</html>
"##;

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
    #[cfg(test)]
    let legacy_participant = query.get("participantSessionId").cloned();
    #[cfg(not(test))]
    let legacy_participant: Option<String> = None;
    let (participant_session_id, claimed_role) = if let Some(participant) = legacy_participant {
        (participant, None)
    } else {
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
        (claims.participant_session_id, Some(claims.role))
    };
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
        collections::{BTreeMap, VecDeque},
        fs,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Mutex,
        },
    };
    use tokio::net::TcpListener;
    use tokio_tungstenite::{connect_async, tungstenite::Message as TungsteniteMessage};
    use tower::ServiceExt;

    use crate::agents::{AgentInitContext, AgentResponse, AgentUtteranceKind, GameAgent};
    use crate::config::{
        AgentOptionConfig, AgentsConfig, AgentsMode, DatabaseConfig, DirectConfig,
        ExperimentIdentityConfig,
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
    fn admin_dashboard_html_reflects_game_scoped_experiment_layout() {
        assert!(ADMIN_GAMES_HTML.contains("app-header"));
        assert!(ADMIN_GAMES_HTML.contains("gameName"));
        assert!(ADMIN_GAMES_HTML.contains("href=\"/admin/privacy\""));
        assert!(ADMIN_GAMES_HTML.contains("New Experiment"));
        assert!(ADMIN_GAMES_HTML.contains("experimentForm"));
        assert!(ADMIN_GAMES_HTML.contains("experimentStudyName"));
        assert!(ADMIN_GAMES_HTML.contains("experimentSource"));
        assert!(ADMIN_GAMES_HTML.contains("experimentAgentsMode"));
        assert!(ADMIN_GAMES_HTML.contains("experimentAgentFactory"));
        assert!(ADMIN_GAMES_HTML.contains("agent_options"));
        assert!(ADMIN_GAMES_HTML.contains("waiting_room_timeout_seconds"));
        assert!(ADMIN_GAMES_HTML.contains("Experiment Details"));
        assert!(ADMIN_GAMES_HTML.contains("data-tab=\"sessions\""));
        assert!(ADMIN_GAMES_HTML.contains("data-tab=\"export\""));
        assert!(ADMIN_GAMES_HTML.contains("data-tab=\"details\""));
        assert!(ADMIN_GAMES_HTML.contains("id=\"sessionsPanel\""));
        assert!(ADMIN_GAMES_HTML.contains("id=\"exportPanel\""));
        assert!(ADMIN_GAMES_HTML.contains("id=\"exportVariant\""));
        assert!(ADMIN_GAMES_HTML.contains("Dialogue ID"));
        assert!(ADMIN_GAMES_HTML.contains("Participant ID"));
        assert!(ADMIN_GAMES_HTML.contains("Delete participant data"));
        assert!(ADMIN_GAMES_HTML.contains("id=\"detailsPanel\""));
        assert!(ADMIN_GAMES_HTML.contains("id=\"sessionDetail\""));
        assert!(ADMIN_GAMES_HTML.contains("sessions-layout"));
        assert!(ADMIN_GAMES_HTML.contains("session-picker"));
        assert!(ADMIN_GAMES_HTML.contains("participant-collapse"));
        assert!(ADMIN_GAMES_HTML.contains("participant-summary-list"));
        assert!(ADMIN_GAMES_HTML.contains("session-participants"));
        assert!(ADMIN_GAMES_HTML.contains("id=\"menuButton\""));
        assert!(ADMIN_GAMES_HTML.contains("sidebar-open"));
        assert!(ADMIN_GAMES_HTML.contains("summary-line"));
        assert!(ADMIN_GAMES_HTML.contains("events-header"));
        assert!(!ADMIN_GAMES_HTML.contains("refreshSessions"));
        assert!(!ADMIN_GAMES_HTML.contains(">Reproducibility<"));
        let sidebar = ADMIN_GAMES_HTML
            .split("id=\"experimentSidebar\"")
            .nth(1)
            .and_then(|html| html.split("</aside>").next())
            .unwrap();
        assert!(!sidebar.contains("<h2>Sessions</h2>"));
        let sessions_panel = ADMIN_GAMES_HTML
            .split("id=\"sessionsPanel\"")
            .nth(1)
            .and_then(|html| html.split("id=\"exportPanel\"").next())
            .unwrap();
        assert!(!sessions_panel.contains("<h2>Players</h2>"));
    }

    /// Confirms the protected privacy routes render one truthful installation-wide status.
    #[tokio::test]
    async fn admin_privacy_status_renders_and_downloads_current_facts() {
        let (config, _tmp) = sqlite_config();
        let router = build_router(TinyAdapter, config, ServeOptions::default())
            .await
            .unwrap();

        let page = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/admin/privacy")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(page.status(), StatusCode::OK);
        let page_html = to_bytes(page.into_body(), usize::MAX).await.unwrap();
        let page_html = String::from_utf8_lossy(&page_html);
        assert!(page_html.contains("Installation-wide facts"));
        assert!(page_html.contains("Not yet bound to a completed DPO platform assessment"));
        assert!(page_html.contains("href=\"/admin/games\""));

        let (status, privacy) = json_request(
            router.clone(),
            http::Method::GET,
            "/api/admin/privacy",
            Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(privacy["privacy_contract_version"], "1");
        assert_eq!(privacy["raw_audio_stored_by_parlando"], false);
        assert_eq!(privacy["exports"]["full_internal_export"], true);
        assert_eq!(privacy["exports"]["research_export"], true);
        assert_eq!(privacy["exports"]["corpus_export"], true);
        assert_eq!(privacy["participant_deletion"]["available"], true);
        assert_eq!(privacy["consent_evidence"]["available"], true);
        assert_eq!(privacy["external_services"], json!([]));

        let markdown = router
            .oneshot(
                Request::builder()
                    .uri("/api/admin/privacy.md")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(markdown.status(), StatusCode::OK);
        assert_eq!(
            markdown
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("text/markdown; charset=utf-8")
        );
        assert!(markdown.headers().contains_key(CONTENT_DISPOSITION));
        let markdown_body = to_bytes(markdown.into_body(), usize::MAX).await.unwrap();
        assert!(String::from_utf8_lossy(&markdown_body).contains("# Parlando privacy status"));
    }

    /// Confirms fixed public exports exclude direct identity while retaining readable ids.
    #[test]
    fn research_and_corpus_exports_apply_fixed_identity_boundaries() {
        let full = json!({
            "participants": [{
                "participant_id": 7,
                "research_id": "calm-blue-otter",
                "participant_kind": "human",
                "external_id": "recruitment-id"
            }],
            "sessions": [{"session_id": 3, "dialogue_id": "softly-amber-harbor", "mode": "direct", "status": "completed", "created_at": "2026-01-01T00:00:00Z"}],
            "session_participants": [{"session_id": 3, "participant_id": 7, "participant_session_id": "ps_secret", "role": "A"}],
            "session_events": [{
                "session_id": 3,
                "event_index": 4,
                "event_type": "conversation_message",
                "actor_participant_id": 7,
                "actor_role": "A",
                "payload": {"text": "hello", "origin": "typed", "sender_participant_session_id": "ps_secret"},
                "created_at": "2026-01-01T00:00:04Z"
            }]
        });
        let research = research_export(full, "1");
        let encoded = serde_json::to_string(&research).unwrap();
        assert!(!encoded.contains("recruitment-id"));
        assert!(!encoded.contains("ps_secret"));
        assert!(encoded.contains("calm-blue-otter"));
        assert!(encoded.contains("softly-amber-harbor"));

        let corpus = corpus_export(research);
        let encoded = serde_json::to_string(&corpus).unwrap();
        assert_eq!(corpus["release_status"], "corpus_candidate");
        assert!(encoded.contains("calm-blue-otter"));
        assert!(encoded.contains("softly-amber-harbor"));
        assert!(!encoded.contains("2026-01-01"));
        assert_eq!(corpus["messages"][0]["text"], "hello");
        assert_eq!(corpus["messages"][0]["time_from_session_start_ms"], 4_000);
    }

    /// Confirms `/admin` reaches setup and the resulting login persists across router restarts.
    #[tokio::test]
    async fn admin_setup_page_creates_one_persistent_credential() {
        let (config, _tmp) = sqlite_config();
        let router = build_router(TinyAdapter, config.clone(), ServeOptions::default())
            .await
            .unwrap();

        let admin_entry = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/admin")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(admin_entry.status().is_redirection());
        assert_eq!(
            admin_entry
                .headers()
                .get(http::header::LOCATION)
                .and_then(|value| value.to_str().ok()),
            Some("/admin/login")
        );

        let setup_page = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/admin/login")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let setup_html = to_bytes(setup_page.into_body(), usize::MAX).await.unwrap();
        assert!(String::from_utf8_lossy(&setup_html).contains("Create administrator"));

        let setup_response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(http::Method::POST)
                    .uri("/api/admin/setup")
                    .header(http::header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({"username": "researcher", "password": "a-long-test-password"})
                            .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(setup_response.status(), StatusCode::OK);
        assert!(setup_response.headers().contains_key(SET_COOKIE));

        let (second_status, _) = json_request(
            router,
            http::Method::POST,
            "/api/admin/setup",
            json!({"username": "second", "password": "another-test-password"}),
        )
        .await;
        assert_eq!(second_status, StatusCode::CONFLICT);

        let restarted = build_router(TinyAdapter, config, ServeOptions::default())
            .await
            .unwrap();
        let login_page = restarted
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/admin/login")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let login_html = to_bytes(login_page.into_body(), usize::MAX).await.unwrap();
        assert!(String::from_utf8_lossy(&login_html).contains("Sign in"));
        let (login_status, _) = json_request(
            restarted,
            http::Method::POST,
            "/api/admin/login",
            json!({"username": "researcher", "password": "a-long-test-password"}),
        )
        .await;
        assert_eq!(login_status, StatusCode::OK);
    }

    #[tokio::test]
    async fn admin_create_experiment_persists_form_configuration() {
        let (mut config, _tmp) = sqlite_config();
        config.experiment.id = Some("bootstrap".to_string());
        config.study.name = "Bootstrap Study".to_string();
        config.agents.available = vec![
            AgentOptionConfig {
                selector: "remote_grpc".to_string(),
                label: "Configured remote".to_string(),
                default_config: json!({"endpoint": "http://127.0.0.1:50051"}),
                ..AgentOptionConfig::default()
            },
            AgentOptionConfig {
                selector: "scripted".to_string(),
                label: "Configured scripted".to_string(),
                default_config: json!({"script": "baseline"}),
                ..AgentOptionConfig::default()
            },
        ];
        let router = build_router(TinyAdapter, config, ServeOptions::default())
            .await
            .unwrap();

        let (status, created) = json_request(
            router.clone(),
            http::Method::POST,
            "/api/admin/experiments",
            json!({
                "experiment_id": "pilot-two",
                "study_name": "Pilot Two",
                "institution": "Test University",
                "status": "draft",
                "agents_mode": "human_vs_agent",
                "agent_factory": "remote_grpc",
                "agent_seed": 123,
                "agent_act_timeout_seconds": 1.5,
                "agent_invalid_action_limit": 4,
                "agent_config": {
                    "endpoint": "http://127.0.0.1:50051",
                    "agent_name": "pilot-agent",
                    "agent_version": "v2"
                },
                "waiting_room_timeout_seconds": 42,
                "reconnect_grace_seconds": 7,
                "notes": "counterbalanced condition B"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(created["experiment_id"], "pilot-two");

        let (status, listed) = json_request(
            router.clone(),
            http::Method::GET,
            "/api/admin/experiments?limit=10",
            Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let agent_selectors = listed["agent_options"]
            .as_array()
            .unwrap()
            .iter()
            .map(|option| option["selector"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(agent_selectors, vec!["remote_grpc", "scripted"]);
        let experiment = listed["experiments"]
            .as_array()
            .unwrap()
            .iter()
            .find(|experiment| experiment["experiment_id"] == "pilot-two")
            .unwrap();
        assert_eq!(experiment["study_name"], "Pilot Two");
        assert_eq!(experiment["status"], "draft");
        assert_eq!(experiment["notes"], "counterbalanced condition B");

        let (status, exported) = json_request(
            router.clone(),
            http::Method::GET,
            "/api/admin/export?experiment_id=pilot-two&variant=full",
            Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            exported["experiment"]["config"]["experiment"]["id"],
            "pilot-two"
        );
        assert_eq!(
            exported["experiment"]["config"]["study"]["name"],
            "Pilot Two"
        );
        assert_eq!(
            exported["experiment"]["config"]["study"]["institution"],
            "Test University"
        );
        assert_eq!(
            exported["experiment"]["config"]["agents"]["mode"],
            "human_vs_agent"
        );
        assert_eq!(
            exported["experiment"]["config"]["agents"]["human_vs_agent"]["factory"],
            "remote_grpc"
        );
        assert_eq!(
            exported["experiment"]["config"]["agents"]["human_vs_agent"]["seed"],
            123
        );
        assert_eq!(
            exported["experiment"]["config"]["agents"]["human_vs_agent"]["act_timeout_seconds"],
            1.5
        );
        assert_eq!(
            exported["experiment"]["config"]["agents"]["human_vs_agent"]["invalid_action_limit"],
            4
        );
        assert_eq!(
            exported["experiment"]["config"]["agents"]["human_vs_agent"]["config"]["agent_name"],
            "pilot-agent"
        );
        assert_eq!(
            exported["experiment"]["config"]["study"]["waiting_room_timeout_seconds"],
            42
        );
        assert_eq!(
            exported["experiment"]["config"]["study"]["reconnect_grace_seconds"],
            7
        );

        let (status, cloned) = json_request(
            router.clone(),
            http::Method::POST,
            "/api/admin/experiments",
            json!({
                "source_experiment_id": "pilot-two",
                "experiment_id": "pilot-two-copy",
                "study_name": "Pilot Two Copy",
                "status": "draft"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(cloned["experiment_id"], "pilot-two-copy");

        let (status, cloned_export) = json_request(
            router,
            http::Method::GET,
            "/api/admin/export?experiment_id=pilot-two-copy&variant=full",
            Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            cloned_export["experiment"]["config"]["experiment"]["id"],
            "pilot-two-copy"
        );
        assert_eq!(
            cloned_export["experiment"]["config"]["study"]["name"],
            "Pilot Two Copy"
        );
        assert_eq!(
            cloned_export["experiment"]["config"]["agents"]["human_vs_agent"]["factory"],
            "remote_grpc"
        );
        assert_eq!(
            cloned_export["experiment"]["config"]["agents"]["human_vs_agent"]["config"]
                ["agent_name"],
            "pilot-agent"
        );
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
                json!({"event": "audio_transport_disconnected"}),
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
        outcome: String,
        dyad_score: i64,
        player_scores: BTreeMap<String, i64>,
    }

    #[derive(Clone)]
    struct TinyAdapter;

    #[derive(Clone)]
    struct NoAvailableActionsAdapter;

    #[derive(Clone)]
    struct LossSummaryAdapter;

    struct NoopAgent;

    #[async_trait]
    impl GameAgent<TinyAdapter> for NoopAgent {
        async fn maybe_act(
            &mut self,
            _available_actions: Option<Vec<TinyAction>>,
        ) -> Result<Option<AgentResponse<TinyAction>>> {
            Ok(None)
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
        script: VecDeque<Option<AgentResponse<TinyAction>>>,
    }

    #[async_trait]
    impl GameAgent<TinyAdapter> for ScriptedAgent {
        async fn maybe_act(
            &mut self,
            _available_actions: Option<Vec<TinyAction>>,
        ) -> Result<Option<AgentResponse<TinyAction>>> {
            Ok(self.script.pop_front().unwrap_or(None))
        }
    }

    struct ScriptedAgentFactory {
        created: AtomicUsize,
        scripts: Mutex<VecDeque<Vec<Option<AgentResponse<TinyAction>>>>>,
    }

    impl ScriptedAgentFactory {
        // Creates a factory that hands one script to each fresh agent instance.
        fn new(scripts: Vec<Vec<Option<AgentResponse<TinyAction>>>>) -> Self {
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

    // Creates a scripted optional response.
    fn scripted_response(
        message: Option<&str>,
        action: Option<TinyAction>,
    ) -> Option<AgentResponse<TinyAction>> {
        Some(AgentResponse {
            message: message.map(str::to_string),
            action,
        })
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
        async fn maybe_act(
            &mut self,
            available_actions: Option<Vec<TinyAction>>,
        ) -> Result<Option<AgentResponse<TinyAction>>> {
            self.seen_actions.lock().unwrap().push(available_actions);
            Ok(None)
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

    struct RecordingObservationsAgent {
        observations: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl GameAgent<TinyAdapter> for RecordingObservationsAgent {
        async fn observe_state(&mut self, current_observation: TinyObservation) -> Result<()> {
            self.observations.lock().unwrap().push(format!(
                "state:{}:{}",
                current_observation.role, current_observation.done
            ));
            Ok(())
        }

        async fn observe_action(
            &mut self,
            actor: PlayerRole,
            action: TinyAction,
            resulting_observation: TinyObservation,
        ) -> Result<()> {
            self.observations.lock().unwrap().push(format!(
                "action:{}:{}:{}",
                actor.as_str(),
                action.finish,
                resulting_observation.done
            ));
            Ok(())
        }

        async fn observe_message(
            &mut self,
            speaker: PlayerRole,
            kind: AgentUtteranceKind,
            text: String,
        ) -> Result<()> {
            self.observations
                .lock()
                .unwrap()
                .push(format!("message:{}:{kind:?}:{text}", speaker.as_str()));
            Ok(())
        }

        async fn maybe_act(
            &mut self,
            _available_actions: Option<Vec<TinyAction>>,
        ) -> Result<Option<AgentResponse<TinyAction>>> {
            Ok(None)
        }
    }

    struct RecordingObservationsAgentFactory {
        observations: Arc<Mutex<Vec<String>>>,
    }

    impl AgentFactory<TinyAdapter> for RecordingObservationsAgentFactory {
        fn create(
            &self,
            _context: AgentInitContext,
        ) -> Result<Box<dyn GameAgent<TinyAdapter> + Send>> {
            Ok(Box::new(RecordingObservationsAgent {
                observations: self.observations.clone(),
            }))
        }
    }

    struct SequencedDecisionAgent {
        log: Arc<Mutex<Vec<String>>>,
        decisions: usize,
    }

    #[async_trait]
    impl GameAgent<TinyAdapter> for SequencedDecisionAgent {
        async fn observe_action(
            &mut self,
            actor: PlayerRole,
            _action: TinyAction,
            _resulting_observation: TinyObservation,
        ) -> Result<()> {
            self.log
                .lock()
                .unwrap()
                .push(format!("observe_action:{}", actor.as_str()));
            Ok(())
        }

        async fn maybe_act(
            &mut self,
            _available_actions: Option<Vec<TinyAction>>,
        ) -> Result<Option<AgentResponse<TinyAction>>> {
            self.decisions += 1;
            self.log
                .lock()
                .unwrap()
                .push(format!("maybe_act:{}", self.decisions));
            if self.decisions == 1 {
                return Ok(scripted_response(
                    None,
                    Some(TinyAction {
                        finish: false,
                        invalid: false,
                    }),
                ));
            }
            Ok(None)
        }
    }

    struct SequencedDecisionAgentFactory {
        log: Arc<Mutex<Vec<String>>>,
    }

    impl AgentFactory<TinyAdapter> for SequencedDecisionAgentFactory {
        fn create(
            &self,
            _context: AgentInitContext,
        ) -> Result<Box<dyn GameAgent<TinyAdapter> + Send>> {
            Ok(Box::new(SequencedDecisionAgent {
                log: self.log.clone(),
                decisions: 0,
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
            TinySummary {
                done: state.done,
                outcome: if state.done { "success" } else { "in_progress" }.to_string(),
                dyad_score: if state.done { 10 } else { 0 },
                player_scores: BTreeMap::from([
                    ("A".to_string(), if state.done { 6 } else { 0 }),
                    ("B".to_string(), if state.done { 4 } else { 0 }),
                ]),
            }
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

    impl GameAdapter for LossSummaryAdapter {
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

        fn available_actions(
            &self,
            state: &Self::State,
            player: PlayerRole,
        ) -> Option<Vec<Self::Action>> {
            TinyAdapter.available_actions(state, player)
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
            let mut summary = TinyAdapter.completion_summary(state);
            if state.done {
                summary.outcome = "loss".to_string();
                summary.dyad_score = 0;
                summary
                    .player_scores
                    .extend([("A".to_string(), 0), ("B".to_string(), 0)]);
            }
            summary
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
                participant_information_version: "test-v1".to_string(),
                participant_information_url: "https://example.test/privacy".to_string(),
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

    fn voice_enabled_config() -> ExperimentConfig {
        let mut config = step_five_config();
        config.voice.enabled = true;
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
            ..AgentsConfig::default()
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

    async fn create_direct_participant(router: Router, _name: &str) -> String {
        let (status, response) = json_request(
            router,
            http::Method::POST,
            "/api/participants",
            json!({"source": "direct"}),
        )
        .await;
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

    // Reads the next JSON WebSocket server message without filtering by type.
    async fn read_next_ws_value<S>(socket: &mut S) -> Value
    where
        S: futures_util::Stream<
                Item = Result<TungsteniteMessage, tokio_tungstenite::tungstenite::Error>,
            > + Unpin,
    {
        loop {
            let message = tokio::time::timeout(Duration::from_secs(2), socket.next())
                .await
                .expect("timed out waiting for WebSocket message")
                .expect("WebSocket closed before next message")
                .expect("WebSocket read failed");
            let TungsteniteMessage::Text(text) = message else {
                continue;
            };
            return serde_json::from_str(&text).unwrap();
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

    // Requests one participant-bound audio plan and verifies that it is enabled.
    async fn request_audio_plan(router: Router, room_id: &str, participant_id: &str) -> Value {
        let (status, plan) = json_request(
            router,
            http::Method::POST,
            &format!("/api/rooms/{room_id}/audio-session"),
            json!({"participant_session_id": participant_id}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(plan["enabled"], true);
        plan
    }

    // Reads past control messages until the next binary audio frame arrives.
    async fn read_audio_binary<S>(socket: &mut S) -> Vec<u8>
    where
        S: futures_util::Stream<
                Item = Result<TungsteniteMessage, tokio_tungstenite::tungstenite::Error>,
            > + Unpin,
    {
        loop {
            let message = tokio::time::timeout(Duration::from_secs(2), socket.next())
                .await
                .expect("timed out waiting for binary audio")
                .expect("audio WebSocket closed before binary audio")
                .expect("audio WebSocket read failed");
            if let TungsteniteMessage::Binary(bytes) = message {
                return bytes.to_vec();
            }
        }
    }

    // Waits until an audio socket reports its initial transcription state.
    async fn wait_for_audio_control<S>(socket: &mut S)
    where
        S: futures_util::Stream<
                Item = Result<TungsteniteMessage, tokio_tungstenite::tungstenite::Error>,
            > + Unpin,
    {
        loop {
            let message = tokio::time::timeout(Duration::from_secs(2), socket.next())
                .await
                .expect("timed out waiting for audio control state")
                .expect("audio WebSocket closed before control state")
                .expect("audio WebSocket read failed");
            if matches!(message, TungsteniteMessage::Text(_)) {
                return;
            }
        }
    }

    // Verifies that no binary audio reaches a socket during a short isolation window.
    async fn assert_no_audio_binary<S>(socket: &mut S)
    where
        S: futures_util::Stream<
                Item = Result<TungsteniteMessage, tokio_tungstenite::tungstenite::Error>,
            > + Unpin,
    {
        let received = tokio::time::timeout(Duration::from_millis(200), async {
            loop {
                let Some(message) = socket.next().await else {
                    return false;
                };
                match message {
                    Ok(TungsteniteMessage::Binary(_)) => return true,
                    Ok(_) => continue,
                    Err(_) => return false,
                }
            }
        })
        .await;
        assert!(!matches!(received, Ok(true)), "audio leaked across rooms");
    }

    // Creates one human-vs-agent waiting room and returns the human and room ids.
    async fn create_human_vs_agent_room(router: Router, name: &str) -> (String, String) {
        let human = create_direct_participant(router.clone(), name).await;
        consent_participant(router.clone(), &human).await;
        let (status, created) = json_request(
            router,
            http::Method::POST,
            "/api/rooms",
            json!({"participant_session_id": human}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(created["role"], "A");
        (
            created["participant_session_id"]
                .as_str()
                .unwrap()
                .to_string(),
            created["room_id"].as_str().unwrap().to_string(),
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
        config.study.institution = "Test University".to_string();
        config.voice.enabled = true;
        config.transcription.enabled = true;
        config.speechmatics.api_key = "test-key".to_string();
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
        assert_eq!(public_config["institution"], "Test University");
        assert_eq!(public_config["consents"][0]["id"], "study");
        assert_eq!(public_config["voice"]["enabled"], true);
        assert_eq!(public_config["voice"]["transport"], "websocket");
        assert_eq!(public_config["transcription"]["enabled"], true);
        assert_eq!(public_config["tts"]["voice_name"], "Agent Voice");
        assert_eq!(public_config["conversation"]["enabled"], true);
        assert_eq!(public_config["agents"]["mode"], "human_vs_human");
        assert_eq!(public_config["agents"]["human_vs_agent"], false);
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
    async fn audio_session_is_disabled_when_voice_is_disabled() {
        let router = build_router(TinyAdapter, step_five_config(), ServeOptions::default())
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
        assert_eq!(audio["enabled"], false);
        assert!(audio["token"].is_null());
    }

    #[tokio::test]
    async fn enabled_audio_session_returns_parlando_websocket_contract() {
        let router = build_router(TinyAdapter, voice_enabled_config(), ServeOptions::default())
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
        assert_eq!(audio["protocol_version"], 1);
        assert_eq!(audio["sample_rate_hz"], 24000);
        assert!(audio["websocket_url"]
            .as_str()
            .unwrap()
            .ends_with(&format!("/ws/audio/{room_id}")));
        assert!(audio["token"]
            .as_str()
            .is_some_and(|value| !value.is_empty()));
    }

    /// Deterministic provider that finalizes the first received audio frame twice.
    struct DuplicateFinalTranscriptionProvider;

    #[async_trait]
    impl TranscriptionProvider for DuplicateFinalTranscriptionProvider {
        /// Starts a test session whose duplicate finals exercise idempotent persistence.
        async fn start_session(
            &self,
            _context: TranscriptionSessionContext,
        ) -> Result<crate::transcription::TranscriptionSessionHandle> {
            let (input, mut inputs) = mpsc::channel(4);
            let (events, event_receiver) = mpsc::channel(4);
            tokio::spawn(async move {
                let _ = events.send(TranscriptionEvent::Ready).await;
                while let Some(message) = inputs.recv().await {
                    if matches!(message, TranscriptionInput::Audio(_)) {
                        let utterance = FinalTranscriptUtterance {
                            start_time_ms: 0,
                            end_time_ms: 20,
                            text: "relay transcript".to_string(),
                            result_ids: vec!["stable-result".to_string()],
                        };
                        let _ = events
                            .send(TranscriptionEvent::FinalUtterance(utterance.clone()))
                            .await;
                        let _ = events
                            .send(TranscriptionEvent::FinalUtterance(utterance))
                            .await;
                        break;
                    }
                }
            });
            Ok(crate::transcription::TranscriptionSessionHandle {
                input,
                events: event_receiver,
            })
        }
    }

    #[tokio::test]
    async fn audio_websocket_relays_pcm_and_commits_one_final_utterance() {
        let mut config = voice_enabled_config();
        config.transcription.enabled = true;
        config.speechmatics.api_key = "server-only-test-key".to_string();
        let router = build_router(
            TinyAdapter,
            config,
            ServeOptions {
                transcription_provider: Some(Arc::new(DuplicateFinalTranscriptionProvider)),
                ..ServeOptions::default()
            },
        )
        .await
        .unwrap();
        let (a, b, room_id) = create_joined_room(router.clone()).await;
        let (_, plan_a) = json_request(
            router.clone(),
            http::Method::POST,
            &format!("/api/rooms/{room_id}/audio-session"),
            json!({"participant_session_id":a}),
        )
        .await;
        let (_, plan_b) = json_request(
            router.clone(),
            http::Method::POST,
            &format!("/api/rooms/{room_id}/audio-session"),
            json!({"participant_session_id":b}),
        )
        .await;
        let (base_url, server) = spawn_test_server(router.clone()).await;
        let host = base_url.trim_start_matches("http://");
        let (mut socket_b, _) = connect_async(format!(
            "ws://{host}/ws/audio/{room_id}?token={}",
            plan_b["token"].as_str().unwrap()
        ))
        .await
        .unwrap();
        let (mut socket_a, _) = connect_async(format!(
            "ws://{host}/ws/audio/{room_id}?token={}",
            plan_a["token"].as_str().unwrap()
        ))
        .await
        .unwrap();
        let frame = AudioFrame {
            sequence: 0,
            timestamp_ms: 0,
            pcm: vec![0; crate::audio::AUDIO_FRAME_BYTES],
        }
        .encode();
        socket_a
            .send(TungsteniteMessage::Binary(frame.clone()))
            .await
            .unwrap();
        loop {
            let message = tokio::time::timeout(Duration::from_secs(2), socket_b.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            if matches!(message, TungsteniteMessage::Binary(bytes) if bytes == frame) {
                break;
            }
        }
        let export = wait_for_export_event(router, "transcript_segment").await;
        assert_eq!(
            export["session_events"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|event| event["event_type"] == "transcript_segment")
                .count(),
            1
        );
        server.abort();
    }

    /// Proves that two simultaneous player pairs receive audio only inside their own rooms.
    #[tokio::test]
    async fn audio_websockets_isolate_two_simultaneous_player_pairs() {
        let router = build_router(TinyAdapter, voice_enabled_config(), ServeOptions::default())
            .await
            .unwrap();
        let (a_one, b_one, room_one) = create_joined_room(router.clone()).await;
        let (a_two, b_two, room_two) = create_joined_room(router.clone()).await;
        assert_ne!(room_one, room_two);

        let plan_a_one = request_audio_plan(router.clone(), &room_one, &a_one).await;
        let plan_b_one = request_audio_plan(router.clone(), &room_one, &b_one).await;
        let plan_a_two = request_audio_plan(router.clone(), &room_two, &a_two).await;
        let plan_b_two = request_audio_plan(router.clone(), &room_two, &b_two).await;
        let tokens = [
            plan_a_one["token"].as_str().unwrap(),
            plan_b_one["token"].as_str().unwrap(),
            plan_a_two["token"].as_str().unwrap(),
            plan_b_two["token"].as_str().unwrap(),
        ];
        for (index, token) in tokens.iter().enumerate() {
            assert!(tokens[..index].iter().all(|existing| existing != token));
        }

        let (base_url, server) = spawn_test_server(router).await;
        let host = base_url.trim_start_matches("http://");
        let connect = |room_id: &str, token: &str| {
            connect_async(format!("ws://{host}/ws/audio/{room_id}?token={token}"))
        };
        let (mut socket_a_one, _) = connect(&room_one, tokens[0]).await.unwrap();
        let (mut socket_b_one, _) = connect(&room_one, tokens[1]).await.unwrap();
        let (mut socket_a_two, _) = connect(&room_two, tokens[2]).await.unwrap();
        let (mut socket_b_two, _) = connect(&room_two, tokens[3]).await.unwrap();

        let frame_one = AudioFrame {
            sequence: 11,
            timestamp_ms: 220,
            pcm: vec![1; crate::audio::AUDIO_FRAME_BYTES],
        }
        .encode();
        let frame_two = AudioFrame {
            sequence: 22,
            timestamp_ms: 440,
            pcm: vec![2; crate::audio::AUDIO_FRAME_BYTES],
        }
        .encode();
        socket_a_one
            .send(TungsteniteMessage::Binary(frame_one.clone()))
            .await
            .unwrap();
        assert_eq!(read_audio_binary(&mut socket_b_one).await, frame_one);
        assert_no_audio_binary(&mut socket_a_two).await;
        assert_no_audio_binary(&mut socket_b_two).await;

        socket_a_two
            .send(TungsteniteMessage::Binary(frame_two.clone()))
            .await
            .unwrap();
        assert_eq!(read_audio_binary(&mut socket_b_two).await, frame_two);
        assert_no_audio_binary(&mut socket_a_one).await;
        assert_no_audio_binary(&mut socket_b_one).await;

        server.abort();
    }

    /// Ensures a replacement connection cannot leave an older socket injecting room audio.
    #[tokio::test]
    async fn replacement_audio_connection_revokes_the_older_socket_generation() {
        let router = build_router(TinyAdapter, voice_enabled_config(), ServeOptions::default())
            .await
            .unwrap();
        let (a, b, room_id) = create_joined_room(router.clone()).await;
        let old_a = request_audio_plan(router.clone(), &room_id, &a).await;
        let plan_b = request_audio_plan(router.clone(), &room_id, &b).await;
        let new_a = request_audio_plan(router.clone(), &room_id, &a).await;
        let (base_url, server) = spawn_test_server(router).await;
        let host = base_url.trim_start_matches("http://");
        let (mut socket_b, _) = connect_async(format!(
            "ws://{host}/ws/audio/{room_id}?token={}",
            plan_b["token"].as_str().unwrap()
        ))
        .await
        .unwrap();
        let (mut old_socket_a, _) = connect_async(format!(
            "ws://{host}/ws/audio/{room_id}?token={}",
            old_a["token"].as_str().unwrap()
        ))
        .await
        .unwrap();
        wait_for_audio_control(&mut old_socket_a).await;
        let (mut new_socket_a, _) = connect_async(format!(
            "ws://{host}/ws/audio/{room_id}?token={}",
            new_a["token"].as_str().unwrap()
        ))
        .await
        .unwrap();
        wait_for_audio_control(&mut new_socket_a).await;

        let stale_frame = AudioFrame {
            sequence: 1,
            timestamp_ms: 20,
            pcm: vec![3; crate::audio::AUDIO_FRAME_BYTES],
        }
        .encode();
        old_socket_a
            .send(TungsteniteMessage::Binary(stale_frame))
            .await
            .unwrap();
        assert_no_audio_binary(&mut socket_b).await;

        let current_frame = AudioFrame {
            sequence: 2,
            timestamp_ms: 40,
            pcm: vec![4; crate::audio::AUDIO_FRAME_BYTES],
        }
        .encode();
        new_socket_a
            .send(TungsteniteMessage::Binary(current_frame.clone()))
            .await
            .unwrap();
        assert_eq!(read_audio_binary(&mut socket_b).await, current_frame);
        server.abort();
    }

    /// Verifies that audio credentials are single-use and cannot cross room boundaries.
    #[tokio::test]
    async fn audio_tokens_are_single_use_and_room_bound() {
        let router = build_router(TinyAdapter, voice_enabled_config(), ServeOptions::default())
            .await
            .unwrap();
        let (a_one, _b_one, room_one) = create_joined_room(router.clone()).await;
        let (_a_two, _b_two, room_two) = create_joined_room(router.clone()).await;
        let wrong_room_plan = request_audio_plan(router.clone(), &room_one, &a_one).await;
        let reusable_plan = request_audio_plan(router.clone(), &room_one, &a_one).await;
        let (base_url, server) = spawn_test_server(router).await;
        let host = base_url.trim_start_matches("http://");

        let wrong_room = connect_async(format!(
            "ws://{host}/ws/audio/{room_two}?token={}",
            wrong_room_plan["token"].as_str().unwrap()
        ))
        .await;
        assert!(matches!(
            wrong_room,
            Err(tokio_tungstenite::tungstenite::Error::Http(response))
                if response.status() == StatusCode::FORBIDDEN
        ));

        let url = format!(
            "ws://{host}/ws/audio/{room_one}?token={}",
            reusable_plan["token"].as_str().unwrap()
        );
        let (_socket, _) = connect_async(url.clone()).await.unwrap();
        let replay = connect_async(url).await;
        assert!(matches!(
            replay,
            Err(tokio_tungstenite::tungstenite::Error::Http(response))
                if response.status() == StatusCode::FORBIDDEN
        ));
        server.abort();
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
            json!({"source": "direct"}),
        )
        .await;

        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(response["raw"], "Direct mode is disabled.");
    }

    #[tokio::test]
    async fn public_participant_creation_rejects_external_identity_sources() {
        let router = build_router(TinyAdapter, step_five_config(), ServeOptions::default())
            .await
            .unwrap();
        let (status, response) = json_request(
            router.clone(),
            http::Method::POST,
            "/api/participants",
            json!({
                "source": "prolific",
                "external_id": "PROLIFIC-1",
                "metadata": {"cohort": "pilot"}
            }),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(response["raw"]
            .as_str()
            .unwrap()
            .contains("direct participants only"));
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
    async fn two_independent_waiting_room_entries_pair_into_one_room() {
        let router = build_router(TinyAdapter, step_five_config(), ServeOptions::default())
            .await
            .unwrap();
        let first = create_direct_participant(router.clone(), "First tab").await;
        let second = create_direct_participant(router.clone(), "Second tab").await;
        consent_participant(router.clone(), &first).await;
        consent_participant(router.clone(), &second).await;

        let (first_status, first_room) = json_request(
            router.clone(),
            http::Method::POST,
            "/api/rooms",
            json!({"participant_session_id": first}),
        )
        .await;
        let (second_status, second_room) = json_request(
            router.clone(),
            http::Method::POST,
            "/api/rooms",
            json!({"participant_session_id": second}),
        )
        .await;

        assert_eq!(first_status, StatusCode::OK);
        assert_eq!(second_status, StatusCode::OK);
        assert_eq!(first_room["role"], "A");
        assert_eq!(second_room["role"], "B");
        assert_eq!(first_room["room_id"], second_room["room_id"]);
        assert!(first_room["presence"]["A"]
            .get("participantSessionId")
            .is_none());
        assert!(first_room["presence"].get("B").is_none());
        assert!(second_room["presence"]["A"]
            .get("participantSessionId")
            .is_none());
        assert!(second_room["presence"]["B"]
            .get("participantSessionId")
            .is_none());

        let (export_status, export) = json_request(
            router,
            http::Method::GET,
            "/api/admin/export?variant=full",
            Value::Null,
        )
        .await;
        assert_eq!(export_status, StatusCode::OK);
        assert_eq!(export["sessions"].as_array().unwrap().len(), 1);
        assert_eq!(export["session_participants"].as_array().unwrap().len(), 2);
        assert!(export["session_participants"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["participant_session_id"] == first && row["role"] == "A"));
        assert!(export["session_participants"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["participant_session_id"] == second && row["role"] == "B"));
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

        let (export_status, export) = json_request(
            router,
            http::Method::GET,
            "/api/admin/export?variant=full",
            Value::Null,
        )
        .await;
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
        assert_eq!(created["role"], "A");
        let room_id = created["room_id"].as_str().unwrap().to_string();
        let (_, joined) = json_request(
            router.clone(),
            http::Method::POST,
            &format!("/api/rooms/{room_id}/join"),
            json!({"participant_session_id": b}),
        )
        .await;
        assert_eq!(joined["role"], "B");

        let (export_status, export) = json_request(
            router,
            http::Method::GET,
            "/api/admin/export?variant=full",
            Value::Null,
        )
        .await;
        assert_eq!(export_status, StatusCode::OK);
        assert_eq!(export["sessions"].as_array().unwrap().len(), 1);
        assert_eq!(export["sessions"][0]["room_id"], room_id);
        assert_eq!(export["session_participants"].as_array().unwrap().len(), 2);
        assert!(export["session_participants"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["participant_session_id"] == a && row["role"] == "A"));
        assert!(export["session_participants"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["participant_session_id"] == b && row["role"] == "B"));
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
        let (base_url, server) = spawn_test_server(router.clone()).await;
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
        let (base_url, server) = spawn_test_server(router.clone()).await;
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
        assert_eq!(completed_a["summary"]["outcome"], "success");
        assert_eq!(completed_a["summary"]["dyad_score"], 10);
        assert_eq!(completed_a["summary"]["player_scores"]["A"], 6);
        assert_eq!(completed_a["summary"]["player_scores"]["B"], 4);
        assert_eq!(completed_b["summary"]["done"], true);
        assert_eq!(completed_b["summary"], completed_a["summary"]);

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
    async fn loss_completion_summary_is_broadcast_and_exported() {
        let router = build_router(
            LossSummaryAdapter,
            step_five_config(),
            ServeOptions::default(),
        )
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
            json!({"type": "submitAction", "action": {"finish": true}}),
        )
        .await;
        let completed = read_ws_type(&mut socket_a, "completed").await;
        assert_eq!(completed["summary"]["done"], true);
        assert_eq!(completed["summary"]["outcome"], "loss");
        assert_eq!(completed["summary"]["dyad_score"], 0);
        assert_eq!(completed["summary"]["player_scores"]["A"], 0);
        assert_eq!(completed["summary"]["player_scores"]["B"], 0);

        let (export_status, export) =
            json_request(router, http::Method::GET, "/api/admin/export", Value::Null).await;
        assert_eq!(export_status, StatusCode::OK);
        let completed_events = export["session_events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|event| event["event_type"] == "session_completed")
            .collect::<Vec<_>>();
        assert_eq!(completed_events.len(), 1);
        assert_eq!(export["sessions"][0]["completion"]["outcome"], "loss");
        server.abort();
    }

    #[tokio::test]
    async fn completed_rooms_reject_late_game_channel_input() {
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
            json!({"type": "submitAction", "action": {"finish": true}}),
        )
        .await;
        let _completed_a = read_ws_type(&mut socket_a, "completed").await;

        send_ws_json(
            &mut socket_a,
            json!({"type": "submitAction", "action": {"finish": false}}),
        )
        .await;
        let action_error = read_ws_type(&mut socket_a, "error").await;
        assert!(action_error["message"]
            .as_str()
            .unwrap()
            .contains("no longer accepts game messages"));

        send_ws_json(
            &mut socket_a,
            json!({"type": "sendChatMessage", "text": "late hello"}),
        )
        .await;
        let chat_error = read_ws_type(&mut socket_a, "error").await;
        assert!(chat_error["message"]
            .as_str()
            .unwrap()
            .contains("no longer accepts game messages"));
        assert_no_ws_type(&mut socket_b, "conversationMessageAdded").await;

        let (transcript_status, transcript_response) = json_request(
            router.clone(),
            http::Method::POST,
            &format!("/api/rooms/{room_id}/transcripts"),
            json!({
                "participant_session_id": a,
                "player": "A",
                "text": "late transcript",
                "metadata": {}
            }),
        )
        .await;
        assert_eq!(transcript_status, StatusCode::NOT_FOUND);
        assert!(transcript_response["raw"].as_str().is_none());

        let (export_status, export) =
            json_request(router, http::Method::GET, "/api/admin/export", Value::Null).await;
        assert_eq!(export_status, StatusCode::OK);
        let events = export["session_events"].as_array().unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| event["event_type"] == "session_completed")
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| event["event_type"] == "conversation_message")
                .count(),
            0
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| event["event_type"] == "transcript_segment")
                .count(),
            0
        );
        server.abort();
    }

    #[tokio::test]
    async fn transcript_endpoints_are_private_and_diagnostics_persist() {
        let (mut config, _temp) = sqlite_config();
        config.privacy.store_voice_diagnostics = true;
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
        assert_eq!(transcript_get_status, StatusCode::NOT_FOUND);
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

        let (transcript_status, _transcript) = json_request(
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
        assert_eq!(transcript_status, StatusCode::NOT_FOUND);

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

        let (export_status, export) = json_request(
            router,
            http::Method::GET,
            "/api/admin/export?variant=full",
            Value::Null,
        )
        .await;
        assert_eq!(export_status, StatusCode::OK);
        let events = export["session_events"].as_array().unwrap();
        assert!(!events
            .iter()
            .any(|event| event["event_type"] == "transcript_segment"));
        assert!(events
            .iter()
            .any(|event| event["event_type"] == "voice_diagnostic"));
    }

    #[tokio::test]
    async fn admin_games_api_reads_actions_from_database() {
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
        assert_eq!(transcript_status, StatusCode::NOT_FOUND);

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
        assert!(poll["events"].as_array().unwrap().is_empty());
        server.abort();
    }

    #[tokio::test]
    async fn human_vs_agent_direct_room_supplies_agent_role_b_immediately() {
        let router = build_router(
            TinyAdapter,
            human_vs_agent_config(),
            ServeOptions {
                agent_factory: Some(Arc::new(NoopAgentFactory)),
                ..ServeOptions::default()
            },
        )
        .await
        .unwrap();
        let human = create_direct_participant(router.clone(), "Human").await;
        consent_participant(router.clone(), &human).await;

        let (status, created) = json_request(
            router.clone(),
            http::Method::POST,
            "/api/rooms",
            json!({"participant_session_id": human}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(created["role"], "A");
        assert_eq!(created["presence"]["B"]["connected"], true);
        assert_eq!(created["presence"]["B"]["audioReady"], true);

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
    async fn agent_runtime_observes_messages_with_speaker_and_modality() {
        let observations = Arc::new(Mutex::new(Vec::new()));
        let router = build_router(
            TinyAdapter,
            human_vs_agent_config(),
            ServeOptions {
                agent_factory: Some(Arc::new(RecordingObservationsAgentFactory {
                    observations: observations.clone(),
                })),
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
        let _ = wait_for_export_event(router.clone(), "agent_started").await;

        send_ws_json(
            &mut socket,
            json!({"type": "sendChatMessage", "text": "typed hello"}),
        )
        .await;
        let _ = read_ws_type(&mut socket, "conversationMessageAdded").await;
        let (status, _) = json_request(
            router,
            http::Method::POST,
            &format!("/api/rooms/{room_id}/transcripts"),
            json!({
                "participant_session_id": human,
                "player": "A",
                "start_time_ms": 0,
                "end_time_ms": 10,
                "text": "spoken hello",
                "metadata": {}
            }),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        for _ in 0..20 {
            let captured = observations.lock().unwrap().clone();
            if captured.contains(&"message:A:Typed:typed hello".to_string()) {
                server.abort();
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        server.abort();
        panic!("expected agent to observe typed messages");
    }

    #[tokio::test]
    async fn agent_runtime_observes_actions_with_resulting_observation() {
        let observations = Arc::new(Mutex::new(Vec::new()));
        let router = build_router(
            TinyAdapter,
            human_vs_agent_config(),
            ServeOptions {
                agent_factory: Some(Arc::new(RecordingObservationsAgentFactory {
                    observations: observations.clone(),
                })),
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
        let _ = wait_for_export_event(router.clone(), "agent_started").await;

        send_ws_json(
            &mut socket,
            json!({"type": "submitAction", "action": {"finish": false}}),
        )
        .await;
        let _ = read_ws_type(&mut socket, "stateChanged").await;

        for _ in 0..20 {
            let captured = observations.lock().unwrap().clone();
            if captured.contains(&"action:A:false:false".to_string()) {
                server.abort();
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        server.abort();
        panic!("expected agent to observe accepted action");
    }

    #[tokio::test]
    async fn agent_runtime_persists_messages_and_validated_actions() {
        let factory = Arc::new(ScriptedAgentFactory::new(vec![vec![
            scripted_response(
                Some("agent says hello"),
                Some(TinyAction {
                    finish: false,
                    invalid: false,
                }),
            ),
            scripted_response(
                None,
                Some(TinyAction {
                    finish: true,
                    invalid: false,
                }),
            ),
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
        let first_update = read_next_ws_value(&mut socket).await;
        assert_eq!(first_update["type"], "stateChanged");
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
    async fn agent_runtime_observes_accepted_action_before_next_decision() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let router = build_router(
            TinyAdapter,
            human_vs_agent_config(),
            ServeOptions {
                agent_factory: Some(Arc::new(SequencedDecisionAgentFactory { log: log.clone() })),
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
        let _ = read_ws_type(&mut socket, "roleAssigned").await;

        for _ in 0..20 {
            let snapshot = log.lock().unwrap().clone();
            if snapshot.iter().any(|entry| entry == "maybe_act:2") {
                assert_eq!(
                    snapshot,
                    vec!["maybe_act:1", "observe_action:B", "maybe_act:2"]
                );
                server.abort();
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        server.abort();
        panic!("timed out waiting for the second agent decision");
    }

    #[tokio::test]
    async fn agent_runtime_stops_invalid_agents_cleanly() {
        let factory = Arc::new(ScriptedAgentFactory::new(vec![vec![
            scripted_response(
                None,
                Some(TinyAction {
                    finish: false,
                    invalid: true,
                }),
            ),
            scripted_response(
                None,
                Some(TinyAction {
                    finish: false,
                    invalid: true,
                }),
            ),
            scripted_response(
                None,
                Some(TinyAction {
                    finish: false,
                    invalid: false,
                }),
            ),
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
    async fn agent_runtime_rejects_empty_responses() {
        let empty_response = Some(AgentResponse {
            message: None,
            action: None,
        });
        let factory = Arc::new(ScriptedAgentFactory::new(vec![vec![
            empty_response.clone(),
            empty_response,
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
        assert!(export["session_events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| {
                event["event_type"] == "agent_error"
                    && event["payload"]["last_error"]
                        .as_str()
                        .unwrap()
                        .contains("empty response")
            }));
        server.abort();
    }

    #[tokio::test]
    async fn agent_tts_records_diagnostics_for_agent_messages() {
        let factory = Arc::new(ScriptedAgentFactory::new(vec![vec![scripted_response(
            Some("speak this"),
            None,
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
            scripted_response(Some("first fails"), None),
            scripted_response(Some("second succeeds"), None),
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

    /// Proves sentinel provider credentials cannot cross the durable configuration boundary.
    #[test]
    fn persisted_configuration_redacts_provider_credentials() {
        let mut config = step_five_config();
        config.speechmatics.api_key = "speechmatics-sentinel-secret".to_string();
        config.tts.api_key = "elevenlabs-sentinel-secret".to_string();

        let serialized = serde_json::to_string(&persistable_config_json(&config).unwrap()).unwrap();

        assert!(!serialized.contains("speechmatics-sentinel-secret"));
        assert!(!serialized.contains("elevenlabs-sentinel-secret"));
    }
}
