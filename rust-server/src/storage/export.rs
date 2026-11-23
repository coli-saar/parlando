//! Consistent experiment exports and session detail projections.
//!
//! The admin detail handler requests a single session row. Corpus export requests the
//! experiment and its linked rows from one SQLite read snapshot. Game-owned JSON travels as
//! structured values. An invalid stored value fails the read.

#[cfg(test)]
use super::session_queries::SessionEventSqlRow;
use super::{
    session_queries::stored_event_from_sql_row, SqliteExperimentStore, StoredSessionEvent,
};
use crate::protocol::SessionEnd;
use anyhow::{Context, Result};
use futures_util::{stream::BoxStream, TryStreamExt};
use serde::{Deserialize, Serialize};
#[cfg(test)]
use serde_json::json;
use serde_json::Value;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
#[cfg(test)]
use sqlx::SqlitePool;
use sqlx::{Row, Sqlite, Transaction};
use std::time::Duration;

/// The session fields used by both the dashboard detail and the corpus projector.
#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct ExportSession {
    pub experiment_id: String,
    pub session_id: i64,
    pub public_session_id: String,
    pub dialogue_id: String,
    pub mode: String,
    pub lifecycle: String,
    pub purpose: String,
    pub config_revision: i64,
    pub game_version: String,
    pub created_at: String,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub completion: Option<Value>,
    pub session_end: Option<SessionEnd>,
}

#[cfg(test)]
/// Related storage records from one read transaction. Game payloads remain opaque.
#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct ExperimentSnapshot {
    pub experiment: Option<Value>,
    pub participants: Vec<Value>,
    pub sessions: Vec<ExportSession>,
    pub session_participants: Vec<Value>,
    pub consent_declarations: Vec<Value>,
    pub session_events: Vec<StoredSessionEvent>,
}

impl SqliteExperimentStore {
    /// Fetches one durable session without loading its event history or related tables.
    pub(crate) async fn session_detail(
        &self,
        experiment_id: &str,
        session_id: i64,
    ) -> Result<Option<ExportSession>> {
        let mut connection = self.pool.acquire().await?;
        Ok(
            read_sessions(&mut connection, experiment_id, Some(session_id))
                .await?
                .pop(),
        )
    }

    #[cfg(test)]
    /// Retrieves the experiment and its linked research records at a single instant.
    pub(crate) async fn export_snapshot(&self, experiment_id: &str) -> Result<ExperimentSnapshot> {
        read_export_snapshot(&self.pool, experiment_id).await
    }
}

/// Maps the same canonical session columns for a single dashboard row or a full experiment.
async fn read_sessions(
    connection: &mut sqlx::SqliteConnection,
    experiment_id: &str,
    session_id: Option<i64>,
) -> Result<Vec<ExportSession>> {
    let mut sql = sqlx::QueryBuilder::new(
        "select experiment_id, session_id, public_session_id, dialogue_id, mode, lifecycle, purpose,
                config_revision, game_version, created_at, started_at, ended_at, completion_json, session_end_json
         from sessions where experiment_id = ",
    );
    sql.push_bind(experiment_id);
    if let Some(session_id) = session_id {
        sql.push(" and session_id = ").push_bind(session_id);
    }
    sql.push(" order by session_id")
        .build()
        .fetch_all(connection)
        .await?
        .into_iter()
        .map(export_session_from_row)
        .collect()
}

/// Decodes the shared session projection and validates its structured columns.
fn export_session_from_row(row: sqlx::sqlite::SqliteRow) -> Result<ExportSession> {
    let session_id = row.try_get("session_id")?;
    let end: Option<String> = row.try_get("session_end_json")?;
    Ok(ExportSession {
        experiment_id: row.try_get("experiment_id")?,
        session_id,
        public_session_id: row.try_get("public_session_id")?,
        dialogue_id: row.try_get("dialogue_id")?,
        mode: row.try_get("mode")?,
        lifecycle: row.try_get("lifecycle")?,
        purpose: row.try_get("purpose")?,
        config_revision: row.try_get("config_revision")?,
        game_version: row.try_get("game_version")?,
        created_at: row.try_get("created_at")?,
        started_at: row.try_get("started_at")?,
        ended_at: row.try_get("ended_at")?,
        completion: parse_optional_json(
            row.try_get("completion_json")?,
            &format!("session {session_id} completion_json"),
        )?,
        session_end: end
            .map(|raw| serde_json::from_str(&raw))
            .transpose()
            .with_context(|| format!("session {session_id} has invalid session_end_json"))?,
    })
}

