//! Serialized live-session changes and role-scoped publication of durable results.
//!
//! Departure, expiry, and delivery failure use the room transition lock to order terminal
//! decisions against gameplay. A terminal write records the event and per-participant
//! results before closing the live room. Action and conversation commits also use this
//! unit to reserve agent delivery before accepting input.
use super::{
    anyhow, broadcast_participant_states, due_live_deadline, json, mpsc, prolific_handoff,
    protocol_json, session_event_record, session_transition_lock, shutdown_session_log,
    AgentObservation, AgentsMode, AppState, Arc, HashMap, ParticipantOutcomeKind,
    ParticipantResult, Result, SessionEnd, SessionEndCause, SessionEventRecord, SessionLifecycle,
    SubmissionRejection,
};

/// Ends one timed-out live session with a shared cause and per-participant results.
pub(super) async fn expire_live_session(
    state: &Arc<AppState>,
    public_session_id: &str,
    reason: &str,
) -> Result<bool> {
    let Some(lock) = session_transition_lock(state, public_session_id).await else {
        return Ok(false);
    };
    let _guard = lock.lock().await;
    expire_live_session_locked(state, public_session_id, reason).await
}

/// Persists and projects expiry while the caller owns the session transition lock.
pub(super) async fn expire_live_session_locked(
    state: &Arc<AppState>,
    public_session_id: &str,
    reason: &str,
) -> Result<bool> {
    let (event, session_end) = {
        let memory = state.memory.read().await;
        let session = memory
            .sessions
            .get(public_session_id)
            .ok_or_else(|| anyhow!("Session not found."))?;
        if session.lifecycle().is_ended() {
            return Ok(false);
        }
        let cause = match reason {
            "waiting_timeout" => SessionEndCause::PartnerUnavailable,
            "idle_timeout" => SessionEndCause::IdleTimedOut,
            "maximum_lifetime" => SessionEndCause::LifetimeTimedOut,
            "reconnect_timeout" => SessionEndCause::ReconnectTimedOut {
                disconnected_role: session
                    .participants
                    .values()
                    .filter(|participant| !participant.connected)
                    .filter_map(|participant| {
                        Some((
                            chrono::DateTime::parse_from_rfc3339(
                                participant.reconnect_deadline_at.as_deref()?,
                            )
                            .ok()?,
                            participant.role,
                        ))
                    })
                    .min_by_key(|(deadline, role)| (*deadline, role.as_str()))
                    .map(|(_, role)| role.as_str().to_string())
                    .unwrap_or_else(|| "unknown".to_string()),
            },
            _ => SessionEndCause::TechnicalFailure,
        };
        let participant_results = session
            .participants
            .values()
            .filter(|participant| participant.source != "agent")
            .map(|participant| {
                let outcome = match &cause {
                    SessionEndCause::PartnerUnavailable => {
                        ParticipantOutcomeKind::PartnerUnavailable
                    }
                    SessionEndCause::ReconnectTimedOut { .. } => {
                        if participant.connected {
                            ParticipantOutcomeKind::PartnerLeft
                        } else {
                            ParticipantOutcomeKind::ConnectionLost
                        }
                    }
                    SessionEndCause::IdleTimedOut => ParticipantOutcomeKind::IdleLimitReached,
                    SessionEndCause::LifetimeTimedOut => {
                        ParticipantOutcomeKind::LifetimeLimitReached
                    }
                    SessionEndCause::TechnicalFailure => ParticipantOutcomeKind::TechnicalFailure,
                    SessionEndCause::GameCompleted | SessionEndCause::ParticipantLeft { .. } => {
                        return Err(anyhow!("invalid expiry cause"));
                    }
                };
                Ok((
                    participant.role.as_str().to_string(),
                    ParticipantResult {
                        handoff: (participant.source == "prolific")
                            .then(|| {
                                prolific_handoff(
                                    &state.context.config,
                                    &outcome,
                                    session.lifecycle() == &SessionLifecycle::Running,
                                )
                            })
                            .flatten(),
                        outcome,
                        reason: reason.to_string(),
                        completion: None,
                        final_observation: Some(protocol_json(
                            &session.state().observation(participant.role),
                        )?),
                    },
                ))
            })
            .collect::<Result<HashMap<_, _>>>()?;
        let end = SessionEnd {
            cause,
            completion: None,
            participant_results,
        };
        let event = SessionEventRecord {
            experiment_id: session.experiment_id.clone(),
            session_id: session.session_id,
            event_type: "session_ended".to_string(),
            actor_participant_id: None,
            actor_role: None,
            payload: serde_json::to_value(&end)?,
            game_state: None,
        };
        (event, end)
    };
    commit_end_locked(state, public_session_id, event, session_end).await
}

