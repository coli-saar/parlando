//! Builds the installation router around the loaded game, shared store, and
//! registered runtime services. The mount registry selects the experiment revision.
//! Runtime construction initializes that revision, and the route declarations connect its
//! participant and administration handlers to Axum.

use super::*;
use tokio::sync::Semaphore;
mod hosted_replay;
pub(super) mod mount_lock;
mod mounts;
mod routes;
mod runtime;
use hosted_replay::ReplayGrants;
use mounts::dispatch_experiment_service;
pub(super) use mounts::ExperimentMounts;
use routes::{dashboard_routes, experiment_routes, http_layers};
use runtime::build_experiment_router;

/// Route trees built around one experiment state before installation-level mounting.
struct BuiltRouters {
    state: Arc<AppState>,
    primary: Router,
    runtime_admin: Router,
}

/// Stable credentials and one-use tickets scoped to an experiment across configuration revisions.
#[derive(Clone, Default)]
struct MountIdentity {
    generation: u64,
    admission: AdmissionQueue,
    creation_rate: Arc<RwLock<ParticipantCreationRate>>,
    participants: Arc<ParticipantAuthenticator>,
    tickets: Arc<UpgradeTicketStore>,
}

/// Shared factory for runtime components which may vary with experiment configuration.
type ServeOptionsFactory = Arc<dyn Fn(&ExperimentConfig) -> Result<ServeOptions> + Send + Sync>;

/// Installation dispatcher that owns the router cache and lazily mounts experiments.
struct GameHost {
    hosted: bool,
    dashboard: Arc<ServerContext>,
    options_factory: ServeOptionsFactory,
    routers: Arc<ExperimentMountCache>,
    health_slots: Semaphore,
}

/// Checks installation storage without constructing an additional experiment runtime.
async fn game_health(State(host): State<Arc<GameHost>>) -> Response {
    let Ok(_slot) = host.health_slots.try_acquire() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"status": "busy", "storage": "check_in_progress"})),
        )
            .into_response();
    };
    match tokio::time::timeout(
        Duration::from_secs(2),
        host.dashboard.installation.store.health_check(),
    )
    .await
    {
        Ok(Ok(())) => Json(json!({"status": "ok", "storage": "read_write"})).into_response(),
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"status": "unavailable", "storage": "health_check_timeout"})),
        )
            .into_response(),
        Ok(Err(error)) => {
            tracing::error!(%error, "game host health check could not acquire SQLite write transaction");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"status": "unavailable", "storage": "unavailable"})),
            )
                .into_response()
        }
    }
}

/// Sends the unscoped game root to the administrator workspace.
async fn game_root() -> Redirect {
    Redirect::temporary("/admin/experiments")
}

/// Builds one game component's multi-experiment router from the bootstrap configuration.
pub async fn build_game_router<F>(
    loaded_game: LoadedGame,
    bootstrap: ExperimentConfig,
    descriptor: GameMetadata,
    options_factory: F,
) -> Result<Router>
where
    F: Fn(&ExperimentConfig) -> Result<ServeOptions> + Send + Sync + 'static,
{
    crate::config::validate_public_origin(
        "server.public_base_url",
        &bootstrap.server.public_base_url,
    )?;
    bootstrap.validate()?;
    validate_game_config_contains_no_secrets(&bootstrap.game)?;
    parse_game_config(&loaded_game, &bootstrap.game)?;
    descriptor.validate()?;
    let router_cache = Arc::new(RwLock::new(HashMap::new()));
    let installation = open_installation(
        loaded_game,
        descriptor,
        &bootstrap.database.url,
        Arc::downgrade(&router_cache),
    )
    .await?;
    let store = &installation.store;
    let cleanup_auth = Arc::downgrade(&installation.admin_auth);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        loop {
            interval.tick().await;
            let Some(cleanup_auth) = cleanup_auth.upgrade() else {
                break;
            };
            if let Err(error) = cleanup_auth.cleanup().await {
                tracing::warn!(%error, "failed to clean expired administrator sessions");
            }
        }
    });
    let deactivated_experiments = store.deactivate_open_experiments().await?;
    if deactivated_experiments > 0 {
        tracing::info!(
            count = deactivated_experiments,
            "closed experiment intake after game-process startup"
        );
    }
    let options = options_factory(&bootstrap)?;
    let cors = configured_cors(&bootstrap)?;
    let dashboard = Arc::new(ServerContext {
        installation,
        config: bootstrap,
        agent_definitions: registered_agent_definitions(&options)?,
        has_tts_provider: options.tts_provider.is_some(),
        has_transcription_provider: options.transcription_provider.is_some(),
    });
    let admin_router = http_layers(dashboard_routes(&dashboard, ""), &dashboard, cors);
    spawn_load_sampler(dashboard.clone());
    let host = Arc::new(GameHost {
        hosted: false,
        dashboard,
        options_factory: Arc::new(options_factory),
        routers: router_cache,
        health_slots: Semaphore::new(1),
    });
    let runtime_host = host.clone();
    let runtime_service = tower::service_fn(move |request| {
        let host = runtime_host.clone();
        async move {
            Ok::<_, std::convert::Infallible>(dispatch_experiment_service(host, request).await)
        }
    });
    let host_router = Router::new()
        .route("/", get(game_root))
        .layer(ConcurrencyLimitLayer::new(256))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(30),
        ))
        .route("/health", get(game_health))
        .with_state(host);
    Ok(admin_router
        .merge(host_router)
        .fallback_service(runtime_service))
}

