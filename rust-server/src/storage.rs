use std::{collections::HashMap, path::Path, sync::Arc, time::Duration};

use crate::{
    game::Seat,
    identity::new_id,
    readable_id::{dialogue_id, participant_id as readable_participant_id},
};
use anyhow::{bail, Result};
use async_trait::async_trait;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
    SqlitePool,
};

/// Returns the current UTC timestamp in ISO-8601/RFC3339 form.
pub fn now_iso() -> String {
    Utc::now().to_rfc3339()
}

/// Generates a startup experiment id when neither CLI nor YAML provided one.
pub fn generated_experiment_id() -> String {
    new_id("exp")
}

/// Builds the base identifier for a non-human participant from durable identity metadata.
fn nonhuman_participant_identifier(participant: &ParticipantRecord) -> String {
    let metadata = participant.metadata.as_object();
    let external_parts = participant
        .external_id
        .as_deref()
        .and_then(|value| value.rsplit_once('@'));
    let agent_type = metadata
        .and_then(|value| value.get("agent_type").or_else(|| value.get("agent_name")))
        .and_then(Value::as_str)
        .or_else(|| external_parts.map(|(name, _)| name))
        .unwrap_or(&participant.identity_provider);
    let agent_name = metadata
        .and_then(|value| value.get("agent_name"))
        .and_then(Value::as_str)
        .filter(|name| *name != agent_type);
    let version = metadata
        .and_then(|value| value.get("agent_version"))
        .and_then(Value::as_str)
        .or_else(|| external_parts.map(|(_, version)| version));

    let mut identity_parts = vec![identifier_component(&participant.participant_kind)];
    identity_parts.push(identifier_component(agent_type));
    if let Some(agent_name) = agent_name {
        identity_parts.push(identifier_component(agent_name));
    }
    let version = version.map(identifier_component).unwrap_or_else(|| {
        if participant.participant_kind == "agent" {
            "unversioned".to_string()
        } else {
            participant
                .external_id
                .as_deref()
                .filter(|external_id| *external_id != agent_type)
                .map(identifier_component)
                .unwrap_or_else(|| "unversioned".to_string())
        }
    });
    format!("{}@{version}", identity_parts.join(":"))
}

/// Restricts one externally supplied identifier component to a compact display-safe alphabet.
fn identifier_component(value: &str) -> String {
    let mut component = String::new();
    let mut previous_was_separator = false;
    for character in value.trim().chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-') {
            component.push(character);
            previous_was_separator = false;
        } else if !previous_was_separator && !component.is_empty() {
            component.push('-');
            previous_was_separator = true;
        }
    }
    while component.ends_with('-') {
        component.pop();
    }
    if component.is_empty() {
        "unknown".to_string()
    } else {
        component
    }
}

/// Produces a unique candidate, using random names only for human participants.
fn participant_identifier_candidate(participant: &ParticipantRecord, attempt: usize) -> String {
    if participant.participant_kind == "human" {
        readable_participant_id()
    } else {
        let base = nonhuman_participant_identifier(participant);
        if attempt == 1 {
            base
        } else {
            format!("{base}~{attempt}")
        }
    }
}

/// Input for creating or updating the durable experiment row.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ExperimentRecord {
    pub experiment_id: String,
    pub config: Value,
    pub server_version: Option<String>,
    pub version_manifest: Option<Value>,
    pub status: String,
    pub notes: Option<String>,
}

/// Durable experiment metadata returned by the experimenter dashboard.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StoredExperimentSummary {
    pub experiment_id: String,
    pub study_name: Option<String>,
    pub created_at: String,
    pub status: String,
    pub server_version: Option<String>,
    pub version_manifest: Option<Value>,
    pub notes: Option<String>,
    pub session_count: i64,
    pub completed_session_count: i64,
    pub last_session_at: Option<String>,
}

/// Input for upserting a durable participant identity.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ParticipantRecord {
    /// Experiment within which exports reuse this participant identifier.
    pub experiment_id: String,
    pub participant_kind: String,
    pub identity_provider: String,
    pub external_id: Option<String>,
    pub metadata: Value,
}

/// Input for creating one game session.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SessionRecord {
    pub experiment_id: String,
    pub room_id: String,
    pub mode: String,
    pub status: String,
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

/// Input for one item-level consent declaration.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ConsentDeclarationRecord {
    pub experiment_id: String,
    pub session_id: Option<i64>,
    pub participant_id: i64,
    pub consent_item_id: String,
    pub accepted: bool,
    pub consent_text_hash: Option<String>,
    pub metadata: Value,
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
    pub created_at: String,
}

/// Durable session summary returned by recent-game database queries.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StoredSessionSummary {
    pub experiment_id: String,
    pub session_id: i64,
    pub room_id: String,
    /// Human-readable random identifier reused for this dialogue in exports and administration.
    pub dialogue_id: String,
    pub mode: String,
    pub status: String,
    pub created_at: String,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub completion: Option<Value>,
    pub participant_count: i64,
    pub event_count: i64,
    pub last_event_at: Option<String>,
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
    pub participant_kind: Option<String>,
    pub metadata: Option<Value>,
}

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
    /// Number of other authored events whose actor reference will be anonymized.
    pub other_event_count: i64,
}

/// Durable administrator credential material loaded by the authentication layer.
///
/// `password_hash` is an Argon2id PHC string. The cleartext password is never
/// accepted by or written to the storage layer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredAdminCredential {
    /// Login name selected during setup.
    pub username: String,
    /// Argon2id PHC password hash; never the cleartext password.
    pub password_hash: String,
    /// Stable authorization role loaded after successful authentication.
    pub role: String,
}

