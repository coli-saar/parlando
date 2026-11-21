//! Hosts lab accounts and their operator UI on a dedicated web origin.

use std::{collections::BTreeMap, fs, future::Future, path::Path};

use anyhow::{ensure, Context, Result};
use axum::{
    extract::{DefaultBodyLimit, Path as RoutePath, Request, State},
    http::{
        header::{AUTHORIZATION, CACHE_CONTROL, CONTENT_TYPE, COOKIE, HOST, ORIGIN, SET_COOKIE},
        request::Parts,
        HeaderMap, HeaderValue, Method, StatusCode,
    },
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};

use super::{
    accounts::{
        AccountInputError, AccountStore, DashboardSession, PasswordWorkBusy,
        SESSION_DURATION_SECONDS,
    },
    bad_request, checked_origin, internal, not_found, require_token, validate_id,
    validate_revision, Host, HttpResult, PackageManifest,
};
use std::sync::Arc;

/// Owns the hosted accounts and allows browser authority on one web origin.
pub(super) struct Dashboard {
    pub accounts: AccountStore,
    origin: String,
    authority: String,
    public_authority: String,
    secure: bool,
}

impl Dashboard {
    /// Opens lab accounts only when a dedicated browser origin is configured.
    pub async fn open(store: &Path, public_url: &str, admin_url: &str) -> Result<Self> {
        let origin = checked_origin(admin_url)?;
        let admin = reqwest::Url::parse(&origin)?;
        let public = reqwest::Url::parse(public_url)?;
        ensure!(
            admin.host_str().map(|value| value.trim_end_matches('.'))
                != public.host_str().map(|value| value.trim_end_matches('.')),
            "the administrator origin requires a different hostname from the participant origin"
        );
        let public_authority = authority(public_url)?.to_owned();
        let authority = authority(&origin)?.to_owned();
        Ok(Self {
            accounts: AccountStore::open(store).await?,
            origin,
            authority,
            public_authority,
            secure: admin.scheme() == "https",
        })
    }

    /// Checks the browser-supplied origin before processing credential or state changes.
    fn verify_origin(&self, headers: &HeaderMap) -> HttpResult<()> {
        if headers.get(ORIGIN).and_then(|value| value.to_str().ok()) == Some(self.origin.as_str()) {
            return Ok(());
        }
        Err(forbidden("Invalid administrator origin"))
    }

    /// Uses a host-only cookie, with prefix enforcement on production HTTPS.
    fn cookie_name(&self) -> &'static str {
        if self.secure {
            "__Host-parlando_host"
        } else {
            "parlando_host"
        }
    }

    /// Formats the session or removal cookie using the same attributes.
    fn cookie(&self, token: &str, max_age: u64) -> HttpResult<HeaderValue> {
        let secure = if self.secure { "Secure; " } else { "" };
        HeaderValue::from_str(&format!(
            "{}={token}; Path=/; Max-Age={max_age}; {secure}HttpOnly; SameSite=Strict",
            self.cookie_name()
        ))
        .map_err(internal)
    }

    /// Looks up a session cookie and checks CSRF on methods with side effects.
    async fn browser_session(&self, parts: &Parts) -> HttpResult<DashboardSession> {
        let token = self
            .session_token(&parts.headers)
            .ok_or_else(unauthorized)?;
        let session = self
            .accounts
            .session(token)
            .await
            .map_err(internal)?
            .ok_or_else(unauthorized)?;
        if !matches!(parts.method, Method::GET | Method::HEAD | Method::OPTIONS) {
            self.verify_origin(&parts.headers)?;
            if parts
                .headers
                .get("x-csrf-token")
                .and_then(|value| value.to_str().ok())
                != Some(session.csrf.as_str())
            {
                return Err(forbidden("Invalid CSRF token"));
            }
        }
        Ok(session)
    }

    /// Reads an opaque cookie without treating unrelated cookies as credentials.
    fn session_token<'a>(&self, headers: &'a HeaderMap) -> Option<&'a str> {
        headers
            .get_all(COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .flat_map(|value| value.split(';'))
            .filter_map(|pair| pair.trim().split_once('='))
            .find_map(|(name, token)| (name == self.cookie_name()).then_some(token))
    }
}

