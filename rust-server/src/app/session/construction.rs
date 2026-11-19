//! Initialization of a durable session, its game, and reserved agent seat.

use crate::app::admission::AdmissionReservation;
use crate::app::{
    agent_event_metadata, broadcast_participant_states, configuration_fingerprint,
    expire_live_session, json, new_id, persist_session_event, protocol_json, session_code,
    session_transition_lock, shutdown_session_log, start_agent_task, AgentContext, AgentTaskInput,
    AppError, AppState, Arc, Duration, Game, GameInitializationContext, GameSessionContext,
    HashMap, LiveSession, Mutex, ParticipantRecord, PlayerRole, Result, SecretValues,
    SessionEventRecord, SessionLifecycle, SessionLogger, SessionParticipant, SessionRecord,
    SharedAgentFactory, StatusCode, Value,
};

/// Validated inputs retained until a live session can supply an agent-scoped logger.
pub(in crate::app) struct PreparedAgentConstruction<A: Game> {
    pub(in crate::app) session_seed: u64,
    agent_seed: u64,
    settings: Value,
    factory_secrets: SecretValues,
    agent_instance_secrets: SecretValues,
    factory: SharedAgentFactory<A>,
    timeout: f64,
    invalid_action_limit: usize,
    metadata: Value,
    external_id: String,
}

/// Validates an agent's factory settings and identity before admission reserves a session.
pub(in crate::app) fn prepare_agent_construction<A: Game>(
    state: &AppState<A>,
) -> Result<PreparedAgentConstruction<A>, AppError> {
    let agent_config = state
        .context
        .config
        .agents
        .human_vs_agent
        .as_ref()
        .ok_or_else(|| {
            AppError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Validated human-versus-agent configuration is missing",
            )
        })?;
    let session_seed = rand::random::<u64>();
    let agent_seed = agent_config.seed.unwrap_or(session_seed);
    let raw_settings = agent_config.config.clone();
    let factory = state
        .agent_factory
        .clone()
        .ok_or_else(|| AppError::new(StatusCode::SERVICE_UNAVAILABLE, "No agent is registered"))?;
    let timeout = agent_config.act_timeout_seconds;
    let definition = factory.definition();
    let settings = definition.normalize_settings(&raw_settings).map_err(|_| {
        AppError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "The configured agent settings are invalid",
        )
    })?;
    let (factory_secrets, agent_instance_secrets) = definition
        .resolve_secrets(&settings, &state.context.config.game_secrets)
        .map_err(|_| {
            AppError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "The configured agent references an unavailable secret",
            )
        })?;
    let identity = factory.identity(&settings)?;
    identity.validate()?;
    let fingerprint = configuration_fingerprint(&definition.id, &settings)?;
    let metadata = json!({
        "agent_name": identity.name,
        "agent_version": identity.version,
        "factory": definition.id,
        "configuration_fingerprint": fingerprint,
    });
    let external_id = format!("{}@{}#{}", identity.name, identity.version, fingerprint);
    Ok(PreparedAgentConstruction {
        session_seed,
        agent_seed,
        settings,
        factory_secrets,
        agent_instance_secrets,
        factory,
        timeout,
        invalid_action_limit: agent_config.invalid_action_limit,
        metadata,
        external_id,
    })
}