/// Backend-neutral storage interface centered on experiment evaluation.
#[async_trait]
pub trait ExperimentStore: Send + Sync {
    /// Loads the singleton administrator credential, if initial setup is complete.
    async fn admin_credential(&self) -> Result<Option<StoredAdminCredential>>;
    /// Atomically creates the singleton administrator credential.
    ///
    /// Returns `false` when another request or process completed setup first.
    async fn create_admin_credential(&self, credential: StoredAdminCredential) -> Result<bool>;
    /// Ensures the process-owned experiment exists, resets it inactive, and returns that status.
    async fn ensure_experiment(&self, experiment: ExperimentRecord) -> Result<String>;
    /// Returns the process-owned experiment with compact session aggregates.
    async fn experiment_summary(
        &self,
        experiment_id: &str,
    ) -> Result<Option<StoredExperimentSummary>>;
    /// Updates the lifecycle status for one experiment row.
    async fn update_experiment_status(&self, experiment_id: &str, status: &str) -> Result<()>;
    /// Creates or reuses a durable participant identity and returns `participant_id`.
    async fn upsert_participant(&self, participant: ParticipantRecord) -> Result<i64>;
    /// Returns the human-readable experiment-specific identifier for a durable participant.
    async fn participant_research_id(&self, participant_id: i64) -> Result<Option<String>>;
    /// Creates a session for a client-facing room id and returns its per-experiment `session_id`.
    async fn create_session(&self, session: SessionRecord) -> Result<i64>;
    /// Adds a participant to a session with a session-local role.
    async fn add_session_participant(&self, participant: SessionParticipantRecord) -> Result<()>;
    /// Updates connection status for a participant's session appearance.
    async fn update_session_participant_connection(
        &self,
        participant_session_id: &str,
        connection_status: &str,
        left_at: Option<String>,
    ) -> Result<()>;
    /// Records one item-level consent declaration.
    async fn record_consent_declaration(&self, declaration: ConsentDeclarationRecord)
        -> Result<()>;
    /// Appends one ordered session event and returns its event index.
    async fn append_session_event(&self, event: SessionEventRecord) -> Result<i64>;
    /// Atomically appends one game transition's events and optional completion update.
    async fn commit_session_transition(
        &self,
        events: Vec<SessionEventRecord>,
        completion: Option<Value>,
    ) -> Result<()>;
    /// Returns ordered events for one session, optionally filtered by event type.
    async fn session_events(
        &self,
        experiment_id: &str,
        session_id: i64,
        event_type: Option<&str>,
    ) -> Result<Vec<StoredSessionEvent>>;
    /// Returns recent sessions with compact aggregate metadata for inspection UIs.
    async fn recent_sessions(
        &self,
        experiment_id: &str,
        limit: i64,
    ) -> Result<Vec<StoredSessionSummary>>;
    /// Returns session-local participant metadata joined to durable participant records.
    async fn session_participants(
        &self,
        experiment_id: &str,
        session_id: i64,
    ) -> Result<Vec<StoredSessionParticipant>>;
    /// Exports all durable evaluation data for one experiment.
    async fn export_experiment(&self, experiment_id: &str) -> Result<Value>;
    /// Exports all durable evaluation data for one session.
    async fn export_session(&self, experiment_id: &str, session_id: i64) -> Result<Value>;
    /// Counts records affected by manual participant-data deletion.
    async fn participant_data_preview(
        &self,
        experiment_id: &str,
        participant_id: i64,
    ) -> Result<ParticipantDataPreview>;
    /// Removes participant-authored content and anonymizes remaining references.
    async fn delete_participant_data(
        &self,
        experiment_id: &str,
        participant_id: i64,
    ) -> Result<ParticipantDataPreview>;
}

/// Shared trait-object handle for the configured experiment store backend.
pub type SharedExperimentStore = Arc<dyn ExperimentStore>;

/// Returns whether SQLite rejected a write because a unique value already exists.
fn sqlite_unique_constraint(error: &sqlx::Error) -> bool {
    error
        .as_database_error()
        .is_some_and(|database_error| database_error.is_unique_violation())
}

/// Returns whether SQLite names the expected unique column in its constraint error.
fn sqlite_unique_constraint_for(error: &sqlx::Error, column: &str) -> bool {
    error.as_database_error().is_some_and(|database_error| {
        database_error.is_unique_violation() && database_error.message().contains(column)
    })
}

/// SQLite implementation of the evaluation-oriented experiment store.
pub struct SqliteExperimentStore {
    pool: SqlitePool,
}

impl SqliteExperimentStore {
    /// Opens a SQLite-backed experiment store and creates the schema when needed.
    pub async fn connect(database_url: &str) -> Result<Self> {
        if database_url.is_empty() || database_url == "sqlite:///:memory:" {
            let pool = SqlitePoolOptions::new()
                .max_connections(1)
                .connect("sqlite::memory:")
                .await?;
            let store = Self { pool };
            store.ensure_schema().await?;
            return Ok(store);
        }
        if !database_url.starts_with("sqlite:///") {
            bail!("unsupported database url scheme for {database_url:?}; only sqlite:/// is implemented");
        }
        let path = database_url.trim_start_matches("sqlite:///");
        if let Some(parent) = Path::new(path).parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let options = path
            .parse::<SqliteConnectOptions>()?
            .create_if_missing(true)
            .busy_timeout(Duration::from_secs(5))
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;
        let store = Self { pool };
        store.ensure_schema().await?;
        Ok(store)
    }

