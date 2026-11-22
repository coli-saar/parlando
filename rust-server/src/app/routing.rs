//! Builds the installation router around the loaded game, shared store, and
//! registered runtime services. The mount registry selects the experiment revision.
//! Runtime construction initializes that revision, and the route declarations connect its
//! participant and administration handlers to Axum.

use super::*;
use tokio::sync::Semaphore;
pub(super) mod mount_lock;
mod mounts;
mod routes;
mod runtime;
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

/// Produces a canonical host selection and rejects values requiring credential redaction.
pub(crate) fn validate_hosted_config(config: &ExperimentConfig) -> Result<Value> {
    validate_game_config_contains_no_secrets(&config.game)?;
    let raw = serde_json::to_value(StoredExperimentConfig::from(config))?;
    let sanitized = persistable_config_json(config)?;
    anyhow::ensure!(
        raw == sanitized,
        "experiment configuration contains inline credentials"
    );
    Ok(sanitized)
}

/// Mounts a host-selected component experiment and its operator-only HTTP surface.
pub(crate) async fn build_deployed_experiment(
    loaded_game: LoadedGame,
    config: ExperimentConfig,
    metadata: GameMetadata,
    wasm_agents: Vec<crate::agent::wasm::WasmAgentFactory>,
) -> Result<(Router, Router, Arc<AppState>)> {
    let id = config
        .experiment
        .id
        .clone()
        .context("missing hosted experiment ID")?;
    config.validate()?;
    validate_game_config_contains_no_secrets(&config.game)?;
    parse_game_config(&loaded_game, &config.game)?;
    let dist = config.server.client_dist_path.clone();
    let mut agents: Vec<crate::agents::SharedAgentFactory> = wasm_agents
        .into_iter()
        .map(|factory| Arc::new(factory) as crate::agents::SharedAgentFactory)
        .collect();
    crate::server::register_default_remote_agent(&mut agents);
    let options = crate::server::experiment_serve_options(&agents, &config)?;
    let installation =
        open_installation(loaded_game, metadata, &config.database.url, Weak::new()).await?;
    installation.store.deactivate_open_experiments().await?;
    let built = build_experiment_router(
        installation,
        config,
        options,
        true,
        MountIdentity {
            generation: 1,
            ..MountIdentity::default()
        },
    )
    .await?;
    let admin = routes::experiment_admin_routes(&format!("/api/hosting/experiments/{id}/admin"))
        .with_state(built.state.clone())
        .layer(Extension(AdminExperimentScope(id.clone())))
        .layer(RequestBodyLimitLayer::new(64 * 1024))
        .layer(middleware::from_fn(security_headers));
    let participant = if let Some(dist) = dist {
        built
            .primary
            .fallback_service(Router::new().nest_service(&format!("/e/{id}"), ServeDir::new(dist)))
    } else {
        built.primary
    };
    Ok((participant, admin, built.state))
}

impl AppState {
    /// Reports the runtime's current intake status for the hosted dashboard.
    pub(crate) async fn hosted_intake_status(&self) -> &'static str {
        self.experiment_lifecycle.read().await.as_str()
    }
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
