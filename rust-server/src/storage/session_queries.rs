//! Read-only session history, progress and participant projections.
//!
//! Admission can recover the durable terminal state for a returning participant. The
//! dashboard uses session, member, event, and progress projections, while runtime startup
//! uses the interrupted-session query to identify rooms requiring reconciliation.

use super::{SqliteExperimentStore, StoredSessionEvent};
use crate::protocol::{ParticipantResult, SessionEnd};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::Row;
use std::collections::BTreeMap;

/// Durable session summary returned by recent-game database queries.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StoredSessionSummary {
    pub experiment_id: String,
    pub session_id: i64,
    /// Client-facing random identifier for this live session.
    pub public_session_id: String,
    /// Human-readable random identifier reused for this dialogue in exports and administration.
    pub dialogue_id: String,
    pub mode: String,
    pub lifecycle: String,
    /// Immutable data-use classification selected from experiment lifecycle at creation.
    pub purpose: String,
    /// Immutable experiment configuration revision used for this session.
    pub config_revision: i64,
    /// Exact compiled game version which executed this session.
    pub game_version: String,
    pub created_at: String,
    /// Fixed start of the unmatched waiting phase.
    pub waiting_started_at: String,
    /// Fixed deadline at which unmatched waiting ends.
    pub waiting_deadline_at: String,
    /// Absolute infrastructure lifetime deadline.
    pub lifetime_deadline_at: String,
    /// Last accepted message or game action, excluding heartbeats.
    pub last_meaningful_activity_at: Option<String>,
    /// Current idle deadline while the game is running.
    pub idle_deadline_at: Option<String>,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub completion: Option<Value>,
    /// Structured shared reason and recipient results for one ended session.
    pub session_end: Option<SessionEnd>,
    pub participant_count: i64,
    pub event_count: i64,
    /// Latest event position on the authoritative game clock.
    pub last_event_game_time_ms: Option<i64>,
}

/// Complete experiment-wide counts used by the administrator progress view.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct StoredSessionProgress {
    /// Number of research sessions in the experiment, regardless of lifecycle.
    pub total_sessions: i64,
    /// Number of research sessions whose lifecycle is terminal.
    pub ended_sessions: i64,
    /// Terminal research-session counts keyed by the serialized session-end cause.
    pub end_causes: BTreeMap<String, i64>,
    /// All sessions in reverse chronological order, including testing sessions.
    pub sessions: Vec<StoredProgressSession>,
}

/// Lightweight durable session entry displayed in the experiment progress log.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct StoredProgressSession {
    /// Database identifier used to open the session in the administrator dashboard.
    pub session_id: i64,
    /// Human-readable session name assigned by Parlando.
    pub dialogue_id: Option<String>,
    /// Research or testing purpose recorded when the session was created.
    pub purpose: String,
    /// Current durable lifecycle.
    pub lifecycle: String,
    /// Durable creation timestamp.
    pub created_at: String,
    /// Structured terminal reason, when the session has ended.
    pub session_end: Option<SessionEnd>,
}

/// Durable participant metadata for one session-local game appearance.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StoredSessionParticipant {
    pub experiment_id: String,
    pub session_id: i64,
    pub participant_id: i64,
    /// Human-readable random identifier reused within this experiment's administration and exports.
    pub research_id: Option<String>,
    pub participant_session_id: String,
    pub role: String,
    pub joined_at: String,
    pub left_at: Option<String>,
    pub connection_status: String,
    /// Most recent transport-disconnection time, when applicable.
    pub disconnected_at: Option<String>,
    /// Fixed deadline for reconnecting after that disconnection.
    pub reconnect_deadline_at: Option<String>,
    pub participant_kind: Option<String>,
    /// Durable recruitment-source classification without provider-specific identifiers.
    pub identity_provider: Option<String>,
    pub metadata: Option<Value>,
    /// Immutable recipient-specific consequence derived when the shared session ended.
    pub terminal_result: Option<ParticipantResult>,
    /// Private Prolific participant id returned only by administrator session inspection.
    pub prolific_participant_id: Option<String>,
    /// Private Prolific study id returned only by administrator session inspection.
    pub prolific_study_id: Option<String>,
    /// Private Prolific submission session id returned only by administrator session inspection.
    pub prolific_session_id: Option<String>,
    /// Last status read from Prolific, if reconciled.
    pub prolific_status: Option<String>,
    /// Completion code currently recorded by Prolific.
    pub prolific_entered_code: Option<String>,
    /// Provider return-request timestamp, when present.
    pub prolific_return_requested_at: Option<String>,
    /// Time at which these provider facts were refreshed.
    pub prolific_reconciled_at: Option<String>,
}