/// Mounts management UI and account endpoints with a shared origin check.
pub(super) fn routes() -> Router<Arc<Host>> {
    Router::new()
        .route("/admin", get(page))
        .route("/admin/", get(page))
        .route("/games/:game/admin", get(game_page))
        .route("/admin/dashboard.js", get(javascript))
        .route("/admin/dashboard.css", get(stylesheet))
        .route("/api/hosting/auth/login", post(login))
        .route("/api/hosting/auth/logout", post(logout))
        .route("/api/hosting/auth/session", get(session))
        .route("/api/hosting/dashboard", get(index))
        .route(
            "/api/hosting/accounts",
            get(list_accounts).post(create_account),
        )
        .route("/api/hosting/accounts/:id", put(change_account))
        .route("/api/hosting/audit", get(audit_events))
        .layer(DefaultBodyLimit::max(16 * 1024))
        .layer(middleware::from_fn(response_headers))
}

/// Gets the authority from a validated canonical origin.
fn authority(url: &str) -> Result<&str> {
    url.split_once("://")
        .map(|(_, value)| value)
        .context("missing origin authority")
}

/// Accepts the original trusted proxy Host header or the HTTP/2 request authority.
fn matches_authority(parts: &Parts, expected: &str) -> bool {
    let header = parts
        .headers
        .get(HOST)
        .and_then(|value| value.to_str().ok());
    let uri = parts.uri.authority().map(|value| value.as_str());
    match (header, uri) {
        (Some(header), Some(uri)) => {
            header.eq_ignore_ascii_case(expected) && uri.eq_ignore_ascii_case(expected)
        }
        (Some(value), None) | (None, Some(value)) => value.eq_ignore_ascii_case(expected),
        (None, None) => false,
    }
}

/// Reads the configured surface for a request after validating its hostname.
fn admin<'a>(host: &'a Host, parts: &Parts) -> HttpResult<&'a Dashboard> {
    host.dashboard
        .as_ref()
        .filter(|dashboard| matches_authority(parts, &dashboard.authority))
        .ok_or_else(not_found)
}

/// Keeps uploaded applications off the dashboard hostname.
pub(super) fn participant_origin_allowed(host: &Host, parts: &Parts) -> bool {
    let Some(dashboard) = &host.dashboard else {
        return true;
    };
    matches_authority(parts, &dashboard.public_authority)
}

/// Resolves a CLI bearer token or an individual browser account.
pub(super) async fn authorize(
    host: &Host,
    parts: &Parts,
    expected: Option<&[u8; 32]>,
    machine_actor: impl FnOnce() -> String,
) -> HttpResult<String> {
    if parts.headers.contains_key(AUTHORIZATION) {
        require_token(&parts.headers, expected)?;
        return Ok(machine_actor());
    }
    let dashboard = admin(host, parts).map_err(|_| unauthorized())?;
    let session = dashboard.browser_session(parts).await?;
    Ok(actor_name(&session))
}

/// Formats an audit identity with its durable ID and display name.
fn actor_name(session: &DashboardSession) -> String {
    format!(
        "account:{}:{}",
        session.account.id, session.account.username
    )
}

/// Writes an audit attempt before allowing a protected operation to perform side effects.
async fn audit(
    host: &Host,
    actor: &str,
    operation: &str,
    target: &str,
    outcome: &str,
) -> HttpResult<()> {
    if let Some(dashboard) = &host.dashboard {
        dashboard
            .accounts
            .audit(actor, operation, target, outcome)
            .await
            .map_err(internal)?;
    }
    Ok(())
}

