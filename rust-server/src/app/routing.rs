use super::*;
use tokio::sync::Semaphore;
use tower::ServiceExt;

/// Shared factory for runtime components which may vary with experiment configuration.
type ServeOptionsFactory<A> =
    Arc<dyn Fn(&ExperimentConfig) -> Result<ServeOptions<A>> + Send + Sync>;

/// Installation-owned resources shared by every experiment runtime.
#[derive(Clone)]
struct RuntimeShared<A: Game> {
    store: SharedExperimentStore,
    admin_auth: Arc<AdminAuthenticator>,
    game_settings: Arc<RwLock<StoredGameSettings>>,
    telemetry: Arc<RuntimeTelemetry>,
    runtime_registry: Arc<RwLock<HashMap<String, Weak<AppState<A>>>>>,
}

/// One compiled game's installation-level dispatcher and lazily built experiment routers.
struct GameHost<A: Game> {
    adapter: Arc<A>,
    bootstrap: ExperimentConfig,
    descriptor: GameMetadata,
    store: SharedExperimentStore,
    admin_auth: Arc<AdminAuthenticator>,
    game_settings: Arc<RwLock<StoredGameSettings>>,
    options_factory: ServeOptionsFactory<A>,
    routers: RwLock<HashMap<String, Router>>,
    telemetry: Arc<RuntimeTelemetry>,
    runtime_registry: Arc<RwLock<HashMap<String, Weak<AppState<A>>>>>,
    admin_router: Router,
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
    match tokio::time::timeout(Duration::from_secs(2), host.store.health_check()).await {
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

impl<A: Game> GameHost<A>
where
    A::State: Serialize,
{
    /// Returns an existing router or constructs one from the experiment's stored revision.
    async fn experiment_router(&self, experiment_id: &str) -> Result<Router> {
        if let Some(router) = self.routers.read().await.get(experiment_id).cloned() {
            return Ok(router);
        }
        let definition = self
            .store
            .experiment_definition(experiment_id)
            .await?
            .ok_or_else(|| anyhow!("experiment {experiment_id:?} was not found"))?;
        let mut config =
            experiment_config_from_json(definition.config, &self.bootstrap, experiment_id)
                .with_context(|| {
                    format!("experiment {experiment_id:?} has invalid configuration")
                })?;
        let stored_secrets = self.store.experiment_secrets(experiment_id).await?;
        apply_bootstrap_settings(&mut config, &self.bootstrap, experiment_id);
        apply_experiment_secrets(&mut config, &stored_secrets);
        let game_secrets = self.store.game_secrets().await?;
        apply_game_provider_secrets(&mut config, &game_secrets);
        parse_game_config(self.adapter.as_ref(), &config.game).with_context(|| {
            format!("experiment {experiment_id:?} has invalid game configuration")
        })?;
        let mut options = (self.options_factory)(&config)?;
        options.game_descriptor = Some(self.descriptor.clone());
        let router = build_router_with_resources(
            self.adapter.clone(),
            config,
            options,
            Some(RuntimeShared {
                store: self.store.clone(),
                admin_auth: self.admin_auth.clone(),
                game_settings: self.game_settings.clone(),
                telemetry: self.telemetry.clone(),
                runtime_registry: self.runtime_registry.clone(),
            }),
            true,
        )
        .await?;
        let mut routers = self.routers.write().await;
        Ok(routers
            .entry(experiment_id.to_string())
            .or_insert_with(|| router.clone())
            .clone())
    }
}

/// Applies process bootstrap and secret values without persisting them into an experiment revision.
fn apply_bootstrap_settings(
    config: &mut ExperimentConfig,
    bootstrap: &ExperimentConfig,
    experiment_id: &str,
) {
    config.experiment.id = Some(experiment_id.to_string());
    config.server = bootstrap.server.clone();
    config.database = bootstrap.database.clone();
    let experiment_path = format!("/e/{experiment_id}");
    config.server.public_base_url = format!(
        "{}{}",
        bootstrap.server.public_base_url.trim_end_matches('/'),
        experiment_path
    );
}

/// Proxies a participant request to the isolated runtime selected by its path.
async fn dispatch_experiment_request<A: Game>(
    State(host): State<Arc<GameHost<A>>>,
    Path((experiment_id, path)): Path<(String, String)>,
    request: Request,
) -> Response
where
    A::State: Serialize,
{
    dispatch_to_experiment(&host, &experiment_id, &path, request).await
}

/// Proxies an experiment-root request to the selected participant client.
async fn dispatch_experiment_root<A: Game>(
    State(_host): State<Arc<GameHost<A>>>,
    Path(experiment_id): Path<String>,
    _request: Request,
) -> Response
where
    A::State: Serialize,
{
    Redirect::temporary(&format!("/e/{experiment_id}/")).into_response()
}

/// Serves the participant client at the canonical trailing-slash experiment URL.
async fn dispatch_experiment_index<A: Game>(
    State(host): State<Arc<GameHost<A>>>,
    Path(experiment_id): Path<String>,
    request: Request,
) -> Response
where
    A::State: Serialize,
{
    dispatch_to_experiment(&host, &experiment_id, "", request).await
}

/// Proxies an experiment-scoped administrator request to runtime or shared storage.
async fn dispatch_admin_runtime_request<A: Game>(
    State(host): State<Arc<GameHost<A>>>,
    Path((experiment_id, path)): Path<(String, String)>,
    request: Request,
) -> Response
where
    A::State: Serialize,
{
    let child_path = format!("api/admin/{path}");
    let storage_only = path == "sessions"
        || path.starts_with("sessions/")
        || path == "export"
        || path.starts_with("participants/");
    if storage_only {
        dispatch_to_router(
            host.admin_router.clone(),
            &child_path,
            request,
            Some(AdminExperimentScope(experiment_id)),
        )
        .await
    } else {
        dispatch_to_experiment(&host, &experiment_id, &child_path, request).await
    }
}

/// Proxies configuration reads and invalidates an inactive runtime after a successful save.
async fn dispatch_experiment_config_request<A: Game>(
    State(host): State<Arc<GameHost<A>>>,
    Path(experiment_id): Path<String>,
    request: Request,
) -> Response
where
    A::State: Serialize,
{
    let mutation = request.method() != Method::GET;
    let child_path = format!("api/admin/experiments/{experiment_id}/config");
    let response = dispatch_to_router(host.admin_router.clone(), &child_path, request, None).await;
    if mutation && response.status().is_success() {
        host.routers.write().await.remove(&experiment_id);
    }
    response
}

/// Proxies storage-only archival and evicts a stale cached runtime after success.
async fn dispatch_experiment_archive_request<A: Game>(
    State(host): State<Arc<GameHost<A>>>,
    Path(experiment_id): Path<String>,
    request: Request,
) -> Response
where
    A::State: Serialize,
{
    let child_path = format!("api/admin/experiments/{experiment_id}/archive");
    let response = dispatch_to_router(host.admin_router.clone(), &child_path, request, None).await;
    if response.status().is_success() {
        host.routers.write().await.remove(&experiment_id);
    }
    response
}

/// Proxies game-server-wide administration through the primary runtime's shared auth surface.
async fn dispatch_primary_request<A: Game>(
    State(host): State<Arc<GameHost<A>>>,
    Path(path): Path<String>,
    request: Request,
) -> Response
where
    A::State: Serialize,
{
    let child_path = format!("api/admin/{path}");
    dispatch_to_router(host.admin_router.clone(), &child_path, request, None).await
}

/// Proxies a fixed administrator root path through the primary experiment router.
async fn dispatch_primary_root<A: Game>(
    State(host): State<Arc<GameHost<A>>>,
    request: Request,
) -> Response
where
    A::State: Serialize,
{
    let path = request.uri().path().trim_start_matches('/').to_string();
    dispatch_to_router(host.admin_router.clone(), &path, request, None).await
}

/// Rewrites one outer path and invokes a fully layered child Axum router.
async fn dispatch_to_experiment<A: Game>(
    host: &Arc<GameHost<A>>,
    experiment_id: &str,
    path: &str,
    request: Request,
) -> Response
where
    A::State: Serialize,
{
    dispatch_to_experiment_scoped(host, experiment_id, path, request, None).await
}

/// Rewrites one outer path and optionally attaches a storage experiment scope.
async fn dispatch_to_experiment_scoped<A: Game>(
    host: &Arc<GameHost<A>>,
    experiment_id: &str,
    path: &str,
    request: Request,
    admin_scope: Option<AdminExperimentScope>,
) -> Response
where
    A::State: Serialize,
{
    let router = match host.experiment_router(experiment_id).await {
        Ok(router) => router,
        Err(error) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": error.to_string()})),
            )
                .into_response()
        }
    };
    dispatch_to_router(router, path, request, admin_scope).await
}

