use super::*;
use tokio::sync::Semaphore;
use tower::ServiceExt;

/// Route trees built around one experiment state before installation-level mounting.
struct BuiltRouters {
    primary: Router,
    runtime_admin: Option<Router>,
}

/// Public namespace selecting one of an experiment runtime's two route surfaces.
#[derive(Clone, Copy)]
enum RuntimeSurface {
    Participant,
    Admin,
}

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
    prolific_preflight_cache: Arc<RwLock<HashMap<String, ProlificPreflightCacheEntry>>>,
    router_cache: Arc<RwLock<HashMap<String, MountedExperimentRouters>>>,
    router_build_locks: Arc<RwLock<HashMap<String, Arc<Mutex<()>>>>>,
}

/// One compiled game's installation-level dispatcher and lazily built experiment routers.
struct GameHost<A: Game> {
    game_factory: Arc<dyn GameFactory<Game = A>>,
    bootstrap: ExperimentConfig,
    descriptor: GameMetadata,
    store: SharedExperimentStore,
    admin_auth: Arc<AdminAuthenticator>,
    game_settings: Arc<RwLock<StoredGameSettings>>,
    options_factory: ServeOptionsFactory<A>,
    routers: Arc<RwLock<HashMap<String, MountedExperimentRouters>>>,
    router_build_locks: Arc<RwLock<HashMap<String, Arc<Mutex<()>>>>>,
    telemetry: Arc<RuntimeTelemetry>,
    runtime_registry: Arc<RwLock<HashMap<String, Weak<AppState<A>>>>>,
    prolific_preflight_cache: Arc<RwLock<HashMap<String, ProlificPreflightCacheEntry>>>,
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
    async fn experiment_router(&self, experiment_id: &str) -> Result<MountedExperimentRouters> {
        if let Some(router) = self.routers.read().await.get(experiment_id).cloned() {
            return Ok(router);
        }
        let build_lock = {
            let mut locks = self.router_build_locks.write().await;
            locks
                .entry(experiment_id.to_string())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        let _build_guard = build_lock.lock().await;
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
        apply_experiment_secrets(&mut config, &stored_secrets);
        let game_secrets = self.store.game_secrets().await?;
        apply_game_provider_secrets(&mut config, &game_secrets);
        parse_game_config(self.game_factory.as_ref(), &config.game).with_context(|| {
            format!("experiment {experiment_id:?} has invalid game configuration")
        })?;
        let mut options = (self.options_factory)(&config)?;
        options.game_descriptor = Some(self.descriptor.clone());
        let built = build_router_with_resources(
            self.game_factory.clone(),
            config,
            options,
            Some(RuntimeShared {
                store: self.store.clone(),
                admin_auth: self.admin_auth.clone(),
                game_settings: self.game_settings.clone(),
                telemetry: self.telemetry.clone(),
                runtime_registry: self.runtime_registry.clone(),
                prolific_preflight_cache: self.prolific_preflight_cache.clone(),
                router_cache: self.routers.clone(),
                router_build_locks: self.router_build_locks.clone(),
            }),
            true,
        )
        .await?;
        let router = MountedExperimentRouters {
            participant: built.primary,
            admin: built
                .runtime_admin
                .expect("persisted runtimes expose administrator routes")
                .layer(Extension(AdminExperimentScope(experiment_id.to_string()))),
        };
        let mut routers = self.routers.write().await;
        Ok(routers
            .entry(experiment_id.to_string())
            .or_insert_with(|| router.clone())
            .clone())
    }
}

/// Resolves one experiment namespace from an unmatched host URI without rewriting it.
async fn dispatch_experiment_service<A: Game>(host: Arc<GameHost<A>>, request: Request) -> Response
where
    A::State: Serialize,
{
    let path = request.uri().path();
    let (surface, relative_path) = if let Some(path) = path.strip_prefix("/e/") {
        (RuntimeSurface::Participant, path)
    } else if let Some(path) = path.strip_prefix("/api/admin/runtime/") {
        (RuntimeSurface::Admin, path)
    } else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let experiment_id = relative_path.split('/').next().filter(|id| !id.is_empty());
    let Some(experiment_id) = experiment_id else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if matches!(surface, RuntimeSurface::Participant) && relative_path == experiment_id {
        return Redirect::temporary(&format!("/e/{experiment_id}/")).into_response();
    }
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
    let router = match surface {
        RuntimeSurface::Participant => router.participant,
        RuntimeSurface::Admin => router.admin,
    };
    router
        .oneshot(request)
        .await
        .unwrap_or_else(|error| match error {})
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
    A::State: Serialize,
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
    let store = experiment_store_from_url(&bootstrap.database.url).await?;
    let admin_auth = Arc::new(AdminAuthenticator::load(store.clone()).await?);
    let game_settings = Arc::new(RwLock::new(store.game_settings().await?));
    let telemetry = Arc::new(RuntimeTelemetry::default());
    let runtime_registry = Arc::new(RwLock::new(HashMap::new()));
    let prolific_preflight_cache = Arc::new(RwLock::new(HashMap::new()));
    let router_cache = Arc::new(RwLock::new(HashMap::new()));
    let router_build_locks = Arc::new(RwLock::new(HashMap::new()));
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
    let game_factory: Arc<dyn GameFactory<Game = A>> = Arc::new(game_factory);
    let options_factory: ServeOptionsFactory<A> = Arc::new(options_factory);
    let shared = RuntimeShared {
        store: store.clone(),
        admin_auth: admin_auth.clone(),
        game_settings: game_settings.clone(),
        telemetry: telemetry.clone(),
        runtime_registry: runtime_registry.clone(),
        prolific_preflight_cache: prolific_preflight_cache.clone(),
        router_cache: router_cache.clone(),
        router_build_locks: router_build_locks.clone(),
    };
    let mut admin_config = bootstrap.clone();
    admin_config.experiment.id = Some("__dashboard__".to_string());
    let mut admin_options = options_factory(&admin_config)?;
    admin_options.game_descriptor = Some(descriptor.clone());
    let admin_router = build_router_with_resources(
        game_factory.clone(),
        admin_config,
        admin_options,
        Some(shared),
        false,
    )
    .await?
    .primary;
    let host = Arc::new(GameHost {
        game_factory,
        bootstrap,
        descriptor,
        store,
        admin_auth,
        game_settings,
        options_factory,
        routers: router_cache,
        router_build_locks,
        telemetry,
        runtime_registry,
        prolific_preflight_cache,
        admin_router,
        health_slots: Semaphore::new(1),
    });
    let admin_router = host.admin_router.clone();
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
    A::State: Serialize,
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
    A::State: Serialize,
{
    Ok(
        build_router_with_resources(Arc::new(game_factory), config, options, None, true)
            .await?
            .primary,
    )
}

/// Builds one experiment router with optional installation-owned storage and authentication.
async fn build_router_with_resources<A: Game>(
    game_factory: Arc<dyn GameFactory<Game = A>>,
    config: ExperimentConfig,
    options: ServeOptions<A>,
    shared: Option<RuntimeShared<A>>,
    persist_experiment: bool,
) -> Result<BuiltRouters>
where
    A::State: Serialize,
{
    config.validate()?;
    validate_game_config_contains_no_secrets(&config.game)?;
    let game_config = parse_game_config(game_factory.as_ref(), &config.game)?;
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
    let mounted_runtime = shared.is_some() && persist_experiment;
    let participant_prefix = mounted_runtime
        .then(|| format!("/e/{experiment_id}"))
        .unwrap_or_default();
    let runtime_admin_prefix = mounted_runtime
        .then(|| format!("/api/admin/runtime/{experiment_id}"))
        .unwrap_or_default();
    let participant_path = |path: &str| format!("{participant_prefix}{path}");
    let runtime_admin_path = |path: &str| format!("{runtime_admin_prefix}{path}");
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
    let audio_sessions = Arc::new(AudioSessionRegistry::default());
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
        Some(Arc::new(SessionAgentAudioPublisher::new(
            audio_sessions.clone(),
            config.voice.jitter_buffer_ms,
        )) as Arc<dyn AgentAudioPublisher>)
    } else {
        None
    };
    let game_settings = if let Some(shared) = shared.as_ref() {
        shared.game_settings.clone()
    } else {
        Arc::new(RwLock::new(store.game_settings().await?))
    };
    let prolific_api_base_url = game_settings.read().await.prolific_api_base_url.clone();
    let prolific_client = (!config.recruitment.prolific.api_token.is_empty())
        .then(|| {
            crate::prolific::ProlificClient::with_base_url(
                config.recruitment.prolific.api_token.clone(),
                prolific_api_base_url,
            )
        })
        .transpose()?;
    let cors = configured_cors(&config)?;
    let admin_auth = if let Some(shared) = shared.as_ref() {
        shared.admin_auth.clone()
    } else {
        Arc::new(AdminAuthenticator::load(store.clone()).await?)
    };
    let telemetry = shared
        .as_ref()
        .map(|shared| shared.telemetry.clone())
        .unwrap_or_else(|| Arc::new(RuntimeTelemetry::default()));
    let runtime_registry = shared
        .as_ref()
        .map(|shared| shared.runtime_registry.clone())
        .unwrap_or_else(|| Arc::new(RwLock::new(HashMap::new())));
    let prolific_preflight_cache = shared
        .as_ref()
        .map(|shared| shared.prolific_preflight_cache.clone())
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
        game_factory,
        config,
        game_config,
        experiment_id,
        game_descriptor,
        game_settings,
        config_revision,
        experiment_lifecycle: RwLock::new(
            ExperimentLifecycle::parse(&lifecycle).map_err(|error| anyhow!(error.message))?,
        ),
        participant_url: RwLock::new(None),
        memory: RwLock::new(MemoryState::default()),
        session_admission: Arc::new(Mutex::new(())),
        store,
        session_buses: RwLock::new(HashMap::new()),
        agent_factory: options.agent_factory,
        agent_definitions,
        started_agents: RwLock::new(HashSet::new()),
        pending_agents: Mutex::new(HashMap::new()),
        agent_inboxes: RwLock::new(HashMap::new()),
        tts_provider,
        audio_publisher,
        audio_sessions,
        transcription_provider,
        prolific_client: RwLock::new(prolific_client),
        prolific_study: RwLock::new(None),
        prolific_preflight_cache,
        committed_transcripts: RwLock::new(HashSet::new()),
        participant_auth: ParticipantAuthenticator::default(),
        upgrade_tickets: UpgradeTicketStore::default(),
        admin_auth,
        participant_creation_window: RwLock::new(ParticipantCreationRate::default()),
        chat_submission_budgets: RwLock::new(HashMap::new()),
        rejection_windows: RwLock::new(HashMap::new()),
        telemetry,
        runtime_registry: runtime_registry.clone(),
        runtime_router_cache: shared.as_ref().map(|shared| shared.router_cache.clone()),
        runtime_router_build_locks: shared
            .as_ref()
            .map(|shared| shared.router_build_locks.clone()),
        session_transition_locks: RwLock::new(HashMap::new()),
        game_connections: RwLock::new(HashMap::new()),
        audio_connections: RwLock::new(HashMap::new()),
        version_manifest,
    });
    finalize_interrupted_sessions(&state).await?;
    if persist_experiment {
        runtime_registry
            .write()
            .await
            .insert(state.experiment_id.clone(), Arc::downgrade(&state));
    } else if !clean_admin_sessions {
        spawn_load_sampler(state.clone());
    }

    let public_routes = Router::new()
        .route(&participant_path("/health"), get(health::<A>))
        .route(&participant_path("/api/config"), get(public_config::<A>))
        .route(
            &participant_path("/api/participants"),
            post(create_participant::<A>),
        );
    let public_admin_routes = Router::new()
        .route(&participant_path("/admin"), get(admin_entry))
        .route(&participant_path("/admin/"), get(admin_entry))
        .route(
            &participant_path("/admin/login"),
            get(admin_login_page::<A>),
        )
        .route(
            &participant_path("/api/admin/setup"),
            post(admin_setup::<A>),
        )
        .route(
            &participant_path("/api/admin/login"),
            post(admin_login::<A>),
        )
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_admin_network::<A>,
        ));
    let participant_routes = Router::new()
        .route(&participant_path("/api/consent"), post(consent::<A>))
        .route(
            &participant_path("/api/participant-state"),
            get(get_participant_state::<A>),
        )
        .route(
            &participant_path("/api/sessions"),
            post(create_session::<A>),
        )
        .route(
            &participant_path("/api/sessions/:public_session_id/leave"),
            post(leave_session::<A>),
        )
        .route(
            &participant_path("/api/sessions/:public_session_id/game-session"),
            post(game_session::<A>),
        )
        .route(
            &participant_path("/api/sessions/:public_session_id/audio-session"),
            post(audio_session::<A>),
        )
        .route(
            &participant_path("/api/sessions/:public_session_id/voice-diagnostics"),
            post(add_voice_diagnostic::<A>),
        )
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_participant_auth::<A>,
        ));
    let admin_routes = Router::new()
        .route(
            &participant_path("/admin/experiments"),
            get(admin_experiments_page),
        )
        .route(
            &participant_path("/admin/assets/admin-dashboard.css"),
            get(admin_dashboard_css),
        )
        .route(
            &participant_path("/admin/assets/admin-dashboard.js"),
            get(admin_dashboard_javascript),
        )
        .route(
            &participant_path("/admin/assets/admin-dashboard-state.js"),
            get(admin_dashboard_state_javascript),
        )
        .route(
            &participant_path("/admin/assets/admin-dashboard-format.js"),
            get(admin_dashboard_format_javascript),
        )
        .route(
            &participant_path("/admin/assets/admin-dashboard-api.js"),
            get(admin_dashboard_api_javascript),
        )
        .route(
            &participant_path("/admin/privacy"),
            get(admin_privacy_page::<A>),
        )
        .route(
            &participant_path("/api/admin/privacy"),
            get(admin_privacy_json::<A>),
        )
        .route(
            &participant_path("/api/admin/privacy.json"),
            get(admin_privacy_json_download::<A>),
        )
        .route(
            &participant_path("/api/admin/privacy.md"),
            get(admin_privacy_markdown_download::<A>),
        )
        .route(
            &participant_path("/api/admin/experiment"),
            get(admin_experiment::<A>),
        )
        .route(
            &participant_path("/api/admin/experiments"),
            get(admin_experiments::<A>).post(admin_create_experiment::<A>),
        )
        .route(
            &participant_path("/api/admin/experiments/:experiment_id/clone"),
            post(admin_clone_experiment::<A>),
        )
        .route(
            &participant_path("/api/admin/experiments/:experiment_id/config"),
            get(admin_experiment_config::<A>).post(admin_save_experiment_config::<A>),
        )
        .route(
            &participant_path("/api/admin/experiments/:experiment_id/privacy"),
            get(admin_experiment_privacy_json::<A>),
        )
        .route(
            &participant_path("/api/admin/experiments/:experiment_id/privacy.json"),
            get(admin_experiment_privacy_json_download::<A>),
        )
        .route(
            &participant_path("/api/admin/experiments/:experiment_id/privacy.md"),
            get(admin_experiment_privacy_markdown_download::<A>),
        )
        .route(
            &participant_path("/api/admin/experiments/:experiment_id/secrets/reveal"),
            post(admin_reveal_experiment_secret::<A>),
        )
        .route(
            &participant_path("/api/admin/experiments/:experiment_id/config/validate"),
            post(admin_validate_game_config::<A>),
        )
        .route(
            &participant_path("/api/admin/experiments/:experiment_id/revisions"),
            get(admin_experiment_revisions::<A>),
        )
        .route(
            &participant_path("/api/admin/experiments/:experiment_id/catalogue"),
            post(admin_update_experiment_catalogue::<A>),
        )
        .route(
            &participant_path("/api/admin/experiments/:experiment_id/archive"),
            post(admin_archive_experiment::<A>),
        )
        .route(
            &participant_path("/api/admin/game/settings"),
            get(admin_game_settings::<A>).post(admin_update_game_settings::<A>),
        )
        .route(
            &participant_path("/api/admin/game/secrets/reveal"),
            post(admin_reveal_game_secret::<A>),
        )
        .route(
            &participant_path("/api/admin/experiment/status"),
            post(admin_update_experiment_status::<A>),
        )
        .route(
            &participant_path("/api/admin/sessions"),
            get(admin_sessions::<A>),
        )
        .route(&participant_path("/api/admin/load"), get(admin_load::<A>))
        .route(
            &participant_path("/api/admin/sessions/:session_id"),
            get(admin_session_detail::<A>),
        )
        .route(
            &participant_path("/api/admin/sessions/:session_id/events"),
            get(admin_session_events::<A>),
        )
        .route(
            &participant_path("/api/admin/export"),
            get(admin_export::<A>),
        )
        .route(
            &participant_path("/api/admin/export-schema"),
            get(admin_corpus_export_schema),
        )
        .route(
            &participant_path("/api/admin/participants/:research_id/deletion"),
            get(admin_participant_deletion_preview::<A>).post(admin_delete_participant_data::<A>),
        )
        .route(
            &participant_path("/api/admin/logout"),
            post(admin_logout::<A>),
        )
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_admin_auth::<A>,
        ))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_admin_network::<A>,
        ));
    let websocket_routes = Router::new()
        .route(
            &participant_path("/ws/game/:public_session_id"),
            get(game_socket::<A>),
        )
        .route(
            &participant_path("/ws/audio/:public_session_id"),
            get(audio_socket::<A>),
        );
    let api = if persist_experiment {
        Router::new()
            .merge(public_routes)
            .merge(public_admin_routes)
            .merge(participant_routes)
            .merge(admin_routes)
            .merge(websocket_routes)
    } else {
        Router::new().merge(public_admin_routes).merge(admin_routes)
    }
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
        spawn_load_sampler(state.clone());
    }

    let api = if let Some(dist) = client_dist
        .filter(|_| persist_experiment)
        .filter(|path| path.join("index.html").is_file())
    {
        let index = dist.join("index.html");
        api.route_service(&participant_path("/"), ServeFile::new(index))
            .nest_service(
                &participant_path("/assets"),
                ServeDir::new(dist.join("assets")),
            )
    } else {
        api
    };
    let runtime_admin = persist_experiment.then(|| {
        Router::new()
            .route(
                &runtime_admin_path("/experiment/status"),
                post(admin_update_experiment_status::<A>),
            )
            .route(&runtime_admin_path("/sessions"), get(admin_sessions::<A>))
            .route(
                &runtime_admin_path("/sessions/:session_id"),
                get(admin_session_detail::<A>),
            )
            .route(
                &runtime_admin_path("/sessions/:session_id/events"),
                get(admin_session_events::<A>),
            )
            .route(&runtime_admin_path("/export"), get(admin_export::<A>))
            .route(
                &runtime_admin_path("/participants/:research_id/deletion"),
                get(admin_participant_deletion_preview::<A>)
                    .post(admin_delete_participant_data::<A>),
            )
            .route_layer(middleware::from_fn_with_state(
                state.clone(),
                require_admin_auth::<A>,
            ))
            .route_layer(middleware::from_fn_with_state(
                state.clone(),
                require_admin_network::<A>,
            ))
            .layer(RequestBodyLimitLayer::new(64 * 1024))
            .layer(middleware::from_fn_with_state(
                state.clone(),
                security_headers::<A>,
            ))
            .layer(middleware::from_fn_with_state(
                state.clone(),
                track_request_load::<A>,
            ))
            .with_state(state)
    });
    Ok(BuiltRouters {
        primary: api,
        runtime_admin,
    })
}
