//! Ordered session event writes and atomic terminal transitions.
//! A terminal result and its event batch become visible together. If an insert
//! fails, a retry can reuse the uncommitted event indices. A competing terminal
//! writer observes the existing ending and cannot append another winning transition.
//!
//! The live transition coordinator uses the commit outcome to decide whether it may install
//! the prepared state and publish observations. The session logger also uses the ordered
//! append operation for attributed entries.

use super::{now_iso, relative_game_time_ms, timestamp_millis, SqliteExperimentStore};
use crate::protocol::SessionEnd;
use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Outcome of writing an action against the durable session lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TransitionCommit {
    /// The action and its optional terminal value were persisted together.
    Committed,
    /// Another terminal writer already ended the session.
    AlreadyEnded,
}

/// A COMMIT error requires checking the durable identity before assuming rollback.
#[derive(Debug)]
pub(crate) struct TransitionCommitUnknown(pub(crate) sqlx::Error);

/// Pauses a test transition after the real commit, then simulates a lost success response.
#[cfg(test)]
pub(crate) struct TransitionAckFault {
    pub(crate) committed: tokio::sync::oneshot::Sender<()>,
    pub(crate) release: tokio::sync::oneshot::Receiver<()>,
}

impl std::fmt::Display for TransitionCommitUnknown {
    /// Reports the database error while distinguishing it from a failed insert.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "could not confirm transition commit: {}", self.0)
    }
}

impl std::error::Error for TransitionCommitUnknown {
    /// Returns the driver error from the attempted COMMIT.
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

/// Input for one ordered event inside a session.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SessionEventRecord {
    pub experiment_id: String,
    pub session_id: i64,
    pub event_type: String,
    pub actor_participant_id: Option<i64>,
    pub actor_role: Option<String>,
    pub payload: Value,
    pub game_state: Option<Value>,
}

/// Durable session event row returned by storage queries.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StoredSessionEvent {
    pub event_id: i64,
    pub experiment_id: String,
    pub session_id: i64,
    pub event_index: i64,
    pub event_type: String,
    pub actor_participant_id: Option<i64>,
    pub actor_role: Option<String>,
    pub payload: Value,
    pub game_state: Option<Value>,
    /// Milliseconds relative to the instant at which this game entered `running`.
    pub game_time_ms: i64,
}

/// Returns game-relative milliseconds, or temporary Unix milliseconds before game start.
///
/// The lookup runs inside the event's write transaction so it cannot race the transaction that
/// establishes the game-start timestamp and rebases waiting-session events.
async fn stored_game_time_ms(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    experiment_id: &str,
    session_id: i64,
    timestamp: &str,
) -> Result<i64> {
    let started_at = sqlx::query_scalar::<_, Option<String>>(
        "select started_at from sessions where experiment_id = ? and session_id = ?",
    )
    .bind(experiment_id)
    .bind(session_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| anyhow!("session not found"))?;
    match started_at {
        Some(started_at) => relative_game_time_ms(&started_at, timestamp),
        None => timestamp_millis(timestamp),
    }
}

/// Persists an ended session and every recipient-specific terminal result in one transaction.
async fn persist_terminal_value(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    experiment_id: &str,
    session_id: i64,
    session_end: &SessionEnd,
) -> Result<()> {
    let ended_at = now_iso();
    sqlx::query(
        "update sessions set lifecycle = 'ended', ended_at = ?, completion_json = ?, session_end_json = ? where experiment_id = ? and session_id = ? and lifecycle != 'ended'",
    )
    .bind(&ended_at)
    .bind(
        session_end
            .completion
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?,
    )
    .bind(serde_json::to_string(session_end)?)
    .bind(experiment_id)
    .bind(session_id)
    .execute(&mut **tx)
    .await?;
    for (role, result) in &session_end.participant_results {
        sqlx::query(
            "update session_participants set terminal_result_json = ?, left_at = coalesce(left_at, ?) where experiment_id = ? and session_id = ? and role = ?",
        )
        .bind(serde_json::to_string(result)?)
        .bind(&ended_at)
        .bind(experiment_id)
        .bind(session_id)
        .bind(role)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

/// Checks the accepted marker while the room lock prevents a later action.
///
/// The event-type index can find the most recent acceptance in insertion order.
/// Other event types can still be appended by the logger during turn resolution.
async fn latest_transition_matches(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    experiment_id: &str,
    session_id: i64,
    turn_id: &str,
) -> Result<bool> {
    Ok(sqlx::query_scalar(
        "select coalesce((select json_extract(payload_json, '$.turn_id') = ? from session_events
         where experiment_id = ? and session_id = ? and event_type = 'game_action_accepted'
         order by event_id desc limit 1), 0)",
    )
    .bind(turn_id)
    .bind(experiment_id)
    .bind(session_id)
    .fetch_one(&mut **tx)
    .await?)
}