/// Carries the deployment's lifecycle decision into an authenticated status mutation.
#[derive(Clone, Copy)]
pub(super) struct HostedGameActive(pub(super) bool);

/// Owns the current published component and all experiments in one game database.
pub(crate) struct HostedRuntime {
    game_id: String,
    host: Arc<GameHost>,
    dashboard: Router,
    replays: ReplayGrants,
}

impl HostedRuntime {
    /// Dispatches only participant paths, keeping account and experiment management separate.
    pub(crate) async fn dispatch_participant(
        &self,
        mut request: Request,
        active: bool,
    ) -> Response {
        let original = request.uri();
        if let Some(token) = ReplayGrants::credential(&self.game_id, original.path()) {
            if request.method() != Method::GET {
                return StatusCode::METHOD_NOT_ALLOWED.into_response();
            }
            return self.replays.redeem(&self.host.dashboard, token).await;
        }
        let prefix = format!("/e/{}~", self.game_id);
        let Some(experiment_path) = original
            .path()
            .strip_prefix(&prefix)
            .filter(|path| !path.is_empty())
        else {
            return StatusCode::NOT_FOUND.into_response();
        };
        if !active {
            let (experiment_id, suffix) = experiment_path
                .split_once('/')
                .unwrap_or((experiment_path, ""));
            if suffix.starts_with("api/") || suffix.starts_with("ws/") {
                return StatusCode::NOT_FOUND.into_response();
            }
            if suffix.is_empty()
                && !self
                    .replays
                    .authorizes_shell(&self.game_id, experiment_id, original)
                    .await
            {
                return StatusCode::NOT_FOUND.into_response();
            }
        }
        if !experiment_path.contains('/') {
            let query = original
                .query()
                .map(|query| format!("?{query}"))
                .unwrap_or_default();
            return Redirect::temporary(&format!("{}/{}", original.path(), query)).into_response();
        }
        let path = format!("/e/{experiment_path}");
        replace_request_path(&mut request, &path);
        dispatch_experiment_service(self.host.clone(), request).await
    }

    /// Dispatches the original dashboard after account ownership and CSRF checks at the host.
    pub(crate) async fn dispatch_admin(
        &self,
        mut request: Request,
        csrf_token: String,
        active: bool,
    ) -> Response {
        let Some(path) = self.game_relative_path(&request) else {
            return StatusCode::NOT_FOUND.into_response();
        };
        if let Err(error) =
            check_admin_network(&self.host.dashboard, direct_peer_ip(&request)).await
        {
            return error.into_response();
        }
        if let Some((experiment_id, session_id)) = ReplayGrants::issue_target(&path) {
            if request.method() != Method::POST {
                return StatusCode::METHOD_NOT_ALLOWED.into_response();
            }
            return self
                .replays
                .issue(
                    &self.host.dashboard,
                    &self.game_id,
                    experiment_id,
                    session_id,
                )
                .await
                .into_response();
        }
        request.extensions_mut().insert(AdminSession {
            role: crate::auth::AdminRole::Administrator,
            csrf_token,
            created_at: 0,
            last_seen_at: 0,
        });
        request.extensions_mut().insert(HostedGameActive(active));
        let runtime_request = path.starts_with("/api/admin/runtime/");
        replace_request_path(&mut request, &path);
        if runtime_request {
            dispatch_experiment_service(self.host.clone(), request).await
        } else {
            tower::ServiceExt::oneshot(self.dashboard.clone(), request)
                .await
                .unwrap_or_else(|error| match error {})
        }
    }

