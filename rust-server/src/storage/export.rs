//! Consistent experiment exports and narrow administrator lookups.
use super::*;

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
    /// Resolves a public identifier only when the participant belongs to this experiment.
    pub(crate) async fn participant_id_for_research_id(
        &self,
        experiment_id: &str,
        research_id: &str,
    ) -> Result<Option<i64>> {
        Ok(sqlx::query_scalar(
            "select p.participant_id
             from participants p
             where p.research_id = ? and exists (
                 select 1 from session_participants sp
                 where sp.experiment_id = ? and sp.participant_id = p.participant_id
             )",
        )
        .bind(research_id)
        .bind(experiment_id)
        .fetch_optional(&self.pool)
        .await?)
    }

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
        .map(|row| {
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
                    .with_context(|| {
                        format!("session {session_id} has invalid session_end_json")
                    })?,
            })
        })
        .collect()
}

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