    /// Creates the relational evaluation schema.
    async fn ensure_schema(&self) -> Result<()> {
        if self.schema_version().await? >= 1 {
            return Ok(());
        }
        for statement in [
            r#"
            create table if not exists administrator_credential (
                singleton integer primary key check (singleton = 1),
                username text not null,
                password_hash text not null,
                role text not null,
                created_at text not null
            )
            "#,
            r#"
            create table if not exists schema_migrations (
                version integer primary key,
                applied_at text not null
            )
            "#,
            r#"
            create table if not exists experiments (
                experiment_id text primary key,
                created_at text not null,
                config_json text not null,
                server_version text,
                version_manifest_json text,
                status text not null default 'inactive',
                notes text
            )
            "#,
            r#"
            create table if not exists participants (
                participant_id integer primary key autoincrement,
                research_id text unique,
                experiment_id text not null,
                participant_kind text not null,
                identity_provider text not null,
                external_id text,
                metadata_json text,
                created_at text not null
            )
            "#,
            r#"
            create table if not exists sessions (
                experiment_id text not null,
                session_id integer not null,
                room_id text not null unique,
                dialogue_id text unique,
                mode text not null,
                status text not null,
                created_at text not null,
                started_at text,
                completed_at text,
                completion_json text,
                primary key (experiment_id, session_id),
                foreign key (experiment_id) references experiments(experiment_id)
            )
            "#,
            r#"
            create table if not exists session_participants (
                experiment_id text not null,
                session_id integer not null,
                participant_id integer not null,
                participant_session_id text not null unique,
                role text not null,
                joined_at text not null,
                left_at text,
                connection_status text not null,
                primary key (experiment_id, session_id, participant_id),
                foreign key (experiment_id, session_id) references sessions(experiment_id, session_id),
                foreign key (participant_id) references participants(participant_id)
            )
            "#,
            r#"
            create table if not exists consent_declarations (
                consent_id integer primary key autoincrement,
                experiment_id text not null,
                session_id integer,
                participant_id integer not null,
                consent_item_id text not null,
                accepted integer not null,
                declared_at text not null,
                consent_text_hash text,
                metadata_json text,
                foreign key (participant_id) references participants(participant_id)
            )
            "#,
            r#"
            create table if not exists session_events (
                event_id integer primary key autoincrement,
                experiment_id text not null,
                session_id integer not null,
                event_index integer not null,
                event_type text not null,
                actor_participant_id integer,
                actor_role text,
                payload_json text not null,
                game_state_json text,
                created_at text not null,
                unique (experiment_id, session_id, event_index),
                foreign key (experiment_id, session_id) references sessions(experiment_id, session_id),
                foreign key (actor_participant_id) references participants(participant_id)
            )
            "#,
            "create index if not exists idx_session_events_session on session_events(experiment_id, session_id)",
            "create index if not exists idx_session_events_session_type on session_events(experiment_id, session_id, event_type)",
            "create index if not exists idx_session_events_actor_created on session_events(actor_participant_id, created_at)",
        ] {
            sqlx::query(statement).execute(&self.pool).await?;
        }
        self.apply_pending_migrations().await?;
        Ok(())
    }

    /// Reads the applied schema version without mutating an already-current database.
    async fn schema_version(&self) -> Result<i64> {
        let migrations_table_exists = sqlx::query_scalar::<_, i64>(
            "select count(*) from sqlite_master where type = 'table' and name = 'schema_migrations'",
        )
        .fetch_one(&self.pool)
        .await?
            > 0;
        if !migrations_table_exists {
            return Ok(0);
        }
        Ok(
            sqlx::query_scalar::<_, Option<i64>>("select max(version) from schema_migrations")
                .fetch_one(&self.pool)
                .await?
                .unwrap_or(0),
        )
    }

