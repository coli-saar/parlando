//! Uploads Wasm game packages and hosts explicitly pinned participant experiments.

mod accounts;
pub mod credentials;
mod dashboard;
mod package;

use std::{
    collections::{BTreeMap, HashMap},
    fs::{self, File, OpenOptions},
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::{bail, ensure, Context, Result};
use axum::{
    body::to_bytes,
    extract::{Path as RoutePath, Request, State},
    http::{header::AUTHORIZATION, HeaderMap, StatusCode},
    response::Response,
    routing::{get, put},
    Json, Router,
};
use fs2::FileExt;
use package::{validate_id, validate_revision, PackageManifest, Upload, UPLOAD_LIMIT};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tokio::sync::{Mutex, RwLock, Semaphore};
use tower::ServiceExt;

pub use package::Deployment;

use crate::component::WasmMetering;

const CONTROL_LIMIT: usize = 256 * 1024;

/// Names the storage and authority configuration of one long-running deployment host.
pub struct HostOptions {
    /// Directory containing immutable packages, experiment pins, and research databases.
    pub store: PathBuf,
    /// External HTTPS origin. Loopback origins can use HTTP for local installation tests.
    pub public_url: String,
    /// Independent upload tokens indexed by the exact game ID each token may publish.
    pub grants: BTreeMap<String, String>,
    /// Operator credential for pinning releases and operating experiments.
    pub admin_token: String,
}

/// Host-owned experiment settings bound to one exact package revision.
#[derive(Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExperimentSelection {
    /// Package game ID.
    pub game: String,
    /// Immutable revision returned by deployment.
    pub revision: String,
    /// Experiment YAML with the same fields as the Parlando experiment configuration.
    pub config: Value,
}

struct HostedExperiment {
    selection: ExperimentSelection,
    participant: Router,
    admin: Router,
    state: Arc<crate::app::AppState>,
}

struct Host {
    wasm_metering: WasmMetering,
    store: PathBuf,
    public_url: String,
    grants: BTreeMap<String, [u8; 32]>,
    admin_token: [u8; 32],
    dashboard: Option<dashboard::Dashboard>,
    experiments: RwLock<HashMap<String, HostedExperiment>>,
    selection_lock: Mutex<()>,
    uploads: Arc<Semaphore>,
    _store_lock: File,
}

type HttpResult<T> = std::result::Result<T, (StatusCode, String)>;

/// Runs a deployment host and reloads every pinned experiment before it accepts traffic.
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
/// The policy covers upload validation, new selections, and reloaded experiments.
/// An optional administrator origin enables the separate browser dashboard.
pub async fn serve_configured(
    address: SocketAddr,
    options: HostOptions,
    admin_url: Option<String>,
    wasm_metering: WasmMetering,
) -> Result<()> {
    let public_url = checked_origin(&options.public_url)?;
    ensure!(
        !options.grants.is_empty() || admin_url.is_some(),
        "configure a game upload grant or an administrator origin"
    );
    validate_token(&options.admin_token)?;
    let admin_token = token_digest(&options.admin_token);
    let mut grants = BTreeMap::new();
    for (game, token) in options.grants {
        validate_id(&game)?;
        validate_token(&token)?;
        let digest = token_digest(&token);
        ensure!(
            digest != admin_token && !grants.values().any(|value| *value == digest),
            "operator and game upload tokens must all be distinct"
        );
        grants.insert(game, digest);
    }
    fs::create_dir_all(&options.store)?;
    let store = options.store.canonicalize()?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(store.join(".host.lock"))?;
    FileExt::try_lock_exclusive(&lock).context("another deployment host is using this store")?;
    let dashboard = match admin_url {
        Some(origin) => Some(dashboard::Dashboard::open(&store, &public_url, &origin).await?),
        None => None,
    };
    let mut host = Host {
        wasm_metering,
        store,
        public_url,
        grants,
        admin_token,
        dashboard,
        experiments: RwLock::new(HashMap::new()),
        selection_lock: Mutex::new(()),
        uploads: Arc::new(Semaphore::new(2)),
        _store_lock: lock,
    };
    host.load_experiments().await?;
    let router = Router::new()
        .route("/health", get(|| async { Json(json!({"status":"ok"})) }))
        .route("/api/hosting/packages/:game", put(upload))
        .route("/api/hosting/experiments/:id", put(select_experiment))
        .merge(dashboard::routes())
        .fallback(dispatch)
        .with_state(Arc::new(host));
    let listener = tokio::net::TcpListener::bind(address).await?;
    tracing::info!(address = %listener.local_addr()?, "started Parlando deployment host");
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(crate::app::termination_signal())
    .await?;
    Ok(())
}