impl SqliteExperimentStore {
    /// Resolves an interrupted COMMIT after any prior SQLite writer has released its transaction.
    pub(crate) async fn session_transition_committed(
        &self,
        experiment_id: &str,
        session_id: i64,
        turn_id: &str,
    ) -> Result<bool> {
        #[cfg(test)]
        if self
            .fail_transition_probe_once
            .swap(false, std::sync::atomic::Ordering::Relaxed)
        {
            return Err(sqlx::Error::PoolTimedOut.into());
        }
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let committed =
            latest_transition_matches(&mut tx, experiment_id, session_id, turn_id).await?;
        Ok(committed)
    }

    /// Appends an event with the next index on its session timeline.
    pub(crate) async fn append_session_event(&self, event: SessionEventRecord) -> Result<i64> {
        let occurred_at = now_iso();
        let mut tx = self.pool.begin().await?;
        let game_time_ms = stored_game_time_ms(
            &mut tx,
            &event.experiment_id,
            event.session_id,
            &occurred_at,
        )
        .await?;
        let event_index = sqlx::query_scalar::<_, i64>(
            "select coalesce(max(event_index), 0) + 1 from session_events where experiment_id = ? and session_id = ?",
        )
        .bind(&event.experiment_id)
        .bind(event.session_id)
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query(
            r#"
            insert into session_events
            (experiment_id, session_id, event_index, event_type, actor_participant_id, actor_role, payload_json, game_state_json, game_time_ms)
            values (?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(event.experiment_id)
        .bind(event.session_id)
        .bind(event_index)
        .bind(event.event_type)
        .bind(event.actor_participant_id)
        .bind(&event.actor_role)
        .bind(serde_json::to_string(&event.payload)?)
        .bind(event.game_state.map(|state| serde_json::to_string(&state)).transpose()?)
        .bind(game_time_ms)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(event_index)
    }

    /// Atomically commits one event batch and its terminal value, if any.
    ///
    /// A `turn_id` on the first accepted-action event identifies retries of the same
    /// prepared batch while the coordinator holds the live room lock. The check runs
    /// under the SQLite writer reservation before checking lifecycle, so a retried
    /// terminal action is idempotent.
    pub(crate) async fn commit_session_transition(
        &self,
        events: Vec<SessionEventRecord>,
        session_end: Option<SessionEnd>,
    ) -> Result<TransitionCommit> {
        let Some(first) = events.first() else {
            bail!("an action transition must contain an event");
        };
        let experiment_id = first.experiment_id.clone();
        let session_id = first.session_id;
        if events
            .iter()
            .any(|event| event.experiment_id != experiment_id || event.session_id != session_id)
        {
            bail!("transition events must belong to one session");
        }
        let occurred_at = now_iso();
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        if let Some(turn_id) = first.payload.get("turn_id").and_then(Value::as_str) {
            if latest_transition_matches(&mut tx, &experiment_id, session_id, turn_id).await? {
                return Ok(TransitionCommit::Committed);
            }
        }
        let game_time_ms =
            stored_game_time_ms(&mut tx, &experiment_id, session_id, &occurred_at).await?;
        let lifecycle = sqlx::query_scalar::<_, String>(
            "select lifecycle from sessions where experiment_id = ? and session_id = ?",
        )
        .bind(&experiment_id)
        .bind(session_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(lifecycle) = lifecycle else {
            bail!("session not found");
        };
        if lifecycle == "ended" {
            tx.rollback().await?;
            return Ok(TransitionCommit::AlreadyEnded);
        }
        let mut event_index = sqlx::query_scalar::<_, i64>(
            "select coalesce(max(event_index), 0) from session_events where experiment_id = ? and session_id = ?",
        )
        .bind(&experiment_id)
        .bind(session_id)
        .fetch_one(&mut *tx)
        .await?;
        for event in events {
            event_index += 1;
            sqlx::query(
                r#"
                insert into session_events
                (experiment_id, session_id, event_index, event_type, actor_participant_id, actor_role, payload_json, game_state_json, game_time_ms)
                values (?, ?, ?, ?, ?, ?, ?, ?, ?)
                "#,
            )
            .bind(event.experiment_id)
            .bind(event.session_id)
            .bind(event_index)
            .bind(event.event_type)
            .bind(event.actor_participant_id)
            .bind(event.actor_role)
            .bind(serde_json::to_string(&event.payload)?)
            .bind(event.game_state.map(|state| serde_json::to_string(&state)).transpose()?)
            .bind(game_time_ms)
            .execute(&mut *tx)
            .await?;
        }
        if let Some(session_end) = session_end.as_ref() {
            persist_terminal_value(&mut tx, &experiment_id, session_id, session_end).await?;
        }
        tx.commit().await.map_err(TransitionCommitUnknown)?;
        #[cfg(test)]
        {
            let fault = self.transition_ack_fault.lock().unwrap().take();
            if let Some(fault) = fault {
                let _ = fault.committed.send(());
                let _ = fault.release.await;
                return Err(TransitionCommitUnknown(sqlx::Error::PoolTimedOut).into());
            }
        }
        Ok(TransitionCommit::Committed)
    }

    /// Atomically records the terminal event and session result.
    pub(crate) async fn end_session(
        &self,
        event: SessionEventRecord,
        session_end: SessionEnd,
    ) -> Result<bool> {
        let occurred_at = now_iso();
        let mut tx = self.pool.begin().await?;
        let game_time_ms = stored_game_time_ms(
            &mut tx,
            &event.experiment_id,
            event.session_id,
            &occurred_at,
        )
        .await?;
        let lifecycle = sqlx::query_scalar::<_, String>(
            "select lifecycle from sessions where experiment_id = ? and session_id = ?",
        )
        .bind(&event.experiment_id)
        .bind(event.session_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(lifecycle) = lifecycle else {
            bail!("session not found");
        };
        if lifecycle == "ended" {
            tx.rollback().await?;
            return Ok(false);
        }
        let event_index = sqlx::query_scalar::<_, i64>(
            "select coalesce(max(event_index), 0) + 1 from session_events where experiment_id = ? and session_id = ?",
        )
        .bind(&event.experiment_id)
        .bind(event.session_id)
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query(
            r#"
            insert into session_events
            (experiment_id, session_id, event_index, event_type, actor_participant_id,
             actor_role, payload_json, game_state_json, game_time_ms)
            values (?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&event.experiment_id)
        .bind(event.session_id)
        .bind(event_index)
        .bind(event.event_type)
        .bind(event.actor_participant_id)
        .bind(event.actor_role)
        .bind(serde_json::to_string(&event.payload)?)
        .bind(
            event
                .game_state
                .map(|state| serde_json::to_string(&state))
                .transpose()?,
        )
        .bind(game_time_ms)
        .execute(&mut *tx)
        .await?;
        persist_terminal_value(
            &mut tx,
            &event.experiment_id,
            event.session_id,
            &session_end,
        )
        .await?;
        tx.commit().await?;
        Ok(true)
    }
}
