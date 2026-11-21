//! Preparation and ordered commitment of a live game action.
//!
//! Browser input and agent decisions enter through `submit_action`. Under the room's
//! transition lock, it derives each role's view from the same proposed game state and
//! reserves any required agent delivery. A successful storage commit authorizes
//! installation and publication. A failed commit or an already-ended room leaves the
//! proposed move unpublished.

use crate::storage::{SessionEventRecord, SqliteExperimentStore, TransitionCommitUnknown};

use super::{
    anyhow, broadcast_participant_states, expire_if_due_locked, json, mpsc, prolific_handoff,
    protocol_json, reserve_agent_delivery, session_event_record, session_ready_for_game,
    shutdown_session_log, speechmatics_readiness_required, touch_session_activity,
    AgentObservation, AgentsMode, AppState, Arc, ComponentValue, ExperimentConfig, GameFrame,
    HashMap, LiveSession, ParticipantOutcomeKind, ParticipantResult, PlayerRole, Result,
    ServerMessage, SessionEnd, SessionEndCause, SubmissionRejection, TransitionCommit, Value,
};

/// All effects derived from a single call to the game transition before the database write.
struct PreparedAction {
    after: GameFrame,
    checkpoint: Option<Value>,
    end: Option<SessionEnd>,
    event: Value,
    participants: Vec<ServerMessage>,
    agent: Option<(mpsc::Sender<AgentObservation>, AgentObservation)>,
}

/// Derives every recipient's view, terminal result, and event from the same revision.
fn prepare_action(
    config: &ExperimentConfig,
    session: &LiveSession,
    actor: PlayerRole,
    action: ComponentValue,
) -> Result<PreparedAction> {
    if session.lifecycle().is_ended() {
        return Err(anyhow!(SubmissionRejection("session_complete")));
    }
    if !session.accepts_interaction() {
        return Err(anyhow!(SubmissionRejection("participant_not_active")));
    }
    if !session_ready_for_game(config, session) {
        let reason = if speechmatics_readiness_required(config) {
            "transcription_not_ready"
        } else {
            "players_not_ready"
        };
        return Err(anyhow!(SubmissionRejection(reason)));
    }
    let after = session.game.step(session.state(), &action, actor)??;
    let checkpoint = after.turn_checkpoint()?;
    let completion = after.completion();
    let mut end = completion
        .as_ref()
        .map(protocol_json)
        .transpose()?
        .map(|completion| SessionEnd {
            cause: SessionEndCause::GameCompleted,
            completion: Some(completion),
            participant_results: HashMap::new(),
        });
    let action_json = protocol_json(&action)?;
    let event = json!({"action": action_json, "metadata": after.transition_metadata()});
    let mut participants = Vec::new();
    let mut agent = None;
    for participant in session.participants.values() {
        let observation = after.observation(participant.role);
        if participant.source == "agent" {
            agent = session.agent_inbox.clone().map(|sender| {
                (
                    sender,
                    AgentObservation::Action {
                        actor,
                        action: action.clone(),
                        resulting_observation: observation,
                        completion: completion.clone(),
                    },
                )
            });
            continue;
        }
        let observation = protocol_json(&observation)?;
        if let Some(end) = end.as_mut() {
            let outcome = ParticipantOutcomeKind::Completed;
            end.participant_results.insert(
                participant.role.as_str().to_string(),
                ParticipantResult {
                    handoff: (participant.source == "prolific")
                        .then(|| prolific_handoff(config, &outcome, true))
                        .flatten(),
                    outcome,
                    reason: "game_completed".to_string(),
                    completion: end.completion.clone(),
                    final_observation: Some(observation.clone()),
                },
            );
        }
        participants.push(super::participant_state::action_message(
            session,
            participant,
            actor,
            &action_json,
            observation,
        ));
    }
    Ok(PreparedAction {
        after,
        checkpoint,
        end,
        event,
        participants,
        agent,
    })
}

/// Validates, durably commits, then publishes one action in session order.
///
/// Human and agent callers share the room lock with leave and expiry. Prepare both
/// role views from one transition and reserve agent delivery before writing. Only
/// the winner of the durable commit may install that state and publish acceptance.
pub(super) async fn submit_action(
    state: Arc<AppState>,
    public_session_id: &str,
    participant_session_id: &str,
    role: PlayerRole,
    action: ComponentValue,
) -> Result<()> {
    let (transition_lock, pending_sender) = {
        let memory = state.memory.read().await;
        let session = memory
            .sessions
            .get(public_session_id)
            .ok_or_else(|| anyhow!(SubmissionRejection("session_complete")))?;
        (
            session.transition_lock.clone(),
            session.action_commit_pending.clone(),
        )
    };
    let mut pending = pending_sender.subscribe();
    let transition_guard = tokio::select! {
        biased;
        Ok(_) = pending.wait_for(|pending| *pending) => {
            return Err(anyhow!("Action commit is awaiting storage confirmation."));
        }
        guard = transition_lock.lock_owned() => guard,
    };
    let public_session_id = public_session_id.to_owned();
    let participant_session_id = participant_session_id.to_owned();
    // The submitting connection may disappear while the database is committing.
    // Keep ownership of the room lock and publication in a task that survives it.
    let task = tokio::spawn(async move {
        let _transition_guard = transition_guard;
        let result = commit_action(
            state,
            &public_session_id,
            &participant_session_id,
            role,
            action,
            &pending_sender,
        )
        .await;
        pending_sender.send_replace(false);
        result
    });
    tokio::select! {
        biased;
        result = task => result?,
        Ok(_) = pending.wait_for(|pending| *pending) => {
            Err(anyhow!("Action commit is awaiting storage confirmation."))
        }
    }
}

