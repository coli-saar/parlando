use std::{collections::HashMap, path::Path, sync::Arc};

use anyhow::{bail, Result};
use async_trait::async_trait;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    SqlitePool,
};
use tokio::sync::RwLock;

use crate::{game::Seat, identity::new_id};

/// Returns the current UTC timestamp in ISO-8601/RFC3339 form.
pub fn now_iso() -> String {
    Utc::now().to_rfc3339()
}

/// Generates a startup experiment id when neither CLI nor YAML provided one.
pub fn generated_experiment_id() -> String {
    new_id("exp")
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
    pub participant_kind: String,
    pub identity_provider: String,
    pub external_id: Option<String>,
    pub display_name: Option<String>,
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
    pub participant_session_id: String,
    pub role: String,
    pub joined_at: String,
    pub left_at: Option<String>,
    pub connection_status: String,
    pub participant_kind: Option<String>,
    pub display_name: Option<String>,
    pub metadata: Option<Value>,
}

/// Backend-neutral storage interface centered on experiment evaluation.
#[async_trait]
pub trait ExperimentStore: Send + Sync {
    /// Ensures that one experiment row exists for this server run.
    async fn ensure_experiment(&self, experiment: ExperimentRecord) -> Result<()>;
    /// Returns durable experiment rows with compact session aggregates.
    async fn list_experiments(&self, limit: i64) -> Result<Vec<StoredExperimentSummary>>;
    /// Updates the lifecycle status for one experiment row.
    async fn update_experiment_status(&self, experiment_id: &str, status: &str) -> Result<()>;
    /// Creates or reuses a durable participant identity and returns `participant_id`.
    async fn upsert_participant(&self, participant: ParticipantRecord) -> Result<i64>;
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
    /// Marks a session complete with an optional completion payload.
    async fn complete_session(
        &self,
        experiment_id: &str,
        session_id: i64,
        completion: Option<Value>,
    ) -> Result<()>;
    /// Exports all durable evaluation data for one experiment.
    async fn export_experiment(&self, experiment_id: &str) -> Result<Value>;
    /// Exports all durable evaluation data for one session.
    async fn export_session(&self, experiment_id: &str, session_id: i64) -> Result<Value>;
}

/// Shared trait-object handle for the configured experiment store backend.
pub type SharedExperimentStore = Arc<dyn ExperimentStore>;

#[derive(Default)]
struct MemoryStoreInner {
    experiments: HashMap<String, Value>,
    participants: Vec<Value>,
    sessions: Vec<Value>,
    session_participants: Vec<Value>,
    consent_declarations: Vec<Value>,
    session_events: Vec<Value>,
    next_participant_id: i64,
}

/// In-memory experiment store used only by focused unit tests and empty local database URLs.
#[derive(Default)]
pub struct MemoryExperimentStore {
    inner: RwLock<MemoryStoreInner>,
}

#[async_trait]
impl ExperimentStore for MemoryExperimentStore {
    async fn ensure_experiment(&self, experiment: ExperimentRecord) -> Result<()> {
        let mut inner = self.inner.write().await;
        inner.experiments.insert(
            experiment.experiment_id.clone(),
            json!({
                "experiment_id": experiment.experiment_id,
                "created_at": now_iso(),
                "config": experiment.config,
                "server_version": experiment.server_version,
                "version_manifest": experiment.version_manifest,
                "status": experiment.status,
                "notes": experiment.notes,
            }),
        );
        Ok(())
    }

    async fn list_experiments(&self, limit: i64) -> Result<Vec<StoredExperimentSummary>> {
        let inner = self.inner.read().await;
        let mut experiments = inner
            .experiments
            .values()
            .map(|row| {
                let experiment_id = row["experiment_id"].as_str().unwrap_or_default();
                let matching_sessions = inner
                    .sessions
                    .iter()
                    .filter(|session| session["experiment_id"].as_str() == Some(experiment_id))
                    .collect::<Vec<_>>();
                StoredExperimentSummary {
                    experiment_id: experiment_id.to_string(),
                    study_name: row
                        .get("config")
                        .and_then(|config| config.get("study"))
                        .and_then(|study| study.get("name"))
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    created_at: row["created_at"].as_str().unwrap_or_default().to_string(),
                    status: row["status"].as_str().unwrap_or("draft").to_string(),
                    server_version: row["server_version"].as_str().map(str::to_string),
                    version_manifest: row
                        .get("version_manifest")
                        .cloned()
                        .filter(|value| !value.is_null()),
                    notes: row["notes"].as_str().map(str::to_string),
                    session_count: matching_sessions.len() as i64,
                    completed_session_count: matching_sessions
                        .iter()
                        .filter(|session| session["status"].as_str() == Some("completed"))
                        .count() as i64,
                    last_session_at: matching_sessions
                        .iter()
                        .filter_map(|session| session["created_at"].as_str())
                        .max()
                        .map(str::to_string),
                }
            })
            .collect::<Vec<_>>();
        experiments.sort_by(|left, right| right.created_at.cmp(&left.created_at));
        experiments.truncate(limit.max(0) as usize);
        Ok(experiments)
    }