#[cfg(test)]
/// Reads the linked rows at a single SQLite snapshot, in durable session and event order.
async fn read_export_snapshot(
    pool: &SqlitePool,
    experiment_id: &str,
) -> Result<ExperimentSnapshot> {
    let mut tx = pool.begin().await?;
    let experiment = {
        let row = sqlx::query_as::<
            _,
            (
                String,
                String,
                i64,
                String,
                String,
                Option<String>,
                Option<String>,
                String,
                Option<String>,
                bool,
            ),
        >(
            "select experiment_id, game_version, config_revision, created_at, config_json, server_version, version_manifest_json, status, notes, pinned from experiments where experiment_id = ?",
        )
        .bind(experiment_id)
        .fetch_optional(&mut *tx)
        .await?;
        row.map(
            |(
                id,
                game_version,
                config_revision,
                created_at,
                config_json,
                server_version,
                version_manifest_json,
                status,
                notes,
                pinned,
            )|
             -> Result<Value> {
                let config = serde_json::from_str::<Value>(&config_json)
                    .with_context(|| format!("experiment {id:?} has invalid config_json"))?;
                let version_manifest = parse_optional_json(
                    version_manifest_json,
                    &format!("experiment {id:?} version_manifest_json"),
                )?;
                Ok(json!({
                "experiment_id": id,
                "game_version": game_version,
                "config_revision": config_revision,
                "created_at": created_at,
                "config": config,
                "server_version": server_version,
                "version_manifest": version_manifest,
                "status": status,
                "notes": notes,
                "pinned": pinned,
                }))
            },
        )
        .transpose()?
    };
    let sessions = read_sessions(&mut tx, experiment_id, None).await?;
    let event_sql = "select event_id, experiment_id, session_id, event_index, event_type, actor_participant_id, actor_role, payload_json, game_state_json, game_time_ms from session_events where experiment_id = ? order by session_id, event_index";
    let event_query = sqlx::query_as::<_, SessionEventSqlRow>(event_sql).bind(experiment_id);
    let events = event_query
        .fetch_all(&mut *tx)
        .await?
        .into_iter()
        .map(stored_event_from_sql_row)
        .collect::<Result<Vec<_>>>()?;
    let participants_sql = r#"
        select distinct p.participant_id, p.research_id, p.participant_kind, p.identity_provider, p.external_id,
               p.metadata_json, p.created_at
        from participants p
        join session_participants sp on sp.participant_id = p.participant_id
        where sp.experiment_id = ?
        order by p.participant_id
        "#;
    let participants_query = sqlx::query_as::<
        _,
        (
            i64,
            Option<String>,
            String,
            String,
            Option<String>,
            Option<String>,
            String,
        ),
    >(participants_sql)
    .bind(experiment_id);
    let participants = participants_query
        .fetch_all(&mut *tx)
        .await?
        .into_iter()
        .map(|row| -> Result<Value> {
            let metadata =
                parse_optional_json(row.5, &format!("participant {} metadata_json", row.0))?;
            Ok(json!({
                "participant_id": row.0,
                "research_id": row.1,
                "participant_kind": row.2,
                "identity_provider": row.3,
                "external_id": row.4,
                "metadata": metadata,
                "created_at": row.6,
            }))
        })
        .collect::<Result<Vec<_>>>()?;
    let session_participants_sql = "select experiment_id, session_id, participant_id, participant_session_id, role, joined_at, left_at, connection_status from session_participants where experiment_id = ? order by session_id, role";
    let session_participants_query = sqlx::query_as::<
        _,
        (
            String,
            i64,
            i64,
            String,
            String,
            String,
            Option<String>,
            String,
        ),
    >(session_participants_sql)
    .bind(experiment_id);
    let session_participants = session_participants_query
        .fetch_all(&mut *tx)
        .await?
        .into_iter()
        .map(|row| {
            json!({
                "experiment_id": row.0,
                "session_id": row.1,
                "participant_id": row.2,
                "participant_session_id": row.3,
                "role": row.4,
                "joined_at": row.5,
                "left_at": row.6,
                "connection_status": row.7,
            })
        })
        .collect::<Vec<_>>();
    let consent_sql = "select consent_id, experiment_id, session_id, participant_id, consent_item_id, accepted, purpose, declared_at, consent_text_hash, metadata_json from consent_declarations where experiment_id = ? order by declared_at, consent_id";
    let consent_query = sqlx::query_as::<
        _,
        (
            i64,
            String,
            Option<i64>,
            i64,
            String,
            i64,
            String,
            String,
            Option<String>,
            Option<String>,
        ),
    >(consent_sql)
    .bind(experiment_id);
    let consent_declarations = consent_query
        .fetch_all(&mut *tx)
        .await?
        .into_iter()
        .map(|row| -> Result<Value> {
            let metadata = parse_optional_json(
                row.9,
                &format!("consent declaration {} metadata_json", row.0),
            )?;
            Ok(json!({
                "consent_id": row.0,
                "experiment_id": row.1,
                "session_id": row.2,
                "participant_id": row.3,
                "consent_item_id": row.4,
                "accepted": row.5 != 0,
                "purpose": row.6,
                "declared_at": row.7,
                "consent_text_hash": row.8,
                "metadata": metadata,
            }))
        })
        .collect::<Result<Vec<_>>>()?;
    tx.commit().await?;
    Ok(ExperimentSnapshot {
        experiment,
        participants,
        sessions,
        session_participants,
        consent_declarations,
        session_events: events,
    })
}

