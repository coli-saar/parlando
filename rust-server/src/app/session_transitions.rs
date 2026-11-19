//! Serialized live-session changes and role-scoped publication of durable results.
use super::*;

/// Ends one timed-out live session with a shared cause and per-participant results.
pub(super) async fn expire_live_session<A: Game>(
    state: &Arc<AppState<A>>,
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
pub(super) async fn expire_live_session_locked<A: Game>(
    state: &Arc<AppState<A>>,
    public_session_id: &str,
    reason: &str,
) -> Result<bool> {
    let (event, session_end) = {
        let memory = state.memory.read().await;
        let session = memory
            .sessions
            .get(public_session_id)
            .ok_or_else(|| anyhow!("Session not found."))?;
        if session.lifecycle.is_ended() {
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
                                    session.lifecycle == SessionLifecycle::Running,
                                )
                            })
                            .flatten(),
                        outcome,
                        reason: reason.to_string(),
                        completion: None,
                        final_observation: Some(protocol_json(
                            &session.game.observation(&session.state, participant.role),
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
async fn commit_end_locked<A: Game>(
    state: &Arc<AppState<A>>,
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
        session.lifecycle = SessionLifecycle::Ended(session_end);
        session.agent_inbox.take();
        session.pause = None;
        session.updated_at = now_iso();
    }
    shutdown_session_connections(&state.audio_connections, public_session_id).await;
    Ok(true)
}

/// Work due for one room during a lifecycle sweep.
pub(super) enum RoomCleanup {
    /// End a forming or running room with its elapsed deadline.
    Expire(&'static str),
    /// Close farewell audio while retaining any remaining game transports.
    CloseFarewell,
    /// Release a terminal room after every human's retention period.
    Evict,
}

/// Determines whether an active room has reached an authoritative deadline.
pub(super) fn due_live_deadline<A: Game>(
    session: &LiveSession<A>,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<&'static str> {
    if session.lifecycle.is_ended() {
        return None;
    }
    let elapsed = |stamp: &str| {
        chrono::DateTime::parse_from_rfc3339(stamp).is_ok_and(|deadline| deadline <= now)
    };
    if elapsed(&session.lifetime_deadline_at) {
        Some("maximum_lifetime")
    } else if session.lifecycle == SessionLifecycle::Forming
        && elapsed(&session.waiting_deadline_at)
    {
        Some("waiting_timeout")
    } else if session
        .participants
        .values()
        .any(|p| !p.connected && p.reconnect_deadline_at.as_deref().is_some_and(elapsed))
    {
        Some("reconnect_timeout")
    } else if session.lifecycle == SessionLifecycle::Running
        && session.idle_deadline_at.as_deref().is_some_and(elapsed)
    {
        Some("idle_timeout")
    } else {
        None
    }
}

/// Keeps terminal rooms for farewell audio, connected browsers, and each human's grace.
/// A human who never opened a socket starts their retention period at the terminal commit.
pub(super) fn room_cleanup<A: Game>(
    session: &LiveSession<A>,
    now: chrono::DateTime<chrono::Utc>,
    grace: chrono::Duration,
) -> Option<RoomCleanup> {
    if !session.lifecycle.is_ended() {
        return due_live_deadline(session, now).map(RoomCleanup::Expire);
    }
    if let Some(end) = session.voice_ends_at.as_deref() {
        return (chrono::DateTime::parse_from_rfc3339(end).ok()? <= now)
            .then_some(RoomCleanup::CloseFarewell);
    }
    let ended_at = chrono::DateTime::parse_from_rfc3339(&session.updated_at).ok()?;
    let released = session
        .participants
        .values()
        .filter(|p| p.source != "agent")
        .all(|p| {
            let until = p
                .reconnect_deadline_at
                .as_deref()
                .and_then(|stamp| chrono::DateTime::parse_from_rfc3339(stamp).ok())
                .unwrap_or(ended_at + grace);
            !p.connected && now >= until
        });
    released.then_some(RoomCleanup::Evict)
}

/// Resolves an elapsed active deadline before allowing the next input to renew it.
/// All callers hold the room guard, including the deadline checker and reconnect path.
pub(super) async fn expire_if_due_locked<A: Game>(
    state: &Arc<AppState<A>>,
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
pub(super) async fn abandon_session<A: Game>(
    state: &Arc<AppState<A>>,
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
pub(super) async fn depart_session_locked<A: Game>(
    state: &Arc<AppState<A>>,
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
        if session.lifecycle.is_ended() {
            return Ok(false);
        }
        let actor_role = session
            .participants
            .get(participant_session_id)
            .ok_or_else(|| anyhow!("Participant not in session."))?
            .role;
        let actor_outcome = if reason == "reconnect_timeout" {
            ParticipantOutcomeKind::ConnectionLost
        } else if session.lifecycle == SessionLifecycle::Forming {
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
                                    session.lifecycle == SessionLifecycle::Running,
                                )
                            })
                            .flatten(),
                        outcome,
                        reason: reason.to_string(),
                        completion: None,
                        final_observation: Some(protocol_json(
                            &session.game.observation(&session.state, participant.role),
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

/// All effects derived from a single call to the game transition before the database write.
struct PreparedAction<G: Game> {
    after: G::State,
    end: Option<SessionEnd>,
    event: Value,
    participants: Vec<ServerMessage>,
    agent: Option<(mpsc::Sender<AgentObservation<G>>, AgentObservation<G>)>,
}

/// Derives every recipient's view, terminal result, and event from the same revision.
fn prepare_action<G: Game>(
    config: &ExperimentConfig,
    session: &LiveSession<G>,
    actor: PlayerRole,
    action: G::Action,
) -> Result<PreparedAction<G>> {
    if session.lifecycle.is_ended() {
        return Err(anyhow!(SubmissionRejection("session_complete")));
    }
    if session.lifecycle != SessionLifecycle::Running || session.pause.is_some() {
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
    let after = session.game.apply_action(&session.state, &action, actor)?;
    let completion = session.game.completion(&after);
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
    let event = json!({"action": action_json, "metadata": session.game.transition_metadata(&session.state, &after, &action, actor)});
    let mut participants = Vec::new();
    let mut agent = None;
    for participant in session.participants.values() {
        let observation = session.game.observation(&after, participant.role);
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
        let available_actions = session
            .game
            .available_actions(&after, participant.role)
            .map(|actions| {
                actions
                    .iter()
                    .map(protocol_json)
                    .collect::<Result<Vec<_>>>()
            })
            .transpose()?;
        participants.push(ServerMessage::targeted(
            participant.participant_session_id.clone(),
            ServerPayload::Transition {
                public_session_id: session.id.clone(),
                actor: actor.as_str().to_string(),
                action: action_json.clone(),
                observation,
                available_actions,
            },
        ));
    }
    Ok(PreparedAction {
        after,
        end,
        event,
        participants,
        agent,
    })
}

/// Validates, durably commits, then publishes one action in session order.
pub(super) async fn submit_action<A: Game>(
    state: Arc<AppState<A>>,
    public_session_id: &str,
    participant_session_id: &str,
    role: PlayerRole,
    action: A::Action,
) -> Result<()> {
    let Some(transition_lock) = session_transition_lock(&state, public_session_id).await else {
        return Err(anyhow!(SubmissionRejection("session_complete")));
    };
    let _transition_guard = transition_lock.lock().await;
    if expire_if_due_locked(&state, public_session_id).await? {
        return Err(anyhow!(SubmissionRejection("session_complete")));
    }
    let PreparedAction {
        after,
        end: session_end,
        event: accepted_payload,
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
    let mut durable_events = vec![
        session_event_record(
            &state,
            public_session_id,
            Some(participant_session_id),
            "game_action_accepted",
            accepted_payload,
            None,
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
    match state
        .context
        .installation
        .store
        .commit_session_transition(durable_events, session_end.clone())
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
        session.state = after;
        if let Some(end) = session_end {
            let completed_at = chrono::Utc::now();
            session.updated_at = completed_at.to_rfc3339();
            session.voice_ends_at = keep_voice_open.then(|| {
                (completed_at
                    + chrono::Duration::seconds(
                        state.context.config.voice.post_completion_seconds as i64,
                    ))
                .to_rfc3339()
            });
            session.lifecycle = SessionLifecycle::Ended(end);
            session.agent_inbox.take();
        }
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
            shutdown_session_connections(&state.audio_connections, public_session_id).await;
        }
        broadcast_participant_states(&state, public_session_id).await;
    }
    if completed && state.context.config.agents.mode == AgentsMode::HumanVsHuman {
        shutdown_session_log(&state, public_session_id).await;
    }
    Ok(())
}

/// Reserves delivery before a commit without blocking the session on its own agent.
/// If a callback cannot keep up, end the room before accepting an undeliverable input.
/// The caller holds the transition lock so accepted actions and messages retain order.
pub(super) async fn reserve_agent_delivery<A: Game>(
    state: &Arc<AppState<A>>,
    public_session_id: &str,
    sender: mpsc::Sender<AgentObservation<A>>,
) -> Result<mpsc::OwnedPermit<AgentObservation<A>>> {
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