/// Durable terminal participant projection used after live-room cleanup.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StoredTerminalParticipantState {
    pub public_session_id: String,
    pub role: String,
    pub result: ParticipantResult,
}

impl SqliteExperimentStore {
    /// Loads the durable terminal result for a participant session.
    pub(crate) async fn terminal_participant_state(
        &self,
        experiment_id: &str,
        participant_session_id: &str,
    ) -> Result<Option<StoredTerminalParticipantState>> {
        let row = sqlx::query_as::<_, (String, String, String)>(
            r#"
            select s.public_session_id, sp.role, sp.terminal_result_json
            from session_participants sp
            join sessions s
              on s.experiment_id = sp.experiment_id and s.session_id = sp.session_id
            where sp.experiment_id = ? and sp.participant_session_id = ?
              and s.lifecycle = 'ended' and sp.terminal_result_json is not null
            "#,
        )
        .bind(experiment_id)
        .bind(participant_session_id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|(public_session_id, role, raw_result)| {
            Ok(StoredTerminalParticipantState {
                public_session_id,
                role,
                result: serde_json::from_str(&raw_result)?,
            })
        })
        .transpose()
    }

    /// Reads the ordered event history for one session.
    pub(crate) async fn session_events(
        &self,
        experiment_id: &str,
        session_id: i64,
        event_type: Option<&str>,
    ) -> Result<Vec<StoredSessionEvent>> {
        let sql = if event_type.is_some() {
            "select event_id, experiment_id, session_id, event_index, event_type, actor_participant_id, actor_role, payload_json, game_state_json, game_time_ms from session_events where experiment_id = ? and session_id = ? and event_type = ? order by event_index"
        } else {
            "select event_id, experiment_id, session_id, event_index, event_type, actor_participant_id, actor_role, payload_json, game_state_json, game_time_ms from session_events where experiment_id = ? and session_id = ? order by event_index"
        };
        let mut query = sqlx::query_as::<
            _,
            (
                i64,
                String,
                i64,
                i64,
                String,
                Option<i64>,
                Option<String>,
                String,
                Option<String>,
                i64,
            ),
        >(sql)
        .bind(experiment_id)
        .bind(session_id);
        if let Some(event_type) = event_type {
            query = query.bind(event_type);
        }
        query
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(stored_event_from_sql_row)
            .collect()
    }

    /// Reads the indexed high-water mark so an idle dashboard poll skips projection.
    pub(crate) async fn latest_event_index(
        &self,
        experiment_id: &str,
        session_id: i64,
    ) -> Result<i64> {
        Ok(sqlx::query_scalar("select coalesce(max(event_index), 0) from session_events where experiment_id = ? and session_id = ?")
            .bind(experiment_id).bind(session_id).fetch_one(&self.pool).await?)
    }