/// Records a control action and its result without persisting its request body.
pub(super) async fn audited<T>(
    host: &Host,
    actor: &str,
    operation: &str,
    target: &str,
    action: impl Future<Output = HttpResult<T>>,
) -> HttpResult<T> {
    audit(host, actor, operation, target, "attempt").await?;
    let result = action.await;
    let outcome = if result.is_ok() { "success" } else { "failed" };
    if let Err(error) = audit(host, actor, operation, target, outcome).await {
        tracing::error!(
            ?error,
            operation,
            "could not record deployment audit outcome"
        );
    }
    result
}

/// Audits experiment API operations whose handler status is returned inside a response.
pub(super) async fn audited_response(
    host: &Host,
    actor: &str,
    operation: &str,
    target: &str,
    action: impl Future<Output = HttpResult<Response>>,
) -> HttpResult<Response> {
    audit(host, actor, operation, target, "attempt").await?;
    let result = action.await;
    let outcome = if result
        .as_ref()
        .is_ok_and(|response| response.status().is_success())
    {
        "success"
    } else {
        "failed"
    };
    if let Err(error) = audit(host, actor, operation, target, outcome).await {
        tracing::error!(
            ?error,
            operation,
            "could not record experiment audit outcome"
        );
    }
    result
}

/// Prevents caching and framing of dashboard UI and account responses.
async fn response_headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    headers.insert("content-security-policy", HeaderValue::from_static(
        "default-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'",
    ));
    response
}

/// Serves the hosted dashboard shell without exposing account information.
async fn page(State(host): State<Arc<Host>>, request: Request) -> HttpResult<Response> {
    asset(
        &host,
        request,
        include_str!("dashboard.html"),
        "text/html; charset=utf-8",
    )
}

/// Serves the same dashboard with the browser scoped to one game.
async fn game_page(
    State(host): State<Arc<Host>>,
    RoutePath(game): RoutePath<String>,
    request: Request,
) -> HttpResult<Response> {
    validate_id(&game).map_err(|_| not_found())?;
    page(State(host), request).await
}

/// Serves the dashboard's own client script.
async fn javascript(State(host): State<Arc<Host>>, request: Request) -> HttpResult<Response> {
    asset(
        &host,
        request,
        include_str!("dashboard.js"),
        "text/javascript; charset=utf-8",
    )
}

/// Serves the dashboard's own stylesheet.
async fn stylesheet(State(host): State<Arc<Host>>, request: Request) -> HttpResult<Response> {
    asset(
        &host,
        request,
        include_str!("dashboard.css"),
        "text/css; charset=utf-8",
    )
}

