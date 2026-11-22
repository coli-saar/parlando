//! Hosts each deployed game with its own current build and research database.

mod accounts;
pub mod credentials;
mod dashboard;
mod package;

use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use anyhow::{bail, ensure, Context, Result};
use axum::{
    body::to_bytes,
    extract::{Path as RoutePath, Request, State},
    http::{
        header::{AUTHORIZATION, CONTENT_TYPE},
        HeaderMap, Method, StatusCode,
    },
    response::{IntoResponse, Redirect, Response},
    routing::{get, post, put},
    Json, Router,
};
use fs2::FileExt;
use package::{recover_build, validate_id, Package, PackageManifest, UPLOAD_LIMIT};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tokio::sync::{Mutex, OwnedSemaphorePermit, RwLock, Semaphore};

pub use package::{write_package_archive, Deployment};

use crate::{
    component::{LoadedGame, WasmMetering},
    GameMetadata,
};

const PACKAGE_CONTENT_TYPE: &str = "application/zip";

/// Names the storage and authority configuration of one long-running deployment host.
pub struct HostOptions {
    /// Directory containing current packages and one research database per game.
    pub store: PathBuf,
    /// External HTTPS origin. Loopback origins can use HTTP for local installation tests.
    pub public_url: String,
    /// Optional operator bearer credential. Omit it to disable bearer-authorized game management.
    pub admin_token: Option<String>,
}

/// Owns the current runtime and participant availability for one deployed game.
struct HostedGame {
    deployment: Deployment,
    active: bool,
    runtime: crate::app::HostedRuntime,
    websocket_upgrades: Arc<AtomicUsize>,
}

/// Keeps asynchronous WebSocket upgrades visible to the host after HTTP dispatch returns.
pub(crate) struct UpgradeLease(Arc<AtomicUsize>);

impl UpgradeLease {
    /// Marks an upgrade as pending before it reaches the underlying game route.
    fn new(upgrades: &Arc<AtomicUsize>) -> Arc<Self> {
        upgrades.fetch_add(1, Ordering::Relaxed);
        Arc::new(Self(upgrades.clone()))
    }
}

impl Drop for UpgradeLease {
    /// Releases the upgrade when Axum drops the callback or the socket task finishes.
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Release);
    }
}

/// Coordinates game operations while owning the exclusive deployment-store lease.
struct Host {
    wasm_metering: WasmMetering,
    store: PathBuf,
    public_url: String,
    admin_token: Option<[u8; 32]>,
    dashboard: Option<dashboard::Dashboard>,
    games: RwLock<HashMap<String, Arc<RwLock<HostedGame>>>>,
    // Only upload operations insert into the game map. Dispatch and activation synchronize
    // against the selected game's lock and do not need this publication lock.
    publication_lock: Mutex<()>,
    uploads: Arc<Semaphore>,
    _store_lock: File,
}

type HttpResult<T> = std::result::Result<T, (StatusCode, String)>;

/// Runs a deployment host and restores every deployed game before it accepts traffic.
pub async fn serve(address: SocketAddr, options: HostOptions) -> Result<()> {
    serve_configured(address, options, None, WasmMetering::Fuel).await
}

/// Runs a deployment host with named dashboard accounts on a separate web origin.
///
/// The administrator hostname must differ from the public participant hostname.
/// Route both hostnames to this listener and preserve the original HTTP authority.
pub async fn serve_with_dashboard(
    address: SocketAddr,
    options: HostOptions,
    admin_url: String,
) -> Result<()> {
    serve_configured(address, options, Some(admin_url), WasmMetering::Fuel).await
}

/// Starts a deployment host with an operator-selected guest execution budget.
/// The policy covers upload validation, new game builds, and restored builds.
/// An optional administrator origin enables the separate browser dashboard.
pub async fn serve_configured(
    address: SocketAddr,
    options: HostOptions,
    admin_url: Option<String>,
    wasm_metering: WasmMetering,
) -> Result<()> {
    let listener = tokio::net::TcpListener::bind(address).await?;
    serve_on_listener(listener, options, admin_url, wasm_metering, || {}).await
}