/// Rewrites one outer request and invokes an already constructed child router.
async fn dispatch_to_router(
    router: Router,
    path: &str,
    mut request: Request,
    admin_scope: Option<AdminExperimentScope>,
) -> Response {
    let query = request
        .uri()
        .query()
        .map(|query| format!("?{query}"))
        .unwrap_or_default();
    let rewritten = format!("/{}{}", path.trim_start_matches('/'), query);
    match rewritten.parse() {
        Ok(uri) => *request.uri_mut() = uri,
        Err(error) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": format!("invalid routed request: {error}")})),
            )
                .into_response()
        }
    }
    // Axum accumulates path captures in a private request extension. A dispatch is a
    // fresh routing boundary, so retaining the outer wildcard would make child
    // `Path<T>` extractors observe both outer and inner parameters.
    let connect_info = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .cloned();
    let on_upgrade = request
        .extensions()
        .get::<hyper::upgrade::OnUpgrade>()
        .cloned();
    *request.extensions_mut() = http::Extensions::new();
    if let Some(connect_info) = connect_info {
        request.extensions_mut().insert(connect_info);
    }
    if let Some(on_upgrade) = on_upgrade {
        request.extensions_mut().insert(on_upgrade);
    }
    if let Some(admin_scope) = admin_scope {
        request.extensions_mut().insert(admin_scope);
    }
    router
        .oneshot(request)
        .await
        .unwrap_or_else(|error| match error {})
}