impl Host {
    /// Restores the exact selection for every previously provisioned experiment.
    async fn load_experiments(&mut self) -> Result<()> {
        let selections = self.store.join("selections");
        fs::create_dir_all(&selections)?;
        fs::create_dir_all(self.store.join("experiments"))?;
        for entry in fs::read_dir(selections)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_none_or(|extension| extension != "json") {
                continue;
            }
            let id = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .context("invalid stored selection filename")?;
            let selection = serde_json::from_slice(&fs::read(&path)?)?;
            let experiment = self
                .build_experiment(id, selection)
                .await
                .with_context(|| format!("restoring hosted experiment {id}"))?;
            self.experiments.get_mut().insert(id.to_owned(), experiment);
        }
        Ok(())
    }

    /// Builds a runtime scoped to its own storage, participant URL, and package revision.
    async fn build_experiment(
        &self,
        id: &str,
        selection: ExperimentSelection,
    ) -> Result<HostedExperiment> {
        validate_id(id)?;
        validate_id(&selection.game)?;
        validate_revision(&selection.revision)?;
        let mut config: crate::config::ExperimentConfig =
            serde_json::from_value(selection.config.clone())?;
        let directory = self
            .store
            .join("packages")
            .join(&selection.game)
            .join(&selection.revision);
        let revision = selection.revision.clone();
        let metering = self.wasm_metering;
        let (loaded_game, metadata, client_dist) = tokio::task::spawn_blocking(move || {
            let manifest = PackageManifest::read(&directory).context("opening selected package")?;
            let (loaded_game, metadata) = manifest.load(&directory, &revision, metering)?;
            Ok::<_, anyhow::Error>((loaded_game, metadata, manifest.client(&directory)))
        })
        .await??;
        ensure!(
            metadata.id == selection.game,
            "selected package has the wrong game ID"
        );
        config.experiment.id = Some(id.to_owned());
        let selection_hash = format!("{:x}", Sha256::digest(serde_json::to_vec(&selection)?));
        let database = self
            .store
            .join("experiments")
            .join(format!("{id}-{selection_hash}.sqlite"));
        let database = database
            .to_str()
            .context("hosted database path is not UTF-8")?;
        ensure!(
            !database.contains(['?', '%']),
            "hosted database path cannot contain ? or %"
        );
        config.database.url = format!("sqlite:///{database}");
        config.server.public_base_url = format!("{}/e/{id}", self.public_url);
        config.server.client_dist_path = client_dist;
        let (participant, admin, state) =
            crate::app::build_deployed_experiment(loaded_game, config, metadata).await?;
        Ok(HostedExperiment {
            selection,
            participant,
            admin,
            state,
        })
    }
}

/// Verifies the scope before reading a bounded upload or performing Wasm compilation.
async fn upload(
    State(host): State<Arc<Host>>,
    RoutePath(game): RoutePath<String>,
    request: Request,
) -> HttpResult<Json<Deployment>> {
    let (parts, body) = request.into_parts();
    let actor = dashboard::authorize(&host, &parts, host.grants.get(&game), || {
        format!("upload-token:{game}")
    })
    .await?;
    validate_id(&game).map_err(bad_request)?;
    let request = Request::from_parts(parts, body);
    dashboard::audited(
        &host,
        &actor,
        "upload",
        &game,
        install_upload(&host, &game, request),
    )
    .await
}