/// Serves a deployment host on an already bound TCP listener. This allows callers to use
/// the assigned port in public URLs. The readiness callback runs once after configuration
/// and stored games have loaded, immediately before the server begins accepting requests.
pub async fn serve_on_listener(
    listener: tokio::net::TcpListener,
    options: HostOptions,
    admin_url: Option<String>,
    wasm_metering: WasmMetering,
    on_ready: impl FnOnce(),
) -> Result<()> {
    run_host(listener, options, admin_url, false, wasm_metering, on_ready).await
}

/// Starts the local administrator dashboard on a loopback listener. The administrator origin
/// must be localhost on the listener's assigned port. The dashboard offers first-operator setup
/// to local browsers while the account store is empty. The readiness callback runs once before
/// the listener starts accepting requests.
pub async fn serve_local_dashboard_on_listener(
    listener: tokio::net::TcpListener,
    options: HostOptions,
    admin_url: String,
    wasm_metering: WasmMetering,
    on_ready: impl FnOnce(),
) -> Result<()> {
    let bind = listener.local_addr()?;
    ensure!(
        bind.ip().is_loopback(),
        "local dashboard setup requires a loopback listener"
    );
    ensure!(
        checked_origin(&admin_url)?
            == checked_origin(&format!("http://localhost:{}", bind.port()))?
            && checked_origin(&options.public_url)? == checked_origin(&format!("http://{bind}"))?,
        "local dashboard setup requires local administrator and participant origins"
    );
    run_host(
        listener,
        options,
        Some(admin_url),
        true,
        wasm_metering,
        on_ready,
    )
    .await
}

/// Restores a store and signals readiness once the requested serving mode is fully initialized.
async fn run_host(
    listener: tokio::net::TcpListener,
    options: HostOptions,
    admin_url: Option<String>,
    local_setup: bool,
    wasm_metering: WasmMetering,
    on_ready: impl FnOnce(),
) -> Result<()> {
    let public_url = checked_origin(&options.public_url)?;
    let admin_token = options
        .admin_token
        .as_deref()
        .map(|token| -> Result<_> {
            validate_token(token)?;
            Ok(token_digest(token))
        })
        .transpose()?;
    fs::create_dir_all(&options.store)?;
    let store = options.store.canonicalize()?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(store.join(".host.lock"))?;
    FileExt::try_lock_exclusive(&lock).context("another deployment host is using this store")?;
    let dashboard = match admin_url {
        Some(origin) => {
            Some(dashboard::Dashboard::open(&store, &public_url, &origin, local_setup).await?)
        }
        None => None,
    };
    let mut host = Host {
        wasm_metering,
        store,
        public_url,
        admin_token,
        dashboard,
        games: RwLock::new(HashMap::new()),
        publication_lock: Mutex::new(()),
        uploads: Arc::new(Semaphore::new(2)),
        _store_lock: lock,
    };
    host.load_games().await?;
    let router = Router::new()
        .route("/health", get(|| async { Json(json!({"status":"ok"})) }))
        .route("/api/hosting/packages", put(dashboard_upload))
        .route("/api/hosting/packages/:game", put(upload))
        .route("/api/hosting/games/:game/activate", post(activate))
        .route("/api/hosting/games/:game/deactivate", post(deactivate))
        .merge(dashboard::routes())
        .fallback(dispatch)
        .with_state(Arc::new(host));
    tracing::info!(address = %listener.local_addr()?, "started Parlando deployment host");
    on_ready();
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(crate::app::termination_signal())
    .await?;
    Ok(())
}