/// Constructs a durable session and publishes it only after its first membership commits.
pub(in crate::app) async fn create_live_session_locked<A: Game>(
    _reservation: &AdmissionReservation,
    state: &AppState<A>,
    participant_session_id: String,
    mode: String,
    role: PlayerRole,
    seed: u64,
) -> Result<(String, PlayerRole), AppError> {
    let participant = state
        .memory
        .read()
        .await
        .participants
        .get(&participant_session_id)
        .ok_or_else(|| AppError::not_found("Participant session not found."))?
        .clone();
    let mut public_session_id = session_code();
    while state
        .memory
        .read()
        .await
        .sessions
        .contains_key(&public_session_id)
    {
        public_session_id = session_code();
    }
    let session_id = state
        .context
        .installation
        .store
        .create_session(SessionRecord {
            experiment_id: state.experiment_id.clone(),
            config_revision: state.config_revision,
            game_version: state
                .context
                .installation
                .game_descriptor
                .version
                .to_string(),
            public_session_id: public_session_id.clone(),
            mode: mode.clone(),
            lifecycle: "forming".to_string(),
            purpose: participant.purpose.clone(),
            waiting_timeout_seconds: state.context.config.session.waiting_session_timeout_seconds,
            maximum_lifetime_seconds: state.context.config.session.session_max_lifetime_seconds,
        })
        .await?;
    let (logger, log_writer) = SessionLogger::live(
        state.context.installation.store.clone(),
        state.experiment_id.clone(),
        session_id,
    );
    let membership = SessionParticipant::human(&participant, role, &state.context.config);
    let mut failure_reason = "game_construction_failed";
    let initialization: Result<_, AppError> = async {
        let game = state
            .context
            .installation
            .game_factory
            .create(GameSessionContext {
                logger: logger.clone(),
            })
            .map_err(|error| AppError::bad_request(error.to_string()))?;
        failure_reason = "game_state_initialization_failed";
        let game_state = game
            .initial_state(GameInitializationContext {
                config: &state.game_config,
                seed,
                secrets: &SecretValues::new(
                    state
                        .context
                        .config
                        .game_secrets
                        .clone()
                        .into_iter()
                        .collect(),
                ),
            })
            .map_err(|error| AppError::bad_request(error.to_string()))?;
        failure_reason = "session_event_persistence_failed";
        let initial_state_json = protocol_json(&game_state)?;
        state
            .context
            .installation
            .store
            .append_session_event(SessionEventRecord {
                experiment_id: state.experiment_id.clone(),
                session_id,
                event_type: "session_created".to_string(),
                actor_participant_id: None,
                actor_role: None,
                payload: json!({"public_session_id": public_session_id, "seed": seed,
                "replay_compatibility": {"state": A::STATE_REPRESENTATION_VERSION,
                    "action": A::ACTION_REPRESENTATION_VERSION, "rules": A::RULES_VERSION}}),
                game_state: Some(initial_state_json),
            })
            .await?;
        failure_reason = "session_admission_failed";
        let timing = state
            .context
            .installation
            .store
            .session_timing(&state.experiment_id, session_id)
            .await?;
        if !state
            .context
            .installation
            .store
            .complete_session_initialization(membership.record(&state.experiment_id, session_id))
            .await?
        {
            return Err(AppError::new(
                StatusCode::CONFLICT,
                "Session initialization no longer has an active durable record",
            ));
        }
        Ok((game, game_state, timing))
    }
    .await;
    let (game, game_state, timing) = match initialization {
        Ok(prepared) => prepared,
        Err(error) => {
            log_writer.shutdown().await;
            if let Err(cause) = state
                .context
                .installation
                .store
                .fail_session_initialization(&state.experiment_id, session_id, failure_reason)
                .await
            {
                tracing::error!(%cause, session_id, "could not finalize failed session initialization");
            }
            return Err(error);
        }
    };
    let participants = HashMap::from([(participant_session_id, membership)]);

    state.memory.write().await.sessions.insert(
        public_session_id.clone(),
        LiveSession {
            id: public_session_id.clone(),
            experiment_id: state.experiment_id.clone(),
            session_id,
            mode,
            purpose: participant.purpose,
            game,
            state: game_state,
            logger,
            log_writer: Some(log_writer),
            agent_inbox: None,
            transition_lock: Arc::new(Mutex::new(())),
            lifecycle: SessionLifecycle::Forming,
            pause: None,
            participants,
            timing: super::LiveSessionTiming::waiting(
                timing.waiting_started_at,
                timing.waiting_deadline_at,
                timing.lifetime_deadline_at,
            ),
        },
    );
    Ok((public_session_id, role))
}

/// Constructs the reserved software participant and admits it through the normal session boundary.
pub(in crate::app) async fn construct_and_join_agent<A: Game>(
    state: Arc<AppState<A>>,
    public_session_id: String,
    prepared: PreparedAgentConstruction<A>,
) {
    if let Err(error) = try_construct_and_join_agent(&state, &public_session_id, prepared).await {
        tracing::error!(message = %error.message, %public_session_id, "software agent could not join its reserved session");
        let _ = expire_live_session(&state, &public_session_id, "agent_construction_failed").await;
        broadcast_participant_states(&state, &public_session_id).await;
        shutdown_session_log(&state, &public_session_id).await;
    }
}