/// Bounds the upload body and installs the validated package off the request executor.
async fn install_upload(host: &Host, game: &str, request: Request) -> HttpResult<Json<Deployment>> {
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
    let dir = host.store.clone();
    let metering = host.wasm_metering;
    let game = game.to_owned();
    let receipt = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let upload: Upload = serde_json::from_slice(&bytes).context("invalid package envelope")?;
        upload.install(&dir, &game, metering)
    })
    .await
    .map_err(internal)?
    .map_err(bad_request)?;
    Ok(Json(receipt))
}

/// Provisions a durable pin. Repeating the same selection is safe and replacing it is rejected.
async fn select_experiment(
    State(host): State<Arc<Host>>,
    RoutePath(id): RoutePath<String>,
    request: Request,
) -> HttpResult<Json<Value>> {
    let (parts, body) = request.into_parts();
    let actor = dashboard::authorize(&host, &parts, Some(&host.admin_token), || {
        "host-operator-token".into()
    })
    .await?;
    validate_id(&id).map_err(bad_request)?;
    let request = Request::from_parts(parts, body);
    let target = id.clone();
    let audit_host = host.clone();
    dashboard::audited(
        &audit_host,
        &actor,
        "select",
        &target,
        pin_experiment(host, id, request),
    )
    .await
}

/// Validates and persists an experiment's immutable package and configuration pin.
async fn pin_experiment(host: Arc<Host>, id: String, request: Request) -> HttpResult<Json<Value>> {
    let bytes = tokio::time::timeout(
        Duration::from_secs(15),
        to_bytes(request.into_body(), CONTROL_LIMIT),
    )
    .await
    .map_err(|_| {
        (
            StatusCode::REQUEST_TIMEOUT,
            "selection request timed out".to_owned(),
        )
    })?
    .map_err(bad_request)?;
    let mut selection: ExperimentSelection = serde_json::from_slice(&bytes).map_err(bad_request)?;
    let config: crate::config::ExperimentConfig =
        serde_json::from_value(selection.config).map_err(bad_request)?;
    selection.config = crate::app::validate_hosted_config(&config).map_err(bad_request)?;
    let revision = selection.revision.clone();
    let experiment_id = id.clone();
    let public_url = host.public_url.clone();
    tokio::spawn(async move {
        let _guard = host.selection_lock.lock().await;
        let existing = host
            .experiments
            .read()
            .await
            .get(&experiment_id)
            .map(|experiment| experiment.selection.clone());
        if let Some(existing) = existing {
            if existing != selection {
                return Err((
                    StatusCode::CONFLICT,
                    "experiment already pins a different revision or configuration".to_owned(),
                ));
            }
            return Ok(());
        }
        let experiment = host
            .build_experiment(&experiment_id, selection)
            .await
            .map_err(bad_request)?;
        persist_selection(&host.store, &experiment_id, &experiment.selection).map_err(internal)?;
        host.experiments
            .write()
            .await
            .insert(experiment_id, experiment);
        Ok(())
    })
    .await
    .map_err(internal)??;
    Ok(Json(json!({
        "experiment": id,
        "revision": revision,
        "url": format!("{public_url}/e/{id}/")
    })))
}