impl Host {
    /// Restores each game's committed build and participant availability before serving requests.
    async fn load_games(&mut self) -> Result<()> {
        for path in [
            self.store.join("selections"),
            self.store.join("experiments"),
        ] {
            if path.exists() {
                ensure!(
                    fs::read_dir(path)?.next().is_none(),
                    "the deployment store contains legacy experiments and requires offline conversion"
                );
            }
        }
        let packages = self.store.join("packages");
        fs::create_dir_all(&packages)?;
        fs::create_dir_all(self.store.join("games"))?;
        File::open(&self.store)?.sync_all()?;
        for entry in fs::read_dir(packages)? {
            let entry = entry?;
            let id = entry.file_name();
            let id = id.to_str().context("invalid stored game ID")?;
            validate_hosted_game_id(id)?;
            let Some((revision, build)) = recover_build(&entry.path())? else {
                continue;
            };
            let mut game = self
                .build_game(id, revision, build)
                .await
                .with_context(|| format!("restoring hosted game {id}"))?;
            game.runtime.recover_interrupted().await?;
            game.active = self.game_directory(id).join("active").try_exists()?;
            self.games
                .get_mut()
                .insert(id.to_owned(), Arc::new(RwLock::new(game)));
        }
        Ok(())
    }

    /// Locates the durable game data independently of the installed build.
    fn game_directory(&self, id: &str) -> PathBuf {
        self.store.join("games").join(id)
    }

    /// Opens the game's existing installation with the prepared package and stable database.
    async fn build_game(&self, id: &str, revision: String, build: PathBuf) -> Result<HostedGame> {
        let metering = self.wasm_metering;
        let package_revision = revision.clone();
        let (loaded_game, metadata, client_dist) = tokio::task::spawn_blocking(move || {
            let manifest = PackageManifest::read(&build).context("opening game package")?;
            let (loaded_game, metadata) = manifest.load(&build, &package_revision, metering)?;
            Ok::<_, anyhow::Error>((loaded_game, metadata, manifest.client(&build)))
        })
        .await??;
        ensure!(metadata.id == id, "installed package has the wrong game ID");
        let deployment = Deployment {
            game: id.to_owned(),
            name: metadata.name.clone(),
            version: metadata.version.clone(),
            revision,
            component_sha256: loaded_game.digest().to_owned(),
        };
        let game_directory = self.game_directory(id);
        fs::create_dir_all(&game_directory)?;
        File::open(self.store.join("games"))?.sync_all()?;
        let database = game_directory.join("game.sqlite");
        let database = database
            .to_str()
            .context("hosted database path is not UTF-8")?;
        ensure!(
            !database.contains(['?', '%']),
            "hosted database path cannot contain ? or %"
        );
        let runtime = crate::app::build_hosted_game(
            loaded_game,
            metadata,
            &format!("sqlite:///{database}"),
            &format!("{}/{id}", self.public_url),
            client_dist.as_deref().map(Path::new),
        )
        .await?;
        File::open(&game_directory)?.sync_all()?;
        Ok(HostedGame {
            deployment,
            active: false,
            runtime,
            websocket_upgrades: Arc::new(AtomicUsize::new(0)),
        })
    }

    /// Looks up the stable lock that owns dispatch and lifecycle changes for one game.
    async fn game(&self, id: &str) -> Option<Arc<RwLock<HostedGame>>> {
        self.games.read().await.get(id).cloned()
    }
}

/// Verifies the scope before reading a bounded upload or performing Wasm compilation.
async fn upload(
    State(host): State<Arc<Host>>,
    RoutePath(game): RoutePath<String>,
    request: Request,
) -> HttpResult<Json<Deployment>> {
    let (parts, body) = request.into_parts();
    let actor = dashboard::upload_actor(&host, &parts).await?;
    validate_hosted_game_id(&game).map_err(bad_request)?;
    let request = Request::from_parts(parts, body);
    dashboard::audited(&host, &actor, "upload", &game, async {
        let (package, permit) = read_upload(&host, request).await?;
        if package.game() != game {
            return Err(bad_request("package game ID does not match upload scope"));
        }
        install_upload(host.clone(), package, permit).await
    })
    .await
}