/// Restricts trusted assets to the configured administrator hostname.
fn asset(
    host: &Host,
    request: Request,
    body: &'static str,
    mime: &'static str,
) -> HttpResult<Response> {
    let (parts, _) = request.into_parts();
    admin(host, &parts)?;
    Ok(([(CONTENT_TYPE, mime)], body).into_response())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SignIn {
    username: String,
    password: String,
}

/// Creates a browser session after checking origin and verifying the password.
async fn login(State(host): State<Arc<Host>>, request: Request) -> HttpResult<Response> {
    let (parts, body) = request.into_parts();
    let dashboard = admin(&host, &parts)?;
    dashboard.verify_origin(&parts.headers)?;
    let Json(input) = parse_json::<SignIn>(parts, body, &host).await?;
    let login = dashboard
        .accounts
        .login(&input.username, input.password)
        .await
        .map_err(account_error)?
        .ok_or_else(unauthorized)?;
    let cookie = dashboard.cookie(&login.token, SESSION_DURATION_SECONDS as u64)?;
    Ok(([(SET_COOKIE, cookie)], Json(session_json(&login.session))).into_response())
}

/// Destroys the current session after authentication and CSRF checking.
async fn logout(State(host): State<Arc<Host>>, request: Request) -> HttpResult<Response> {
    let (parts, _) = request.into_parts();
    let dashboard = admin(&host, &parts)?;
    dashboard.browser_session(&parts).await?;
    let token = dashboard
        .session_token(&parts.headers)
        .ok_or_else(unauthorized)?;
    dashboard.accounts.logout(token).await.map_err(internal)?;
    Ok((
        [(SET_COOKIE, dashboard.cookie("", 0)?)],
        StatusCode::NO_CONTENT,
    )
        .into_response())
}

/// Returns the authenticated identity and its CSRF token to trusted scripts.
async fn session(State(host): State<Arc<Host>>, request: Request) -> HttpResult<Json<Value>> {
    let (parts, _) = request.into_parts();
    let session = admin(&host, &parts)?.browser_session(&parts).await?;
    Ok(Json(session_json(&session)))
}

/// Exposes the identity without returning session credentials or password hashes.
fn session_json(session: &DashboardSession) -> Value {
    json!({"id": session.account.id, "username": session.account.username,
           "role": session.account.role, "csrf_token": session.csrf})
}

/// Lists every game and experiment for a signed-in lab member.
async fn index(State(host): State<Arc<Host>>, request: Request) -> HttpResult<Json<Value>> {
    let (parts, _) = request.into_parts();
    admin(&host, &parts)?.browser_session(&parts).await?;
    let store = host.store.clone();
    let games = tokio::task::spawn_blocking(move || package_index(&store))
        .await
        .map_err(internal)?
        .map_err(internal)?;
    let experiments = {
        let experiments = host.experiments.read().await;
        experiments
            .iter()
            .map(|(id, experiment)| {
                (
                    id.clone(),
                    (experiment.selection.clone(), experiment.state.clone()),
                )
            })
            .collect::<BTreeMap<_, _>>()
    };
    let mut rows = Vec::with_capacity(experiments.len());
    for (id, (selection, state)) in experiments {
        rows.push(
            json!({"id": id, "game": selection.game, "revision": selection.revision,
            "config": selection.config, "status": state.hosted_intake_status().await,
            "participant_url": format!("{}/e/{id}/", host.public_url)}),
        );
    }
    Ok(Json(json!({"games": games, "experiments": rows})))
}

/// Reads public metadata for installed revisions, excluding staging directories.
fn package_index(store: &Path) -> Result<Vec<Value>> {
    let mut games = BTreeMap::new();
    let packages = store.join("packages");
    if !packages.exists() {
        return Ok(Vec::new());
    }
    for game in fs::read_dir(packages)? {
        let game = game?;
        let id = game.file_name().to_string_lossy().into_owned();
        if !game.file_type()?.is_dir() || validate_id(&id).is_err() {
            continue;
        }
        let mut revisions = BTreeMap::new();
        for revision in fs::read_dir(game.path())? {
            let revision = revision?;
            let name = revision.file_name().to_string_lossy().into_owned();
            if !revision.file_type()?.is_dir() || validate_revision(&name).is_err() {
                continue;
            }
            let manifest = PackageManifest::read(&revision.path())?;
            revisions.insert(
                name.clone(),
                json!({"revision":name,"version":manifest.version()}),
            );
        }
        games.insert(
            id.clone(),
            json!({"id":id,"revisions":revisions.into_values().collect::<Vec<_>>()}),
        );
    }
    Ok(games.into_values().collect())
}

/// Requires an operator session or the infrastructure bootstrap token.
async fn operator<'a>(host: &'a Host, parts: &Parts) -> HttpResult<(&'a Dashboard, String)> {
    let dashboard = admin(host, parts)?;
    if parts.headers.contains_key(AUTHORIZATION) {
        require_token(&parts.headers, Some(&host.admin_token))?;
        return Ok((dashboard, "host-operator-token".into()));
    }
    let session = dashboard.browser_session(parts).await?;
    if session.account.role != "operator" {
        return Err(forbidden("Operator role required"));
    }
    Ok((dashboard, actor_name(&session)))
}