/// Performs fallible agent construction before atomically attaching the resulting participant.
async fn try_construct_and_join_agent<A: Game>(
    state: &Arc<AppState<A>>,
    public_session_id: &str,
    prepared: PreparedAgentConstruction<A>,
) -> Result<(), AppError> {
    let (purpose, logger) = {
        let memory = state.memory.read().await;
        let session = memory
            .sessions
            .get(public_session_id)
            .filter(|session| session.lifecycle == SessionLifecycle::Forming)
            .ok_or_else(|| AppError::new(StatusCode::CONFLICT, "Session is no longer waiting"))?;
        (session.purpose.clone(), session.logger.clone())
    };
    let event_metadata = prepared.metadata.clone();
    let participant_id = state
        .context
        .installation
        .store
        .upsert_participant(ParticipantRecord {
            experiment_id: state.experiment_id.clone(),
            participant_kind: "agent".to_string(),
            identity_provider: "agent".to_string(),
            external_id: Some(prepared.external_id),
            metadata: event_metadata.clone(),
        })
        .await?;
    let research_id = state
        .context
        .installation
        .store
        .participant_research_id(participant_id)
        .await?
        .ok_or_else(|| {
            AppError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Agent identifier is missing",
            )
        })?;
    let agent_session_id = new_id("ps");
    let context = AgentContext {
        role: PlayerRole::B,
        seed: prepared.agent_seed,
        settings: prepared.settings,
        factory_secrets: prepared.factory_secrets,
        agent_instance_secrets: prepared.agent_instance_secrets,
        logger: logger.for_agent(participant_id, PlayerRole::B),
    };
    let mut agent = match tokio::time::timeout(
        Duration::from_secs_f64(prepared.timeout),
        prepared.factory.create(context),
    )
    .await
    {
        Ok(Ok(agent)) => agent,
        Ok(Err(error)) => {
            return Err(AppError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                error.to_string(),
            ))
        }
        Err(_) => {
            return Err(AppError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "The configured agent could not be initialized",
            ))
        }
    };
    let event_metadata = agent_event_metadata(&event_metadata);
    let Some(transition_lock) = session_transition_lock(state, public_session_id).await else {
        let _ = tokio::time::timeout(Duration::from_secs(5), agent.shutdown()).await;
        return Err(AppError::not_found("Session not found."));
    };
    let transition_guard = transition_lock.lock().await;
    let membership = SessionParticipant::agent(agent_session_id.clone(), participant_id);
    let attach_result: Result<(), AppError> = async {
        let session_id = state
            .memory
            .read()
            .await
            .sessions
            .get(public_session_id)
            .filter(|session| session.lifecycle == SessionLifecycle::Forming)
            .map(|session| session.session_id)
            .ok_or_else(|| AppError::new(StatusCode::CONFLICT, "Session is no longer waiting"))?;
        state
            .context
            .installation
            .store
            .add_session_participant(membership.record(&state.experiment_id, session_id))
            .await?;
        let mut memory = state.memory.write().await;
        memory.create_participant_with_id(
            agent_session_id.clone(),
            participant_id,
            research_id,
            "agent".to_string(),
            purpose,
        );
        memory
            .sessions
            .get_mut(public_session_id)
            .expect("session guard protects reserved room")
            .participants
            .insert(agent_session_id.clone(), membership);
        Ok(())
    }
    .await;
    if let Err(error) = attach_result {
        drop(transition_guard);
        let _ = tokio::time::timeout(Duration::from_secs(5), agent.shutdown()).await;
        return Err(error);
    }
    persist_session_event(
        state,
        public_session_id,
        Some(&agent_session_id),
        "participant_joined",
        json!({"role": "B", "kind": "agent", "agent": event_metadata}),
        None,
    )
    .await;
    drop(transition_guard);
    broadcast_participant_states(state, public_session_id).await;
    start_agent_task(
        state.clone(),
        public_session_id.to_string(),
        agent_session_id,
        PlayerRole::B,
        AgentTaskInput {
            agent,
            timeout: prepared.timeout,
            invalid_action_limit: prepared.invalid_action_limit,
            metadata: event_metadata,
        },
    )
    .await;
    Ok(())
}
