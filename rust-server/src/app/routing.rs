use super::*;
use tokio::sync::Semaphore;
use tower::ServiceExt;

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
    admission: Arc<Mutex<()>>,
    creation_rate: Arc<RwLock<ParticipantCreationRate>>,
    participants: Arc<ParticipantAuthenticator>,
    tickets: Arc<UpgradeTicketStore>,
}

/// A mounted experiment has one intake owner and may have previous owners draining existing rooms.
pub(super) struct ExperimentMounts<A: Game> {
    current: Option<Arc<BuiltRouters<A>>>,
    draining: HashMap<u64, Arc<BuiltRouters<A>>>,
    identity: MountIdentity,
}

impl<A: Game> ExperimentMounts<A> {
    /// Closes admission on the previous owner while retaining its room and participant services.
    pub(super) fn retire(&mut self) {
        if let Some(owner) = self.current.take() {
            owner
                .state
                .retired
                .store(true, std::sync::atomic::Ordering::Release);
            self.draining.insert(owner.state.mount_id, owner);
        }
    }

    /// Returns owners of existing rooms for repeat Prolific admission.
    pub(super) fn draining_states(&self) -> Vec<Arc<AppState<A>>> {
        self.draining
            .values()
            .map(|owner| owner.state.clone())
            .collect()
    }

    /// Releases the old router after its last live room and registration are gone.
    pub(super) fn finish_draining(&mut self, generation: u64) {
        self.draining.remove(&generation);
    }
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

impl<A: Game> GameHost<A> {
    /// Returns the admission owner for the current configuration, mounting it once if necessary.
    async fn experiment_router(&self, experiment_id: &str) -> Result<Arc<BuiltRouters<A>>> {
        if let Some(router) = self
            .routers
            .read()
            .await
            .get(experiment_id)
            .and_then(|entry| entry.current.clone())
        {
            return Ok(router);
        }
        let _guard = lock_runtime_router_build(&self.dashboard, experiment_id).await;
        let mut identity = {
            let entries = self.routers.read().await;
            match entries.get(experiment_id) {
                Some(entry) => {
                    if let Some(router) = &entry.current {
                        return Ok(router.clone());
                    }
                    entry.identity.clone()
                }
                None => MountIdentity::default(),
            }
        };
        let config = hydrated_experiment_config(&self.dashboard, experiment_id)
            .await
            .map_err(|error| anyhow!(error.message))?;
        let options = (self.options_factory)(&config)?;
        identity.generation += 1;
        let mut built = build_experiment_router(
            self.dashboard.installation.clone(),
            config,
            options,
            true,
            identity.clone(),
        )
        .await?;
        built.runtime_admin = built
            .runtime_admin
            .layer(Extension(AdminExperimentScope(experiment_id.to_string())));
        let router = Arc::new(built);
        let mut entries = self.routers.write().await;
        let entry = entries
            .entry(experiment_id.to_string())
            .or_insert_with(|| ExperimentMounts {
                current: None,
                draining: HashMap::new(),
                identity: identity.clone(),
            });
        entry.identity = identity;
        entry.current = Some(router.clone());
        Ok(router)
    }

    /// Keeps assigned rooms on their original owner. Reopened direct intake can adopt a
    /// still-unassigned registration under the current purpose and consent configuration.
    async fn participant_owner(
        old: &Arc<BuiltRouters<A>>,
        current: Option<&Arc<BuiltRouters<A>>>,
        participant: &str,
    ) -> Arc<BuiltRouters<A>> {
        let Some(current) = current else {
            return old.clone();
        };
        {
            let memory = old.state.memory.read().await;
            if memory
                .sessions
                .values()
                .any(|room| room.participants.contains_key(participant))
            {
                return old.clone();
            }
        }
        let Ok(lifecycle) = require_open_experiment(&current.state).await else {
            return old.clone();
        };
        let _admission = current.state.session_admission.lock().await;
        let mut old_memory = old.state.memory.write().await;
        let Some(registration) = old_memory.participants.remove(participant) else {
            return current.clone();
        };
        drop(old_memory);
        let config = &current.state.context.config;
        if registration.source == "direct"
            && config.direct.enabled
            && !config.recruitment.prolific.enabled
        {
            current
                .state
                .memory
                .write()
                .await
                .create_participant_with_id(
                    registration.id,
                    registration.participant_id,
                    registration.research_id,
                    registration.source,
                    lifecycle.data_purpose().to_owned(),
                );
            old.state
                .participant_auth
                .transfer_registration(participant, old.state.mount_id, current.state.mount_id)
                .await;
        } else {
            // Repeat the configured recruitment flow before admitting a newly eligible seat.
            old.state
                .participant_auth
                .revoke_participant_session(participant, old.state.mount_id)
                .await;
        }
        current.clone()
    }