    /// Closes intake and ends interrupted sessions on process startup.
    pub(crate) async fn recover_interrupted(&self) -> Result<()> {
        let context = &self.host.dashboard;
        let store = &context.installation.store;
        store.deactivate_open_experiments().await?;
        for experiment_id in store.interrupted_experiment_ids().await? {
            let configuration = hydrated_experiment_config(context, &experiment_id)
                .await
                .map_err(|error| anyhow!(error.message))?;
            finalize_interrupted_experiment(
                &context.installation.store,
                &experiment_id,
                &configuration,
            )
            .await?;
        }
        Ok(())
    }

    /// Reports whether replacing the loaded component would interrupt intake or a live room.
    /// The deployment host holds the game's exclusive lock through this check and replacement.
    pub(crate) async fn replacement_blocker(&self) -> Result<Option<&'static str>> {
        if self
            .host
            .dashboard
            .installation
            .store
            .has_open_experiments()
            .await?
        {
            return Ok(Some(
                "This game has an experiment open for testing or participant intake.",
            ));
        }
        if !self
            .host
            .dashboard
            .installation
            .store
            .interrupted_experiment_ids()
            .await?
            .is_empty()
        {
            return Ok(Some("This game still has unfinished participant sessions."));
        }
        let registry = self
            .host
            .dashboard
            .installation
            .runtime_registry
            .read()
            .await;
        for runtime in registry.values().flatten().filter_map(Weak::upgrade) {
            if runtime.game_connections.count().await != 0
                || runtime.audio_connections.count().await != 0
                || runtime
                    .memory
                    .read()
                    .await
                    .sessions
                    .values()
                    .any(|room| room.timing().voice_ends_at.is_some())
            {
                return Ok(Some(
                    "Participant connections are still open for this game.",
                ));
            }
        }
        Ok(None)
    }

    /// Strips exactly this game's namespace, leaving the internal route shape.
    fn game_relative_path(&self, request: &Request) -> Option<String> {
        let suffix = request
            .uri()
            .path()
            .strip_prefix('/')?
            .strip_prefix(&self.game_id)?;
        if suffix.is_empty() {
            Some("/".to_string())
        } else {
            suffix.starts_with('/').then(|| suffix.to_string())
        }
    }
}

/// Rewrites host dispatch paths without losing the request query, headers or upgrade state.
fn replace_request_path(request: &mut Request, path: &str) {
    let original = request.uri();
    let path_and_query = match original.query() {
        Some(query) => format!("{path}?{query}"),
        None => path.to_string(),
    };
    let mut parts = original.clone().into_parts();
    parts.path_and_query = Some(path_and_query.parse().expect("valid routed path and query"));
    *request.uri_mut() = http::Uri::from_parts(parts).expect("valid routed request URI");
}

/// Builds the hosted installation for a published package and its stable game database.
/// Recovery is explicit so preparing a replacement does not mutate experiment lifecycle.
pub(crate) async fn build_hosted_game(
    loaded_game: LoadedGame,
    metadata: GameMetadata,
    database_url: &str,
    participant_base_url: &str,
    client_dist: Option<&std::path::Path>,
    wasm_agents: Vec<crate::agent::wasm::WasmAgentFactory>,
) -> Result<HostedRuntime> {
    let mut config = ExperimentConfig::default();
    config.database.url = database_url.to_string();
    let game_path = format!("/{}", metadata.id);
    let participant_origin = participant_base_url
        .trim_end_matches('/')
        .strip_suffix(&game_path)
        .context("participant URL must end with the published game ID")?;
    crate::config::validate_public_origin("hosted participant origin", participant_origin)?;
    config.server.public_base_url = format!("{participant_origin}/e/{}~", metadata.id);
    config.server.client_dist_path = client_dist
        .map(|path| {
            path.to_str()
                .context("participant client directory is not UTF-8")
        })
        .transpose()?
        .map(str::to_string);
    config.validate()?;
    let game_id = metadata.id.clone();
    let mut agents: Vec<crate::agents::SharedAgentFactory> = wasm_agents
        .into_iter()
        .map(|factory| Arc::new(factory) as crate::agents::SharedAgentFactory)
        .collect();
    crate::server::register_default_remote_agent(&mut agents);
    let options_factory: ServeOptionsFactory =
        Arc::new(move |config| crate::server::experiment_serve_options(&agents, config));
    let options = options_factory(&config)?;
    let routers = Arc::new(RwLock::new(HashMap::new()));
    let installation = open_installation(
        loaded_game,
        metadata,
        database_url,
        Arc::downgrade(&routers),
    )
    .await?;
    let context = Arc::new(ServerContext {
        installation,
        config,
        agent_definitions: registered_agent_definitions(&options)?,
        has_tts_provider: options.tts_provider.is_some(),
        has_transcription_provider: options.transcription_provider.is_some(),
    });
    let dashboard = routes::hosted_dashboard_routes(&context);
    spawn_load_sampler(context.clone());
    Ok(HostedRuntime {
        game_id,
        dashboard,
        replays: ReplayGrants::default(),
        host: Arc::new(GameHost {
            hosted: true,
            dashboard: context,
            options_factory,
            routers,
            health_slots: Semaphore::new(1),
        }),
    })
}

