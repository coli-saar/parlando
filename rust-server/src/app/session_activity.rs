//! Session activity timestamps and lifecycle queries.

use super::{
    broadcast_participant_states, expire_if_due_locked, persist_event, AppError, AppState, Arc,
    Game, Mutex, PlayerRole, Result, SessionLifecycle,
};

/// Returns the serialization lock that orders durable transitions for one session.
pub(super) async fn session_transition_lock<A: Game>(
    state: &Arc<AppState<A>>,
    public_session_id: &str,
) -> Option<Arc<Mutex<()>>> {
    state
        .memory
        .read()
        .await
        .sessions
        .get(public_session_id)
        .map(|room| room.transition_lock.clone())
}

/// Refreshes activity reported by a transport if the room is still running.
pub(super) async fn touch_transport_activity<A: Game>(
    state: &Arc<AppState<A>>,
    public_session_id: &str,
) {
    let Some(lock) = session_transition_lock(state, public_session_id).await else {
        return;
    };
    let _guard = lock.lock().await;
    if matches!(
        expire_if_due_locked(state, public_session_id).await,
        Ok(false)
    ) && session_is_active(state, public_session_id).await
    {
        touch_session_activity(state, public_session_id).await;
    }
}

/// Refreshes a session's meaningful-activity timestamp without treating heartbeats as activity.
pub(super) async fn touch_session_activity<A: Game>(
    state: &Arc<AppState<A>>,
    public_session_id: &str,
) {
    let activity_at = chrono::Utc::now();
    let idle_deadline_at = activity_at
        + chrono::Duration::seconds(state.context.config.session.session_idle_timeout_seconds);
    let durable = {
        let mut memory = state.memory.write().await;
        memory.sessions.get_mut(public_session_id).map(|session| {
            session.record_activity(activity_at, idle_deadline_at);
            (session.experiment_id.clone(), session.session_id)
        })
    };
    if let Some((experiment_id, session_id)) = durable {
        let activity_at = activity_at.to_rfc3339();
        let idle_deadline_at = idle_deadline_at.to_rfc3339();
        persist_event(
            "session activity",
            state.context.installation.store.touch_session_activity(
                &experiment_id,
                session_id,
                &activity_at,
                &idle_deadline_at,
            ),
        )
        .await;
        broadcast_participant_states(state, public_session_id).await;
    }
}

/// Closes and drains the log writer owned by one terminal live session.
pub(super) async fn shutdown_session_log<A: Game>(
    state: &Arc<AppState<A>>,
    public_session_id: &str,
) {
    let writer = state
        .memory
        .write()
        .await
        .sessions
        .get_mut(public_session_id)
        .and_then(|session| session.log_writer.take());
    if let Some(writer) = writer {
        writer.shutdown().await;
    }
}

/// Verifies that a participant belongs to the requested session.
pub(super) async fn require_session<A: Game>(
    state: &Arc<AppState<A>>,
    public_session_id: &str,
) -> Result<(), AppError> {
    if state
        .memory
        .read()
        .await
        .sessions
        .contains_key(public_session_id)
    {
        Ok(())
    } else {
        Err(AppError::not_found("Session not found."))
    }
}

/// Looks up the participant's role in the active session.
pub(super) async fn participant_role<A: Game>(
    state: &Arc<AppState<A>>,
    public_session_id: &str,
    participant_session_id: &str,
) -> Result<PlayerRole, AppError> {
    let memory = state.memory.read().await;
    let session = memory
        .sessions
        .get(public_session_id)
        .ok_or_else(|| AppError::not_found("Session not found."))?;
    session
        .participants
        .get(participant_session_id)
        .map(|participant| participant.role)
        .ok_or_else(|| AppError::forbidden("Participant is not in this session."))
}

/// Returns whether a session has already left the waiting-session phase.
pub(super) async fn session_has_started<A: Game>(
    state: &Arc<AppState<A>>,
    public_session_id: &str,
) -> bool {
    let memory = state.memory.read().await;
    memory
        .sessions
        .get(public_session_id)
        .is_some_and(|session| session.lifecycle() != &SessionLifecycle::Forming)
}

/// Returns whether meaningful participant input, including transcription, is currently active.
pub(super) async fn session_is_active<A: Game>(
    state: &Arc<AppState<A>>,
    public_session_id: &str,
) -> bool {
    let memory = state.memory.read().await;
    memory
        .sessions
        .get(public_session_id)
        .is_some_and(|session| session.accepts_interaction())
}

/// Allows ordinary voice relay and the bounded post-completion farewell period.
pub(super) async fn session_accepts_audio<A: Game>(
    state: &Arc<AppState<A>>,
    public_session_id: &str,
) -> bool {
    let now = chrono::Utc::now();
    let memory = state.memory.read().await;
    memory
        .sessions
        .get(public_session_id)
        .is_some_and(|session| match session.lifecycle() {
            SessionLifecycle::Forming | SessionLifecycle::Running => session.pause().is_none(),
            SessionLifecycle::Ended(_) => session
                .timing()
                .voice_ends_at
                .as_deref()
                .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
                .is_some_and(|deadline| deadline.with_timezone(&chrono::Utc) > now),
        })
}
