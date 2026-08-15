use super::*;

/// Builds and runs a Parlando HTTP/WebSocket server on the provided socket address.
pub async fn serve<A: GameAdapter>(
    adapter: A,
    config: ExperimentConfig,
    bind_addr: SocketAddr,
    options: ServeOptions<A>,
) -> Result<()> {
    if !bind_addr.ip().is_loopback() && !config.server.public_base_url.starts_with("https://") {
        return Err(anyhow!("public binding requires an https public_base_url"));
    }
    if !bind_addr.ip().is_loopback()
        && config
            .server
            .allowed_origins
            .iter()
            .any(|origin| !origin.starts_with("https://"))
    {
        return Err(anyhow!("public binding allows only https browser origins"));
    }
    let router = build_router(adapter, config, options).await?;
    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}

/// Builds an Axum router for tests or for embedding in a custom server runner.
pub async fn build_router<A: GameAdapter>(
    adapter: A,
    mut config: ExperimentConfig,
    options: ServeOptions<A>,
) -> Result<Router>
where
    A::State: Serialize,
{
    config.validate()?;
    let speechmatics_api_key = std::mem::take(&mut config.speechmatics.api_key);
    let tts_api_key = std::mem::take(&mut config.tts.api_key);
    let store = experiment_store_from_url(&config.database.url).await?;
    let experiment_id = config
        .experiment
        .id
        .clone()
        .unwrap_or_else(generated_experiment_id);
    let version_manifest = version_manifest(options.game_version_manifest.clone());
    let lifecycle = store
        .ensure_experiment(ExperimentRecord {
            experiment_id: experiment_id.clone(),
            config: persistable_config_json(&config)?,
            server_version: Some(env!("CARGO_PKG_VERSION").to_string()),
            version_manifest: Some(version_manifest.clone()),
            status: "inactive".to_string(),
            notes: None,
        })
        .await?;
    let client_dist = config.server.client_dist_path.as_ref().map(PathBuf::from);
    let tts_provider = if options.tts_provider.is_some() {
        options.tts_provider
    } else if config.tts.enabled {
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
    } else if config.transcription.enabled && config.transcription.provider == "speechmatics" {
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
    let admin_auth = AdminAuthenticator::load(store.clone()).await?;
    let state = Arc::new(AppState {
        adapter,
        config,
        experiment_id,
        experiment_lifecycle: RwLock::new(
            ExperimentLifecycle::parse(&lifecycle).map_err(|error| anyhow!(error.message))?,
        ),
        memory: RwLock::new(MemoryState::default()),
        store,
        room_buses: RwLock::new(HashMap::new()),
        agent_factory: options.agent_factory,
        started_agents: RwLock::new(HashSet::new()),
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
        game_connection_limit: Arc::new(Semaphore::new(1_000)),
        audio_connection_limit: Arc::new(Semaphore::new(200)),
        provider_connection_limit: Arc::new(Semaphore::new(32)),
        room_transition_locks: RwLock::new(HashMap::new()),
        game_connections: RwLock::new(HashMap::new()),
        audio_connections: RwLock::new(HashMap::new()),
        version_manifest,
    });

    let public_routes = Router::new()
        .route("/health", get(health))
        .route("/api/config", get(public_config::<A>))
        .route("/api/participants", post(create_participant::<A>))
        .route("/admin", get(admin_entry))
        .route("/admin/", get(admin_entry))
        .route("/admin/login", get(admin_login_page::<A>))
        .route("/api/admin/setup", post(admin_setup::<A>))
        .route("/api/admin/login", post(admin_login::<A>));
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
            "/api/admin/experiment/status",
            post(admin_update_experiment_status::<A>),
        )
        .route("/api/admin/sessions", get(admin_sessions::<A>))
        .route(
            "/api/admin/sessions/:session_id",
            get(admin_session_detail::<A>),
        )
        .route(
            "/api/admin/sessions/:session_id/events",
            get(admin_session_events::<A>),
        )
        .route("/api/admin/export", get(admin_export::<A>))
        .route(
            "/api/admin/participants/:research_id/deletion",
            get(admin_participant_deletion_preview::<A>).post(admin_delete_participant_data::<A>),
        )
        .route("/api/admin/logout", post(admin_logout::<A>))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_admin_auth::<A>,
        ));
    let websocket_routes = Router::new()
        .route("/ws/game/:room_id", get(game_socket::<A>))
        .route("/ws/audio/:room_id", get(audio_socket::<A>));
    let api = Router::new()
        .merge(public_routes)
        .merge(participant_routes)
        .merge(admin_routes)
        .merge(websocket_routes)
        .layer(RequestBodyLimitLayer::new(64 * 1024))
        .layer(ConcurrencyLimitLayer::new(2_048))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(30),
        ))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            security_headers::<A>,
        ))
        .layer(cors)
        .with_state(state.clone());

    spawn_security_cleanup(state);

    if let Some(dist) = client_dist.filter(|path| path.join("index.html").is_file()) {
        let index = dist.join("index.html");
        Ok(api
            .nest_service("/assets", ServeDir::new(dist.join("assets")))
            .fallback_service(ServeDir::new(dist).fallback(ServeFile::new(index))))
    } else {
        Ok(api)
    }
}
