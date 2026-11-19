//! Session initialization, membership and game-relative clocks.

use super::{
    deadline_after_seconds, now_iso, relative_game_time_ms, timestamp_millis, SqliteExperimentStore,
};
use crate::protocol::SessionEnd;
use crate::readable_id::dialogue_id;
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;

/// Input for creating one game session.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SessionRecord {
    pub experiment_id: String,
    /// Configuration revision selected when the session was created.
    pub config_revision: i64,
    /// Exact game version which executed the session.
    pub game_version: String,
    pub public_session_id: String,
    pub mode: String,
    pub lifecycle: String,
    /// Immutable `testing` or `research` data-use purpose.
    pub purpose: String,
    /// Fixed maximum duration of the unmatched waiting phase.
    pub waiting_timeout_seconds: i64,
    /// Fixed maximum wall-clock lifetime selected from this config revision.
    pub maximum_lifetime_seconds: i64,
}

/// Fixed phase clocks assigned atomically with one durable session row.
#[derive(Clone, Debug)]
pub struct SessionTiming {
    pub waiting_started_at: String,
    pub waiting_deadline_at: String,
    pub lifetime_deadline_at: String,
}

/// Input for placing a participant into a session with a session-local role.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SessionParticipantRecord {
    pub experiment_id: String,
    pub session_id: i64,
    pub participant_id: i64,
    pub participant_session_id: String,
    pub role: String,
    pub connection_status: String,
}

