//! Participant readiness and activation of a fully assembled session.

use super::{
    expire_if_due_locked, json, reserve_agent_delivery, send_role_assignment,
    session_ready_for_game, session_transition_lock, voice_message, AgentObservation, AppState,
    Arc, Game, SessionLifecycle, SessionParticipant,
};

/// Marks one participant's session-local audio/STT setup as complete.
pub(super) async fn mark_participant_audio_ready<A: Game>(
    state: &Arc<AppState<A>>,
    public_session_id: &str,
    participant_session_id: &str,
) {
    let changed = {
        let mut memory = state.memory.write().await;
        memory
            .sessions
            .get_mut(public_session_id)
            .and_then(|session| session.participants.get_mut(participant_session_id))
            .is_some_and(SessionParticipant::mark_audio_ready)
    };
    if changed {
        tracing::debug!(%public_session_id, %participant_session_id, "participant transcription initialized");
    }
}

/// Publishes whether this participant's audio can currently enter game transcription.
pub(super) async fn publish_transcription_readiness<A: Game>(
    state: &Arc<AppState<A>>,
    public_session_id: &str,
    role: &str,
) {
    let ready = {
        let memory = state.memory.read().await;
        let Some(session) = memory.sessions.get(public_session_id) else {
            return;
        };
        let Some(participant) = session
            .participants
            .values()
            .find(|p| p.role.as_str() == role)
        else {
            return;
        };
        session.accepts_interaction() && participant.audio_ready()
    };
    let message = if !ready {
        "Waiting for game activation"
    } else if state.context.config.transcription.enabled {
        "ASR listening"
    } else {
        "ASR idle"
    };
    state
        .audio_sessions
        .send_control(
            public_session_id,
            role,
            json!({"type":"transcriptionStatus","ready":ready,"message":message}).to_string(),
        )
        .await;
}

/// Starts a session once all humans/agents and required audio setup are ready.
pub(super) async fn maybe_start_game<A: Game>(state: Arc<AppState<A>>, public_session_id: &str) {
    let Some(transition_lock) = session_transition_lock(&state, public_session_id).await else {
        return;
    };
    let _transition_guard = transition_lock.lock().await;
    match expire_if_due_locked(&state, public_session_id).await {
        Ok(false) => {}
        Ok(true) => return,
        Err(error) => {
            tracing::error!(%error, public_session_id, "could not evaluate activation deadline");
            return;
        }
    }
    let durable_session = {
        let memory = state.memory.read().await;
        memory.sessions.get(public_session_id).and_then(|session| {
            (session.lifecycle() == &SessionLifecycle::Forming
                && session_ready_for_game::<A>(&state.context.config, session))
            .then(|| (session.experiment_id.clone(), session.session_id))
        })
    };
    let Some((experiment_id, session_id)) = durable_session else {
        return;
    };
    let started_at = chrono::Utc::now();
    let idle_deadline_at = started_at
        + chrono::Duration::seconds(state.context.config.session.session_idle_timeout_seconds);
    match state
        .context
        .installation
        .store
        .start_session(
            &experiment_id,
            session_id,
            &started_at.to_rfc3339(),
            &idle_deadline_at.to_rfc3339(),
        )
        .await
    {
        Ok(true) => {}
        Ok(false) => {
            tracing::warn!(
                public_session_id,
                session_id,
                "waiting session was not durably startable"
            );
            return;
        }
        Err(error) => {
            tracing::error!(%error, public_session_id, session_id, "could not durably start session");
            return;
        }
    }
    {
        let mut memory = state.memory.write().await;
        let Some(session) = memory.sessions.get_mut(public_session_id) else {
            return;
        };
        session.activate(started_at, idle_deadline_at);
    }

    let participants = {
        let memory = state.memory.read().await;
        memory
            .sessions
            .get(public_session_id)
            .map(|session| {
                session
                    .participants
                    .values()
                    .map(|participant| {
                        (participant.participant_session_id.clone(), participant.role)
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };
    let sender = state
        .memory
        .read()
        .await
        .sessions
        .get(public_session_id)
        .and_then(|session| session.agent_inbox.clone());
    if let Some(sender) = sender {
        let Ok(permit) = reserve_agent_delivery(&state, public_session_id, sender).await else {
            return;
        };
        permit.send(AgentObservation::Activated);
    }
    let _ = state
        .session_bus(public_session_id)
        .await
        .send(voice_message(&state, public_session_id).await);
    for (participant_session_id, role) in participants {
        publish_transcription_readiness(&state, public_session_id, role.as_str()).await;
        send_role_assignment(&state, public_session_id, &participant_session_id, role).await;
    }
}