/// Decodes an optional stored JSON column without conflating corruption with SQL NULL.
fn parse_optional_json(raw: Option<String>, label: &str) -> Result<Option<Value>> {
    raw.map(|value| {
        serde_json::from_str(&value).with_context(|| format!("{label} contains invalid JSON"))
    })
    .transpose()
}

/// Owns an export's private SQLite connection and the read transaction established on it.
/// Its streams retain at most SQLite's bounded row queue and one projected record at a time.
pub(crate) struct CorpusRead {
    transaction: Transaction<'static, Sqlite>,
    experiment_id: String,
}

/// Counts records using the same eligibility rules as the export streams.
#[derive(Default)]
pub(crate) struct CorpusInventory {
    pub participants: i64,
    pub sessions: i64,
    pub actions: i64,
    pub messages: i64,
    pub logs: i64,
}

const SESSION_COLUMNS: &str = "experiment_id, session_id, public_session_id, dialogue_id, mode,
    lifecycle, purpose, config_revision, game_version, created_at, started_at, ended_at,
    completion_json, session_end_json";

/// Reusable SQL relations for corpus eligibility and the pseudonymous participant registry.
const CORPUS_SESSIONS: &str = "select session_id from sessions
    where experiment_id = ?1 and purpose <> 'testing' and started_at is not null";
const CORPUS_PARTICIPANTS: &str = "select distinct sp.participant_id from session_participants sp
    join eligible on eligible.session_id = sp.session_id where sp.experiment_id = ?1";
const SEMANTIC_EVENTS: &str = "(event_type in ('game_action_accepted', 'conversation_message')
    or (event_type = 'log' and game_time_ms >= 0))";

