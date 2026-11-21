//! Execution, observation delivery, and response handling for a live agent.
//!
//! Room construction supplies a fresh agent and its assigned role. The agent task
//! consumes ordered role-specific observations from its bounded inbox before requesting each
//! decision. Agent responses use the same action and conversation commit
//! paths as other live input.

use super::{
    agent_event_metadata, broadcast_participant_states, commit_conversation_message,
    expire_live_session, json, maybe_start_game, mpsc, persist_session_event, protocol_json,
    session_transition_lock, shutdown_session_log, speak_agent_message, submit_action,
    ActionRejection, Agent, AgentObservation, AgentResponse, AgentTaskInput, AppState, Arc,
    ConversationOrigin, Duration, PlayerRole, Result, SessionLifecycle,
};

/// Builds the stable map key used for one session-local agent instance.
fn agent_key(public_session_id: &str, participant_session_id: &str) -> String {
    format!("{public_session_id}:{participant_session_id}")
}

/// Converts an agent timeout result into a normal anyhow result.
fn flatten_agent_timeout<T>(
    result: std::result::Result<Result<T>, tokio::time::error::Elapsed>,
    timeout_message: &str,
) -> Result<T> {
    match result {
        Ok(result) => result,
        Err(_) => anyhow::bail!("{timeout_message}"),
    }
}

/// Persists and applies one validated agent response.
async fn handle_agent_response(
    state: Arc<AppState>,
    public_session_id: &str,
    participant_session_id: &str,
    role: PlayerRole,
    response: AgentResponse,
) -> Result<()> {
    let (action, message) = response.into_parts();
    if let Some(action) = action {
        let observed_action = protocol_json(&action)?;
        persist_session_event(
            &state,
            public_session_id,
            Some(participant_session_id),
            "agent_action",
            json!({"action": observed_action}),
            None,
        )
        .await;
        submit_action(
            state.clone(),
            public_session_id,
            participant_session_id,
            role,
            action,
        )
        .await?;
    }
    if let Some(text) = message {
        if let Ok(message) = commit_conversation_message(
            &state,
            public_session_id,
            participant_session_id,
            ConversationOrigin::Agent,
            text,
            None,
            json!({}),
        )
        .await
        {
            speak_agent_message(&state, public_session_id, &message).await;
        }
    }
    Ok(())
}

/// Asks one agent for an optional response and applies it when present.
async fn request_agent_decision(
    state: Arc<AppState>,
    agent: &mut dyn Agent,
    public_session_id: &str,
    participant_session_id: &str,
    role: PlayerRole,
    timeout: f64,
) -> Result<()> {
    let result = tokio::time::timeout(Duration::from_secs_f64(timeout), agent.respond()).await;
    let Some(response) = flatten_agent_timeout(result, "agent respond timeout")? else {
        return Ok(());
    };
    handle_agent_response(
        state.clone(),
        public_session_id,
        participant_session_id,
        role,
        response,
    )
    .await
}

