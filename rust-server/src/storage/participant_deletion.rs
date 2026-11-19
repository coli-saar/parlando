//! Preview and transactional deletion of participant-linked records.

use super::SqliteExperimentStore;
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use sqlx::SqliteConnection;

/// Counts participant-linked records before an administrator confirms deletion.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ParticipantDataPreview {
    /// Durable participant identity selected for deletion.
    pub participant_id: i64,
    /// Number of session appearances associated with the identity.
    pub session_count: i64,
    /// Number of stored consent declarations associated with the identity.
    pub consent_count: i64,
    /// Number of authored message or transcript events that will be removed.
    pub content_event_count: i64,
    /// Number of other authored events removed with the affected sessions.
    pub other_event_count: i64,
    /// Dashboard session identifiers that will be deleted with the participant.
    pub session_ids: Vec<String>,
    /// Other dashboard participant identifiers whose shared sessions will also disappear.
    pub other_participant_ids: Vec<String>,
    /// Whether at least one affected session can still receive runtime writes.
    pub has_non_terminal_session: bool,
}

/// Counts participant-linked rows in SQLite without modifying them.
async fn sqlite_participant_data_preview(
    connection: &mut SqliteConnection,
    experiment_id: &str,
    participant_id: i64,
) -> Result<ParticipantDataPreview> {
    let session_count = sqlx::query_scalar::<_, i64>(
        "select count(*) from session_participants where experiment_id = ? and participant_id = ?",
    )
    .bind(experiment_id)
    .bind(participant_id)
    .fetch_one(&mut *connection)
    .await?;
    let consent_count = sqlx::query_scalar::<_, i64>(
        "select count(*) from consent_declarations where experiment_id = ? and participant_id = ?",
    )
    .bind(experiment_id)
    .bind(participant_id)
    .fetch_one(&mut *connection)
    .await?;
    let content_event_count = sqlx::query_scalar::<_, i64>(
        "select count(*) from session_events where experiment_id = ? and actor_participant_id = ? and event_type in ('conversation_message', 'transcript_segment')",
    )
    .bind(experiment_id)
    .bind(participant_id)
    .fetch_one(&mut *connection)
    .await?;
    let other_event_count = sqlx::query_scalar::<_, i64>(
        "select count(*) from session_events where experiment_id = ? and actor_participant_id = ? and event_type not in ('conversation_message', 'transcript_segment')",
    )
    .bind(experiment_id)
    .bind(participant_id)
    .fetch_one(&mut *connection)
    .await?;
    let sessions = sqlx::query_as::<_, (String, String)>(
        r#"
        select s.dialogue_id, s.lifecycle from sessions s
        join session_participants sp
          on sp.experiment_id = s.experiment_id and sp.session_id = s.session_id
        where sp.experiment_id = ? and sp.participant_id = ?
        order by s.dialogue_id
        "#,
    )
    .bind(experiment_id)
    .bind(participant_id)
    .fetch_all(&mut *connection)
    .await?;
    let other_participant_ids = sqlx::query_scalar::<_, String>(
        r#"
        select distinct p.research_id from participants p
        join session_participants other on other.participant_id = p.participant_id
        where other.experiment_id = ? and other.participant_id != ?
          and other.session_id in (
              select session_id from session_participants
              where experiment_id = ? and participant_id = ?
          )
          and p.research_id is not null
        order by p.research_id
        "#,
    )
    .bind(experiment_id)
    .bind(participant_id)
    .bind(experiment_id)
    .bind(participant_id)
    .fetch_all(&mut *connection)
    .await?;
    Ok(ParticipantDataPreview {
        participant_id,
        session_count,
        consent_count,
        content_event_count,
        other_event_count,
        session_ids: sessions.iter().map(|(id, _)| id.clone()).collect(),
        other_participant_ids,
        has_non_terminal_session: sessions.iter().any(|(_, lifecycle)| lifecycle != "ended"),
    })
}

impl SqliteExperimentStore {
    /// Counts the records affected by deleting a participant.
    pub(crate) async fn participant_data_preview(
        &self,
        experiment_id: &str,
        participant_id: i64,
    ) -> Result<ParticipantDataPreview> {
        let mut tx = self.pool.begin().await?;
        let preview =
            sqlite_participant_data_preview(&mut tx, experiment_id, participant_id).await?;
        tx.commit().await?;
        Ok(preview)
    }

    /// Deletes the participant, their consents and every shared session they joined.
    ///
    /// Reserve the SQLite writer before collecting affected session IDs. The live-room
    /// check, returned preview and deletes then describe the same snapshot. Partner
    /// identities and unrelated sessions survive. An error rolls back every delete.
    pub(crate) async fn delete_participant_data(
        &self,
        experiment_id: &str,
        participant_id: i64,
    ) -> Result<ParticipantDataPreview> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let preview =
            sqlite_participant_data_preview(&mut tx, experiment_id, participant_id).await?;
        if preview.has_non_terminal_session {
            bail!("affected sessions are live or non-terminal");
        }
        sqlx::query(
            r#"
            delete from consent_declarations
            where experiment_id = ? and (
                participant_id = ? or session_id in (
                    select session_id from session_participants
                    where experiment_id = ? and participant_id = ?
                )
            )
            "#,
        )
        .bind(experiment_id)
        .bind(participant_id)
        .bind(experiment_id)
        .bind(participant_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            r#"
            delete from session_events
            where experiment_id = ? and session_id in (
                select session_id from session_participants
                where experiment_id = ? and participant_id = ?
            )
            "#,
        )
        .bind(experiment_id)
        .bind(experiment_id)
        .bind(participant_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            r#"
            delete from session_participants
            where experiment_id = ? and session_id in (
                select affected.session_id from (
                    select session_id from session_participants
                    where experiment_id = ? and participant_id = ?
                ) affected
            )
            "#,
        )
        .bind(experiment_id)
        .bind(experiment_id)
        .bind(participant_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            r#"
            delete from sessions
            where experiment_id = ? and dialogue_id in (
                select value from json_each(?)
            )
            "#,
        )
        .bind(experiment_id)
        .bind(serde_json::to_string(&preview.session_ids)?)
        .execute(&mut *tx)
        .await?;
        sqlx::query("delete from participants where experiment_id = ? and participant_id = ?")
            .bind(experiment_id)
            .bind(participant_id)
            .execute(&mut *tx)
            .await?;
        let remaining = sqlx::query_scalar::<_, i64>(
            "select count(*) from participants where experiment_id = ? and participant_id = ?",
        )
        .bind(experiment_id)
        .bind(participant_id)
        .fetch_one(&mut *tx)
        .await?;
        if remaining != 0 {
            bail!("participant deletion verification failed");
        }
        tx.commit().await?;
        Ok(preview)
    }
}