/// Owns one room transition from validation through durable publication.
async fn commit_action(
    state: Arc<AppState>,
    public_session_id: &str,
    participant_session_id: &str,
    role: PlayerRole,
    action: ComponentValue,
    pending: &tokio::sync::watch::Sender<bool>,
) -> Result<()> {
    if expire_if_due_locked(&state, public_session_id).await? {
        return Err(anyhow!(SubmissionRejection("session_complete")));
    }
    let PreparedAction {
        after,
        checkpoint,
        end: session_end,
        event: mut accepted_payload,
        participants: browser_messages,
        agent,
    } = {
        let memory = state.memory.read().await;
        let session = memory
            .sessions
            .get(public_session_id)
            .ok_or_else(|| anyhow!("Session not found."))?;
        prepare_action(&state.context.config, session, role, action)?
    };
    let completed = session_end.is_some();
    let agent_delivery = match agent {
        Some((sender, observation)) => Some((
            reserve_agent_delivery(&state, public_session_id, sender).await?,
            observation,
        )),
        None => None,
    };
    let turn_id = uuid::Uuid::new_v4().to_string();
    accepted_payload["turn_id"] = json!(turn_id);
    if checkpoint.is_some() {
        accepted_payload["recovery_checkpoint"] = json!(true);
    }
    let mut durable_events = vec![
        session_event_record(
            &state,
            public_session_id,
            Some(participant_session_id),
            "game_action_accepted",
            accepted_payload,
            checkpoint,
        )
        .await?,
    ];
    if let Some(completion_json) = session_end.as_ref().and_then(|end| end.completion.as_ref()) {
        durable_events.push(
            session_event_record(
                &state,
                public_session_id,
                Some(participant_session_id),
                "session_completed",
                completion_json.clone(),
                None,
            )
            .await?,
        );
    }
    match persist_prepared_action(
        &state.context.installation.store,
        &durable_events,
        session_end.as_ref(),
        &turn_id,
        pending,
    )
    .await
    {
        Ok(TransitionCommit::Committed) => {}
        Ok(TransitionCommit::AlreadyEnded) => {
            return Err(anyhow!(SubmissionRejection("session_complete")))
        }
        Err(error) => {
            tracing::error!(%error, public_session_id, "could not commit game transition");
            return Err(anyhow!("Action could not be committed."));
        }
    }
    state
        .context
        .installation
        .telemetry
        .record_action_accepted();
    let keep_voice_open = completed
        && state.context.config.voice.enabled
        && state.context.config.agents.mode == AgentsMode::HumanVsHuman
        && state.audio_sessions.has_human_pair(public_session_id).await;
    {
        let mut memory = state.memory.write().await;
        let session = memory
            .sessions
            .get_mut(public_session_id)
            .ok_or_else(|| anyhow!("Session not found."))?;
        let farewell = keep_voice_open.then(|| {
            chrono::Duration::seconds(state.context.config.voice.post_completion_seconds as i64)
        });
        session.install_action(after, session_end, farewell);
    }
    if !completed {
        touch_session_activity(&state, public_session_id).await;
    }
    let bus = state.session_bus(public_session_id).await;
    for packet in browser_messages {
        let _ = bus.send(packet);
    }
    if let Some((permit, observation)) = agent_delivery {
        permit.send(observation);
    }
    if completed {
        if !keep_voice_open {
            state
                .audio_connections
                .shutdown_session(public_session_id)
                .await;
        }
        broadcast_participant_states(&state, public_session_id).await;
    }
    if completed && state.context.config.agents.mode == AgentsMode::HumanVsHuman {
        shutdown_session_log(&state, public_session_id).await;
    }
    Ok(())
}

/// Retries identical records and resolves an uncertain COMMIT while the room is locked.
///
/// Until SQLite can prove the outcome, the room cannot advance or run another guest
/// action against an unconfirmed checkpoint. A proven rollback leaves the original
/// checkpoint in the room, so the next step can restore from it.
async fn persist_prepared_action(
    store: &SqliteExperimentStore,
    events: &[SessionEventRecord],
    end: Option<&SessionEnd>,
    turn_id: &str,
    pending: &tokio::sync::watch::Sender<bool>,
) -> Result<TransitionCommit> {
    let first = &events[0];
    let mut retries_remaining = 2;
    loop {
        match store
            .commit_session_transition(events.to_vec(), end.cloned())
            .await
        {
            Ok(outcome) => return Ok(outcome),
            Err(error) => {
                if error.is::<TransitionCommitUnknown>() {
                    tracing::warn!(%error, turn_id, "checking uncertain game transition commit");
                    loop {
                        match store
                            .session_transition_committed(
                                &first.experiment_id,
                                first.session_id,
                                turn_id,
                            )
                            .await
                        {
                            Ok(true) => return Ok(TransitionCommit::Committed),
                            Ok(false) => break,
                            Err(error) => {
                                if !pending.send_replace(true) {
                                    tracing::warn!(%error, turn_id, "game transition commit remains unconfirmed");
                                }
                                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                            }
                        }
                    }
                }
                if retries_remaining == 0 {
                    return Err(error);
                }
                retries_remaining -= 1;
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        }
    }
}