    /// Applies each historical compatibility migration once per database.
    async fn apply_pending_migrations(&self) -> Result<()> {
        let version =
            sqlx::query_scalar::<_, Option<i64>>("select max(version) from schema_migrations")
                .fetch_one(&self.pool)
                .await?
                .unwrap_or(0);
        if version >= 1 {
            return Ok(());
        }
        self.ensure_column("experiments", "version_manifest_json", "text")
            .await?;
        self.ensure_column("experiments", "status", "text not null default 'inactive'")
            .await?;
        self.ensure_column("participants", "research_id", "text")
            .await?;
        self.ensure_column("participants", "experiment_id", "text")
            .await?;
        self.ensure_column("sessions", "dialogue_id", "text")
            .await?;
        sqlx::query("drop index if exists idx_participants_provider_external")
            .execute(&self.pool)
            .await?;
        self.scope_legacy_participants().await?;
        sqlx::query(
            "create unique index if not exists idx_participants_experiment_provider_external on participants(experiment_id, identity_provider, external_id) where external_id is not null",
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "create unique index if not exists idx_participants_research_id on participants(research_id) where research_id is not null",
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "create unique index if not exists idx_sessions_dialogue_id on sessions(dialogue_id) where dialogue_id is not null",
        )
        .execute(&self.pool)
        .await?;
        self.backfill_readable_ids().await?;
        self.drop_column_if_exists("participants", "display_name")
            .await?;
        sqlx::query("insert into schema_migrations (version, applied_at) values (1, ?)")
            .bind(now_iso())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Splits legacy participant rows that were shared by more than one experiment.
    async fn scope_legacy_participants(&self) -> Result<()> {
        let participant_ids = sqlx::query_scalar::<_, i64>(
            "select participant_id from participants where experiment_id is null",
        )
        .fetch_all(&self.pool)
        .await?;
        for participant_id in participant_ids {
            let experiment_ids = sqlx::query_scalar::<_, String>(
                r#"
                select experiment_id from session_participants where participant_id = ?
                union select experiment_id from consent_declarations where participant_id = ?
                union select experiment_id from session_events where actor_participant_id = ?
                order by experiment_id
                "#,
            )
            .bind(participant_id)
            .bind(participant_id)
            .bind(participant_id)
            .fetch_all(&self.pool)
            .await?;
            let Some((first_experiment, remaining_experiments)) = experiment_ids.split_first()
            else {
                sqlx::query(
                    "update participants set experiment_id = 'legacy_unassigned' where participant_id = ?",
                )
                .bind(participant_id)
                .execute(&self.pool)
                .await?;
                continue;
            };
            sqlx::query("update participants set experiment_id = ? where participant_id = ?")
                .bind(first_experiment)
                .bind(participant_id)
                .execute(&self.pool)
                .await?;
            for experiment_id in remaining_experiments {
                let source = sqlx::query_as::<
                    _,
                    (String, String, Option<String>, Option<String>, String),
                >(
                    "select participant_kind, identity_provider, external_id, metadata_json, created_at from participants where participant_id = ?",
                )
                .bind(participant_id)
                .fetch_one(&self.pool)
                .await?;
                let new_participant_id = loop {
                    let result = sqlx::query(
                        r#"
                        insert into participants
                        (research_id, experiment_id, participant_kind, identity_provider, external_id, metadata_json, created_at)
                        values (?, ?, ?, ?, ?, ?, ?)
                        "#,
                    )
                    .bind(readable_participant_id())
                    .bind(experiment_id)
                    .bind(&source.0)
                    .bind(&source.1)
                    .bind(&source.2)
                    .bind(&source.3)
                    .bind(&source.4)
                    .execute(&self.pool)
                    .await;
                    match result {
                        Ok(result) => break result.last_insert_rowid(),
                        Err(error)
                            if sqlite_unique_constraint_for(&error, "participants.research_id") =>
                        {
                            continue
                        }
                        Err(error) => return Err(error.into()),
                    }
                };
                for table in ["session_participants", "consent_declarations"] {
                    sqlx::query(&format!(
                        "update {table} set participant_id = ? where experiment_id = ? and participant_id = ?"
                    ))
                    .bind(new_participant_id)
                    .bind(experiment_id)
                    .bind(participant_id)
                    .execute(&self.pool)
                    .await?;
                }
                sqlx::query(
                    "update session_events set actor_participant_id = ? where experiment_id = ? and actor_participant_id = ?",
                )
                .bind(new_participant_id)
                .bind(experiment_id)
                .bind(participant_id)
                .execute(&self.pool)
                .await?;
            }
        }
        Ok(())
    }

    /// Backfills human random names, descriptive non-human ids, and missing dialogue ids.
    async fn backfill_readable_ids(&self) -> Result<()> {
        let participants =
            sqlx::query_as::<_, (i64, String, String, String, Option<String>, Option<String>)>(
                r#"
            select participant_id, experiment_id, participant_kind, identity_provider,
                   external_id, metadata_json
            from participants
            where (
                participant_kind = 'human'
                and (research_id is null or research_id like 'research_%')
            ) or (
                participant_kind not in ('human', 'deleted')
                and (research_id is null or research_id not like participant_kind || ':%')
            )
            "#,
            )
            .fetch_all(&self.pool)
            .await?;
        for (
            participant_id,
            experiment_id,
            participant_kind,
            identity_provider,
            external_id,
            metadata_json,
        ) in participants
        {
            let participant = ParticipantRecord {
                experiment_id,
                participant_kind,
                identity_provider,
                external_id,
                metadata: metadata_json
                    .as_deref()
                    .map(serde_json::from_str)
                    .transpose()?
                    .unwrap_or(Value::Null),
            };
            let mut identifier_attempt = 1;
            loop {
                let candidate = participant_identifier_candidate(&participant, identifier_attempt);
                let result =
                    sqlx::query("update participants set research_id = ? where participant_id = ?")
                        .bind(candidate)
                        .bind(participant_id)
                        .execute(&self.pool)
                        .await;
                match result {
                    Ok(_) => break,
                    Err(error) if sqlite_unique_constraint(&error) => {
                        identifier_attempt += 1;
                        continue;
                    }
                    Err(error) => return Err(error.into()),
                }
            }
        }
        let sessions = sqlx::query_as::<_, (String, i64)>(
            "select experiment_id, session_id from sessions where dialogue_id is null",
        )
        .fetch_all(&self.pool)
        .await?;
        for (experiment_id, session_id) in sessions {
            loop {
                let candidate = dialogue_id();
                let result = sqlx::query(
                    "update sessions set dialogue_id = ? where experiment_id = ? and session_id = ?",
                )
                .bind(candidate)
                .bind(&experiment_id)
                .bind(session_id)
                .execute(&self.pool)
                .await;
                match result {
                    Ok(_) => break,
                    Err(error) if sqlite_unique_constraint(&error) => continue,
                    Err(error) => return Err(error.into()),
                }
            }
        }
        Ok(())
    }

    /// Drops a legacy SQLite column when it is present.
    async fn drop_column_if_exists(&self, table: &str, column: &str) -> Result<()> {
        let pragma = format!("pragma table_info({table})");
        let columns = sqlx::query_as::<_, (i64, String, String, i64, Option<String>, i64)>(&pragma)
            .fetch_all(&self.pool)
            .await?;
        if columns.iter().any(|row| row.1 == column) {
            sqlx::query(&format!("alter table {table} drop column {column}"))
                .execute(&self.pool)
                .await?;
        }
        Ok(())
    }

    async fn ensure_column(&self, table: &str, column: &str, definition: &str) -> Result<()> {
        let pragma = format!("pragma table_info({table})");
        let columns = sqlx::query_as::<_, (i64, String, String, i64, Option<String>, i64)>(&pragma)
            .fetch_all(&self.pool)
            .await?;
        if columns.iter().any(|row| row.1 == column) {
            return Ok(());
        }
        sqlx::query(&format!(
            "alter table {table} add column {column} {definition}"
        ))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Returns SQLite user table names for schema tests.
    #[cfg(test)]
    async fn table_names(&self) -> Result<Vec<String>> {
        Ok(sqlx::query_scalar::<_, String>(
            "select name from sqlite_master where type = 'table' and name not like 'sqlite_%' order by name",
        )
        .fetch_all(&self.pool)
        .await?)
    }
}

#[async_trait]
impl ExperimentStore for SqliteExperimentStore {
    async fn admin_credential(&self) -> Result<Option<StoredAdminCredential>> {
        Ok(sqlx::query_as::<_, (String, String, String)>(
            "select username, password_hash, role from administrator_credential where singleton = 1",
        )
        .fetch_optional(&self.pool)
        .await?
        .map(|(username, password_hash, role)| StoredAdminCredential {
            username,
            password_hash,
            role,
        }))
    }

    async fn create_admin_credential(&self, credential: StoredAdminCredential) -> Result<bool> {
        let result = sqlx::query(
            r#"
            insert into administrator_credential
                (singleton, username, password_hash, role, created_at)
            values (1, ?, ?, ?, ?)
            on conflict(singleton) do nothing
            "#,
        )
        .bind(credential.username)
        .bind(credential.password_hash)
        .bind(credential.role)
        .bind(now_iso())
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    async fn ensure_experiment(&self, experiment: ExperimentRecord) -> Result<String> {
        let status = sqlx::query_scalar::<_, String>(
            r#"
            insert into experiments (experiment_id, created_at, config_json, server_version, version_manifest_json, status, notes)
            values (?, ?, ?, ?, ?, ?, ?)
            on conflict(experiment_id) do update set
                config_json = excluded.config_json,
                server_version = excluded.server_version,
                version_manifest_json = excluded.version_manifest_json,
                status = 'inactive'
            returning status
            "#,
        )
        .bind(experiment.experiment_id)
        .bind(now_iso())
        .bind(serde_json::to_string(&experiment.config)?)
        .bind(experiment.server_version)
        .bind(
            experiment
                .version_manifest
                .map(|value| serde_json::to_string(&value))
                .transpose()?,
        )
        .bind(experiment.status)
        .bind(experiment.notes)
        .fetch_one(&self.pool)
        .await?;
        Ok(status)
    }

    async fn experiment_summary(
        &self,
        experiment_id: &str,
    ) -> Result<Option<StoredExperimentSummary>> {
        let row = sqlx::query_as::<
            _,
            (
                String,
                String,
                String,
                Option<String>,
                Option<String>,
                String,
                Option<String>,
                i64,
                i64,
                Option<String>,
            ),
        >(
            r#"
            select e.experiment_id, e.created_at, e.config_json, e.server_version,
                   e.version_manifest_json, e.status, e.notes,
                   count(s.session_id) as session_count,
                   sum(case when s.status = 'completed' then 1 else 0 end) as completed_session_count,
                   max(s.created_at) as last_session_at
            from experiments e
            left join sessions s on s.experiment_id = e.experiment_id
            where e.experiment_id = ?
            group by e.experiment_id
            "#,
        )
        .bind(experiment_id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| {
            Ok(StoredExperimentSummary {
                experiment_id: row.0,
                study_name: serde_json::from_str::<Value>(&row.2)
                    .ok()
                    .and_then(|config| {
                        config
                            .get("study")
                            .and_then(|study| study.get("name"))
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    }),
                created_at: row.1,
                server_version: row.3,
                version_manifest: row
                    .4
                    .map(|raw| serde_json::from_str::<Value>(&raw))
                    .transpose()?,
                status: row.5,
                notes: row.6,
                session_count: row.7,
                completed_session_count: row.8,
                last_session_at: row.9,
            })
        })
        .transpose()
    }

    async fn update_experiment_status(&self, experiment_id: &str, status: &str) -> Result<()> {
        sqlx::query("update experiments set status = ? where experiment_id = ?")
            .bind(status)
            .bind(experiment_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn upsert_participant(&self, participant: ParticipantRecord) -> Result<i64> {
        if let Some(external_id) = participant.external_id.as_deref() {
            if let Some(participant_id) = sqlx::query_scalar::<_, i64>(
                "select participant_id from participants where experiment_id = ? and identity_provider = ? and external_id = ?",
            )
            .bind(&participant.experiment_id)
            .bind(&participant.identity_provider)
            .bind(external_id)
            .fetch_optional(&self.pool)
            .await?
            {
                return Ok(participant_id);
            }
        }
        let mut identifier_attempt = 1;
        loop {
            let identifier = participant_identifier_candidate(&participant, identifier_attempt);
            let result = sqlx::query(
                r#"
                insert into participants
                (research_id, experiment_id, participant_kind, identity_provider, external_id, metadata_json, created_at)
                values (?, ?, ?, ?, ?, ?, ?)
                "#,
            )
            .bind(identifier)
            .bind(&participant.experiment_id)
            .bind(&participant.participant_kind)
            .bind(&participant.identity_provider)
            .bind(&participant.external_id)
            .bind(serde_json::to_string(&participant.metadata)?)
            .bind(now_iso())
            .execute(&self.pool)
            .await;
            match result {
                Ok(result) => return Ok(result.last_insert_rowid()),
                Err(error) if sqlite_unique_constraint_for(&error, "participants.research_id") => {
                    identifier_attempt += 1;
                    continue;
                }
                Err(error) => return Err(error.into()),
            }
        }
    }

    async fn participant_research_id(&self, participant_id: i64) -> Result<Option<String>> {
        Ok(sqlx::query_scalar::<_, String>(
            "select research_id from participants where participant_id = ?",
        )
        .bind(participant_id)
        .fetch_optional(&self.pool)
        .await?)
    }

    async fn create_session(&self, session: SessionRecord) -> Result<i64> {
        let mut tx = self.pool.begin().await?;
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
        sqlx::query(
            r#"
            insert into sessions
            (experiment_id, session_id, room_id, dialogue_id, mode, status, created_at)
            values (?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(session.experiment_id)
        .bind(next_session_id)
        .bind(session.room_id)
        .bind(readable_dialogue_id)
        .bind(session.mode)
        .bind(session.status)
        .bind(now_iso())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(next_session_id)
    }

    async fn add_session_participant(&self, participant: SessionParticipantRecord) -> Result<()> {
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
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn update_session_participant_connection(
        &self,
        participant_session_id: &str,
        connection_status: &str,
        left_at: Option<String>,
    ) -> Result<()> {
        sqlx::query(
            "update session_participants set connection_status = ?, left_at = ? where participant_session_id = ?",
        )
        .bind(connection_status)
        .bind(left_at)
        .bind(participant_session_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn record_consent_declaration(
        &self,
        declaration: ConsentDeclarationRecord,
    ) -> Result<()> {
        sqlx::query(
            r#"
            insert into consent_declarations
            (experiment_id, session_id, participant_id, consent_item_id, accepted, declared_at, consent_text_hash, metadata_json)
            values (?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(declaration.experiment_id)
        .bind(declaration.session_id)
        .bind(declaration.participant_id)
        .bind(declaration.consent_item_id)
        .bind(if declaration.accepted { 1 } else { 0 })
        .bind(now_iso())
        .bind(declaration.consent_text_hash)
        .bind(serde_json::to_string(&declaration.metadata)?)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn append_session_event(&self, event: SessionEventRecord) -> Result<i64> {
        let mut tx = self.pool.begin().await?;
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
            (experiment_id, session_id, event_index, event_type, actor_participant_id, actor_role, payload_json, game_state_json, created_at)
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
        .bind(now_iso())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(event_index)
    }

    async fn commit_session_transition(
        &self,
        events: Vec<SessionEventRecord>,
        completion: Option<Value>,
    ) -> Result<()> {
        let Some(first) = events.first() else {
            return Ok(());
        };
        let experiment_id = first.experiment_id.clone();
        let session_id = first.session_id;
        if events
            .iter()
            .any(|event| event.experiment_id != experiment_id || event.session_id != session_id)
        {
            bail!("transition events must belong to one session");
        }
        let mut tx = self.pool.begin().await?;
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
                (experiment_id, session_id, event_index, event_type, actor_participant_id, actor_role, payload_json, game_state_json, created_at)
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
            .bind(now_iso())
            .execute(&mut *tx)
            .await?;
        }
        if let Some(completion) = completion {
            sqlx::query(
                "update sessions set status = 'completed', completed_at = ?, completion_json = ? where experiment_id = ? and session_id = ?",
            )
            .bind(now_iso())
            .bind(serde_json::to_string(&completion)?)
            .bind(&experiment_id)
            .bind(session_id)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    async fn session_events(
        &self,
        experiment_id: &str,
        session_id: i64,
        event_type: Option<&str>,
    ) -> Result<Vec<StoredSessionEvent>> {
        let sql = if event_type.is_some() {
            "select event_id, experiment_id, session_id, event_index, event_type, actor_participant_id, actor_role, payload_json, game_state_json, created_at from session_events where experiment_id = ? and session_id = ? and event_type = ? order by event_index"
        } else {
            "select event_id, experiment_id, session_id, event_index, event_type, actor_participant_id, actor_role, payload_json, game_state_json, created_at from session_events where experiment_id = ? and session_id = ? order by event_index"
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
                String,
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

    async fn recent_sessions(
        &self,
        experiment_id: &str,
        limit: i64,
    ) -> Result<Vec<StoredSessionSummary>> {
        let limit = limit.clamp(1, 500);
        let sessions = sqlx::query_as::<
            _,
            (
                String,
                i64,
                String,
                String,
                String,
                String,
                String,
                Option<String>,
                Option<String>,
                Option<String>,
                i64,
                i64,
                Option<String>,
            ),
        >(
            r#"
            select s.experiment_id, s.session_id, s.room_id, s.dialogue_id, s.mode, s.status,
                   s.created_at, s.started_at, s.completed_at, s.completion_json,
                   count(distinct sp.participant_id) as participant_count,
                   count(distinct se.event_id) as event_count,
                   max(se.created_at) as last_event_at
            from sessions s
            left join session_participants sp
                on sp.experiment_id = s.experiment_id and sp.session_id = s.session_id
            left join session_events se
                on se.experiment_id = s.experiment_id and se.session_id = s.session_id
            where s.experiment_id = ?
            group by s.experiment_id, s.session_id
            order by s.created_at desc, s.session_id desc
            limit ?
            "#,
        )
        .bind(experiment_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(|row| {
            Ok(StoredSessionSummary {
                experiment_id: row.0,
                session_id: row.1,
                room_id: row.2,
                dialogue_id: row.3,
                mode: row.4,
                status: row.5,
                created_at: row.6,
                started_at: row.7,
                completed_at: row.8,
                completion: row
                    .9
                    .map(|raw| serde_json::from_str::<Value>(&raw))
                    .transpose()?,
                participant_count: row.10,
                event_count: row.11,
                last_event_at: row.12,
            })
        })
        .collect::<Result<Vec<_>>>()?;
        Ok(sessions)
    }

    async fn session_participants(
        &self,
        experiment_id: &str,
        session_id: i64,
    ) -> Result<Vec<StoredSessionParticipant>> {
        let participants = sqlx::query_as::<
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
                Option<String>,
                Option<String>,
                Option<String>,
            ),
        >(
            r#"
            select sp.experiment_id, sp.session_id, sp.participant_id,
                   sp.participant_session_id, sp.role, sp.joined_at, sp.left_at,
                   sp.connection_status, p.research_id, p.participant_kind, p.metadata_json
            from session_participants sp
            left join participants p on p.participant_id = sp.participant_id
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
                experiment_id: row.0,
                session_id: row.1,
                participant_id: row.2,
                participant_session_id: row.3,
                role: row.4,
                joined_at: row.5,
                left_at: row.6,
                connection_status: row.7,
                research_id: row.8,
                participant_kind: row.9,
                metadata: row
                    .10
                    .map(|raw| serde_json::from_str::<Value>(&raw))
                    .transpose()?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
        Ok(participants)
    }

    async fn export_experiment(&self, experiment_id: &str) -> Result<Value> {
        export_rows(&self.pool, Some((experiment_id, None))).await
    }

    async fn export_session(&self, experiment_id: &str, session_id: i64) -> Result<Value> {
        export_rows(&self.pool, Some((experiment_id, Some(session_id)))).await
    }

    async fn participant_data_preview(
        &self,
        experiment_id: &str,
        participant_id: i64,
    ) -> Result<ParticipantDataPreview> {
        sqlite_participant_data_preview(&self.pool, experiment_id, participant_id).await
    }

    async fn delete_participant_data(
        &self,
        experiment_id: &str,
        participant_id: i64,
    ) -> Result<ParticipantDataPreview> {
        let preview =
            sqlite_participant_data_preview(&self.pool, experiment_id, participant_id).await?;
        let mut tx = self.pool.begin().await?;
        let participant_session_ids = sqlx::query_scalar::<_, String>(
            "select participant_session_id from session_participants where experiment_id = ? and participant_id = ?",
        )
        .bind(experiment_id)
        .bind(participant_id)
        .fetch_all(&mut *tx)
        .await?;
        sqlx::query(
            "delete from consent_declarations where experiment_id = ? and participant_id = ?",
        )
        .bind(experiment_id)
        .bind(participant_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "delete from session_events where experiment_id = ? and actor_participant_id = ? and event_type in ('conversation_message', 'transcript_segment')",
        )
        .bind(experiment_id)
        .bind(participant_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "update session_events set actor_participant_id = null, actor_role = 'deleted_participant' where experiment_id = ? and actor_participant_id = ?",
        )
        .bind(experiment_id)
        .bind(participant_id)
        .execute(&mut *tx)
        .await?;
        for participant_session_id in &participant_session_ids {
            sqlx::query(
                "update session_events set payload_json = replace(payload_json, ?, 'deleted_participant'), game_state_json = replace(game_state_json, ?, 'deleted_participant') where experiment_id = ?",
            )
            .bind(participant_session_id)
            .bind(participant_session_id)
            .bind(experiment_id)
            .execute(&mut *tx)
            .await?;
        }
        for participant_session_id in participant_session_ids {
            sqlx::query(
                "update session_participants set participant_session_id = ?, connection_status = 'deleted', left_at = ? where participant_session_id = ?",
            )
            .bind(new_id("deleted"))
            .bind(now_iso())
            .bind(participant_session_id)
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query(
            "update participants set research_id = null, participant_kind = 'deleted', external_id = null, metadata_json = '{}' where participant_id = ?",
        )
        .bind(participant_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(preview)
    }
}

/// Counts participant-linked rows in SQLite without modifying them.
async fn sqlite_participant_data_preview(
    pool: &SqlitePool,
    experiment_id: &str,
    participant_id: i64,
) -> Result<ParticipantDataPreview> {
    let session_count = sqlx::query_scalar::<_, i64>(
        "select count(*) from session_participants where experiment_id = ? and participant_id = ?",
    )
    .bind(experiment_id)
    .bind(participant_id)
    .fetch_one(pool)
    .await?;
    let consent_count = sqlx::query_scalar::<_, i64>(
        "select count(*) from consent_declarations where experiment_id = ? and participant_id = ?",
    )
    .bind(experiment_id)
    .bind(participant_id)
    .fetch_one(pool)
    .await?;
    let content_event_count = sqlx::query_scalar::<_, i64>(
        "select count(*) from session_events where experiment_id = ? and actor_participant_id = ? and event_type in ('conversation_message', 'transcript_segment')",
    )
    .bind(experiment_id)
    .bind(participant_id)
    .fetch_one(pool)
    .await?;
    let other_event_count = sqlx::query_scalar::<_, i64>(
        "select count(*) from session_events where experiment_id = ? and actor_participant_id = ? and event_type not in ('conversation_message', 'transcript_segment')",
    )
    .bind(experiment_id)
    .bind(participant_id)
    .fetch_one(pool)
    .await?;
    Ok(ParticipantDataPreview {
        participant_id,
        session_count,
        consent_count,
        content_event_count,
        other_event_count,
    })
}

/// Creates the configured experiment store from `database.url`.
pub async fn experiment_store_from_url(database_url: &str) -> Result<SharedExperimentStore> {
    if database_url.is_empty() {
        bail!(
            "database.url is required; use sqlite:///:memory: only in tests or a sqlite:///... file for actual runs"
        )
    } else {
        Ok(Arc::new(
            SqliteExperimentStore::connect(database_url).await?,
        ))
    }
}

type SessionEventSqlRow = (
    i64,
    String,
    i64,
    i64,
    String,
    Option<i64>,
    Option<String>,
    String,
    Option<String>,
    String,
);

fn stored_event_from_sql_row(row: SessionEventSqlRow) -> Result<StoredSessionEvent> {
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
        created_at: row.9,
    })
}

/// Exports SQLite rows as JSON objects for the current admin/evaluation export.
async fn export_rows(pool: &SqlitePool, scope: Option<(&str, Option<i64>)>) -> Result<Value> {
    let (experiment_id, session_id) = scope.unwrap_or(("", None));
    let experiment = if experiment_id.is_empty() {
        json!(null)
    } else {
        let row = sqlx::query_as::<_, (String, String, String, Option<String>, Option<String>, String, Option<String>)>(
            "select experiment_id, created_at, config_json, server_version, version_manifest_json, status, notes from experiments where experiment_id = ?",
        )
        .bind(experiment_id)
        .fetch_optional(pool)
        .await?;
        json!(row.map(
            |(id, created_at, config_json, server_version, version_manifest_json, status, notes)| json!({
                "experiment_id": id,
                "created_at": created_at,
                "config": serde_json::from_str::<Value>(&config_json).unwrap_or(Value::Null),
                "server_version": server_version,
                "version_manifest": version_manifest_json.and_then(|raw| serde_json::from_str::<Value>(&raw).ok()),
                "status": status,
                "notes": notes,
            })
        ))
    };
    let sessions_sql = if session_id.is_some() {
        "select experiment_id, session_id, room_id, dialogue_id, mode, status, created_at, started_at, completed_at, completion_json from sessions where experiment_id = ? and session_id = ? order by session_id"
    } else {
        "select experiment_id, session_id, room_id, dialogue_id, mode, status, created_at, started_at, completed_at, completion_json from sessions where experiment_id = ? order by session_id"
    };
    let mut sessions_query = sqlx::query_as::<
        _,
        (
            String,
            i64,
            String,
            String,
            String,
            String,
            String,
            Option<String>,
            Option<String>,
            Option<String>,
        ),
    >(sessions_sql)
    .bind(experiment_id);
    if let Some(session_id) = session_id {
        sessions_query = sessions_query.bind(session_id);
    }
    let sessions = sessions_query
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|row| {
            json!({
                "experiment_id": row.0,
                "session_id": row.1,
                "room_id": row.2,
                "dialogue_id": row.3,
                "mode": row.4,
                "status": row.5,
                "created_at": row.6,
                "started_at": row.7,
                "completed_at": row.8,
                "completion": row.9.and_then(|raw| serde_json::from_str::<Value>(&raw).ok()),
            })
        })
        .collect::<Vec<_>>();
    let event_sql = if session_id.is_some() {
        "select event_id, experiment_id, session_id, event_index, event_type, actor_participant_id, actor_role, payload_json, game_state_json, created_at from session_events where experiment_id = ? and session_id = ? order by session_id, event_index"
    } else {
        "select event_id, experiment_id, session_id, event_index, event_type, actor_participant_id, actor_role, payload_json, game_state_json, created_at from session_events where experiment_id = ? order by session_id, event_index"
    };
    let mut event_query = sqlx::query_as::<_, SessionEventSqlRow>(event_sql).bind(experiment_id);
    if let Some(session_id) = scope.and_then(|(_, id)| id) {
        event_query = event_query.bind(session_id);
    }
    let events = event_query
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(stored_event_from_sql_row)
        .collect::<Result<Vec<_>>>()?;
    let participants_sql = if session_id.is_some() {
        r#"
        select distinct p.participant_id, p.research_id, p.participant_kind, p.identity_provider, p.external_id,
               p.metadata_json, p.created_at
        from participants p
        join session_participants sp on sp.participant_id = p.participant_id
        where sp.experiment_id = ? and sp.session_id = ?
        order by p.participant_id
        "#
    } else {
        r#"
        select distinct p.participant_id, p.research_id, p.participant_kind, p.identity_provider, p.external_id,
               p.metadata_json, p.created_at
        from participants p
        join session_participants sp on sp.participant_id = p.participant_id
        where sp.experiment_id = ?
        order by p.participant_id
        "#
    };
    let mut participants_query = sqlx::query_as::<
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
    if let Some(session_id) = scope.and_then(|(_, id)| id) {
        participants_query = participants_query.bind(session_id);
    }
    let participants = participants_query
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|row| {
            json!({
                "participant_id": row.0,
                "research_id": row.1,
                "participant_kind": row.2,
                "identity_provider": row.3,
                "external_id": row.4,
                "metadata": row.5.and_then(|raw| serde_json::from_str::<Value>(&raw).ok()),
                "created_at": row.6,
            })
        })
        .collect::<Vec<_>>();
    let session_participants_sql = if session_id.is_some() {
        "select experiment_id, session_id, participant_id, participant_session_id, role, joined_at, left_at, connection_status from session_participants where experiment_id = ? and session_id = ? order by session_id, role"
    } else {
        "select experiment_id, session_id, participant_id, participant_session_id, role, joined_at, left_at, connection_status from session_participants where experiment_id = ? order by session_id, role"
    };
    let mut session_participants_query = sqlx::query_as::<
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
    if let Some(session_id) = scope.and_then(|(_, id)| id) {
        session_participants_query = session_participants_query.bind(session_id);
    }
    let session_participants = session_participants_query
        .fetch_all(pool)
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
    let consent_sql = if session_id.is_some() {
        "select consent_id, experiment_id, session_id, participant_id, consent_item_id, accepted, declared_at, consent_text_hash, metadata_json from consent_declarations where experiment_id = ? and session_id = ? order by declared_at, consent_id"
    } else {
        "select consent_id, experiment_id, session_id, participant_id, consent_item_id, accepted, declared_at, consent_text_hash, metadata_json from consent_declarations where experiment_id = ? order by declared_at, consent_id"
    };
    let mut consent_query = sqlx::query_as::<
        _,
        (
            i64,
            String,
            Option<i64>,
            i64,
            String,
            i64,
            String,
            Option<String>,
            Option<String>,
        ),
    >(consent_sql)
    .bind(experiment_id);
    if let Some(session_id) = scope.and_then(|(_, id)| id) {
        consent_query = consent_query.bind(session_id);
    }
    let consent_declarations = consent_query
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|row| {
            json!({
                "consent_id": row.0,
                "experiment_id": row.1,
                "session_id": row.2,
                "participant_id": row.3,
                "consent_item_id": row.4,
                "accepted": row.5 != 0,
                "declared_at": row.6,
                "consent_text_hash": row.7,
                "metadata": row.8.and_then(|raw| serde_json::from_str::<Value>(&raw).ok()),
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "experiment": experiment,
        "participants": participants,
        "sessions": sessions,
        "session_participants": session_participants,
        "consent_declarations": consent_declarations,
        "session_events": events,
    }))
}

/// Runtime participant session record used by active server tasks.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ParticipantSession {
    pub id: String,
    pub participant_id: i64,
    pub research_id: String,
    pub source: String,
    pub status: String,
    pub study_id: Option<String>,
    pub consent_decisions: HashMap<String, bool>,
    pub created_at: String,
    pub updated_at: String,
}

/// Runtime record for one participant's membership in a room.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RoomParticipant {
    pub participant_session_id: String,
    pub participant_id: i64,
    /// Runtime participant source, used to distinguish humans from agents.
    pub source: String,
    pub role: Seat,
    pub connected: bool,
    /// Whether this participant has completed required audio/STT setup for this room.
    pub audio_ready: bool,
    pub consent_decisions: HashMap<String, bool>,
    pub joined_at: String,
    pub updated_at: String,
}

/// Runtime room record parameterized by a concrete game state type.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GameRoom<S> {
    pub id: String,
    pub experiment_id: String,
    pub session_id: i64,
    pub mode: String,
    pub state: S,
    /// Runtime lifecycle status: `waiting`, `playing`, or `completed`.
    pub status: String,
    pub study_id: Option<String>,
    pub participants: HashMap<String, RoomParticipant>,
    pub created_at: String,
    pub updated_at: String,
}

/// Stored transcript segment received from the browser transcription flow.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TranscriptSegment {
    pub id: String,
    pub room_id: String,
    pub participant_session_id: String,
    pub player: String,
    pub start_time_ms: i64,
    pub end_time_ms: i64,
    pub text: String,
    pub metadata: Value,
    pub created_at: String,
}

/// In-memory active-session cache used for live WebSocket and game execution state.
#[derive(Clone, Debug)]
pub struct MemoryState<S> {
    pub participants: HashMap<String, ParticipantSession>,
    pub rooms: HashMap<String, GameRoom<S>>,
}

impl<S> Default for MemoryState<S> {
    fn default() -> Self {
        Self {
            participants: HashMap::new(),
            rooms: HashMap::new(),
        }
    }
}

impl<S: Clone + Serialize> MemoryState<S> {
    /// Creates and stores an active participant session after durable identity creation.
    pub fn create_participant(
        &mut self,
        participant_id: i64,
        research_id: String,
        source: String,
        study_id: Option<String>,
    ) -> ParticipantSession {
        let now = now_iso();
        let participant = ParticipantSession {
            id: new_id("ps"),
            participant_id,
            research_id,
            source,
            status: "created".to_string(),
            study_id,
            consent_decisions: HashMap::new(),
            created_at: now.clone(),
            updated_at: now,
        };
        self.participants
            .insert(participant.id.clone(), participant.clone());
        participant
    }
}

#[cfg(test)]
mod tests;