/// Builds one compiled game's multi-experiment router around a migration seed configuration.
pub async fn build_game_router<A, F>(
    adapter: A,
    bootstrap: ExperimentConfig,
    descriptor: GameMetadata,
    options_factory: F,
) -> Result<Router>
where
    A: Game,
    A::State: Serialize,
    F: Fn(&ExperimentConfig) -> Result<ServeOptions<A>> + Send + Sync + 'static,
{
    bootstrap.validate()?;
    validate_game_config_contains_no_secrets(&bootstrap.game)?;
    parse_game_config(&adapter, &bootstrap.game)?;
    descriptor.validate()?;
    let store = experiment_store_from_url(&bootstrap.database.url).await?;
    let admin_auth = Arc::new(AdminAuthenticator::load(store.clone()).await?);
    let game_settings = Arc::new(RwLock::new(store.game_settings().await?));
    let telemetry = Arc::new(RuntimeTelemetry::default());
    let runtime_registry = Arc::new(RwLock::new(HashMap::new()));
    let cleanup_auth = admin_auth.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        loop {
            interval.tick().await;
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
    let adapter = Arc::new(adapter);
    let options_factory: ServeOptionsFactory<A> = Arc::new(options_factory);
    let shared = RuntimeShared {
        store: store.clone(),
        admin_auth: admin_auth.clone(),
        game_settings: game_settings.clone(),
        telemetry: telemetry.clone(),
        runtime_registry: runtime_registry.clone(),
    };
    let mut admin_config = bootstrap.clone();
    admin_config.experiment.id = Some("__dashboard__".to_string());
    let mut admin_options = options_factory(&admin_config)?;
    admin_options.game_descriptor = Some(descriptor.clone());
    let admin_router = build_router_with_resources(
        adapter.clone(),
        admin_config,
        admin_options,
        Some(shared),
        false,
    )
    .await?;
    let host = Arc::new(GameHost {
        adapter,
        bootstrap,
        descriptor,
        store,
        admin_auth,
        game_settings,
        options_factory,
        routers: RwLock::new(HashMap::new()),
        telemetry,
        runtime_registry,
        admin_router,
        health_slots: Semaphore::new(1),
    });
    Ok(Router::new()
        .route("/", get(game_root))
        .route("/e/:experiment_id", any(dispatch_experiment_root::<A>))
        .route("/e/:experiment_id/", any(dispatch_experiment_index::<A>))
        .route(
            "/e/:experiment_id/*path",
            any(dispatch_experiment_request::<A>),
        )
        .route(
            "/api/admin/runtime/:experiment_id/*path",
            any(dispatch_admin_runtime_request::<A>),
        )
        .route(
            "/api/admin/experiments/:experiment_id/config",
            any(dispatch_experiment_config_request::<A>),
        )
        .route(
            "/api/admin/experiments/:experiment_id/archive",
            any(dispatch_experiment_archive_request::<A>),
        )
        .route("/admin", any(dispatch_primary_root::<A>))
        .route("/admin/", any(dispatch_primary_root::<A>))
        .route("/admin/login", any(dispatch_primary_root::<A>))
        .route("/admin/experiments", any(dispatch_primary_root::<A>))
        .route("/admin/privacy", any(dispatch_primary_root::<A>))
        .route("/api/admin/*path", any(dispatch_primary_request::<A>))
        .layer(ConcurrencyLimitLayer::new(256))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(30),
        ))
        .route("/health", get(game_health::<A>))
        .with_state(host))
}

