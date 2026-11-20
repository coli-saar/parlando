//! Preparation and ordered commitment of a live game action.
//!
//! Browser input and agent decisions enter through `submit_action`. Under the room's
//! transition lock, it derives each role's view from the same proposed game state and
//! reserves any required agent delivery. A successful storage commit authorizes
//! installation and publication. A failed commit or an already-ended room leaves the
//! proposed move unpublished.

use super::{
    anyhow, broadcast_participant_states, expire_if_due_locked, json, mpsc, prolific_handoff,
    protocol_json, reserve_agent_delivery, session_event_record, session_ready_for_game,
    session_transition_lock, shutdown_session_log, speechmatics_readiness_required,
    touch_session_activity, AgentObservation, AgentsMode, AppState, Arc, ExperimentConfig, Game,
    HashMap, LiveSession, ParticipantOutcomeKind, ParticipantResult, PlayerRole, Result,
    ServerMessage, SessionEnd, SessionEndCause, SubmissionRejection, TransitionCommit, Value,
};

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
    let after = session.game.apply_action(session.state(), &action, actor)?;
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
    let event = json!({"action": action_json, "metadata": session.game.transition_metadata(session.state(), &after, &action, actor)});
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
        participants.push(super::participant_state::action_message(
            session,
            &after,
            participant,
            actor,
            &action_json,
            observation,
        )?);
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
///
/// Human and agent callers share the room lock with leave and expiry. Prepare both
/// role views from one transition and reserve agent delivery before writing. Only
/// the winner of the durable commit may install that state and publish acceptance.
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