/// Authorizes a dashboard session and installs the archive in its declared game scope.
async fn dashboard_upload(
    State(host): State<Arc<Host>>,
    request: Request,
) -> HttpResult<Json<Deployment>> {
    let (parts, body) = request.into_parts();
    let actor = dashboard::upload_actor(&host, &parts).await?;
    match read_upload(&host, Request::from_parts(parts, body)).await {
        Ok((package, permit)) => {
            let game = package.game().to_owned();
            dashboard::audited(
                &host,
                &actor,
                "upload",
                &game,
                install_upload(host.clone(), package, permit),
            )
            .await
        }
        Err(error) => {
            dashboard::audited(&host, &actor, "upload", "packages", async { Err(error) }).await
        }
    }
}

/// Bounds upload time, wire size, and concurrent decoding before handing a package to installation.
async fn read_upload(host: &Host, request: Request) -> HttpResult<(Package, OwnedSemaphorePermit)> {
    if request
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_none_or(|value| !value.eq_ignore_ascii_case(PACKAGE_CONTENT_TYPE))
    {
        return Err((
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "upload must have content type application/zip".to_owned(),
        ));
    }
    let permit = host.uploads.clone().try_acquire_owned().map_err(|_| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "retry upload later".to_owned(),
        )
    })?;
    let bytes = tokio::time::timeout(
        Duration::from_secs(60),
        to_bytes(request.into_body(), UPLOAD_LIMIT),
    )
    .await
    .map_err(|_| (StatusCode::REQUEST_TIMEOUT, "upload timed out".to_owned()))?
    .map_err(|_| {
        (
            StatusCode::PAYLOAD_TOO_LARGE,
            "upload exceeds body limit or is incomplete".to_owned(),
        )
    })?;
    tokio::task::spawn_blocking(move || -> Result<_> { Ok((Package::decode(&bytes)?, permit)) })
        .await
        .map_err(internal)?
        .map_err(bad_request)
}

/// Installs a candidate only when the current game has no experiment or session in progress.
async fn install_upload(
    host: Arc<Host>,
    package: Package,
    permit: OwnedSemaphorePermit,
) -> HttpResult<Json<Deployment>> {
    let id = package.game().to_owned();
    validate_hosted_game_id(&id).map_err(bad_request)?;
    // Finish disk and memory publication together if the upload connection closes.
    tokio::spawn(async move {
        let _permit = permit;
        let _publication = host.publication_lock.lock().await;
        let game = host.game(&id).await;
        let mut existing = match &game {
            Some(game) => Some(game.write().await),
            None => None,
        };
        let revision = package.revision();
        if let Some(current) = &existing {
            require_idle(current).await?;
            if current.deployment.revision == revision {
                return Ok(Json(current.deployment.clone()));
            }
        }
        let package_dir = host.store.join("packages").join(&id);
        if existing.is_some() {
            File::open(&package_dir)
                .and_then(|directory| directory.sync_all())
                .map_err(internal)?;
        }
        let staged = tokio::task::spawn_blocking({
            let package_dir = package_dir.clone();
            let revision = revision.clone();
            move || package.stage(&package_dir, &revision)
        })
        .await
        .map_err(internal)?
        .map_err(internal)?;
        let mut replacement = host
            .build_game(&id, revision.clone(), staged.directory.clone())
            .await
            .map_err(bad_request)?;
        let receipt = replacement.deployment.clone();
        if let Some(current) = &existing {
            replacement.active = current.active;
        }
        let durable = staged.publish(&revision).map_err(internal)?;
        match &mut existing {
            Some(current) => {
                **current = replacement;
                // If the pointer flush failed, restart may see either pointer. Keep both
                // candidates recoverable until the next successful flush.
                if durable {
                    if let Err(error) = recover_build(&package_dir) {
                        tracing::warn!(%error, game = id, "could not remove replaced game build");
                    }
                }
            }
            None => {
                host.games
                    .write()
                    .await
                    .insert(id, Arc::new(RwLock::new(replacement)));
            }
        }
        Ok(Json(receipt))
    })
    .await
    .map_err(internal)?
}