/// Builds and runs one compiled game's multi-experiment HTTP/WebSocket server.
pub async fn serve_game<A, F>(
    adapter: A,
    bootstrap: ExperimentConfig,
    descriptor: GameMetadata,
    bind_addr: SocketAddr,
    options_factory: F,
) -> Result<()>
where
    A: Game,
    A::State: Serialize,
    F: Fn(&ExperimentConfig) -> Result<ServeOptions<A>> + Send + Sync + 'static,
{
    let router = build_game_router(adapter, bootstrap, descriptor, options_factory).await?;
    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}

/// Builds an Axum router for tests or for embedding in a custom server runner.
#[cfg(any(test, feature = "internal-tools"))]
pub async fn build_router<A: Game>(
    adapter: A,
    config: ExperimentConfig,
    options: ServeOptions<A>,
) -> Result<Router>
where
    A::State: Serialize,
{
    build_router_with_resources(Arc::new(adapter), config, options, None, true).await
}

/// Builds one experiment router with optional installation-owned storage and authentication.
async fn build_router_with_resources<A: Game>(
    adapter: Arc<A>,
    config: ExperimentConfig,
    options: ServeOptions<A>,
    shared: Option<RuntimeShared<A>>,
    persist_experiment: bool,
) -> Result<Router>
where
    A::State: Serialize,
{
    config.validate()?;
    validate_game_config_contains_no_secrets(&config.game)?;
    let game_config = parse_game_config(adapter.as_ref(), &config.game)?;
    let clean_admin_sessions = shared.is_none();
    let game_descriptor = options
        .game_descriptor
        .clone()
        .unwrap_or_else(|| GameMetadata {
            id: "embedded-game".to_string(),
            name: "Embedded game".to_string(),
            version: semver::Version::parse(env!("CARGO_PKG_VERSION"))
                .expect("parlando package version is semantic"),
            build_manifest: options.game_version_manifest.clone().unwrap_or(Value::Null),
        });
    game_descriptor.validate()?;
    let speechmatics_api_key = config.speechmatics.api_key.clone();
    let tts_api_key = config.tts.api_key.clone();
    let store = if let Some(shared) = shared.as_ref() {
        shared.store.clone()
    } else {
        experiment_store_from_url(&config.database.url).await?
    };
    let experiment_id = config
        .experiment
        .id
        .clone()
        .unwrap_or_else(generated_experiment_id);
    let version_manifest = version_manifest(options.game_version_manifest.clone());
    let (lifecycle, config_revision) = if persist_experiment {
        let lifecycle = store
            .ensure_experiment(ExperimentRecord {
                experiment_id: experiment_id.clone(),
                game_version: game_descriptor.version.to_string(),
                config: persistable_config_json(&config)?,
                server_version: Some(env!("CARGO_PKG_VERSION").to_string()),
                version_manifest: Some(version_manifest.clone()),
                status: "inactive".to_string(),
                notes: None,
            })
            .await?;
        let stored_experiment = store
            .experiment_definition(&experiment_id)
            .await?
            .ok_or_else(|| anyhow!("configured experiment was not stored"))?;
        (lifecycle, stored_experiment.config_revision)
    } else {
        ("inactive".to_string(), 0)
    };
    let client_dist = config.server.client_dist_path.as_ref().map(PathBuf::from);
    let tts_provider = if options.tts_provider.is_some() {
        options.tts_provider
    } else if config.tts.enabled && !tts_api_key.is_empty() && !config.tts.voice_id.is_empty() {
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
    } else if config.transcription.enabled
        && config.transcription.provider == "speechmatics"
        && !speechmatics_api_key.is_empty()
    {
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
    let admin_auth = if let Some(shared) = shared.as_ref() {
        shared.admin_auth.clone()
    } else {
        Arc::new(AdminAuthenticator::load(store.clone()).await?)
    };
    let game_settings = if let Some(shared) = shared.as_ref() {
        shared.game_settings.clone()
    } else {
        Arc::new(RwLock::new(store.game_settings().await?))
    };
    let telemetry = shared
        .as_ref()
        .map(|shared| shared.telemetry.clone())
        .unwrap_or_else(|| Arc::new(RuntimeTelemetry::default()));
    let runtime_registry = shared
        .as_ref()
        .map(|shared| shared.runtime_registry.clone())
        .unwrap_or_else(|| Arc::new(RwLock::new(HashMap::new())));
    let agent_definitions = if options.agent_definitions.is_empty() {
        options
            .agent_factory
            .iter()
            .map(|factory| factory.definition())
            .collect()
    } else {
        options.agent_definitions
    };
    let mut definition_ids = HashSet::new();
    for definition in &agent_definitions {
        definition.validate()?;
        if !definition_ids.insert(definition.id.clone()) {
            anyhow::bail!(
                "agent definition {:?} is registered more than once",
                definition.id
            );
        }
    }
    let state = Arc::new(AppState {
        adapter,
        config,
        game_config,
        experiment_id,
        game_descriptor,
        game_settings,
        config_revision,
        experiment_lifecycle: RwLock::new(
            ExperimentLifecycle::parse(&lifecycle).map_err(|error| anyhow!(error.message))?,
        ),
        memory: RwLock::new(MemoryState::default()),
        store,
        room_buses: RwLock::new(HashMap::new()),
        agent_factory: options.agent_factory,
        agent_definitions,
        started_agents: RwLock::new(HashSet::new()),
        pending_agents: Mutex::new(HashMap::new()),
        agent_inboxes: RwLock::new(HashMap::new()),
        tts_provider,
        audio_publisher,
        audio_rooms,
        transcription_provider,
        committed_transcripts: RwLock::new(HashSet::new()),
        participant_auth: ParticipantAuthenticator::default(),
        upgrade_tickets: UpgradeTicketStore::default(),
        admin_auth,
        participant_creation_window: RwLock::new(ParticipantCreationRate::default()),
        chat_submission_budgets: RwLock::new(HashMap::new()),
        rejection_windows: RwLock::new(HashMap::new()),
        telemetry,
        runtime_registry: runtime_registry.clone(),
        room_transition_locks: RwLock::new(HashMap::new()),
        game_connections: RwLock::new(HashMap::new()),
        audio_connections: RwLock::new(HashMap::new()),
        version_manifest,
    });
    if persist_experiment {
        runtime_registry
            .write()
            .await
            .insert(state.experiment_id.clone(), Arc::downgrade(&state));
    } else if !clean_admin_sessions {
        spawn_load_sampler(state.clone());
    }

    let public_routes = Router::new()
        .route("/health", get(health::<A>))
        .route("/api/config", get(public_config::<A>))
        .route("/api/participants", post(create_participant::<A>));
    let public_admin_routes = Router::new()
        .route("/admin", get(admin_entry))
        .route("/admin/", get(admin_entry))
        .route("/admin/login", get(admin_login_page::<A>))
        .route("/api/admin/setup", post(admin_setup::<A>))
        .route("/api/admin/login", post(admin_login::<A>))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_admin_network::<A>,
        ));
    let participant_routes = Router::new()
        .route("/api/consent", post(consent::<A>))
        .route("/api/rooms", post(create_room::<A>))
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
        .route("/admin/experiments", get(admin_experiments_page))
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
        .route("/api/admin/experiment", get(admin_experiment::<A>))
        .route(
            "/api/admin/experiments",
            get(admin_experiments::<A>).post(admin_create_experiment::<A>),
        )
        .route(
            "/api/admin/experiments/:experiment_id/clone",
            post(admin_clone_experiment::<A>),
        )
        .route(
            "/api/admin/experiments/:experiment_id/config",
            get(admin_experiment_config::<A>).post(admin_save_experiment_config::<A>),
        )
        .route(
            "/api/admin/experiments/:experiment_id/privacy",
            get(admin_experiment_privacy_json::<A>),
        )
        .route(
            "/api/admin/experiments/:experiment_id/privacy.json",
            get(admin_experiment_privacy_json_download::<A>),
        )
        .route(
            "/api/admin/experiments/:experiment_id/privacy.md",
            get(admin_experiment_privacy_markdown_download::<A>),
        )
        .route(
            "/api/admin/experiments/:experiment_id/secrets/reveal",
            post(admin_reveal_experiment_secret::<A>),
        )
        .route(
            "/api/admin/experiments/:experiment_id/config/validate",
            post(admin_validate_game_config::<A>),
        )
        .route(
            "/api/admin/experiments/:experiment_id/revisions",
            get(admin_experiment_revisions::<A>),
        )
        .route(
            "/api/admin/experiments/:experiment_id/catalogue",
            post(admin_update_experiment_catalogue::<A>),
        )
        .route(
            "/api/admin/experiments/:experiment_id/archive",
            post(admin_archive_experiment::<A>),
        )
        .route(
            "/api/admin/game/settings",
            get(admin_game_settings::<A>).post(admin_update_game_settings::<A>),
        )
        .route(
            "/api/admin/game/secrets/reveal",
            post(admin_reveal_game_secret::<A>),
        )
        .route(
            "/api/admin/experiment/status",
            post(admin_update_experiment_status::<A>),
        )
        .route("/api/admin/sessions", get(admin_sessions::<A>))
        .route("/api/admin/load", get(admin_load::<A>))
        .route(
            "/api/admin/sessions/:session_id",
            get(admin_session_detail::<A>),
        )
        .route(
            "/api/admin/sessions/:session_id/events",
            get(admin_session_events::<A>),
        )
        .route("/api/admin/export", get(admin_export::<A>))
        .route("/api/admin/export-schema", get(admin_corpus_export_schema))
        .route(
            "/api/admin/participants/:research_id/deletion",
            get(admin_participant_deletion_preview::<A>).post(admin_delete_participant_data::<A>),
        )
        .route("/api/admin/logout", post(admin_logout::<A>))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_admin_auth::<A>,
        ))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_admin_network::<A>,
        ));
    let websocket_routes = Router::new()
        .route("/ws/game/:room_id", get(game_socket::<A>))
        .route("/ws/audio/:room_id", get(audio_socket::<A>));
    let api = Router::new()
        .merge(public_routes)
        .merge(public_admin_routes)
        .merge(participant_routes)
        .merge(admin_routes)
        .merge(websocket_routes)
        .layer(RequestBodyLimitLayer::new(64 * 1024))
        .layer(ConcurrencyLimitLayer::new(256))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(30),
        ))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            security_headers::<A>,
        ))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            track_request_load::<A>,
        ))
        .layer(cors)
        .with_state(state.clone());

    spawn_security_cleanup(state.clone(), clean_admin_sessions);
    if clean_admin_sessions {
        spawn_load_sampler(state);
    }

    if let Some(dist) = client_dist.filter(|path| path.join("index.html").is_file()) {
        let index = dist.join("index.html");
        Ok(api
            .nest_service("/assets", ServeDir::new(dist.join("assets")))
            .fallback_service(ServeDir::new(dist).fallback(ServeFile::new(index))))
    } else {
        Ok(api)
    }
}