    /// Reads one chronological dashboard page and the auxiliary speech records needed
    /// to display its messages. Recovery checkpoints are never part of a monitor read.
    pub(crate) async fn timeline_page(
        &self,
        experiment_id: &str,
        session_id: i64,
        before: Option<i64>,
        after: Option<i64>,
    ) -> Result<TimelinePage> {
        const PAGE_SIZE: usize = 100;
        let mut tx = self.pool.begin().await?;
        let mut query = sqlx::QueryBuilder::new(
            "select event_id, experiment_id, session_id, event_index, event_type,
                    actor_participant_id, actor_role, payload_json, null, game_time_ms
             from session_events where experiment_id = ",
        );
        query
            .push_bind(experiment_id)
            .push(" and session_id = ")
            .push_bind(session_id);
        if let Some(after) = after {
            query
                .push(" and event_index > ")
                .push_bind(after)
                .push(" order by event_index asc");
        } else {
            if let Some(before) = before {
                query.push(" and event_index < ").push_bind(before);
            }
            query.push(" order by event_index desc");
        }
        query.push(" limit ").push_bind((PAGE_SIZE + 1) as i64);
        let mut rows = query
            .build_query_as::<SessionEventSqlRow>()
            .fetch_all(&mut *tx)
            .await?;
        let has_more = rows.len() > PAGE_SIZE;
        rows.truncate(PAGE_SIZE);
        if after.is_none() {
            rows.reverse();
        }
        let first_index = rows.first().map(|row| row.3);
        let last_index = rows.last().map_or(after.unwrap_or(0), |row| row.3);
        let mut events = rows
            .into_iter()
            .map(stored_event_from_sql_row)
            .collect::<Result<Vec<_>>>()?;
        events.retain(|event| {
            !matches!(
                event.event_type.as_str(),
                "conversation_speech_timing" | "tts_diagnostic"
            )
        });
        let message_ids: Vec<&str> = events
            .iter()
            .filter(|event| event.event_type == "conversation_message")
            .filter_map(|event| event.payload["id"].as_str())
            .collect();
        if !message_ids.is_empty() {
            let support = sqlx::query_as::<_, SessionEventSqlRow>(
                "with ids(id) as (select value from json_each(?1)),
                 supporting(event_id, last_index) as (
                   select event_id, max(event_index) from session_events
                   where experiment_id=?2 and session_id=?3 and event_type='conversation_speech_timing'
                   and payload_json ->> 'message_id' in (select id from ids)
                   group by payload_json ->> 'message_id'
                   union all
                   select event_id, max(event_index) from session_events
                   where experiment_id=?2 and session_id=?3 and event_type='tts_diagnostic'
                   and payload_json ->> 'event'='tts_publish_completed'
                   and json_extract(payload_json, '$.metadata.bytes') > 0
                   and json_extract(payload_json, '$.metadata.message_id') in (select id from ids)
                   group by json_extract(payload_json, '$.metadata.message_id'))
                 select event_id, experiment_id, session_id, event_index, event_type, actor_participant_id,
                        actor_role, payload_json, null, game_time_ms from session_events
                 where event_id in (select event_id from supporting) order by event_index"
            ).bind(serde_json::to_string(&message_ids)?).bind(experiment_id).bind(session_id)
             .fetch_all(&mut *tx).await?;
            for row in support {
                events.push(stored_event_from_sql_row(row)?);
            }
        }
        let after_terminal = if let Some(first) =
            first_index.filter(|_| has_more || after.is_some())
        {
            sqlx::query_scalar::<_, bool>(
                "select exists(select 1 from session_events where experiment_id=? and session_id=?
                 and event_type in ('session_completed','session_abandoned','session_expired','participant_disconnected')
                 and event_index < ?)"
            ).bind(experiment_id).bind(session_id).bind(first).fetch_one(&mut *tx).await?
        } else {
            false
        };
        tx.commit().await?;
        Ok(TimelinePage {
            older_before: first_index.filter(|_| has_more && after.is_none()),
            last_index,
            events,
            after_terminal,
        })
    }

    /// Identifies experiments with unfinished sessions, including retired runtime revisions.
    pub(crate) async fn interrupted_experiment_ids(&self) -> Result<Vec<String>> {
        Ok(sqlx::query_scalar(
            "SELECT DISTINCT experiment_id FROM sessions WHERE lifecycle != 'ended'",
        )
        .fetch_all(&self.pool)
        .await?)
    }

    /// Lists all unfinished sessions for process recovery without dashboard limits or event joins.
    pub(crate) async fn interrupted_sessions(
        &self,
        experiment_id: &str,
    ) -> Result<Vec<(i64, bool)>> {
        Ok(sqlx::query_as("SELECT session_id, started_at IS NOT NULL FROM sessions WHERE experiment_id = ? AND lifecycle != 'ended' ORDER BY session_id")
            .bind(experiment_id).fetch_all(&self.pool).await?)
    }

    /// Filters and limits sessions before reading participant and event summaries.
    pub(crate) async fn recent_sessions(
        &self,
        experiment_id: &str,
        lifecycle: Option<&str>,
        prolific_pid_suffix: Option<&str>,
        limit: i64,
    ) -> Result<Vec<StoredSessionSummary>> {
        let limit = limit.clamp(1, 10_000);
        let sessions = sqlx::query(
            r#"
            with recent as (
                select s.*
                from sessions s
                where s.experiment_id = ?1
                  and (?2 is null or s.lifecycle = ?2)
                  and (?3 is null or exists (
                      select 1
                      from session_participants sp
                      where sp.experiment_id = s.experiment_id and sp.session_id = s.session_id
                        and sp.participant_id in (
                            select ps.participant_id
                            from prolific_submissions ps
                            where ps.experiment_id = ?1
                              and ps.verification_method != 'synthetic_testing'
                              and substr(ps.prolific_participant_id, -length(?3)) = ?3
                        )
                  ))
                order by s.created_at desc, s.session_id desc
                limit ?4
            )
            select s.*,
                   (select count(*) from session_participants sp
                    where sp.experiment_id = s.experiment_id and sp.session_id = s.session_id) as participant_count,
                   (select count(*) from session_events se
                    where se.experiment_id = s.experiment_id and se.session_id = s.session_id) as event_count,
                   (select max(se.game_time_ms) from session_events se
                    where se.experiment_id = s.experiment_id and se.session_id = s.session_id) as last_event_game_time_ms
            from recent s
            order by s.created_at desc, s.session_id desc
            "#,
        )
        .bind(experiment_id)
        .bind(lifecycle)
        .bind(prolific_pid_suffix)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(|row| {
            Ok(StoredSessionSummary {
                experiment_id: experiment_id.to_string(),
                session_id: row.try_get("session_id")?,
                public_session_id: row.try_get("public_session_id")?,
                dialogue_id: row.try_get("dialogue_id")?,
                mode: row.try_get("mode")?,
                lifecycle: row.try_get("lifecycle")?,
                purpose: row.try_get("purpose")?,
                config_revision: row.try_get("config_revision")?,
                game_version: row.try_get("game_version")?,
                created_at: row.try_get("created_at")?,
                waiting_started_at: row.try_get("waiting_started_at")?,
                waiting_deadline_at: row.try_get("waiting_deadline_at")?,
                lifetime_deadline_at: row.try_get("lifetime_deadline_at")?,
                last_meaningful_activity_at: row.try_get("last_meaningful_activity_at")?,
                idle_deadline_at: row.try_get("idle_deadline_at")?,
                started_at: row.try_get("started_at")?,
                ended_at: row.try_get("ended_at")?,
                completion: row
                    .try_get::<Option<String>, _>("completion_json")?
                    .map(|raw| serde_json::from_str::<Value>(&raw))
                    .transpose()?,
                session_end: row
                    .try_get::<Option<String>, _>("session_end_json")?
                    .map(|raw| serde_json::from_str::<SessionEnd>(&raw))
                    .transpose()?,
                participant_count: row.try_get("participant_count")?,
                event_count: row.try_get("event_count")?,
                last_event_game_time_ms: row.try_get("last_event_game_time_ms")?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
        Ok(sessions)
    }

    /// Summarizes session progress for the selected experiment.
    pub(crate) async fn session_progress(
        &self,
        experiment_id: &str,
    ) -> Result<StoredSessionProgress> {
        let total_sessions = sqlx::query_scalar::<_, i64>(
            "select count(*) from sessions where experiment_id = ? and purpose = 'research'",
        )
        .bind(experiment_id)
        .fetch_one(&self.pool)
        .await?;
        let rows = sqlx::query(
            r#"
            select coalesce(json_extract(session_end_json, '$.cause.type'), 'unavailable') as cause,
                   count(*) as session_count
            from sessions
            where experiment_id = ? and purpose = 'research' and lifecycle = 'ended'
            group by cause
            order by cause
            "#,
        )
        .bind(experiment_id)
        .fetch_all(&self.pool)
        .await?;
        let mut end_causes = BTreeMap::new();
        let mut ended_sessions = 0;
        for row in rows {
            let cause = row.try_get::<String, _>("cause")?;
            let count = row.try_get::<i64, _>("session_count")?;
            ended_sessions += count;
            end_causes.insert(cause, count);
        }
        let sessions = sqlx::query(
            r#"
            select session_id, dialogue_id, purpose, lifecycle, created_at, session_end_json
            from sessions
            where experiment_id = ?
            order by created_at desc, session_id desc
            "#,
        )
        .bind(experiment_id)
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(|row| {
            Ok(StoredProgressSession {
                session_id: row.try_get("session_id")?,
                dialogue_id: row.try_get("dialogue_id")?,
                purpose: row.try_get("purpose")?,
                lifecycle: row.try_get("lifecycle")?,
                created_at: row.try_get("created_at")?,
                session_end: row
                    .try_get::<Option<String>, _>("session_end_json")?
                    .map(|raw| serde_json::from_str::<SessionEnd>(&raw))
                    .transpose()?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
        Ok(StoredSessionProgress {
            total_sessions,
            ended_sessions,
            end_causes,
            sessions,
        })
    }

    /// Lists the participant assignments for one session.
    pub(crate) async fn session_participants(
        &self,
        experiment_id: &str,
        session_id: i64,
    ) -> Result<Vec<StoredSessionParticipant>> {
        let participants = sqlx::query(
            r#"
            select sp.experiment_id, sp.session_id, sp.participant_id,
                   sp.participant_session_id, sp.role, sp.joined_at, sp.left_at,
                   sp.connection_status, sp.disconnected_at, sp.reconnect_deadline_at,
                   p.research_id, p.participant_kind,
                   case when ps.verification_method = 'synthetic_testing' then 'local_preview'
                        else p.identity_provider end as identity_provider,
                   p.metadata_json,
                   sp.terminal_result_json, ps.prolific_participant_id, ps.prolific_study_id,
                   ps.prolific_session_id, ps.provider_status as prolific_status,
                   ps.entered_completion_code as prolific_entered_code,
                   ps.return_requested_at as prolific_return_requested_at,
                   ps.reconciled_at as prolific_reconciled_at
            from session_participants sp
            left join participants p on p.participant_id = sp.participant_id
            left join prolific_submissions ps
              on ps.experiment_id = sp.experiment_id and ps.participant_id = sp.participant_id
            where sp.experiment_id = ? and sp.session_id = ?
            order by sp.role, sp.joined_at
            "#,
        )
        .bind(experiment_id)
        .bind(session_id)
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(|row| {
            Ok(StoredSessionParticipant {
                experiment_id: row.try_get("experiment_id")?,
                session_id: row.try_get("session_id")?,
                participant_id: row.try_get("participant_id")?,
                participant_session_id: row.try_get("participant_session_id")?,
                role: row.try_get("role")?,
                joined_at: row.try_get("joined_at")?,
                left_at: row.try_get("left_at")?,
                connection_status: row.try_get("connection_status")?,
                disconnected_at: row.try_get("disconnected_at")?,
                reconnect_deadline_at: row.try_get("reconnect_deadline_at")?,
                research_id: row.try_get("research_id")?,
                participant_kind: row.try_get("participant_kind")?,
                identity_provider: row.try_get("identity_provider")?,
                metadata: row
                    .try_get::<Option<String>, _>("metadata_json")?
                    .map(|raw| serde_json::from_str::<Value>(&raw))
                    .transpose()?,
                terminal_result: row
                    .try_get::<Option<String>, _>("terminal_result_json")?
                    .map(|raw| serde_json::from_str::<ParticipantResult>(&raw))
                    .transpose()?,
                prolific_participant_id: row.try_get("prolific_participant_id")?,
                prolific_study_id: row.try_get("prolific_study_id")?,
                prolific_session_id: row.try_get("prolific_session_id")?,
                prolific_status: row.try_get("prolific_status")?,
                prolific_entered_code: row.try_get("prolific_entered_code")?,
                prolific_return_requested_at: row.try_get("prolific_return_requested_at")?,
                prolific_reconciled_at: row.try_get("prolific_reconciled_at")?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
        Ok(participants)
    }
}

/// Ordered rows for one monitor window, with enough surrounding context for speech and teardown.
pub(crate) struct TimelinePage {
    pub events: Vec<StoredSessionEvent>,
    pub after_terminal: bool,
    pub older_before: Option<i64>,
    pub last_index: i64,
}

pub(super) type SessionEventSqlRow = (
    i64,
    String,
    i64,
    i64,
    String,
    Option<i64>,
    Option<String>,
    String,
    Option<String>,
    i64,
);

/// Decodes a persisted event and its optional game state.
pub(super) fn stored_event_from_sql_row(row: SessionEventSqlRow) -> Result<StoredSessionEvent> {
    Ok(StoredSessionEvent {
        event_id: row.0,
        experiment_id: row.1,
        session_id: row.2,
        event_index: row.3,
        event_type: row.4,
        actor_participant_id: row.5,
        actor_role: row.6,
        payload: serde_json::from_str::<Value>(&row.7)?,
        game_state: row
            .8
            .map(|raw| serde_json::from_str::<Value>(&raw))
            .transpose()?,
        game_time_ms: row.9,
    })
}