/// Owns initialization and execution of one attached agent, disposing of it on every exit.
pub(super) async fn start_agent_task(
    state: Arc<AppState>,
    public_session_id: String,
    participant_session_id: String,
    role: PlayerRole,
    mut pending: AgentTaskInput,
) {
    let key = agent_key(&public_session_id, &participant_session_id);
    {
        let mut started = state.started_agents.write().await;
        if !started.insert(key.clone()) {
            let _ = pending.agent.shutdown().await;
            return;
        }
    }
    tokio::spawn(async move {
        let mut agent = pending.agent;
        let timeout = pending.timeout;
        let limit = pending.invalid_action_limit;
        let role_name = role.as_str().to_string();
        let agent_metadata = pending.metadata;
        async {
            let initial_observation = {
                let memory = state.memory.read().await;
                memory
                    .sessions
                    .get(&public_session_id)
                    .filter(|session| session.lifecycle() == &SessionLifecycle::Forming)
                    .map(|session| session.state().observation(role))
            };
            let Some(initial_observation) = initial_observation else {
                return;
            };
            let started = tokio::time::timeout(
                Duration::from_secs_f64(timeout),
                agent.start(initial_observation),
            )
            .await;
            if let Err(error) = flatten_agent_timeout(started, "agent start timeout") {
                persist_session_event(
                    &state,
                    &public_session_id,
                    Some(&participant_session_id),
                    "agent_error",
                    json!({"last_error": error.to_string(), "phase": "start"}),
                    None,
                )
                .await;
                let _ = expire_live_session(&state, &public_session_id, "agent_start_failed").await;
                broadcast_participant_states(&state, &public_session_id).await;
                return;
            }
            let Some(transition_lock) = session_transition_lock(&state, &public_session_id).await
            else {
                return;
            };
            let transition_guard = transition_lock.lock().await;
            let (sender, mut receiver) = mpsc::channel(64);
            {
                let mut memory = state.memory.write().await;
                let Some(session) = memory
                    .sessions
                    .get_mut(&public_session_id)
                    .filter(|session| session.lifecycle() == &SessionLifecycle::Forming)
                else {
                    return;
                };
                let Some(participant) = session.participants.get_mut(&participant_session_id)
                else {
                    return;
                };
                session.agent_inbox = Some(sender);
                participant.declare_ready();
            }
            persist_session_event(
                &state,
                &public_session_id,
                Some(&participant_session_id),
                "participant_connected",
                json!({"source":"agent","agent":agent_event_metadata(&agent_metadata)}),
                None,
            )
            .await;
            persist_session_event(
                &state,
                &public_session_id,
                Some(&participant_session_id),
                "agent_started",
                json!({"role":role_name,"agent":agent_event_metadata(&agent_metadata)}),
                None,
            )
            .await;
            drop(transition_guard);
            maybe_start_game(state.clone(), &public_session_id).await;
            // Activation is queued before public input can arrive. An ended forming session
            // closes the inbox instead, so it cannot start autonomous decision processing.
            if !matches!(receiver.recv().await, Some(AgentObservation::Activated)) {
                return;
            }
            // Finish publishing the activation snapshot before an autonomous response can
            // advance the game. The activation path holds this same transition lock.
            drop(transition_lock.lock().await);
            let mut invalid_actions = 0usize;
            let mut last_error = None;
            'agent_loop: loop {
                if receiver.is_empty() && !receiver.is_closed() {
                    match request_agent_decision(
                        state.clone(),
                        agent.as_mut(),
                        &public_session_id,
                        &participant_session_id,
                        role,
                        timeout,
                    )
                    .await
                    {
                        Ok(()) => {}
                        // A terminal input can arrive while respond is awaiting user code.
                        // Deliver the committed observations before closing the agent.
                        Err(_) if receiver.is_closed() => {}
                        Err(error) => {
                            last_error = Some(error.to_string());
                            if error.downcast_ref::<ActionRejection>().is_some() {
                                invalid_actions += 1;
                                if invalid_actions < limit {
                                    continue;
                                }
                            } else {
                                persist_session_event(
                                    &state,
                                    &public_session_id,
                                    Some(&participant_session_id),
                                    "agent_error",
                                    json!({"last_error": last_error, "phase": "runtime"}),
                                    None,
                                )
                                .await;
                                break;
                            }
                        }
                    }
                    if invalid_actions >= limit {
                        persist_session_event(
                            &state,
                            &public_session_id,
                            Some(&participant_session_id),
                            "agent_error",
                            json!({"last_error": last_error}),
                            None,
                        )
                        .await;
                        break;
                    }
                }

                let Some(observation) = receiver.recv().await else {
                    break;
                };
                let (observed, completion) = match observation {
                    AgentObservation::Activated => unreachable!("a session activates only once"),
                    AgentObservation::Action {
                        actor,
                        action,
                        resulting_observation,
                        completion,
                    } => (
                        tokio::time::timeout(
                            Duration::from_secs_f64(timeout),
                            agent.observe_transition(actor, action, resulting_observation),
                        )
                        .await,
                        completion,
                    ),
                    AgentObservation::Message { speaker, text } => (
                        tokio::time::timeout(
                            Duration::from_secs_f64(timeout),
                            agent.observe_message(speaker, text),
                        )
                        .await,
                        None,
                    ),
                };
                if let Err(error) = flatten_agent_timeout(observed, "agent observe timeout") {
                    last_error = Some(error.to_string());
                    invalid_actions += 1;
                }
                if let Some(completion) = completion {
                    let finished = tokio::time::timeout(
                        Duration::from_secs_f64(timeout),
                        agent.finish(completion),
                    )
                    .await;
                    if let Err(error) = flatten_agent_timeout(finished, "agent finish timeout") {
                        persist_session_event(
                            &state,
                            &public_session_id,
                            Some(&participant_session_id),
                            "agent_error",
                            json!({"last_error": error.to_string(), "phase": "finish"}),
                            None,
                        )
                        .await;
                    }
                    break;
                }
                if invalid_actions >= limit {
                    persist_session_event(
                        &state,
                        &public_session_id,
                        Some(&participant_session_id),
                        "agent_error",
                        json!({"last_error": last_error}),
                        None,
                    )
                    .await;
                    break;
                }
                continue 'agent_loop;
            }
        }
        .await;
        if let Some(session) = state
            .memory
            .write()
            .await
            .sessions
            .get_mut(&public_session_id)
        {
            session.agent_inbox.take();
        }
        match tokio::time::timeout(Duration::from_secs(5), agent.shutdown()).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => tracing::warn!(%error, public_session_id, "agent shutdown failed"),
            Err(error) => tracing::warn!(%error, public_session_id, "agent shutdown timed out"),
        }
        state.started_agents.write().await.remove(&key);
        shutdown_session_log(&state, &public_session_id).await;
    });
}