impl SqliteExperimentStore {
    /// Opens a dedicated read-only WAL connection for one download, without queuing behind writers.
    pub(crate) async fn corpus_read(&self, experiment_id: &str) -> Result<CorpusRead> {
        let mut transaction = if let Some(path) = self.database_path.as_ref() {
            let options = path
                .to_str()
                .expect("SQLite path comes from a database URL")
                .parse::<SqliteConnectOptions>()?
                .read_only(true)
                .busy_timeout(Duration::from_secs(5))
                .row_buffer_size(8)
                .pragma("temp_store", "FILE");
            SqlitePoolOptions::new()
                .max_connections(1)
                .connect_with(options)
                .await?
                .begin()
                .await?
        } else {
            // A literal in-memory store can only be read on its original connection.
            self.pool.begin().await?
        };
        // SQLite's main database stays read-only. The temporary index only contains one
        // session's alignments and supports merging them without an in-memory map.
        sqlx::query("create temp table export_timings(message_id text primary key, metadata text) without rowid")
            .execute(&mut *transaction).await?;
        Ok(CorpusRead {
            transaction,
            experiment_id: experiment_id.to_owned(),
        })
    }
}

impl CorpusRead {
    /// Establishes the WAL snapshot on the first main-database read.
    pub(crate) async fn experiment(&mut self) -> Result<Option<(String, String, Value)>> {
        let row: Option<(String, String, String)> = sqlx::query_as(
            "select status, game_version, config_json from experiments where experiment_id = ?",
        )
        .bind(&self.experiment_id)
        .fetch_optional(&mut *self.transaction)
        .await?;
        row.map(|(status, version, config)| Ok((status, version, serde_json::from_str(&config)?)))
            .transpose()
    }

    /// Counts only eligible sessions, participants and semantic events, inside the export snapshot.
    pub(crate) async fn inventory(&mut self) -> Result<CorpusInventory> {
        let sql = format!(
            "with eligible as ({CORPUS_SESSIONS}), participants as ({CORPUS_PARTICIPANTS})
            select (select count(*) from participants), (select count(*) from eligible),
                   count(case when event_type = 'game_action_accepted' then 1 end),
                   count(case when event_type = 'conversation_message' then 1 end),
                   count(case when event_type = 'log' then 1 end)
            from session_events join eligible using (session_id)
            where experiment_id = ?1 and {SEMANTIC_EVENTS}"
        );
        let (participants, sessions, actions, messages, logs) = sqlx::query_as(&sql)
            .bind(&self.experiment_id)
            .fetch_one(&mut *self.transaction)
            .await?;
        Ok(CorpusInventory {
            participants,
            sessions,
            actions,
            messages,
            logs,
        })
    }

