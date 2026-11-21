//! SQLite experiment store and stable storage record types.
//!
//! Runtime, authentication, and administrator operations share one SQLite pool. The child
//! modules own the SQL and transaction boundary for each operation. Session transitions
//! commit ordered events and any terminal result together. Queries and export expose
//! committed records to the dashboard and corpus projector.

mod export;
pub(crate) use export::{ExperimentSnapshot, ExportSession};

mod admin_auth;
mod events;
mod experiments;
mod game_settings;
mod participant_deletion;
mod participants;
mod schema;
mod session_queries;
mod sessions;

pub use admin_auth::{StoredAdminCredential, StoredAdminSession};
#[cfg(test)]
pub(crate) use events::TransitionAckFault;
pub use events::{SessionEventRecord, StoredSessionEvent};
pub(crate) use events::{TransitionCommit, TransitionCommitUnknown};
pub use experiments::ExperimentRecord;
pub(crate) use game_settings::GameSettingsUpdate;
pub use game_settings::StoredGameSettings;
pub use participants::{ConsentDeclarationRecord, ParticipantRecord, ProlificSubmissionRecord};
pub use sessions::{SessionParticipantRecord, SessionRecord};

use crate::identity::new_id;
use anyhow::{anyhow, bail, Result};
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::SqlitePool;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// Returns the current UTC timestamp in ISO-8601/RFC3339 form.
pub fn now_iso() -> String {
    Utc::now().to_rfc3339()
}

/// Converts one RFC3339 wall-clock timestamp to Unix milliseconds.
fn timestamp_millis(timestamp: &str) -> Result<i64> {
    Ok(DateTime::parse_from_rfc3339(timestamp)?.timestamp_millis())
}

/// Computes a timestamp's position on a session's authoritative game clock.
fn relative_game_time_ms(game_started_at: &str, timestamp: &str) -> Result<i64> {
    timestamp_millis(timestamp)?
        .checked_sub(timestamp_millis(game_started_at)?)
        .ok_or_else(|| anyhow!("game-clock subtraction overflowed"))
}

/// Adds seconds to one RFC3339 timestamp without using a client clock.
fn deadline_after_seconds(timestamp: &str, seconds: i64) -> Result<String> {
    Ok(
        (DateTime::parse_from_rfc3339(timestamp)? + chrono::Duration::seconds(seconds))
            .to_rfc3339(),
    )
}

/// Generates a startup experiment id when neither CLI nor YAML provided one.
pub fn generated_experiment_id() -> String {
    new_id("exp")
}

/// Filesystem and database size information used to stop only new session admission.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct StorageCapacity {
    /// Total bytes on the filesystem containing SQLite.
    pub total_bytes: u64,
    /// Bytes currently available to the server process.
    pub available_bytes: u64,
    /// Current main SQLite database file size.
    pub database_bytes: u64,
    /// Current SQLite write-ahead log size, which can be substantial under load.
    pub wal_bytes: u64,
    /// Current SQLite shared-memory sidecar size.
    pub shm_bytes: u64,
    /// Total current SQLite footprint including the main file and sidecars.
    pub database_total_bytes: u64,
}

/// Shared handle for the one supported SQLite experiment store.
pub type SharedExperimentStore = Arc<SqliteExperimentStore>;

/// Returns whether SQLite names the expected unique column in its constraint error.
fn sqlite_unique_constraint_for(error: &sqlx::Error, column: &str) -> bool {
    error.as_database_error().is_some_and(|database_error| {
        database_error.is_unique_violation() && database_error.message().contains(column)
    })
}

/// SQLite implementation of the evaluation-oriented experiment store.
pub struct SqliteExperimentStore {
    pool: SqlitePool,
    database_path: Option<PathBuf>,
    #[cfg(test)]
    pub(crate) transition_ack_fault: std::sync::Mutex<Option<events::TransitionAckFault>>,
    #[cfg(test)]
    pub(crate) fail_transition_probe_once: std::sync::atomic::AtomicBool,
}

impl SqliteExperimentStore {
    /// Opens a SQLite-backed experiment store and creates the schema when needed.
    pub async fn connect(database_url: &str) -> Result<Self> {
        if database_url.is_empty() || database_url == "sqlite:///:memory:" {
            let pool = SqlitePoolOptions::new()
                .max_connections(1)
                .connect("sqlite::memory:")
                .await?;
            let store = Self {
                pool,
                database_path: None,
                #[cfg(test)]
                transition_ack_fault: Default::default(),
                #[cfg(test)]
                fail_transition_probe_once: Default::default(),
            };
            store.initialize_or_validate_schema().await?;
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
        let store = Self {
            pool,
            database_path: Some(PathBuf::from(path)),
            #[cfg(test)]
            transition_ack_fault: Default::default(),
            #[cfg(test)]
            fail_transition_probe_once: Default::default(),
        };
        store.initialize_or_validate_schema().await?;
        Ok(store)
    }
}

impl SqliteExperimentStore {
    /// Verifies that SQLite can acquire a write transaction.
    pub(crate) async fn health_check(&self) -> Result<()> {
        let mut connection = self.pool.acquire().await?;
        sqlx::query("begin immediate")
            .execute(&mut *connection)
            .await?;
        let check = sqlx::query_scalar::<_, i64>("select 1")
            .fetch_one(&mut *connection)
            .await;
        let rollback = sqlx::query("rollback").execute(&mut *connection).await;
        check?;
        rollback?;
        Ok(())
    }

    /// Measures database files and available space on their filesystem.
    pub(crate) async fn storage_capacity(&self) -> Result<Option<StorageCapacity>> {
        let Some(path) = self.database_path.clone() else {
            return Ok(None);
        };
        tokio::task::spawn_blocking(move || {
            let filesystem_path = path.parent().unwrap_or(Path::new("."));
            let database_bytes = std::fs::metadata(&path)
                .map(|metadata| metadata.len())
                .unwrap_or(0);
            let wal_bytes = std::fs::metadata(format!("{}-wal", path.display()))
                .map(|metadata| metadata.len())
                .unwrap_or(0);
            let shm_bytes = std::fs::metadata(format!("{}-shm", path.display()))
                .map(|metadata| metadata.len())
                .unwrap_or(0);
            Ok(Some(StorageCapacity {
                total_bytes: fs2::total_space(filesystem_path)?,
                available_bytes: fs2::available_space(filesystem_path)?,
                database_bytes,
                wal_bytes,
                shm_bytes,
                database_total_bytes: database_bytes
                    .saturating_add(wal_bytes)
                    .saturating_add(shm_bytes),
            }))
        })
        .await?
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

#[cfg(test)]
mod tests;
