//! Standalone loopback emulator for the narrow Prolific contract consumed by Parlando.

use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::Arc,
};

use anyhow::{Context, Result};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::Utc;
use clap::Parser;
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use rand::rngs::OsRng;
use rsa::{
    pkcs8::{EncodePrivateKey, LineEnding},
    traits::PublicKeyParts,
    RsaPrivateKey,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::RwLock;

const DEFAULT_TOKEN: &str = "prolific-test-token";
const DEFAULT_WORKSPACE_ID: &str = "workspace1";
const DEFAULT_PROJECT_ID: &str = "project1";
const DEFAULT_STUDY_ID: &str = "study1";
const SIGNING_KEY_ID: &str = "parlando-prolific-mock-key";

/// Command-line options for one independent mock process.
#[derive(Debug, Parser)]
#[command(about = "Standalone Prolific API and Secure external URL emulator")]
struct Args {
    /// Loopback address on which the emulator listens.
    #[arg(long, default_value_t = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 4101))]
    bind: SocketAddr,
}

/// Shared mutable provider facts and immutable signing material.
#[derive(Clone)]
struct MockState {
    data: Arc<RwLock<MockData>>,
    signing: Arc<SigningMaterial>,
}

/// Provider-owned records and the observable request journal.
struct MockData {
    token: String,
    workspace: Value,
    project: Value,
    study: Value,
    submissions: HashMap<String, Value>,
    faults: HashMap<String, u16>,
    requests: Vec<RequestRecord>,
}

/// One RSA signing identity exposed through the emulated JWKS endpoint.
struct SigningMaterial {
    encoding_key: EncodingKey,
    jwk: Value,
}

/// Redacted evidence for one provider endpoint call.
#[derive(Clone, Debug, Serialize)]
struct RequestRecord {
    sequence: usize,
    method: String,
    path: String,
    authorized: bool,
}

/// Complete mock-study replacement accepted by the control endpoint.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigureRequest {
    external_study_url: String,
    #[serde(default = "default_true")]
    secure: bool,
    #[serde(default)]
    action_overrides: HashMap<String, String>,
}

/// One authoritative submission inserted through the control endpoint.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SubmissionRequest {
    id: String,
    participant: String,
    #[serde(default = "default_study_id")]
    study_id: String,
    #[serde(default = "default_submission_status")]
    status: String,
    entered_code: Option<String>,
    return_requested: Option<String>,
}

/// Launch values signed through the mock's private control endpoint.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SignRequest {
    participant_id: String,
    #[serde(default = "default_study_id")]
    study_id: String,
    session_id: String,
    #[serde(default = "default_workspace_id")]
    workspace_id: String,
    audience: String,
    #[serde(default = "default_token_lifetime")]
    lifetime_seconds: i64,
}

/// Serialized RS256 launch claims matching Prolific's documented payload names.
#[derive(Debug, Serialize)]
struct LaunchClaims {
    iss: String,
    aud: String,
    sub: String,
    iat: i64,
    exp: i64,
    prolific: Value,
}

/// One path-specific response override used for fault scenarios.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FaultRequest {
    path: String,
    status: u16,
}

/// Reports the configured origin and stable fixture identifiers at startup.
#[derive(Debug, Serialize)]
struct ReadyResponse {
    status: &'static str,
    base_url: String,
    workspace_id: &'static str,
    project_id: &'static str,
    study_id: &'static str,
}

/// Starts the standalone mock without linking its lifecycle to a test runner.
#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    if !args.bind.ip().is_loopback() {
        anyhow::bail!("prolific-mock may listen only on a loopback address");
    }
    let signing = Arc::new(generate_signing_material()?);
    let state = MockState {
        data: Arc::new(RwLock::new(default_data())),
        signing,
    };
    let app = Router::new()
        .route("/__mock/health", get(mock_health))
        .route("/__mock/configure", post(configure))
        .route("/__mock/submissions", post(add_submission))
        .route("/__mock/sign", post(sign_launch))
        .route("/__mock/fault", post(set_fault))
        .route("/__mock/reset", post(reset))
        .route("/__mock/requests", get(requests))
        .route("/.well-known/study/jwks.json", get(jwks))
        .route("/api/v1/workspaces/:id/", get(workspace))
        .route("/api/v1/projects/:id/", get(project))
        .route("/api/v1/studies/:id/", get(study))
        .route("/api/v1/submissions/:id/", get(submission))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(args.bind).await?;
    let address = listener.local_addr()?;
    println!(
        "{}",
        serde_json::to_string(&ReadyResponse {
            status: "ready",
            base_url: format!("http://{address}"),
            workspace_id: DEFAULT_WORKSPACE_ID,
            project_id: DEFAULT_PROJECT_ID,
            study_id: DEFAULT_STUDY_ID,
        })?
    );
    axum::serve(listener, app).await?;
    Ok(())
}