/// Enables participant requests for the selected game.
async fn activate(
    State(host): State<Arc<Host>>,
    RoutePath(id): RoutePath<String>,
    request: Request,
) -> HttpResult<Json<Value>> {
    set_activation(host, id, request, true).await
}

/// Disables participant requests once the selected game's experiments and sessions have stopped.
async fn deactivate(
    State(host): State<Arc<Host>>,
    RoutePath(id): RoutePath<String>,
    request: Request,
) -> HttpResult<Json<Value>> {
    set_activation(host, id, request, false).await
}

/// Updates participant availability under the same lock used to replace a running runtime.
async fn set_activation(
    host: Arc<Host>,
    id: String,
    request: Request,
    active: bool,
) -> HttpResult<Json<Value>> {
    let (parts, _) = request.into_parts();
    let actor = dashboard::authorize(&host, &parts, host.admin_token.as_ref(), || {
        "host-operator-token".into()
    })
    .await?;
    let operation = if active { "activate" } else { "deactivate" };
    dashboard::audited(&host, &actor, operation, &id, async {
        let game = host.game(&id).await.ok_or_else(not_found)?;
        let mut game = game.write().await;
        if game.active != active {
            if !active {
                require_idle(&game).await?;
            }
            let directory = host.game_directory(&id);
            let marker = directory.join("active");
            if active {
                File::create(marker).map_err(internal)?;
            } else {
                fs::remove_file(marker).map_err(internal)?;
            }
            game.active = active;
            if let Err(error) = File::open(directory).and_then(|directory| directory.sync_all()) {
                tracing::error!(%error, game = id, "could not flush game activation change");
            }
        }
        Ok(Json(json!({ "active": game.active })))
    })
    .await
}

/// Confirms that lifecycle state and draining sessions permit replacing or retiring a runtime.
async fn require_idle(game: &HostedGame) -> HttpResult<()> {
    if let Some(reason) = game.runtime.replacement_blocker().await.map_err(internal)? {
        return Err((StatusCode::CONFLICT, reason.to_owned()));
    }
    if game.websocket_upgrades.load(Ordering::Acquire) != 0 {
        return Err((
            StatusCode::CONFLICT,
            "Participant connections are still open for this game.".to_owned(),
        ));
    }
    Ok(())
}