/// Builds and runs one game component's multi-experiment HTTP and WebSocket server.
pub async fn serve_game<F>(
    loaded_game: LoadedGame,
    bootstrap: ExperimentConfig,
    descriptor: GameMetadata,
    bind_addr: SocketAddr,
    options_factory: F,
) -> Result<()>
where
    F: Fn(&ExperimentConfig) -> Result<ServeOptions> + Send + Sync + 'static,
{
    let router = build_game_router(loaded_game, bootstrap, descriptor, options_factory).await?;
    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(termination_signal())
    .await?;
    Ok(())
}

/// Resolves ordinary process termination signals into Axum graceful shutdown.
pub(crate) async fn termination_signal() {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("install SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = terminate.recv() => {},
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// Builds an Axum router for tests or for embedding in a custom server runner.
#[cfg(any(test, feature = "internal-tools"))]
pub async fn build_router(
    loaded_game: LoadedGame,
    config: ExperimentConfig,
    options: ServeOptions,
) -> Result<Router> {
    Ok(build_standalone_router(loaded_game, config, options)
        .await?
        .primary)
}

/// Builds the single-experiment runtime before exposing its participant routes.
#[cfg(any(test, feature = "internal-tools"))]
async fn build_standalone_router(
    loaded_game: LoadedGame,
    config: ExperimentConfig,
    options: ServeOptions,
) -> Result<BuiltRouters> {
    let descriptor = options
        .game_descriptor
        .clone()
        .unwrap_or_else(|| GameMetadata {
            id: "embedded-game".to_string(),
            name: "Embedded game".to_string(),
            version: semver::Version::parse(env!("CARGO_PKG_VERSION"))
                .expect("semantic crate version"),
            build_manifest: Value::Null,
        });
    let installation =
        open_installation(loaded_game, descriptor, &config.database.url, Weak::new()).await?;
    build_experiment_router(
        installation,
        config,
        options,
        false,
        false,
        MountIdentity::default(),
    )
    .await
}

/// Gives transition tests access to the live coordinator and its real backing store.
#[cfg(test)]
pub(super) async fn build_router_with_state(
    loaded_game: LoadedGame,
    config: ExperimentConfig,
) -> Result<(Router, Arc<AppState>)> {
    let built = build_standalone_router(loaded_game, config, ServeOptions::default()).await?;
    Ok((built.primary, built.state))
}

/// Opens installation services shared by the catalogue and all experiment revisions.
async fn open_installation(
    loaded_game: LoadedGame,
    mut descriptor: GameMetadata,
    database_url: &str,
    router_cache: Weak<ExperimentMountCache>,
) -> Result<Arc<Installation>> {
    descriptor.validate()?;
    descriptor.pin_component(&loaded_game)?;
    let store = experiment_store_from_url(database_url).await?;
    Ok(Arc::new(Installation {
        loaded_game,
        version_manifest: version_manifest(Some(descriptor.build_manifest.clone())),
        game_descriptor: descriptor,
        game_settings: RwLock::new(store.game_settings().await?),
        admin_auth: Arc::new(AdminAuthenticator::load(store.clone()).await?),
        store,
        telemetry: Default::default(),
        runtime_registry: Default::default(),
        prolific_preflight_cache: Default::default(),
        runtime_router_cache: router_cache,
        runtime_router_build_locks: std::array::from_fn(|_| Arc::new(Mutex::new(()))),
    }))
}

/// Validates the agent catalogue before any routes advertise it to experimenters.
fn registered_agent_definitions(options: &ServeOptions) -> Result<Vec<AgentDefinition>> {
    let definitions = if options.agent_definitions.is_empty() {
        options
            .agent_factory
            .iter()
            .map(|factory| factory.definition())
            .collect()
    } else {
        options.agent_definitions.clone()
    };
    let mut ids = HashSet::new();
    for definition in &definitions {
        definition.validate()?;
        if !ids.insert(&definition.id) {
            anyhow::bail!(
                "agent definition {:?} is registered more than once",
                definition.id
            );
        }
    }
    Ok(definitions)
}