/// Commits a terminal event before closing the room and its agent delivery channel.
async fn commit_end_locked(
    state: &Arc<AppState>,
    public_session_id: &str,
    event: SessionEventRecord,
    session_end: SessionEnd,
) -> Result<bool> {
    if !state
        .context
        .installation
        .store
        .end_session(event, session_end.clone())
        .await?
    {
        return Ok(false);
    }
    if let Some(session) = state
        .memory
        .write()
        .await
        .sessions
        .get_mut(public_session_id)
    {
        session.terminate(session_end);
    }
    state
        .audio_connections
        .shutdown_session(public_session_id)
        .await;
    Ok(true)
}

/// Resolves an elapsed active deadline before allowing the next input to renew it.
/// All callers hold the room guard, including the deadline checker and reconnect path.
pub(super) async fn expire_if_due_locked(
    state: &Arc<AppState>,
    public_session_id: &str,
) -> Result<bool> {
    let reason = state
        .memory
        .read()
        .await
        .sessions
        .get(public_session_id)
        .and_then(|room| due_live_deadline(room, chrono::Utc::now()));
    let Some(reason) = reason else {
        return Ok(false);
    };
    if expire_live_session_locked(state, public_session_id, reason).await? {
        broadcast_participant_states(state, public_session_id).await;
    }
    Ok(true)
}

/// Atomically records an intentional departure and closes the session to further game input.
pub(super) async fn abandon_session(
    state: &Arc<AppState>,
    public_session_id: &str,
    participant_session_id: &str,
    reason: &str,
) -> Result<bool> {
    let Some(transition_lock) = session_transition_lock(state, public_session_id).await else {
        return Ok(false);
    };
    let _transition_guard = transition_lock.lock().await;
    depart_session_locked(state, public_session_id, participant_session_id, reason).await
}

/// Commits the identified participant's departure, including a zero-length reconnect grace.
pub(super) async fn depart_session_locked(
    state: &Arc<AppState>,
    public_session_id: &str,
    participant_session_id: &str,
    reason: &str,
) -> Result<bool> {
    let session_end = {
        let memory = state.memory.read().await;
        let session = memory
            .sessions
            .get(public_session_id)
            .ok_or_else(|| anyhow!("Session not found."))?;
        if session.lifecycle().is_ended() {
            return Ok(false);
        }
        let actor_role = session
            .participants
            .get(participant_session_id)
            .ok_or_else(|| anyhow!("Participant not in session."))?
            .role;
        let actor_outcome = if reason == "reconnect_timeout" {
            ParticipantOutcomeKind::ConnectionLost
        } else if session.lifecycle() == &SessionLifecycle::Forming {
            ParticipantOutcomeKind::LeftWaitingRoom
        } else {
            ParticipantOutcomeKind::LeftGame
        };
        let participant_results = session
            .participants
            .values()
            .filter(|participant| participant.source != "agent")
            .map(|participant| {
                let outcome = if participant.participant_session_id == participant_session_id {
                    actor_outcome.clone()
                } else {
                    ParticipantOutcomeKind::PartnerLeft
                };
                Ok((
                    participant.role.as_str().to_string(),
                    ParticipantResult {
                        handoff: (participant.source == "prolific")
                            .then(|| {
                                prolific_handoff(
                                    &state.context.config,
                                    &outcome,
                                    session.lifecycle() == &SessionLifecycle::Running,
                                )
                            })
                            .flatten(),
                        outcome,
                        reason: reason.to_string(),
                        completion: None,
                        final_observation: Some(protocol_json(
                            &session.state().observation(participant.role),
                        )?),
                    },
                ))
            })
            .collect::<Result<HashMap<_, _>>>()?;
        SessionEnd {
            cause: if reason == "reconnect_timeout" {
                SessionEndCause::ReconnectTimedOut {
                    disconnected_role: actor_role.as_str().to_string(),
                }
            } else {
                SessionEndCause::ParticipantLeft {
                    actor: actor_role.as_str().to_string(),
                }
            },
            completion: None,
            participant_results,
        }
    };
    let event = session_event_record(
        state,
        public_session_id,
        Some(participant_session_id),
        "session_abandoned",
        json!({"reason": reason}),
        None,
    )
    .await?;
    if !commit_end_locked(state, public_session_id, event, session_end).await? {
        return Ok(false);
    }
    if state.context.config.agents.mode == AgentsMode::HumanVsHuman {
        shutdown_session_log(state, public_session_id).await;
    }
    Ok(true)
}

/// Reserves delivery before a commit without blocking the session on its own agent.
/// If a callback cannot keep up, end the room before accepting an undeliverable input.
/// The caller holds the transition lock so accepted actions and messages retain order.
pub(super) async fn reserve_agent_delivery(
    state: &Arc<AppState>,
    public_session_id: &str,
    sender: mpsc::Sender<AgentObservation>,
) -> Result<mpsc::OwnedPermit<AgentObservation>> {
    match sender.try_reserve_owned() {
        Ok(permit) => Ok(permit),
        Err(_) => {
            if expire_live_session_locked(state, public_session_id, "agent_delivery_failed").await?
            {
                broadcast_participant_states(state, public_session_id).await;
            }
            Err(anyhow!(SubmissionRejection("session_complete")))
        }
    }
}