/// Produces independent mock records rather than importing Parlando's adapter types.
fn default_data() -> MockData {
    MockData {
        token: DEFAULT_TOKEN.to_string(),
        workspace: json!({"id": DEFAULT_WORKSPACE_ID, "title": "Parlando test workspace"}),
        project: json!({"id": DEFAULT_PROJECT_ID, "workspace": DEFAULT_WORKSPACE_ID}),
        study: study_value("http://127.0.0.1/replace-me", true, &HashMap::new()),
        submissions: HashMap::new(),
        faults: HashMap::new(),
        requests: Vec::new(),
    }
}

/// Creates a fresh RSA key and its corresponding public JWK for this process.
fn generate_signing_material() -> Result<SigningMaterial> {
    let private = RsaPrivateKey::new(&mut OsRng, 2048)?;
    let pem = private
        .to_pkcs8_pem(LineEnding::LF)
        .context("could not encode mock signing key")?;
    let encoding_key = EncodingKey::from_rsa_pem(pem.as_bytes())?;
    let public = private.to_public_key();
    let jwk = json!({
        "kty": "RSA",
        "use": "sig",
        "kid": SIGNING_KEY_ID,
        "alg": "RS256",
        "n": URL_SAFE_NO_PAD.encode(public.n().to_bytes_be()),
        "e": URL_SAFE_NO_PAD.encode(public.e().to_bytes_be()),
    });
    Ok(SigningMaterial { encoding_key, jwk })
}

/// Builds the exact study fields currently consumed by Parlando preflight.
fn study_value(external_url: &str, secure: bool, overrides: &HashMap<String, String>) -> Value {
    let paths = [
        ("completed", "DONECODE", "AUTOMATICALLY_APPROVE"),
        ("partner_left", "PARTNERCODE", "AUTOMATICALLY_APPROVE"),
        ("partner_unavailable", "UNMATCHEDCODE", "REQUEST_RETURN"),
        ("timed_out", "TIMEOUTCODE", "REQUEST_RETURN"),
        (
            "technical_failure",
            "TECHNICALCODE",
            "AUTOMATICALLY_APPROVE",
        ),
    ];
    let completion_codes = paths
        .into_iter()
        .map(|(name, code, action)| {
            json!({
                "code": code,
                "code_type": "OTHER",
                "actions": [{"action": overrides.get(name).map(String::as_str).unwrap_or(action)}]
            })
        })
        .collect::<Vec<_>>();
    json!({
        "id": DEFAULT_STUDY_ID,
        "external_study_url": external_url,
        "prolific_id_option": "url_parameters",
        "completion_codes": completion_codes,
        "estimated_completion_time": 20,
        "maximum_allowed_time": 240.0,
        "is_external_study_url_secure": secure,
        "device_compatibility": ["desktop"],
        "peripheral_requirements": [],
        "project": DEFAULT_PROJECT_ID,
        "status": "UNPUBLISHED"
    })
}

/// Returns true for omitted secure-mode settings.
fn default_true() -> bool {
    true
}

/// Returns the stable fixture study id for control requests.
fn default_study_id() -> String {
    DEFAULT_STUDY_ID.to_string()
}

/// Returns the stable fixture workspace id for signed launches.
fn default_workspace_id() -> String {
    DEFAULT_WORKSPACE_ID.to_string()
}

/// Returns a provider-like active submission status.
fn default_submission_status() -> String {
    "ACTIVE".to_string()
}

/// Keeps generated launch tokens short-lived while avoiding timing flakes.
fn default_token_lifetime() -> i64 {
    300
}

/// Exposes readiness without mutating or journaling provider state.
async fn mock_health() -> Json<Value> {
    Json(json!({"status": "ok"}))
}

/// Replaces the study contract for one deterministic scenario.
async fn configure(
    State(state): State<MockState>,
    Json(request): Json<ConfigureRequest>,
) -> Json<Value> {
    state.data.write().await.study = study_value(
        &request.external_study_url,
        request.secure,
        &request.action_overrides,
    );
    Json(json!({"ok": true}))
}

/// Inserts or replaces one authoritative submission fixture.
async fn add_submission(
    State(state): State<MockState>,
    Json(request): Json<SubmissionRequest>,
) -> Json<Value> {
    let value = json!({
        "id": request.id,
        "participant": request.participant,
        "study_id": request.study_id,
        "status": request.status,
        "entered_code": request.entered_code,
        "return_requested": request.return_requested,
    });
    let id = value["id"].as_str().unwrap_or_default().to_string();
    state.data.write().await.submissions.insert(id, value);
    Json(json!({"ok": true}))
}

/// Signs a realistic Secure external URL token with the mock's private key.
async fn sign_launch(
    State(state): State<MockState>,
    Json(request): Json<SignRequest>,
) -> Result<Json<Value>, MockError> {
    let now = Utc::now().timestamp();
    let claims = LaunchClaims {
        iss: "https://www.prolific.com".to_string(),
        aud: request.audience,
        sub: request.session_id.clone(),
        iat: now,
        exp: now + request.lifetime_seconds,
        prolific: json!({
            "PROLIFIC_PID": request.participant_id,
            "STUDY_ID": request.study_id,
            "SESSION_ID": request.session_id,
            "workspace_id": request.workspace_id,
            "organisation_id": "organisation1"
        }),
    };
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(SIGNING_KEY_ID.to_string());
    let token = encode(&header, &claims, &state.signing.encoding_key)
        .context("could not sign launch token")?;
    Ok(Json(json!({"token": token})))
}