    async fn update_experiment_status(&self, experiment_id: &str, status: &str) -> Result<()> {
        if let Some(row) = self.inner.write().await.experiments.get_mut(experiment_id) {
            row["status"] = json!(status);
        }
        Ok(())
    }

    async fn upsert_participant(&self, participant: ParticipantRecord) -> Result<i64> {
        let mut inner = self.inner.write().await;
        if let Some(external_id) = participant.external_id.as_deref() {
            if let Some(existing) = inner.participants.iter().find(|row| {
                row["identity_provider"] == participant.identity_provider
                    && row["external_id"].as_str() == Some(external_id)
            }) {
                return Ok(existing["participant_id"].as_i64().unwrap());
            }
        }
        inner.next_participant_id += 1;
        let participant_id = inner.next_participant_id;
        inner.participants.push(json!({
            "participant_id": participant_id,
            "participant_kind": participant.participant_kind,
            "identity_provider": participant.identity_provider,
            "external_id": participant.external_id,
            "display_name": participant.display_name,
            "metadata": participant.metadata,
            "created_at": now_iso(),
        }));
        Ok(participant_id)
    }

    async fn create_session(&self, session: SessionRecord) -> Result<i64> {
        let mut inner = self.inner.write().await;
        let session_id = inner
            .sessions
            .iter()
            .filter(|row| row["experiment_id"] == session.experiment_id)
            .filter_map(|row| row["session_id"].as_i64())
            .max()
            .unwrap_or(0)
            + 1;
        inner.sessions.push(json!({
            "experiment_id": session.experiment_id,
            "session_id": session_id,
            "room_id": session.room_id,
            "mode": session.mode,
            "status": session.status,
            "created_at": now_iso(),
        }));
        Ok(session_id)
    }

    async fn add_session_participant(&self, participant: SessionParticipantRecord) -> Result<()> {
        self.inner.write().await.session_participants.push(json!({
            "experiment_id": participant.experiment_id,
            "session_id": participant.session_id,
            "participant_id": participant.participant_id,
            "participant_session_id": participant.participant_session_id,
            "role": participant.role,
            "joined_at": now_iso(),
            "connection_status": participant.connection_status,
        }));
        Ok(())
    }

    async fn update_session_participant_connection(
        &self,
        participant_session_id: &str,
        connection_status: &str,
        left_at: Option<String>,
    ) -> Result<()> {
        let mut inner = self.inner.write().await;
        if let Some(row) = inner
            .session_participants
            .iter_mut()
            .find(|row| row["participant_session_id"].as_str() == Some(participant_session_id))
        {
            row["connection_status"] = json!(connection_status);
            row["left_at"] = json!(left_at);
        }
        Ok(())
    }

    async fn record_consent_declaration(
        &self,
        declaration: ConsentDeclarationRecord,
    ) -> Result<()> {
        self.inner.write().await.consent_declarations.push(json!({
            "experiment_id": declaration.experiment_id,
            "session_id": declaration.session_id,
            "participant_id": declaration.participant_id,
            "consent_item_id": declaration.consent_item_id,
            "accepted": declaration.accepted,
            "declared_at": now_iso(),
            "consent_text_hash": declaration.consent_text_hash,
            "metadata": declaration.metadata,
        }));
        Ok(())
    }

    async fn append_session_event(&self, event: SessionEventRecord) -> Result<i64> {
        let mut inner = self.inner.write().await;
        let event_index = inner
            .session_events
            .iter()
            .filter(|row| {
                row["experiment_id"] == event.experiment_id
                    && row["session_id"].as_i64() == Some(event.session_id)
            })
            .filter_map(|row| row["event_index"].as_i64())
            .max()
            .unwrap_or(0)
            + 1;
        let event_id = inner.session_events.len() + 1;
        inner.session_events.push(json!({
            "event_id": event_id,
            "experiment_id": event.experiment_id,
            "session_id": event.session_id,
            "event_index": event_index,
            "event_type": event.event_type,
            "actor_participant_id": event.actor_participant_id,
            "actor_role": event.actor_role,
            "payload": event.payload,
            "game_state": event.game_state,
            "created_at": now_iso(),
        }));
        Ok(event_index)
    }