/// Dispatches game-scoped requests while excluding runtime replacement and deactivation.
async fn dispatch(State(host): State<Arc<Host>>, request: Request) -> HttpResult<Response> {
    let (parts, body) = request.into_parts();
    let path = parts.uri.path();
    let participant_game = if let Some(path) = path.strip_prefix("/e/") {
        path.split('/')
            .next()
            .and_then(|scope| scope.split_once('~'))
            .map(|(game_id, _)| game_id)
    } else {
        path.strip_prefix("/api/admin/replays/")
            .and_then(|path| path.split_once('/'))
            .map(|(game_id, _)| game_id)
    };
    if let Some(game_id) = participant_game.map(str::to_owned) {
        return dispatch_participant(&host, &game_id, parts, body).await;
    }
    let (id, tail) = path
        .strip_prefix('/')
        .map(|rest| rest.split_once('/').unwrap_or((rest, "")))
        .ok_or_else(not_found)?;
    let game = host.game(id).await.ok_or_else(not_found)?;
    if dashboard::administrator_origin_allowed(&host, &parts)
        && (tail.is_empty() || tail.starts_with("api/admin/") || tail.starts_with("admin/assets/"))
    {
        let (actor, csrf_token) = match dashboard::game_session(&host, &parts).await {
            Ok(session) => session,
            Err((StatusCode::UNAUTHORIZED, _))
                if tail.is_empty() && parts.method == Method::GET =>
            {
                return Ok(Redirect::temporary(&format!("/admin?next=/{id}")).into_response());
            }
            Err(error) => return Err(error),
        };
        let action = match (parts.method.as_str(), tail) {
            ("GET" | "HEAD", tail) if tail.ends_with("/export") => Some("export"),
            ("POST", tail) if tail.ends_with("/status") => Some("intake"),
            ("POST", tail) if tail.ends_with("/config") => Some("save-experiment-config"),
            ("POST", "api/admin/experiments") => Some("create-experiment"),
            ("POST", tail) if tail.ends_with("/deletion") => Some("delete-participant-data"),
            ("POST", tail) if tail.ends_with("/replay-link") => Some("issue-replay-link"),
            (_, _) => None,
        };
        let target = id.to_owned();
        let request = Request::from_parts(parts, body);
        let host_lease = host.clone();
        let response = async {
            // A disconnected operator must not release the deployment guard while a
            // lifecycle update is still being committed by the runtime.
            tokio::spawn(async move {
                let _host_lease = host_lease;
                let game = game.read().await;
                game.runtime
                    .dispatch_admin(request, csrf_token, game.active)
                    .await
            })
            .await
            .map_err(internal)
        };
        return match action {
            Some(action) => {
                dashboard::audited_response(&host, &actor, action, &target, response).await
            }
            None => response.await,
        };
    }
    Err(not_found())
}

/// Checks the public origin before routing participant and capability-backed replay requests.
async fn dispatch_participant(
    host: &Host,
    game_id: &str,
    parts: axum::http::request::Parts,
    body: axum::body::Body,
) -> HttpResult<Response> {
    if !dashboard::participant_origin_allowed(host, &parts) {
        return Err(not_found());
    }
    let game = host.game(game_id).await.ok_or_else(not_found)?;
    let game = game.read().await;
    let mut request = Request::from_parts(parts, body);
    if request
        .uri()
        .path()
        .strip_prefix("/e/")
        .and_then(|path| path.split_once('/'))
        .is_some_and(|(_, tail)| tail.starts_with("ws/"))
    {
        request
            .extensions_mut()
            .insert(UpgradeLease::new(&game.websocket_upgrades));
    }
    Ok(game
        .runtime
        .dispatch_participant(request, game.active)
        .await)
}

/// Reserves the host's own route names while accepting normal package game identifiers.
fn validate_hosted_game_id(id: &str) -> Result<()> {
    validate_id(id)?;
    ensure!(
        !matches!(id, "admin" | "api" | "health"),
        "game ID {id:?} is reserved by the deployment host"
    );
    Ok(())
}

/// Validates a built game ZIP and sends its original bytes with a server-wide upload token.
/// This installs the current build for the archive’s game on a deployment host.
pub async fn deploy(host: &str, archive: &Path, token: &str) -> Result<Deployment> {
    let path = archive.to_owned();
    let (game, bytes) = tokio::task::spawn_blocking(move || -> Result<_> {
        let (package, bytes) = Package::read_archive(&path)?;
        Ok((package.game().to_owned(), bytes))
    })
    .await??;
    let request = management_request(host, "PUT", &format!("packages/{game}"), token)?
        .header(CONTENT_TYPE, PACKAGE_CONTENT_TYPE)
        .body(bytes);
    management_response(request).await
}

/// Reads the account credential saved for this server and uploads the supplied game archive.
pub async fn deploy_with_saved_credentials(host: &str, archive: &Path) -> Result<Deployment> {
    deploy(host, archive, &credentials::upload_token(host)?).await
}

