//! Initializes the resources and shared state of a mounted experiment.
//!
//! The mount registry supplies installation services, an experiment configuration, and
//! factories for the compiled game. Construction prepares the session cache, transport
//! registries, providers, and background tasks, then gives initialized state to the route
//! builder.

use super::*;

/// Constructs resources for one real experiment. Mounted experiments use namespaced routes.
pub(super) async fn build_experiment_router<A: Game>(
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
        game_connections: ConnectionRegistry::default(),
        audio_connections: ConnectionRegistry::default(),
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
    Ok(experiment_routes(state, mounted_runtime, cors, client_dist))
}
