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
struct BuiltRouters<A: Game> {
    state: Arc<AppState<A>>,
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
type ServeOptionsFactory<A> =
    Arc<dyn Fn(&ExperimentConfig) -> Result<ServeOptions<A>> + Send + Sync>;

/// Installation dispatcher that owns the router cache and lazily mounts experiments.
struct GameHost<A: Game> {
    dashboard: Arc<ServerContext<A>>,
    options_factory: ServeOptionsFactory<A>,
    routers: Arc<ExperimentMountCache<A>>,
    health_slots: Semaphore,
}

/// Checks installation storage without constructing an additional experiment runtime.
async fn game_health<A: Game>(State(host): State<Arc<GameHost<A>>>) -> Response {
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

/// Builds one compiled game's multi-experiment router around a migration seed configuration.
pub async fn build_game_router<A, GF, F>(
    game_factory: GF,
    bootstrap: ExperimentConfig,
    descriptor: GameMetadata,
    options_factory: F,
) -> Result<Router>
where
    A: Game,
    GF: GameFactory<Game = A>,
    F: Fn(&ExperimentConfig) -> Result<ServeOptions<A>> + Send + Sync + 'static,
{
    crate::config::validate_public_origin(
        "server.public_base_url",
        &bootstrap.server.public_base_url,
    )?;
    bootstrap.validate()?;
    validate_game_config_contains_no_secrets(&bootstrap.game)?;
    parse_game_config(&game_factory, &bootstrap.game)?;
    descriptor.validate()?;
    let router_cache = Arc::new(RwLock::new(HashMap::new()));
    let installation = open_installation(
        Arc::new(game_factory),
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
        .route("/health", get(game_health::<A>))
        .with_state(host);
    Ok(admin_router
        .merge(host_router)
        .fallback_service(runtime_service))
}

/// Builds and runs one compiled game's multi-experiment HTTP/WebSocket server.
pub async fn serve_game<A, GF, F>(
    game_factory: GF,
    bootstrap: ExperimentConfig,
    descriptor: GameMetadata,
    bind_addr: SocketAddr,
    options_factory: F,
) -> Result<()>
where
    A: Game,
    GF: GameFactory<Game = A>,
    F: Fn(&ExperimentConfig) -> Result<ServeOptions<A>> + Send + Sync + 'static,
{
    let router = build_game_router(game_factory, bootstrap, descriptor, options_factory).await?;
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
async fn termination_signal() {
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
pub async fn build_router<A: Game, GF: GameFactory<Game = A>>(
    game_factory: GF,
    config: ExperimentConfig,
    options: ServeOptions<A>,
) -> Result<Router>
where
{
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
    let installation = open_installation(
        Arc::new(game_factory),
        descriptor,
        &config.database.url,
        Weak::new(),
    )
    .await?;
    Ok(build_experiment_router(
        installation,
        config,
        options,
        false,
        MountIdentity::default(),
    )
    .await?
    .primary)
}

/// Opens installation services shared by the catalogue and all experiment revisions.
async fn open_installation<A: Game>(
    game_factory: Arc<dyn GameFactory<Game = A>>,
    descriptor: GameMetadata,
    database_url: &str,
    router_cache: Weak<ExperimentMountCache<A>>,
) -> Result<Arc<Installation<A>>> {
    descriptor.validate()?;
    let store = experiment_store_from_url(database_url).await?;
    Ok(Arc::new(Installation {
        game_factory,
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
fn registered_agent_definitions<A: Game>(
    options: &ServeOptions<A>,
) -> Result<Vec<AgentDefinition>> {
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