    async fn session_events(
        &self,
        experiment_id: &str,
        session_id: i64,
        event_type: Option<&str>,
    ) -> Result<Vec<StoredSessionEvent>> {
        let inner = self.inner.read().await;
        let mut events = inner
            .session_events
            .iter()
            .filter(|row| row["experiment_id"].as_str() == Some(experiment_id))
            .filter(|row| row["session_id"].as_i64() == Some(session_id))
            .filter(|row| {
                event_type.is_none_or(|event_type| row["event_type"].as_str() == Some(event_type))
            })
            .map(|row| StoredSessionEvent {
                event_id: row["event_id"].as_i64().unwrap_or_default(),
                experiment_id: row["experiment_id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                session_id: row["session_id"].as_i64().unwrap_or_default(),
                event_index: row["event_index"].as_i64().unwrap_or_default(),
                event_type: row["event_type"].as_str().unwrap_or_default().to_string(),
                actor_participant_id: row["actor_participant_id"].as_i64(),
                actor_role: row["actor_role"].as_str().map(str::to_string),
                payload: row["payload"].clone(),
                game_state: row
                    .get("game_state")
                    .cloned()
                    .filter(|value| !value.is_null()),
                created_at: row["created_at"].as_str().unwrap_or_default().to_string(),
            })
            .collect::<Vec<_>>();
        events.sort_by_key(|event| event.event_index);
        Ok(events)
    }

    async fn recent_sessions(
        &self,
        experiment_id: &str,
        limit: i64,
    ) -> Result<Vec<StoredSessionSummary>> {
        let inner = self.inner.read().await;
        let mut sessions = inner
            .sessions
            .iter()
            .filter(|row| row["experiment_id"].as_str() == Some(experiment_id))
            .map(|row| {
                let session_id = row["session_id"].as_i64().unwrap_or_default();
                let participant_count = inner
                    .session_participants
                    .iter()
                    .filter(|participant| {
                        participant["experiment_id"].as_str() == Some(experiment_id)
                            && participant["session_id"].as_i64() == Some(session_id)
                    })
                    .count() as i64;
                let matching_events = inner
                    .session_events
                    .iter()
                    .filter(|event| {
                        event["experiment_id"].as_str() == Some(experiment_id)
                            && event["session_id"].as_i64() == Some(session_id)
                    })
                    .collect::<Vec<_>>();
                StoredSessionSummary {
                    experiment_id: experiment_id.to_string(),
                    session_id,
                    room_id: row["room_id"].as_str().unwrap_or_default().to_string(),
                    mode: row["mode"].as_str().unwrap_or_default().to_string(),
                    status: row["status"].as_str().unwrap_or_default().to_string(),
                    created_at: row["created_at"].as_str().unwrap_or_default().to_string(),
                    started_at: row["started_at"].as_str().map(str::to_string),
                    completed_at: row["completed_at"].as_str().map(str::to_string),
                    completion: row
                        .get("completion")
                        .cloned()
                        .filter(|value| !value.is_null()),
                    participant_count,
                    event_count: matching_events.len() as i64,
                    last_event_at: matching_events
                        .iter()
                        .filter_map(|event| event["created_at"].as_str())
                        .max()
                        .map(str::to_string),
                }
            })
            .collect::<Vec<_>>();
        sessions.sort_by(|left, right| {
            right
                .created_at
                .cmp(&left.created_at)
                .then_with(|| right.session_id.cmp(&left.session_id))
        });
        sessions.truncate(limit.max(0) as usize);
        Ok(sessions)
    }

    async fn session_participants(
        &self,
        experiment_id: &str,
        session_id: i64,
    ) -> Result<Vec<StoredSessionParticipant>> {
        let inner = self.inner.read().await;
        let mut participants = inner
            .session_participants
            .iter()
            .filter(|row| row["experiment_id"].as_str() == Some(experiment_id))
            .filter(|row| row["session_id"].as_i64() == Some(session_id))
            .map(|row| {
                let participant_id = row["participant_id"].as_i64().unwrap_or_default();
                let participant = inner.participants.iter().find(|participant| {
                    participant["participant_id"].as_i64() == Some(participant_id)
                });
                StoredSessionParticipant {
                    experiment_id: experiment_id.to_string(),
                    session_id,
                    participant_id,
                    participant_session_id: row["participant_session_id"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                    role: row["role"].as_str().unwrap_or_default().to_string(),
                    joined_at: row["joined_at"].as_str().unwrap_or_default().to_string(),
                    left_at: row["left_at"].as_str().map(str::to_string),
                    connection_status: row["connection_status"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                    participant_kind: participant
                        .and_then(|participant| participant["participant_kind"].as_str())
                        .map(str::to_string),
                    display_name: participant
                        .and_then(|participant| participant["display_name"].as_str())
                        .map(str::to_string),
                    metadata: participant
                        .and_then(|participant| participant.get("metadata"))
                        .cloned()
                        .filter(|value| !value.is_null()),
                }
            })
            .collect::<Vec<_>>();
        participants.sort_by(|left, right| left.role.cmp(&right.role));
        Ok(participants)
    }

    async fn complete_session(
        &self,
        experiment_id: &str,
        session_id: i64,
        completion: Option<Value>,
    ) -> Result<()> {
        let mut inner = self.inner.write().await;
        if let Some(row) = inner.sessions.iter_mut().find(|row| {
            row["experiment_id"].as_str() == Some(experiment_id)
                && row["session_id"].as_i64() == Some(session_id)
        }) {
            row["status"] = json!("completed");
            row["completed_at"] = json!(now_iso());
            row["completion"] = json!(completion);
        }
        Ok(())
    }

    async fn export_experiment(&self, experiment_id: &str) -> Result<Value> {
        let inner = self.inner.read().await;
        Ok(json!({
            "experiment": inner.experiments.get(experiment_id),
            "participants": inner.participants,
            "sessions": inner.sessions.iter().filter(|row| row["experiment_id"].as_str() == Some(experiment_id)).collect::<Vec<_>>(),
            "session_participants": inner.session_participants.iter().filter(|row| row["experiment_id"].as_str() == Some(experiment_id)).collect::<Vec<_>>(),
            "consent_declarations": inner.consent_declarations.iter().filter(|row| row["experiment_id"].as_str() == Some(experiment_id)).collect::<Vec<_>>(),
            "session_events": inner.session_events.iter().filter(|row| row["experiment_id"].as_str() == Some(experiment_id)).collect::<Vec<_>>(),
        }))
    }

    async fn export_session(&self, experiment_id: &str, session_id: i64) -> Result<Value> {
        let inner = self.inner.read().await;
        Ok(json!({
            "experiment_id": experiment_id,
            "session_id": session_id,
            "sessions": inner.sessions.iter().filter(|row| row["experiment_id"].as_str() == Some(experiment_id) && row["session_id"].as_i64() == Some(session_id)).collect::<Vec<_>>(),
            "session_participants": inner.session_participants.iter().filter(|row| row["experiment_id"].as_str() == Some(experiment_id) && row["session_id"].as_i64() == Some(session_id)).collect::<Vec<_>>(),
            "consent_declarations": inner.consent_declarations.iter().filter(|row| row["experiment_id"].as_str() == Some(experiment_id) && row["session_id"].as_i64() == Some(session_id)).collect::<Vec<_>>(),
            "session_events": inner.session_events.iter().filter(|row| row["experiment_id"].as_str() == Some(experiment_id) && row["session_id"].as_i64() == Some(session_id)).collect::<Vec<_>>(),
        }))
    }
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
                .max_connections(5)
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
            .create_if_missing(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(options)
            .await?;
        let store = Self { pool };
        store.ensure_schema().await?;
        Ok(store)
    }

    /// Creates the relational evaluation schema.
    async fn ensure_schema(&self) -> Result<()> {
        for statement in [
            r#"
            create table if not exists experiments (
                experiment_id text primary key,
                created_at text not null,
                config_json text not null,
                server_version text,
                version_manifest_json text,
                status text not null default 'draft',
                notes text
            )
            "#,
            r#"
            create table if not exists participants (
                participant_id integer primary key autoincrement,
                participant_kind text not null,
                identity_provider text not null,
                external_id text,
                display_name text,
                metadata_json text,
                created_at text not null
            )
            "#,
            r#"
            create unique index if not exists idx_participants_provider_external
            on participants(identity_provider, external_id)
            where external_id is not null
            "#,
            r#"
            create table if not exists sessions (
                experiment_id text not null,
                session_id integer not null,
                room_id text not null unique,
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
        self.ensure_column("experiments", "version_manifest_json", "text")
            .await?;
        self.ensure_column("experiments", "status", "text not null default 'draft'")
            .await?;
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
    async fn ensure_experiment(&self, experiment: ExperimentRecord) -> Result<()> {
        sqlx::query(
            r#"
            insert into experiments (experiment_id, created_at, config_json, server_version, version_manifest_json, status, notes)
            values (?, ?, ?, ?, ?, ?, ?)
            on conflict(experiment_id) do update set
                config_json = excluded.config_json,
                server_version = excluded.server_version,
                version_manifest_json = excluded.version_manifest_json,
                status = excluded.status,
                notes = excluded.notes
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
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn list_experiments(&self, limit: i64) -> Result<Vec<StoredExperimentSummary>> {
        let rows = sqlx::query_as::<
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
            group by e.experiment_id
            order by e.created_at desc
            limit ?
            "#,
        )
        .bind(limit.clamp(1, 500))
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
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
            .collect()
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
                "select participant_id from participants where identity_provider = ? and external_id = ?",
            )
            .bind(&participant.identity_provider)
            .bind(external_id)
            .fetch_optional(&self.pool)
            .await?
            {
                return Ok(participant_id);
            }
        }
        let result = sqlx::query(
            r#"
            insert into participants
            (participant_kind, identity_provider, external_id, display_name, metadata_json, created_at)
            values (?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(participant.participant_kind)
        .bind(participant.identity_provider)
        .bind(participant.external_id)
        .bind(participant.display_name)
        .bind(serde_json::to_string(&participant.metadata)?)
        .bind(now_iso())
        .execute(&self.pool)
        .await?;
        Ok(result.last_insert_rowid())
    }

    async fn create_session(&self, session: SessionRecord) -> Result<i64> {
        let mut tx = self.pool.begin().await?;
        let next_session_id = sqlx::query_scalar::<_, i64>(
            "select coalesce(max(session_id), 0) + 1 from sessions where experiment_id = ?",
        )
        .bind(&session.experiment_id)
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query(
            r#"
            insert into sessions
            (experiment_id, session_id, room_id, mode, status, created_at)
            values (?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(session.experiment_id)
        .bind(next_session_id)
        .bind(session.room_id)
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
                Option<String>,
                Option<String>,
                Option<String>,
                i64,
                i64,
                Option<String>,
            ),
        >(
            r#"
            select s.experiment_id, s.session_id, s.room_id, s.mode, s.status,
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
                mode: row.3,
                status: row.4,
                created_at: row.5,
                started_at: row.6,
                completed_at: row.7,
                completion: row
                    .8
                    .map(|raw| serde_json::from_str::<Value>(&raw))
                    .transpose()?,
                participant_count: row.9,
                event_count: row.10,
                last_event_at: row.11,
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
                   sp.connection_status, p.participant_kind, p.display_name, p.metadata_json
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
                participant_kind: row.8,
                display_name: row.9,
                metadata: row
                    .10
                    .map(|raw| serde_json::from_str::<Value>(&raw))
                    .transpose()?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
        Ok(participants)
    }

    async fn complete_session(
        &self,
        experiment_id: &str,
        session_id: i64,
        completion: Option<Value>,
    ) -> Result<()> {
        sqlx::query(
            "update sessions set status = 'completed', completed_at = ?, completion_json = ? where experiment_id = ? and session_id = ?",
        )
        .bind(now_iso())
        .bind(completion.map(|value| serde_json::to_string(&value)).transpose()?)
        .bind(experiment_id)
        .bind(session_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn export_experiment(&self, experiment_id: &str) -> Result<Value> {
        export_rows(&self.pool, Some((experiment_id, None))).await
    }

    async fn export_session(&self, experiment_id: &str, session_id: i64) -> Result<Value> {
        export_rows(&self.pool, Some((experiment_id, Some(session_id)))).await
    }
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
        "select experiment_id, session_id, room_id, mode, status, created_at, started_at, completed_at, completion_json from sessions where experiment_id = ? and session_id = ? order by session_id"
    } else {
        "select experiment_id, session_id, room_id, mode, status, created_at, started_at, completed_at, completion_json from sessions where experiment_id = ? order by session_id"
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
                "mode": row.3,
                "status": row.4,
                "created_at": row.5,
                "started_at": row.6,
                "completed_at": row.7,
                "completion": row.8.and_then(|raw| serde_json::from_str::<Value>(&raw).ok()),
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
        select distinct p.participant_id, p.participant_kind, p.identity_provider, p.external_id,
               p.display_name, p.metadata_json, p.created_at
        from participants p
        join session_participants sp on sp.participant_id = p.participant_id
        where sp.experiment_id = ? and sp.session_id = ?
        order by p.participant_id
        "#
    } else {
        r#"
        select distinct p.participant_id, p.participant_kind, p.identity_provider, p.external_id,
               p.display_name, p.metadata_json, p.created_at
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
            String,
            String,
            Option<String>,
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
                "participant_kind": row.1,
                "identity_provider": row.2,
                "external_id": row.3,
                "display_name": row.4,
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
    pub source: String,
    pub status: String,
    pub display_name: Option<String>,
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
    pub matchmaking_queues: HashMap<String, Vec<String>>,
}

impl<S> Default for MemoryState<S> {
    fn default() -> Self {
        Self {
            participants: HashMap::new(),
            rooms: HashMap::new(),
            matchmaking_queues: HashMap::new(),
        }
    }
}

impl<S: Clone + Serialize> MemoryState<S> {
    /// Creates and stores an active participant session after durable identity creation.
    pub fn create_participant(
        &mut self,
        participant_id: i64,
        source: String,
        display_name: Option<String>,
        study_id: Option<String>,
    ) -> ParticipantSession {
        let now = now_iso();
        let participant = ParticipantSession {
            id: new_id("ps"),
            participant_id,
            source,
            status: "created".to_string(),
            display_name,
            study_id,
            consent_decisions: HashMap::new(),
            created_at: now.clone(),
            updated_at: now,
        };
        self.participants
            .insert(participant.id.clone(), participant.clone());
        participant
    }

    /// Finds the first room containing the participant session.
    pub fn room_for_participant(&self, participant_session_id: &str) -> Option<&GameRoom<S>> {
        self.rooms
            .values()
            .find(|room| room.participants.contains_key(participant_session_id))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tempfile::tempdir;

    use super::*;

    #[tokio::test]
    async fn sqlite_schema_has_only_evaluation_tables() {
        let store = SqliteExperimentStore::connect("sqlite:///:memory:")
            .await
            .unwrap();
        let tables = store.table_names().await.unwrap();

        assert_eq!(
            tables,
            vec![
                "consent_declarations",
                "experiments",
                "participants",
                "session_events",
                "session_participants",
                "sessions",
            ]
        );
    }

    #[tokio::test]
    async fn sqlite_experiment_sessions_participants_and_events_are_queryable() {
        let temp = tempdir().expect("tempdir");
        let database_url = format!("sqlite:///{}", temp.path().join("eval.sqlite").display());
        let store = SqliteExperimentStore::connect(&database_url).await.unwrap();
        store
            .ensure_experiment(ExperimentRecord {
                experiment_id: "exp_eval".to_string(),
                config: json!({"study": "demo"}),
                server_version: Some("test".to_string()),
                version_manifest: None,
                status: "active".to_string(),
                notes: None,
            })
            .await
            .unwrap();
        let participant_id = store
            .upsert_participant(ParticipantRecord {
                participant_kind: "human".to_string(),
                identity_provider: "prolific".to_string(),
                external_id: Some("PID123".to_string()),
                display_name: None,
                metadata: json!({"source": "fixture"}),
            })
            .await
            .unwrap();
        let same_participant_id = store
            .upsert_participant(ParticipantRecord {
                participant_kind: "human".to_string(),
                identity_provider: "prolific".to_string(),
                external_id: Some("PID123".to_string()),
                display_name: Some("Ignored on reuse".to_string()),
                metadata: Value::Null,
            })
            .await
            .unwrap();
        assert_eq!(participant_id, same_participant_id);

        let session_one = store
            .create_session(SessionRecord {
                experiment_id: "exp_eval".to_string(),
                room_id: "ROOM1".to_string(),
                mode: "direct".to_string(),
                status: "waiting".to_string(),
            })
            .await
            .unwrap();
        let session_two = store
            .create_session(SessionRecord {
                experiment_id: "exp_eval".to_string(),
                room_id: "ROOM2".to_string(),
                mode: "direct".to_string(),
                status: "waiting".to_string(),
            })
            .await
            .unwrap();
        assert_eq!((session_one, session_two), (1, 2));

        store
            .add_session_participant(SessionParticipantRecord {
                experiment_id: "exp_eval".to_string(),
                session_id: session_one,
                participant_id,
                participant_session_id: "ps_1".to_string(),
                role: "A".to_string(),
                connection_status: "joined".to_string(),
            })
            .await
            .unwrap();
        store
            .record_consent_declaration(ConsentDeclarationRecord {
                experiment_id: "exp_eval".to_string(),
                session_id: Some(session_one),
                participant_id,
                consent_item_id: "study".to_string(),
                accepted: true,
                consent_text_hash: None,
                metadata: Value::Null,
            })
            .await
            .unwrap();
        let first_event = store
            .append_session_event(SessionEventRecord {
                experiment_id: "exp_eval".to_string(),
                session_id: session_one,
                event_type: "game_action_accepted".to_string(),
                actor_participant_id: Some(participant_id),
                actor_role: Some("A".to_string()),
                payload: json!({"action": "noop"}),
                game_state: Some(json!({"step": 1})),
            })
            .await
            .unwrap();
        let second_event = store
            .append_session_event(SessionEventRecord {
                experiment_id: "exp_eval".to_string(),
                session_id: session_one,
                event_type: "state_changed".to_string(),
                actor_participant_id: None,
                actor_role: None,
                payload: json!({"reason": "test"}),
                game_state: Some(json!({"step": 2})),
            })
            .await
            .unwrap();
        assert_eq!((first_event, second_event), (1, 2));

        let exported = store.export_session("exp_eval", session_one).await.unwrap();
        assert_eq!(exported["sessions"].as_array().unwrap().len(), 1);
        assert_eq!(exported["session_events"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn sqlite_allows_returning_participant_in_multiple_sessions_with_different_roles() {
        let store = SqliteExperimentStore::connect("sqlite:///:memory:")
            .await
            .unwrap();
        store
            .ensure_experiment(ExperimentRecord {
                experiment_id: "exp_returning".to_string(),
                config: json!({"condition": "repeat-play"}),
                server_version: None,
                version_manifest: None,
                status: "active".to_string(),
                notes: Some("same Prolific participant appears twice".to_string()),
            })
            .await
            .unwrap();
        let participant_id = store
            .upsert_participant(ParticipantRecord {
                participant_kind: "human".to_string(),
                identity_provider: "prolific".to_string(),
                external_id: Some("PROLIFIC-REPEAT".to_string()),
                display_name: None,
                metadata: json!({"first_seen_batch": 7}),
            })
            .await
            .unwrap();
        let first_session = store
            .create_session(SessionRecord {
                experiment_id: "exp_returning".to_string(),
                room_id: "ROOM_A".to_string(),
                mode: "human_vs_human".to_string(),
                status: "playing".to_string(),
            })
            .await
            .unwrap();
        let second_session = store
            .create_session(SessionRecord {
                experiment_id: "exp_returning".to_string(),
                room_id: "ROOM_B".to_string(),
                mode: "role_swap_replay".to_string(),
                status: "playing".to_string(),
            })
            .await
            .unwrap();

        store
            .add_session_participant(SessionParticipantRecord {
                experiment_id: "exp_returning".to_string(),
                session_id: first_session,
                participant_id,
                participant_session_id: "ps_repeat_a".to_string(),
                role: "A".to_string(),
                connection_status: "connected".to_string(),
            })
            .await
            .unwrap();
        store
            .add_session_participant(SessionParticipantRecord {
                experiment_id: "exp_returning".to_string(),
                session_id: second_session,
                participant_id,
                participant_session_id: "ps_repeat_b".to_string(),
                role: "B".to_string(),
                connection_status: "connected".to_string(),
            })
            .await
            .unwrap();

        let experiment = store.export_experiment("exp_returning").await.unwrap();
        assert_eq!(experiment["participants"].as_array().unwrap().len(), 1);
        assert_eq!(
            experiment["session_participants"].as_array().unwrap().len(),
            2
        );
        assert!(experiment["participants"][0].get("role").is_none());
        assert_eq!(experiment["session_participants"][0]["role"], "A");
        assert_eq!(experiment["session_participants"][1]["role"], "B");

        let second = store
            .export_session("exp_returning", second_session)
            .await
            .unwrap();
        assert_eq!(second["participants"].as_array().unwrap().len(), 1);
        assert_eq!(
            second["session_participants"][0]["participant_session_id"],
            "ps_repeat_b"
        );
    }

    #[tokio::test]
    async fn sqlite_holds_mixed_participant_and_event_shapes_for_weird_experiments() {
        let store = SqliteExperimentStore::connect("sqlite:///:memory:")
            .await
            .unwrap();
        store
            .ensure_experiment(ExperimentRecord {
                experiment_id: "exp_weird".to_string(),
                config: json!({
                    "conditions": ["voice", "agent", "worker"],
                    "nested": {"levels": [{"id": 1}, {"id": 2}]}
                }),
                server_version: Some("test".to_string()),
                version_manifest: None,
                status: "active".to_string(),
                notes: None,
            })
            .await
            .unwrap();

        let direct_one = store
            .upsert_participant(ParticipantRecord {
                participant_kind: "human".to_string(),
                identity_provider: "direct".to_string(),
                external_id: None,
                display_name: Some("Casey".to_string()),
                metadata: json!({"typed_name": true}),
            })
            .await
            .unwrap();
        let direct_two = store
            .upsert_participant(ParticipantRecord {
                participant_kind: "human".to_string(),
                identity_provider: "direct".to_string(),
                external_id: None,
                display_name: Some("Casey".to_string()),
                metadata: json!({"typed_name": true, "second_signup": true}),
            })
            .await
            .unwrap();
        assert_ne!(direct_one, direct_two);

        let prolific = store
            .upsert_participant(ParticipantRecord {
                participant_kind: "human".to_string(),
                identity_provider: "prolific".to_string(),
                external_id: Some("PID-WEIRD".to_string()),
                display_name: None,
                metadata: json!({"study_id": "STUDY42", "session_id": "SESSION99"}),
            })
            .await
            .unwrap();
        let agent = store
            .upsert_participant(ParticipantRecord {
                participant_kind: "agent".to_string(),
                identity_provider: "agent".to_string(),
                external_id: Some("back-and-forth@v2".to_string()),
                display_name: Some("Back And Forth".to_string()),
                metadata: json!({"temperature": 0, "seed": 1234}),
            })
            .await
            .unwrap();
        let worker = store
            .upsert_participant(ParticipantRecord {
                participant_kind: "worker".to_string(),
                identity_provider: "worker".to_string(),
                external_id: Some("transcriber-1".to_string()),
                display_name: None,
                metadata: json!({"provider": "speechmatics"}),
            })
            .await
            .unwrap();
        let session_id = store
            .create_session(SessionRecord {
                experiment_id: "exp_weird".to_string(),
                room_id: "ROOM_WEIRD".to_string(),
                mode: "human_agent_with_worker".to_string(),
                status: "playing".to_string(),
            })
            .await
            .unwrap();

        for (participant_id, handle, role) in [
            (prolific, "ps_prolific", "A"),
            (agent, "ps_agent", "B"),
            (worker, "worker-transcriber-1", "worker"),
        ] {
            store
                .add_session_participant(SessionParticipantRecord {
                    experiment_id: "exp_weird".to_string(),
                    session_id,
                    participant_id,
                    participant_session_id: handle.to_string(),
                    role: role.to_string(),
                    connection_status: "connected".to_string(),
                })
                .await
                .unwrap();
        }

        store
            .record_consent_declaration(ConsentDeclarationRecord {
                experiment_id: "exp_weird".to_string(),
                session_id: None,
                participant_id: prolific,
                consent_item_id: "screening".to_string(),
                accepted: true,
                consent_text_hash: Some("hash-screening".to_string()),
                metadata: json!({"before_matchmaking": true}),
            })
            .await
            .unwrap();
        store
            .record_consent_declaration(ConsentDeclarationRecord {
                experiment_id: "exp_weird".to_string(),
                session_id: Some(session_id),
                participant_id: prolific,
                consent_item_id: "record_audio".to_string(),
                accepted: false,
                consent_text_hash: Some("hash-audio".to_string()),
                metadata: json!({"reason": "declined_optional"}),
            })
            .await
            .unwrap();

        let event_payloads = [
            (
                "transcript_segment",
                Some(prolific),
                Some("A"),
                json!({"text": "hello there", "alternatives": [], "confidence": 0.91}),
                None,
            ),
            (
                "game_action_accepted",
                Some(agent),
                Some("B"),
                json!({"action": {"type": "toggle", "target": "aux"}, "events": [{"kind": "system_on"}]}),
                Some(json!({"systems": {"aux": true}, "history": [{"actor": "B"}]})),
            ),
            (
                "voice_diagnostic",
                Some(worker),
                Some("worker"),
                json!({"event": "temporary_key_minted", "latency_ms": 42}),
                None,
            ),
        ];
        for (event_type, actor_participant_id, actor_role, payload, game_state) in event_payloads {
            store
                .append_session_event(SessionEventRecord {
                    experiment_id: "exp_weird".to_string(),
                    session_id,
                    event_type: event_type.to_string(),
                    actor_participant_id,
                    actor_role: actor_role.map(str::to_string),
                    payload,
                    game_state,
                })
                .await
                .unwrap();
        }

        let exported = store.export_session("exp_weird", session_id).await.unwrap();
        assert_eq!(exported["participants"].as_array().unwrap().len(), 3);
        assert_eq!(
            exported["session_participants"].as_array().unwrap().len(),
            3
        );
        assert_eq!(
            exported["consent_declarations"].as_array().unwrap().len(),
            1,
            "session export should include only session-scoped consent"
        );
        assert_eq!(exported["session_events"].as_array().unwrap().len(), 3);
        assert_eq!(exported["session_events"][0]["event_index"], 1);
        assert_eq!(exported["session_events"][1]["event_index"], 2);
        assert_eq!(
            exported["session_events"][1]["game_state"]["systems"]["aux"],
            true
        );

        let experiment = store.export_experiment("exp_weird").await.unwrap();
        assert_eq!(
            experiment["consent_declarations"].as_array().unwrap().len(),
            2
        );
    }

    #[tokio::test]
    async fn unsupported_database_scheme_fails_clearly() {
        let error = match experiment_store_from_url("postgres://localhost/parlando").await {
            Ok(_) => panic!("unsupported scheme should fail"),
            Err(error) => error,
        };

        assert!(error
            .to_string()
            .contains("unsupported database url scheme"));
    }

    #[tokio::test]
    async fn empty_database_url_is_rejected() {
        let error = match experiment_store_from_url("").await {
            Ok(_) => panic!("empty database url should fail"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("database.url is required"));
    }
}