/// Returns account status to the operator.
async fn list_accounts(State(host): State<Arc<Host>>, request: Request) -> HttpResult<Json<Value>> {
    let (parts, _) = request.into_parts();
    let (dashboard, _) = operator(&host, &parts).await?;
    Ok(Json(
        json!({"accounts": dashboard.accounts.list().await.map_err(internal)?}),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewAccount {
    username: String,
    password: String,
    role: Option<String>,
}

/// Provisions an individually named experimenter or operator account.
async fn create_account(State(host): State<Arc<Host>>, request: Request) -> HttpResult<Response> {
    let (parts, body) = request.into_parts();
    let (dashboard, actor) = operator(&host, &parts).await?;
    let Json(input) = parse_json::<NewAccount>(parts, body, &host).await?;
    let target = input.username.clone();
    let account = audited(&host, &actor, "create-account", &target, async {
        dashboard
            .accounts
            .create(
                &input.username,
                input.password,
                input.role.as_deref().unwrap_or("experimenter"),
            )
            .await
            .map_err(account_error)
    })
    .await?;
    Ok((StatusCode::CREATED, Json(account)).into_response())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AccountChange {
    password: Option<String>,
    disabled: Option<bool>,
}

/// Resets a password or disables an account, revoking its sessions.
async fn change_account(
    State(host): State<Arc<Host>>,
    RoutePath(id): RoutePath<i64>,
    request: Request,
) -> HttpResult<StatusCode> {
    let (parts, body) = request.into_parts();
    let (dashboard, actor) = operator(&host, &parts).await?;
    let Json(input) = parse_json::<AccountChange>(parts, body, &host).await?;
    let target = id.to_string();
    match (input.password, input.disabled) {
        (Some(password), None) => {
            audited(&host, &actor, "reset-password", &target, async {
                if dashboard
                    .accounts
                    .reset_password(id, password)
                    .await
                    .map_err(account_error)?
                {
                    Ok(())
                } else {
                    Err(not_found())
                }
            })
            .await?
        }
        (None, Some(true)) => {
            audited(&host, &actor, "disable-account", &target, async {
                if dashboard
                    .accounts
                    .disable(id)
                    .await
                    .map_err(account_error)?
                {
                    Ok(())
                } else {
                    Err(not_found())
                }
            })
            .await?
        }
        _ => return Err(bad_request("Supply a password or disabled: true")),
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Lists recent host changes without revealing credentials or participant payloads.
async fn audit_events(State(host): State<Arc<Host>>, request: Request) -> HttpResult<Json<Value>> {
    let (parts, _) = request.into_parts();
    let (dashboard, _) = operator(&host, &parts).await?;
    Ok(Json(
        json!({"events": dashboard.accounts.events().await.map_err(internal)?}),
    ))
}

/// Parses authenticated management JSON with Axum's body limit and safe diagnostics.
async fn parse_json<T: serde::de::DeserializeOwned>(
    parts: Parts,
    body: axum::body::Body,
    host: &Arc<Host>,
) -> HttpResult<Json<T>> {
    use axum::extract::FromRequest;
    Json::from_request(Request::from_parts(parts, body), host)
        .await
        .map_err(|error| {
            (
                error.status(),
                "Invalid or oversized management request".into(),
            )
        })
}

/// Maps an expected account validation error without exposing database internals.
fn account_error(error: anyhow::Error) -> (StatusCode, String) {
    if error.is::<PasswordWorkBusy>() {
        (
            StatusCode::TOO_MANY_REQUESTS,
            "Password service is busy; retry shortly".into(),
        )
    } else if error.is::<AccountInputError>() {
        bad_request(error)
    } else {
        internal(error)
    }
}

/// Signals a missing or invalid authentication credential.
fn unauthorized() -> (StatusCode, String) {
    (
        StatusCode::UNAUTHORIZED,
        "Authentication required or invalid credentials".into(),
    )
}

/// Signals that an authenticated request failed an authority check.
fn forbidden(message: &str) -> (StatusCode, String) {
    (StatusCode::FORBIDDEN, message.into())
}