/// Sets one persistent provider response fault by exact request path.
async fn set_fault(
    State(state): State<MockState>,
    Json(request): Json<FaultRequest>,
) -> Result<Json<Value>, MockError> {
    StatusCode::from_u16(request.status).context("invalid HTTP fault status")?;
    state
        .data
        .write()
        .await
        .faults
        .insert(request.path, request.status);
    Ok(Json(json!({"ok": true})))
}

/// Clears mutable submissions, faults, and request evidence while retaining signing identity.
async fn reset(State(state): State<MockState>) -> Json<Value> {
    let mut data = state.data.write().await;
    let external_url = data.study["external_study_url"]
        .as_str()
        .unwrap_or("http://127.0.0.1/replace-me")
        .to_string();
    *data = default_data();
    data.study = study_value(&external_url, true, &HashMap::new());
    Json(json!({"ok": true}))
}

/// Returns the complete redacted provider request journal.
async fn requests(State(state): State<MockState>) -> Json<Value> {
    Json(json!({"requests": state.data.read().await.requests}))
}

/// Returns the active signing key without requiring provider authentication.
async fn jwks(State(state): State<MockState>, headers: HeaderMap) -> Response {
    let path = "/.well-known/study/jwks.json";
    if let Some(response) = journal_and_fault(&state, &headers, "GET", path, false).await {
        return response;
    }
    Json(json!({"keys": [state.signing.jwk.clone()]})).into_response()
}

/// Returns the configured workspace after enforcing token authentication.
async fn workspace(
    State(state): State<MockState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    provider_record(
        &state,
        &headers,
        &format!("/api/v1/workspaces/{id}/"),
        "workspace",
        &id,
    )
    .await
}

/// Returns the configured project after enforcing token authentication.
async fn project(
    State(state): State<MockState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    provider_record(
        &state,
        &headers,
        &format!("/api/v1/projects/{id}/"),
        "project",
        &id,
    )
    .await
}

/// Returns the configured study after enforcing token authentication.
async fn study(
    State(state): State<MockState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    provider_record(
        &state,
        &headers,
        &format!("/api/v1/studies/{id}/"),
        "study",
        &id,
    )
    .await
}

/// Returns one configured submission after enforcing token authentication.
async fn submission(
    State(state): State<MockState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let path = format!("/api/v1/submissions/{id}/");
    if let Some(response) = journal_and_fault(&state, &headers, "GET", &path, true).await {
        return response;
    }
    state
        .data
        .read()
        .await
        .submissions
        .get(&id)
        .cloned()
        .map(Json)
        .map(IntoResponse::into_response)
        .unwrap_or_else(|| StatusCode::NOT_FOUND.into_response())
}

/// Serves one singleton provider record selected independently by record kind.
async fn provider_record(
    state: &MockState,
    headers: &HeaderMap,
    path: &str,
    kind: &str,
    requested_id: &str,
) -> Response {
    if let Some(response) = journal_and_fault(state, headers, "GET", path, true).await {
        return response;
    }
    let data = state.data.read().await;
    let record = match kind {
        "workspace" => &data.workspace,
        "project" => &data.project,
        "study" => &data.study,
        _ => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    if record["id"] != requested_id {
        return StatusCode::NOT_FOUND.into_response();
    }
    Json(record.clone()).into_response()
}

/// Records one provider call and returns authentication or injected-fault responses.
async fn journal_and_fault(
    state: &MockState,
    headers: &HeaderMap,
    method: &str,
    path: &str,
    require_auth: bool,
) -> Option<Response> {
    let mut data = state.data.write().await;
    let expected = format!("Token {}", data.token);
    let authorized = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == expected);
    let sequence = data.requests.len() + 1;
    data.requests.push(RequestRecord {
        sequence,
        method: method.to_string(),
        path: path.to_string(),
        authorized,
    });
    if require_auth && !authorized {
        return Some(StatusCode::UNAUTHORIZED.into_response());
    }
    data.faults.get(path).copied().map(|status| {
        StatusCode::from_u16(status)
            .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
            .into_response()
    })
}

/// Converts internal control failures into concise HTTP error responses.
struct MockError(anyhow::Error);

impl<E> From<E> for MockError
where
    E: Into<anyhow::Error>,
{
    fn from(error: E) -> Self {
        Self(error.into())
    }
}

impl IntoResponse for MockError {
    /// Keeps control errors observable without exposing process internals.
    fn into_response(self) -> Response {
        (StatusCode::BAD_REQUEST, self.0.to_string()).into_response()
    }
}
