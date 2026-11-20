//! Resolves durable event identities and appends session events.
//!
//! Runtime operations provide live session and actor identities. This module resolves
//! their durable identifiers for storage. Transitions can construct a record for an
//! atomic batch, callers that depend on an append can propagate its failure, and
//! diagnostic paths can log an unsuccessful write.

use super::{anyhow, AppState, Arc, Future, Game, Result, SessionEventRecord, Value};

/// Logs storage failures while preserving the current active-session request behavior.
pub(super) async fn persist_event<F, T>(event_name: &'static str, record: F)
where
    F: Future<Output = Result<T>>,
{
    if let Err(error) = record.await {
        tracing::warn!(%error, event_name, "failed to persist event");
    }
}

/// Appends a session event after resolving the session and optional actor from the active cache.
pub(super) async fn persist_session_event<A: Game>(
    state: &Arc<AppState<A>>,
    public_session_id: &str,
    participant_session_id: Option<&str>,
    event_type: &'static str,
    payload: Value,
    game_state: Option<Value>,
) {
    let Ok(record) = session_event_record(
        state,
        public_session_id,
        participant_session_id,
        event_type,
        payload,
        game_state,
    )
    .await
    else {
        return;
    };
    persist_event(
        event_type,
        state
            .context
            .installation
            .store
            .append_session_event(record),
    )
    .await;
}

/// Appends a session event and propagates any persistence failure to the caller.
pub(super) async fn persist_session_event_required<A: Game>(
    state: &Arc<AppState<A>>,
    public_session_id: &str,
    participant_session_id: Option<&str>,
    event_type: &'static str,
    payload: Value,
    game_state: Option<Value>,
) -> Result<()> {
    let record = session_event_record(
        state,
        public_session_id,
        participant_session_id,
        event_type,
        payload,
        game_state,
    )
    .await?;
    state
        .context
        .installation
        .store
        .append_session_event(record)
        .await?;
    Ok(())
}

/// Resolves one session event record without writing it, for atomic transition batches.
pub(super) async fn session_event_record<A: Game>(
    state: &Arc<AppState<A>>,
    public_session_id: &str,
    participant_session_id: Option<&str>,
    event_type: &'static str,
    payload: Value,
    game_state: Option<Value>,
) -> Result<SessionEventRecord> {
    let (experiment_id, session_id, actor) = {
        let memory = state.memory.read().await;
        let session = memory
            .sessions
            .get(public_session_id)
            .ok_or_else(|| anyhow!("Session not found."))?;
        let actor = participant_session_id.and_then(|id| {
            session.participants.get(id).map(|participant| {
                (
                    participant.participant_id,
                    participant.role.as_str().to_string(),
                )
            })
        });
        (session.experiment_id.clone(), session.session_id, actor)
    };
    if session_id <= 0 {
        return Err(anyhow!("Session does not have a durable session."));
    }
    let (actor_participant_id, actor_role) = actor
        .map(|(participant_id, role)| (Some(participant_id), Some(role)))
        .unwrap_or((None, None));
    Ok(SessionEventRecord {
        experiment_id,
        session_id,
        event_type: event_type.to_string(),
        actor_participant_id,
        actor_role,
        payload,
        game_state,
    })
}