/// Calls a host management route without forwarding credentials to redirect targets.
pub async fn control_request<T: Serialize + ?Sized, R: serde::de::DeserializeOwned>(
    host: &str,
    method: &str,
    path: &str,
    token: &str,
    body: Option<&T>,
) -> Result<R> {
    let mut request = management_request(host, method, path, token)?;
    if let Some(body) = body {
        request = request.json(body);
    }
    management_response(request).await
}

/// Applies the shared origin, credential, redirect, and timeout rules for management requests.
fn management_request(
    host: &str,
    method: &str,
    path: &str,
    token: &str,
) -> Result<reqwest::RequestBuilder> {
    validate_token(token)?;
    let host = checked_origin(host)?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(180))
        .build()?;
    let url = format!("{host}/api/hosting/{path}");
    Ok(client.request(method.parse()?, url).bearer_auth(token))
}

/// Decodes a management receipt or reports a host rejection with its status and detail.
async fn management_response<R: serde::de::DeserializeOwned>(
    request: reqwest::RequestBuilder,
) -> Result<R> {
    let response = request.send().await?;
    let status = response.status();
    if !status.is_success() {
        let detail = response.text().await?;
        bail!("deployment host returned {status}: {detail}");
    }
    Ok(response.json().await?)
}

/// Validates a bare deployment origin and restricts unencrypted connections to loopback.
pub fn checked_origin(host: &str) -> Result<String> {
    credentials::origin(host)
}

/// Validates a built ZIP and returns its declared game ID for credential resolution.
pub fn package_game(archive: &Path) -> Result<String> {
    Ok(Package::read_archive(archive)?.0.game().to_owned())
}

/// Compiles the component from a built ZIP and validates its ABI and game schema with the selected
/// Wasm metering. The returned metadata identifies the canonical package revision and component.
pub fn load_package(archive: &Path, metering: WasmMetering) -> Result<(LoadedGame, GameMetadata)> {
    Package::read_archive(archive)?.0.load(metering)
}

/// Validates a game identifier before using it in a package filename or management URL.
pub fn validate_game_id(id: &str) -> Result<()> {
    validate_id(id)
}

/// Validates the basic format of a bearer secret before storing it or sending a request.
pub fn validate_token(token: &str) -> Result<()> {
    ensure!(
        token.len() >= 32
            && token.len() <= 256
            && token.bytes().all(|byte| byte.is_ascii_graphic()),
        "deployment tokens require 32–256 printable, non-space ASCII characters"
    );
    Ok(())
}

/// Enforces exact bearer-token authentication using constant-time digest comparison.
fn require_token(headers: &HeaderMap, expected: Option<&[u8; 32]>) -> HttpResult<()> {
    let accepted = headers
        .get(AUTHORIZATION)
        .and_then(|header| header.to_str().ok())
        .and_then(|header| header.strip_prefix("Bearer "))
        .zip(expected)
        .is_some_and(|(token, expected)| bool::from(token_digest(token).ct_eq(expected)));
    if accepted {
        Ok(())
    } else {
        Err((
            StatusCode::UNAUTHORIZED,
            "valid deployment credential required".to_owned(),
        ))
    }
}

/// Hashes credentials for comparison without retaining the configured plaintext in host state.
fn token_digest(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

/// Returns a generic unknown deployment route response.
fn not_found() -> (StatusCode, String) {
    (
        StatusCode::NOT_FOUND,
        "deployment route or game not found".to_owned(),
    )
}

/// Returns input validation details to the already authenticated caller.
fn bad_request(error: impl std::fmt::Display) -> (StatusCode, String) {
    (StatusCode::BAD_REQUEST, error.to_string())
}

/// Records internal I/O failures without exposing paths or state to HTTP clients.
fn internal(error: impl std::fmt::Display) -> (StatusCode, String) {
    tracing::error!(%error, "deployment host request failed");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        "deployment host request failed".to_owned(),
    )
}