impl SqliteExperimentStore {
    /// Allocates a durable session with its initial phase deadlines.
    pub(crate) async fn create_session(&self, session: SessionRecord) -> Result<i64> {
        let mut tx = self.pool.begin().await?;
        if !matches!(session.purpose.as_str(), "testing" | "research") {
            return Err(anyhow::anyhow!(
                "session purpose must be testing or research"
            ));
        }
        let next_session_id = sqlx::query_scalar::<_, i64>(
            "select coalesce(max(session_id), 0) + 1 from sessions where experiment_id = ?",
        )
        .bind(&session.experiment_id)
        .fetch_one(&mut *tx)
        .await?;
        let readable_dialogue_id = loop {
            let candidate = dialogue_id();
            let exists = sqlx::query_scalar::<_, bool>(
                "select exists(select 1 from sessions where dialogue_id = ?)",
            )
            .bind(&candidate)
            .fetch_one(&mut *tx)
            .await?;
            if !exists {
                break candidate;
            }
        };
        let created_at = now_iso();
        let waiting_deadline_at =
            deadline_after_seconds(&created_at, session.waiting_timeout_seconds)?;
        let lifetime_deadline_at =
            deadline_after_seconds(&created_at, session.maximum_lifetime_seconds)?;
        let started_at = (session.lifecycle == "running").then(|| created_at.clone());
        sqlx::query(
            r#"
            insert into sessions
            (experiment_id, session_id, public_session_id, dialogue_id, mode, lifecycle, purpose,
             created_at, waiting_started_at, waiting_deadline_at, lifetime_deadline_at,
             started_at, config_revision, game_version)
            values (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(session.experiment_id)
        .bind(next_session_id)
        .bind(session.public_session_id)
        .bind(readable_dialogue_id)
        .bind(session.mode)
        .bind(session.lifecycle)
        .bind(session.purpose)
        .bind(&created_at)
        .bind(&created_at)
        .bind(waiting_deadline_at)
        .bind(lifetime_deadline_at)
        .bind(started_at)
        .bind(session.config_revision)
        .bind(session.game_version)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(next_session_id)
    }

    /// Reads the fixed deadlines assigned when a session was created.
    pub(crate) async fn session_timing(
        &self,
        experiment_id: &str,
        session_id: i64,
    ) -> Result<SessionTiming> {
        let (waiting_started_at, waiting_deadline_at, lifetime_deadline_at) =
            sqlx::query_as::<_, (String, String, String)>(
                "select waiting_started_at, waiting_deadline_at, lifetime_deadline_at from sessions where experiment_id = ? and session_id = ?",
            )
            .bind(experiment_id)
            .bind(session_id)
            .fetch_one(&self.pool)
            .await?;
        Ok(SessionTiming {
            waiting_started_at,
            waiting_deadline_at,
            lifetime_deadline_at,
        })
    }

    /// Atomically completes initialization and admits the first human member.
    pub(crate) async fn complete_session_initialization(
        &self,
        participant: SessionParticipantRecord,
    ) -> Result<bool> {
        let mut tx = self.pool.begin().await?;
        let result = sqlx::query(
            "update sessions set initialization_complete = 1 where experiment_id = ? and session_id = ? and lifecycle = 'forming' and initialization_complete = 0",
        )
        .bind(&participant.experiment_id)
        .bind(participant.session_id)
        .execute(&mut *tx)
        .await?;
        if result.rows_affected() == 0 {
            return Ok(false);
        }
        Self::insert_session_participant(&mut tx, participant).await?;
        tx.commit().await?;
        Ok(true)
    }

    /// Records the terminal failure of an incompletely initialized session.
    pub(crate) async fn fail_session_initialization(
        &self,
        experiment_id: &str,
        session_id: i64,
        reason_code: &str,
    ) -> Result<()> {
        let occurred_at = now_iso();
        let mut tx = self.pool.begin().await?;
        let event_index = sqlx::query_scalar::<_, i64>(
            "select coalesce(max(event_index), 0) + 1 from session_events where experiment_id = ? and session_id = ?",
        )
        .bind(experiment_id)
        .bind(session_id)
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query(
            r#"
            insert into session_events
            (experiment_id, session_id, event_index, event_type, actor_participant_id, actor_role, payload_json, game_state_json, game_time_ms)
            values (?, ?, ?, 'session_initialization_failed', null, null, ?, null, ?)
            "#,
        )
        .bind(experiment_id)
        .bind(session_id)
        .bind(event_index)
        .bind(serde_json::to_string(&json!({"reason_code": reason_code}))?)
        .bind(timestamp_millis(&occurred_at)?)
        .execute(&mut *tx)
        .await?;
        let session_end = SessionEnd {
            cause: crate::protocol::SessionEndCause::TechnicalFailure,
            completion: None,
            participant_results: HashMap::new(),
        };
        sqlx::query(
            "update sessions set lifecycle = 'ended', initialization_complete = 1, ended_at = ?, session_end_json = ? where experiment_id = ? and session_id = ? and lifecycle = 'forming'",
        )
        .bind(occurred_at)
        .bind(serde_json::to_string(&session_end)?)
        .bind(experiment_id)
        .bind(session_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Starts the game clock and rebases its existing events atomically.
    pub(crate) async fn start_session(
        &self,
        experiment_id: &str,
        session_id: i64,
        started_at: &str,
        idle_deadline_at: &str,
    ) -> Result<bool> {
        let started_at_ms = timestamp_millis(started_at)?;
        let mut tx = self.pool.begin().await?;
        let result = sqlx::query(
            "update sessions set lifecycle = 'running', started_at = ?, last_meaningful_activity_at = ?, idle_deadline_at = ? where experiment_id = ? and session_id = ? and lifecycle = 'forming' and initialization_complete = 1",
        )
        .bind(started_at)
        .bind(started_at)
        .bind(idle_deadline_at)
        .bind(experiment_id)
        .bind(session_id)
        .execute(&mut *tx)
        .await?;
        if result.rows_affected() == 1 {
            // Events written while the session was waiting temporarily contain Unix milliseconds;
            // rebasing them in this transaction gives every durable event the same game clock.
            sqlx::query(
                "update session_events set game_time_ms = game_time_ms - ? where experiment_id = ? and session_id = ?",
            )
            .bind(started_at_ms)
            .bind(experiment_id)
            .bind(session_id)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(result.rows_affected() == 1)
    }

    /// Records meaningful session activity and updates the idle deadline.
    pub(crate) async fn touch_session_activity(
        &self,
        experiment_id: &str,
        session_id: i64,
        activity_at: &str,
        idle_deadline_at: &str,
    ) -> Result<()> {
        sqlx::query(
            "update sessions set last_meaningful_activity_at = ?, idle_deadline_at = ? where experiment_id = ? and session_id = ? and lifecycle = 'running'",
        )
        .bind(activity_at)
        .bind(idle_deadline_at)
        .bind(experiment_id)
        .bind(session_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Converts a timestamp to milliseconds on the session game clock.
    pub(crate) async fn session_game_time_ms(
        &self,
        experiment_id: &str,
        session_id: i64,
        timestamp: &str,
    ) -> Result<i64> {
        let started_at = sqlx::query_scalar::<_, Option<String>>(
            "select started_at from sessions where experiment_id = ? and session_id = ?",
        )
        .bind(experiment_id)
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await?
        .flatten()
        .ok_or_else(|| anyhow!("session does not have a game-start time"))?;
        relative_game_time_ms(&started_at, timestamp)
    }

    /// Durably admits a member before making the session-local assignment visible.
    pub(crate) async fn add_session_participant(
        &self,
        participant: SessionParticipantRecord,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        Self::insert_session_participant(&mut tx, participant).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Inserts the assignment within the caller's session admission transaction.
    async fn insert_session_participant(
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        participant: SessionParticipantRecord,
    ) -> Result<()> {
        sqlx::query(
            r#"
            insert into session_participants
            (experiment_id, session_id, participant_id, participant_session_id, role, joined_at, connection_status)
            values (?, ?, ?, ?, ?, ?, ?)
            on conflict(experiment_id, session_id, participant_id) do update set
                participant_session_id = excluded.participant_session_id,
                role = excluded.role,
                connection_status = excluded.connection_status
            "#,
        )
        .bind(participant.experiment_id)
        .bind(participant.session_id)
        .bind(participant.participant_id)
        .bind(participant.participant_session_id)
        .bind(participant.role)
        .bind(now_iso())
        .bind(participant.connection_status)
        .execute(&mut **tx)
        .await?;
        Ok(())
    }

    /// Persists the latest connection status for a session member.
    pub(crate) async fn update_session_participant_connection(
        &self,
        participant_session_id: &str,
        connection_status: &str,
        left_at: Option<String>,
        reconnect_deadline_at: Option<String>,
    ) -> Result<()> {
        let disconnected_at = (connection_status == "disconnected")
            .then(|| left_at.clone())
            .flatten();
        sqlx::query(
            "update session_participants set connection_status = ?, left_at = ?, disconnected_at = ?, reconnect_deadline_at = ? where participant_session_id = ?",
        )
        .bind(connection_status)
        .bind(left_at)
        .bind(disconnected_at)
        .bind(reconnect_deadline_at)
        .bind(participant_session_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