    /// Finds a previous mount by a credential's issued owner or by the existing session key.
    /// This only selects the route. Its normal authentication and ticket consumption still apply.
    async fn cached_owner(
        &self,
        experiment: &str,
        surface: RuntimeSurface,
        suffix: &str,
        headers: &HeaderMap,
    ) -> Option<Arc<BuiltRouters<A>>> {
        let (current, participants, draining) = {
            let entries = self.routers.read().await;
            let entry = entries.get(experiment)?;
            if entry.draining.is_empty() {
                return entry.current.clone();
            }
            (
                entry.current.clone(),
                entry.identity.participants.clone(),
                entry.draining.clone(),
            )
        };
        if matches!(surface, RuntimeSurface::Participant)
            && (matches!(
                suffix,
                "/api/participant-state" | "/api/consent" | "/api/sessions"
            ) || suffix.starts_with("/api/sessions/"))
        {
            if let Some(credential) = bearer_token(headers) {
                if let Some(principal) = participants.authenticate(credential).await {
                    if let Some(owner) = principal.mount_id.and_then(|id| draining.get(&id)) {
                        return Some(
                            Self::participant_owner(
                                owner,
                                current.as_ref(),
                                &principal.participant_session_id,
                            )
                            .await,
                        );
                    }
                }
            }
            return current;
        }
        let public_id = matches!(surface, RuntimeSurface::Participant)
            .then(|| {
                suffix
                    .strip_prefix("/ws/game/")
                    .or_else(|| suffix.strip_prefix("/ws/audio/"))
            })
            .flatten();
        let durable_id = match surface {
            RuntimeSurface::Admin => suffix.strip_prefix("/sessions/"),
            RuntimeSurface::Participant => suffix.strip_prefix("/api/admin/sessions/"),
        }
        .and_then(|tail| tail.split('/').next()?.parse::<i64>().ok());
        if public_id.is_none() && durable_id.is_none() {
            return current;
        }
        for owner in draining.into_values() {
            let memory = owner.state.memory.read().await;
            let owns = match public_id {
                Some(id) => memory.sessions.contains_key(id),
                None => memory
                    .sessions
                    .values()
                    .any(|room| Some(room.session_id) == durable_id),
            };
            drop(memory);
            if owns {
                return Some(owner);
            }
        }
        current
    }
}

/// Resolves one experiment namespace from an unmatched host URI without rewriting it.
async fn dispatch_experiment_service<A: Game>(
    host: Arc<GameHost<A>>,
    request: Request,
) -> Response {
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
    let suffix = &relative_path[experiment_id.len()..];
    let owner = match host
        .cached_owner(experiment_id, surface, suffix, request.headers())
        .await
    {
        Some(owner) => owner,
        None => match host.experiment_router(experiment_id).await {
            Ok(owner) => host
                .cached_owner(experiment_id, surface, suffix, request.headers())
                .await
                .unwrap_or(owner),
            Err(error) => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(json!({"error":error.to_string()})),
                )
                    .into_response()
            }
        },
    };
    let router = match surface {
        RuntimeSurface::Participant => owner.primary.clone(),
        RuntimeSurface::Admin => owner.runtime_admin.clone(),
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

/// Constructs resources for one real experiment. Mounted experiments use namespaced routes.
async fn build_experiment_router<A: Game>(
    installation: Arc<Installation<A>>,
    config: ExperimentConfig,
    options: ServeOptions<A>,
    mounted_runtime: bool,
    identity: MountIdentity,
) -> Result<BuiltRouters<A>> {
    config.validate()?;
    validate_game_config_contains_no_secrets(&config.game)?;
    let game_config = parse_game_config(installation.game_factory.as_ref(), &config.game)?;
    let agent_definitions = registered_agent_definitions(&options)?;
    let experiment_id = config
        .experiment
        .id
        .clone()
        .unwrap_or_else(generated_experiment_id);
    let speechmatics_api_key = config.speechmatics.api_key.clone();
    let tts_api_key = config.tts.api_key.clone();
    let store = &installation.store;
    let participant_prefix = if mounted_runtime {
        format!("/e/{experiment_id}")
    } else {
        String::new()
    };
    let runtime_admin_prefix = if mounted_runtime {
        format!("/api/admin/runtime/{experiment_id}")
    } else {
        String::new()
    };
    let participant_path = |path: &str| format!("{participant_prefix}{path}");
    let (lifecycle, config_revision) = {
        let lifecycle = store
            .ensure_experiment(ExperimentRecord {
                experiment_id: experiment_id.clone(),
                game_version: installation.game_descriptor.version.to_string(),
                config: persistable_config_json(&config)?,
                server_version: Some(env!("CARGO_PKG_VERSION").to_string()),
                version_manifest: Some(installation.version_manifest.clone()),
                status: "inactive".to_string(),
                notes: None,
            })
            .await?;
        let stored_experiment = store
            .experiment_definition(&experiment_id)
            .await?
            .ok_or_else(|| anyhow!("configured experiment was not stored"))?;
        (lifecycle, stored_experiment.config_revision)
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
    let prolific_api_base_url = installation
        .game_settings
        .read()
        .await
        .prolific_api_base_url
        .clone();
    let prolific_client = (!config.recruitment.prolific.api_token.is_empty())
        .then(|| {
            crate::prolific::ProlificClient::with_base_url(
                config.recruitment.prolific.api_token.clone(),
                prolific_api_base_url,
            )
        })
        .transpose()?;
    let cors = configured_cors(&config)?;
    let context = Arc::new(ServerContext {
        installation,
        config,
        agent_definitions,
        has_tts_provider: tts_provider.is_some(),
        has_transcription_provider: transcription_provider.is_some(),
    });
    let state = Arc::new(AppState {
        context: context.clone(),
        game_config,
        experiment_id,
        config_revision,
        experiment_lifecycle: RwLock::new(
            ExperimentLifecycle::parse(&lifecycle).map_err(|error| anyhow!(error.message))?,
        ),
        participant_url: RwLock::new(None),
        memory: RwLock::new(MemoryState::default()),
        session_admission: identity.admission,
        session_buses: RwLock::new(HashMap::new()),
        agent_factory: options.agent_factory,
        started_agents: RwLock::new(HashSet::new()),
        tts_provider,
        audio_publisher,
        audio_sessions,
        transcription_provider,
        prolific_client: RwLock::new(prolific_client),
        prolific_study: RwLock::new(None),
        committed_transcripts: RwLock::new(HashSet::new()),
        mount_id: identity.generation,
        retired: Default::default(),
        participant_auth: identity.participants,
        upgrade_tickets: identity.tickets,
        participant_creation_window: identity.creation_rate,
        chat_submission_budgets: RwLock::new(HashMap::new()),
        rejection_windows: RwLock::new(HashMap::new()),
        game_connections: RwLock::new(HashMap::new()),
        audio_connections: RwLock::new(HashMap::new()),
    });
    if !mounted_runtime || state.mount_id == 1 {
        finalize_interrupted_sessions(&state).await?;
    }
    {
        let mut registry = context.installation.runtime_registry.write().await;
        let runtimes = registry.entry(state.experiment_id.clone()).or_default();
        runtimes.retain(|runtime| runtime.strong_count() > 0);
        runtimes.push(Arc::downgrade(&state));
    }
    let public_routes = Router::new()
        .route(&participant_path("/health"), get(health::<A>))
        .route(&participant_path("/api/config"), get(public_config::<A>))
        .route(
            &participant_path("/api/participants"),
            post(create_participant::<A>),
        );
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
    let websocket_routes = Router::new()
        .route(
            &participant_path("/ws/game/:public_session_id"),
            get(game_socket::<A>),
        )
        .route(
            &participant_path("/ws/audio/:public_session_id"),
            get(audio_socket::<A>),
        );
    let experiment_admin = Router::new()
        .route(
            &participant_path("/api/admin/experiment"),
            get(admin_experiment::<A>),
        )
        .merge(experiment_admin_routes(&format!(
            "{participant_prefix}/api/admin"
        )))
        .route_layer(middleware::from_fn_with_state(
            context.clone(),
            require_admin_auth::<A>,
        ))
        .route_layer(middleware::from_fn_with_state(
            context.clone(),
            require_admin_network::<A>,
        ));
    let api = Router::new()
        .merge(public_routes)
        .merge(participant_routes)
        .merge(experiment_admin)
        .merge(websocket_routes)
        .with_state(state.clone())
        .merge(dashboard_routes(&context, &participant_prefix));
    let api = http_layers(api, &context, cors);
    spawn_security_cleanup(state.clone(), !mounted_runtime);
    if !mounted_runtime {
        spawn_load_sampler(context.clone());
    }

    let api = if let Some(dist) = client_dist.filter(|path| path.join("index.html").is_file()) {
        let index = dist.join("index.html");
        api.route_service(&participant_path("/"), ServeFile::new(index))
            .nest_service(
                &participant_path("/assets"),
                ServeDir::new(dist.join("assets")),
            )
    } else {
        api
    };
    let runtime_admin = {
        experiment_admin_routes(&runtime_admin_prefix)
            .route_layer(middleware::from_fn_with_state(
                context.clone(),
                require_admin_auth::<A>,
            ))
            .route_layer(middleware::from_fn_with_state(
                context.clone(),
                require_admin_network::<A>,
            ))
            .layer(RequestBodyLimitLayer::new(64 * 1024))
            .layer(middleware::from_fn(security_headers))
            .layer(middleware::from_fn_with_state(
                context.clone(),
                track_request_load::<A>,
            ))
            .with_state(state.clone())
    };
    Ok(BuiltRouters {
        state,
        primary: api,
        runtime_admin,
    })
}

/// Defines the live experiment operations served under both administrator URL prefixes.
fn experiment_admin_routes<A: Game>(prefix: &str) -> Router<Arc<AppState<A>>> {
    let path = |suffix: &str| format!("{prefix}{suffix}");
    Router::new()
        .route(
            &path("/experiment/status"),
            post(admin_update_experiment_status::<A>),
        )
        .route(&path("/sessions"), get(admin_sessions::<A>))
        .route(
            &path("/progress"),
            get(admin_progress::<A>).post(admin_refresh_progress::<A>),
        )
        .route(
            &path("/sessions/:session_id"),
            get(admin_session_detail::<A>),
        )
        .route(
            &path("/sessions/:session_id/events"),
            get(admin_session_events::<A>),
        )
        .route(
            &path("/sessions/:session_id/replay"),
            get(admin_session_replay::<A>),
        )
        .route(&path("/export"), get(admin_export::<A>))
        .route(
            &path("/participants/:research_id/deletion"),
            get(admin_participant_deletion_preview::<A>).post(admin_delete_participant_data::<A>),
        )
}

/// Mounts catalogue and installation routes without allocating a live experiment.
fn dashboard_routes<A: Game>(state: &Arc<ServerContext<A>>, prefix: &str) -> Router {
    let participant_path = |path: &str| format!("{prefix}{path}");
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
    let admin_routes = Router::new()
        .route(
            &participant_path("/api/admin/export-schema"),
            get(admin_corpus_export_schema),
        )
        .route(&participant_path("/api/admin/load"), get(admin_load::<A>))
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
    public_admin_routes
        .merge(admin_routes)
        .with_state(state.clone())
}

/// Applies the same HTTP limits and response policy to installation and experiment routes.
fn http_layers<A: Game>(api: Router, state: &Arc<ServerContext<A>>, cors: CorsLayer) -> Router {
    api.layer(RequestBodyLimitLayer::new(64 * 1024))
        .layer(ConcurrencyLimitLayer::new(256))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(30),
        ))
        .layer(middleware::from_fn(security_headers))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            track_request_load::<A>,
        ))
        .layer(cors)
}