    /// Streams pseudonymous registry rows in durable participant order.
    pub(crate) fn participants(
        &mut self,
    ) -> BoxStream<'_, Result<(Option<String>, String, Option<Value>)>> {
        let rows = sqlx::query_as::<_, (Option<String>, String, Option<String>, i64)>(
            "select p.research_id, p.participant_kind, p.metadata_json, p.participant_id
            from participants p where p.participant_id in (
                select sp.participant_id from session_participants sp join sessions s
                  on s.experiment_id = sp.experiment_id and s.session_id = sp.session_id
                where sp.experiment_id = ?1
                  and s.purpose <> 'testing' and s.started_at is not null
            ) order by p.participant_id",
        )
        .bind(&self.experiment_id)
        .fetch(&mut *self.transaction)
        .map_err(anyhow::Error::from)
        .and_then(|(id, kind, metadata, internal_id)| async move {
            Ok((
                id,
                kind,
                parse_optional_json(
                    metadata,
                    &format!("participant {internal_id} metadata_json"),
                )?,
            ))
        });
        Box::pin(rows)
    }

    /// Advances the session keyset without holding a session cursor across nested queries.
    pub(crate) async fn next_session(
        &mut self,
        after: Option<i64>,
    ) -> Result<Option<ExportSession>> {
        let mut sql = sqlx::QueryBuilder::new(format!(
            "select {SESSION_COLUMNS} from sessions where purpose <> 'testing'
            and started_at is not null and experiment_id = "
        ));
        sql.push_bind(&self.experiment_id);
        if let Some(after) = after {
            sql.push(" and session_id > ").push_bind(after);
        }
        sql.push(" order by session_id limit 1")
            .build()
            .fetch_optional(&mut *self.transaction)
            .await?
            .map(export_session_from_row)
            .transpose()
    }

    /// Reads up to three rows, enough to detect an invalid assignment count.
    pub(crate) async fn assignments(
        &mut self,
        session_id: i64,
    ) -> Result<Vec<(Option<String>, String)>> {
        Ok(sqlx::query_as(
            "select p.research_id, sp.role from session_participants sp
            join participants p using (participant_id)
            where sp.experiment_id = ? and sp.session_id = ? order by sp.role limit 3",
        )
        .bind(&self.experiment_id)
        .bind(session_id)
        .fetch_all(&mut *self.transaction)
        .await?)
    }

    /// Replaces the disk-backed timing index with the last alignment for each message ID.
    pub(crate) async fn index_timings(&mut self, session_id: i64) -> Result<()> {
        sqlx::query("delete from temp.export_timings")
            .execute(&mut *self.transaction)
            .await?;
        sqlx::query(
            "insert or replace into temp.export_timings
            select payload_json ->> 'message_id', payload_json -> 'metadata'
            from session_events where experiment_id = ? and session_id = ?
            and event_type = 'conversation_speech_timing'
            and json_type(payload_json, '$.message_id') = 'text'
            and json_type(payload_json, '$.metadata') is not null
            order by event_index",
        )
        .bind(&self.experiment_id)
        .bind(session_id)
        .execute(&mut *self.transaction)
        .await?;
        Ok(())
    }
    /// Streams semantic events with their eligible actor and the most recent speech timing.
    pub(crate) fn events(
        &mut self,
        session_id: i64,
    ) -> BoxStream<'_, Result<(StoredSessionEvent, Option<String>, Option<Value>)>> {
        let rows = sqlx::query(
            "select e.*, p.research_id, t.metadata as timing_metadata
            from session_events e
            left join participants p on p.participant_id = e.actor_participant_id and (
                exists (
                    select 1 from session_participants sp
                    where sp.experiment_id = e.experiment_id and sp.session_id = e.session_id
                      and sp.participant_id = p.participant_id
                ) or exists (
                    select 1 from session_participants sp join sessions s
                      on s.experiment_id = sp.experiment_id and s.session_id = sp.session_id
                    where sp.experiment_id = e.experiment_id and sp.participant_id = p.participant_id
                      and s.purpose <> 'testing' and s.started_at is not null
                )
            )
            left join temp.export_timings t on t.message_id = case
                when e.event_type = 'conversation_message' and json_type(e.payload_json, '$.id') = 'text'
                then e.payload_json ->> 'id' end
            where e.experiment_id = ? and e.session_id = ?
              and (event_type in ('game_action_accepted', 'conversation_message')
                   or (event_type = 'log' and game_time_ms >= 0))
            order by event_index"
        )
        .bind(&self.experiment_id).bind(session_id).fetch(&mut *self.transaction)
        .map_err(anyhow::Error::from)
        .and_then(|row| async move {
            let event = stored_event_from_sql_row((
                row.try_get("event_id")?, row.try_get("experiment_id")?, row.try_get("session_id")?,
                row.try_get("event_index")?, row.try_get("event_type")?, row.try_get("actor_participant_id")?,
                row.try_get("actor_role")?, row.try_get("payload_json")?, row.try_get("game_state_json")?,
                row.try_get("game_time_ms")?,
            ))?;
            let timing = parse_optional_json(row.try_get("timing_metadata")?, "speech timing metadata")?;
            Ok((event, row.try_get("research_id")?, timing))
        });
        Box::pin(rows)
    }

    /// Frees the read snapshot and its temporary table before a client receives the artifact.
    pub(crate) async fn finish(self) -> Result<()> {
        self.transaction.rollback().await?;
        Ok(())
    }
}