/// Routes participant and operator requests to the pinned runtime, including WebSocket upgrades.
async fn dispatch(State(host): State<Arc<Host>>, request: Request) -> HttpResult<Response> {
    let (parts, body) = request.into_parts();
    let path = parts.uri.path();
    let (id, admin) = if let Some(rest) = path.strip_prefix("/e/") {
        if !dashboard::participant_origin_allowed(&host, &parts) {
            return Err(not_found());
        }
        let (id, tail) = rest.split_once('/').unwrap_or((rest, ""));
        if tail == "admin"
            || tail.starts_with("admin/")
            || tail == "api/admin"
            || tail.starts_with("api/admin/")
        {
            return Err(not_found());
        }
        (id, false)
    } else if let Some((id, tail)) = path
        .strip_prefix("/api/hosting/experiments/")
        .and_then(|rest| rest.split_once('/'))
    {
        if !tail.starts_with("admin/") {
            return Err(not_found());
        }
        (id, true)
    } else {
        return Err(not_found());
    };
    let actor = if admin {
        Some(
            dashboard::authorize(&host, &parts, Some(&host.admin_token), || {
                "host-operator-token".into()
            })
            .await?,
        )
    } else {
        None
    };
    let router = {
        let experiments = host.experiments.read().await;
        let experiment = experiments.get(id).ok_or_else(not_found)?;
        if admin {
            experiment.admin.clone()
        } else {
            experiment.participant.clone()
        }
    };
    let action = match (actor.as_ref(), parts.method.as_str(), path) {
        (Some(actor), "GET" | "HEAD", path) if path.ends_with("/admin/export") => {
            Some((actor, "export"))
        }
        (Some(actor), "POST", path) if path.ends_with("/admin/experiment/status") => {
            Some((actor, "intake"))
        }
        (Some(actor), "POST", path) if path.ends_with("/deletion") => {
            Some((actor, "delete-participant-data"))
        }
        _ => None,
    };
    let target = id.to_owned();
    let request = Request::from_parts(parts, body);
    let response = async {
        Ok(router
            .oneshot(request)
            .await
            .expect("Axum routers are infallible"))
    };
    match action {
        Some((actor, operation)) => {
            dashboard::audited_response(&host, actor, operation, &target, response).await
        }
        None => response.await,
    }
}

/// Writes an operator selection atomically so startup never sees a half-written experiment.
fn persist_selection(store: &Path, id: &str, selection: &ExperimentSelection) -> Result<()> {
    use std::io::Write as _;

    let directory = store.join("selections");
    let temporary = directory.join(format!(".selection-{}", uuid::Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let result = (|| -> Result<()> {
        let mut file = options.open(&temporary)?;
        serde_json::to_writer(&mut file, selection)?;
        file.flush()?;
        file.sync_all()?;
        fs::rename(&temporary, directory.join(format!("{id}.json")))?;
        File::open(&directory)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// Sends a built package using a game-scoped upload token to an HTTPS host.
pub async fn deploy(host: &str, directory: &Path, token: &str) -> Result<Deployment> {
    let manifest = PackageManifest::read(directory)?;
    let game = manifest.id();
    let payload = Upload::read(directory, &manifest)?;
    control_request(
        host,
        "PUT",
        &format!("packages/{game}"),
        token,
        Some(&payload),
    )
    .await
}

/// Calls a host management route without forwarding credentials to redirect targets.
pub async fn control_request<T: Serialize + ?Sized, R: serde::de::DeserializeOwned>(
    host: &str,
    method: &str,
    path: &str,
    token: &str,
    body: Option<&T>,
) -> Result<R> {
    validate_token(token)?;
    let host = checked_origin(host)?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(180))
        .build()?;
    let url = format!("{host}/api/hosting/{path}");
    let mut request = client.request(method.parse()?, url).bearer_auth(token);
    if let Some(body) = body {
        request = request.json(body);
    }
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

/// Reads the game ID needed to look up an upload credential for a built package.
pub fn package_game(directory: &Path) -> Result<String> {
    Ok(PackageManifest::read(directory)?.id().to_owned())
}

/// Validates an experiment identifier before including it in a management URL.
pub fn validate_experiment_id(id: &str) -> Result<()> {
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
        "deployment route or experiment not found".to_owned(),
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
