use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use crate::{
    game::{Game, Seat},
    identity::new_id,
    protocol::{ParticipantResult, SessionEnd},
    readable_id::{dialogue_id, participant_id as readable_participant_id},
    session_log::SessionLogWriter,
};
use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
    Row, SqlitePool,
};

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
    /// Exact semantic version of the compiled game which owns this experiment.
    pub game_version: String,
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
    /// Exact semantic version required for activation.
    pub game_version: String,
    pub created_at: String,
    pub status: String,
    pub server_version: Option<String>,
    pub version_manifest: Option<Value>,
    pub notes: Option<String>,
    /// Whether the experiment should sort ahead of ordinary inactive experiments.
    pub pinned: bool,
    /// Immutable configuration revision currently selected by the experiment.
    pub config_revision: i64,
    pub session_count: i64,
    pub completed_session_count: i64,
    pub last_session_at: Option<String>,
}

/// Complete durable experiment definition used to construct an in-process runtime.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StoredExperimentDefinition {
    /// Stable experiment identifier used in participant routes and stored data.
    pub experiment_id: String,
    /// Exact game version required for activation.
    pub game_version: String,
    /// Current normalized, secret-free experiment configuration.
    pub config: Value,
    /// Current immutable configuration revision number.
    pub config_revision: i64,
    /// Current participant-availability lifecycle.
    pub status: String,
    /// Optional researcher-authored catalogue notes.
    pub notes: Option<String>,
    /// Whether this experiment is pinned in the dashboard.
    pub pinned: bool,
}

/// One immutable stored configuration revision.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StoredExperimentRevision {
    /// Stable experiment identifier owning the revision.
    pub experiment_id: String,
    /// Monotonically increasing experiment-local revision number.
    pub revision: i64,
    /// Normalized, secret-free configuration JSON.
    pub config: Value,
    /// UTC creation time.
    pub created_at: String,
    /// Optional administrator-supplied summary of the change.
    pub change_summary: Option<String>,
}

/// Settings shared by every experiment hosted by one compiled game process.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StoredGameSettings {
    /// Institution displayed by every experiment of this game process.
    pub institution: String,
    /// Direct-peer CIDR ranges allowed to access administrator surfaces.
    pub admin_allowed_ip_ranges: Vec<String>,
    /// Speechmatics endpoint default copied into newly created experiments.
    pub speechmatics_realtime_url: String,
    /// Default ElevenLabs WebSocket service origin copied into new experiments.
    pub tts_base_url: String,
    /// Prolific API and study-JWKS origin used by every experiment in this game process.
    pub prolific_api_base_url: String,
    /// Prolific workspace bound to this installation's protected API token.
    pub prolific_workspace_id: String,
    /// Last provider-verified workspace title, used only for administrator display.
    pub prolific_workspace_title: String,
    /// Optimistic-concurrency revision for dashboard updates.
    pub revision: i64,
}

impl Default for StoredGameSettings {
    /// Supplies safe installation defaults before durable settings are loaded.
    fn default() -> Self {
        Self {
            institution: String::new(),
            admin_allowed_ip_ranges: Vec::new(),
            speechmatics_realtime_url: "wss://eu.rt.speechmatics.com/v2".to_string(),
            tts_base_url: "wss://api.elevenlabs.io".to_string(),
            prolific_api_base_url: "https://api.prolific.com".to_string(),
            prolific_workspace_id: String::new(),
            prolific_workspace_title: String::new(),
            revision: 1,
        }
    }
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

/// Verified Prolific launch facts used to create or resume one durable admission.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ProlificSubmissionRecord {
    pub experiment_id: String,
    pub prolific_participant_id: String,
    pub prolific_study_id: String,
    pub prolific_session_id: String,
    /// `signed_url` or `submission_api`, recorded without retaining a launch token.
    pub verification_method: String,
}

/// Stable provider-neutral admission returned for one verified Prolific submission.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ProlificAdmission {
    pub participant_id: i64,
    pub research_id: String,
    pub participant_session_id: String,
}

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

/// Input for one item-level consent declaration.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ConsentDeclarationRecord {
    pub experiment_id: String,
    pub session_id: Option<i64>,
    pub participant_id: i64,
    /// Immutable data-use classification inherited from participant intake.
    pub purpose: String,
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
    /// Milliseconds relative to the instant at which this game entered `running`.
    pub game_time_ms: i64,
}

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
    /// Dashboard session identifiers that will be deleted with the participant.
    pub session_ids: Vec<String>,
    /// Other dashboard participant identifiers whose shared sessions will also disappear.
    pub other_participant_ids: Vec<String>,
    /// Whether at least one affected session can still receive runtime writes.
    pub has_non_terminal_session: bool,
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

/// Durable server-side administrator session identified by a hash of its browser token.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredAdminSession {
    /// Unkeyed SHA-256 digest of a 256-bit random token; the bearer token is never stored.
    pub token_digest: String,
    /// Stable authorization role captured when the session was issued.
    pub role: String,
    /// Random synchronizer token required on state-changing requests.
    pub csrf_token: String,
    /// Unix timestamp at which the administrator authenticated.
    pub created_at: i64,
    /// Unix timestamp of the latest periodically persisted authenticated use.
    pub last_seen_at: i64,
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

/// Backend-neutral storage interface centered on experiment evaluation.
#[async_trait]
pub trait ExperimentStore: Send + Sync {
    /// Confirms the SQLite store can acquire a write transaction and execute a read.
    async fn health_check(&self) -> Result<()>;
    /// Reports disk capacity for file-backed SQLite, or `None` for in-memory tests.
    async fn storage_capacity(&self) -> Result<Option<StorageCapacity>>;
    /// Loads the singleton administrator credential, if initial setup is complete.
    async fn admin_credential(&self) -> Result<Option<StoredAdminCredential>>;
    /// Atomically creates the singleton administrator credential.
    ///
    /// Returns `false` when another request or process completed setup first.
    async fn create_admin_credential(&self, credential: StoredAdminCredential) -> Result<bool>;
    /// Persists a newly authenticated administrator session without storing its bearer token.
    async fn save_admin_session(&self, session: StoredAdminSession) -> Result<()>;
    /// Loads one administrator session by its bearer-token digest.
    async fn admin_session(&self, token_digest: &str) -> Result<Option<StoredAdminSession>>;
    /// Advances the durable idle timestamp for one administrator session.
    async fn touch_admin_session(&self, token_digest: &str, last_seen_at: i64) -> Result<()>;
    /// Revokes one administrator session by its bearer-token digest.
    async fn delete_admin_session(&self, token_digest: &str) -> Result<()>;
    /// Removes administrator sessions outside either the idle or absolute lifetime.
    async fn delete_expired_admin_sessions(
        &self,
        idle_before: i64,
        absolute_before: i64,
    ) -> Result<()>;
    /// Ensures the bootstrap experiment exists without changing its durable lifecycle.
    async fn ensure_experiment(&self, experiment: ExperimentRecord) -> Result<String>;
    /// Lists all experiments owned by the compiled game process.
    async fn list_experiments(&self, limit: i64) -> Result<Vec<StoredExperimentSummary>>;
    /// Loads the complete current definition needed to construct a runtime.
    async fn experiment_definition(
        &self,
        experiment_id: &str,
    ) -> Result<Option<StoredExperimentDefinition>>;
    /// Creates a new inactive experiment and its immutable first configuration revision.
    async fn create_experiment(&self, experiment: ExperimentRecord) -> Result<()>;
    /// Loads write-only experiment credentials for runtime construction.
    async fn experiment_secrets(&self, experiment_id: &str) -> Result<HashMap<String, String>>;
    /// Atomically saves a configuration revision and its independent secret changes.
    async fn save_experiment_configuration(
        &self,
        experiment_id: &str,
        expected_revision: i64,
        config: Value,
        change_summary: Option<String>,
        secret_updates: HashMap<String, String>,
        secret_deletions: Vec<String>,
    ) -> Result<i64>;
    /// Lists immutable configuration revisions newest first.
    async fn experiment_revisions(
        &self,
        experiment_id: &str,
    ) -> Result<Vec<StoredExperimentRevision>>;
    /// Updates researcher-facing catalogue metadata independently of lifecycle.
    async fn update_experiment_catalogue(
        &self,
        experiment_id: &str,
        pinned: bool,
        notes: Option<String>,
    ) -> Result<()>;
    /// Loads settings shared by all experiments in this game process.
    async fn game_settings(&self) -> Result<StoredGameSettings>;
    /// Loads provider credentials shared by every experiment in this game process.
    async fn game_secrets(&self) -> Result<HashMap<String, String>>;
    /// Updates shared game settings when the caller edited the current revision.
    async fn update_game_settings(
        &self,
        expected_revision: i64,
        institution: String,
        admin_allowed_ip_ranges: Vec<String>,
        speechmatics_realtime_url: String,
        tts_base_url: String,
        prolific_api_base_url: String,
        prolific_workspace_id: String,
        prolific_workspace_title: String,
        secret_updates: HashMap<String, String>,
        secret_deletions: Vec<String>,
    ) -> Result<i64>;
    /// Returns one experiment with compact session aggregates.
    async fn experiment_summary(
        &self,
        experiment_id: &str,
    ) -> Result<Option<StoredExperimentSummary>>;
    /// Updates the lifecycle status for one experiment row.
    async fn update_experiment_status(&self, experiment_id: &str, status: &str) -> Result<()>;
    /// Archives an inactive or completed experiment without loading its runtime configuration.
    ///
    /// This one-way catalogue operation deliberately cannot restore an experiment and rejects
    /// open intake or an already archived row. It is intended for legacy configurations that
    /// the current game binary can no longer parse.
    async fn archive_experiment(&self, experiment_id: &str) -> Result<()>;
    /// Closes all intake that was open before the current game-process startup.
    async fn deactivate_open_experiments(&self) -> Result<u64>;
    /// Creates or reuses a durable participant identity and returns `participant_id`.
    async fn upsert_participant(&self, participant: ParticipantRecord) -> Result<i64>;
    /// Atomically creates or resumes one admission for a verified Prolific submission.
    async fn admit_prolific_submission(
        &self,
        submission: ProlificSubmissionRecord,
    ) -> Result<ProlificAdmission>;
    /// Stores the narrow, payment-free provider reconciliation projection.
    async fn reconcile_prolific_submission(
        &self,
        experiment_id: &str,
        prolific_session_id: &str,
        status: &str,
        entered_code: Option<String>,
        return_requested_at: Option<String>,
    ) -> Result<()>;
    /// Returns the human-readable experiment-specific identifier for a durable participant.
    async fn participant_research_id(&self, participant_id: i64) -> Result<Option<String>>;
    /// Creates a session for a client-facing session id and returns its per-experiment `session_id`.
    async fn create_session(&self, session: SessionRecord) -> Result<i64>;
    /// Reads the immutable phase clocks created with a session.
    async fn session_timing(&self, experiment_id: &str, session_id: i64) -> Result<SessionTiming>;
    /// Moves one successfully constructed session from `initializing` to `waiting`.
    async fn complete_session_initialization(
        &self,
        experiment_id: &str,
        session_id: i64,
    ) -> Result<bool>;
    /// Terminates initialization and appends a bounded failure event atomically.
    async fn fail_session_initialization(
        &self,
        experiment_id: &str,
        session_id: i64,
        reason_code: &str,
    ) -> Result<()>;
    /// Atomically moves one waiting session to running and records its first start time.
    async fn start_session(
        &self,
        experiment_id: &str,
        session_id: i64,
        started_at: &str,
        idle_deadline_at: &str,
    ) -> Result<bool>;
    /// Persists accepted participant activity and advances the fixed idle deadline.
    async fn touch_session_activity(
        &self,
        experiment_id: &str,
        session_id: i64,
        activity_at: &str,
        idle_deadline_at: &str,
    ) -> Result<()>;
    /// Maps an RFC3339 timestamp onto one running session's authoritative game clock.
    async fn session_game_time_ms(
        &self,
        experiment_id: &str,
        session_id: i64,
        timestamp: &str,
    ) -> Result<i64>;
    /// Adds a participant to a session with a session-local role.
    async fn add_session_participant(&self, participant: SessionParticipantRecord) -> Result<()>;
    /// Updates connection status for a participant's session appearance.
    async fn update_session_participant_connection(
        &self,
        participant_session_id: &str,
        connection_status: &str,
        left_at: Option<String>,
        reconnect_deadline_at: Option<String>,
    ) -> Result<()>;
    /// Records one item-level consent declaration.
    async fn record_consent_declaration(&self, declaration: ConsentDeclarationRecord)
        -> Result<()>;
    /// Appends one ordered session event and returns its event index.
    async fn append_session_event(&self, event: SessionEventRecord) -> Result<i64>;
    /// Atomically appends one game transition and optionally commits its terminal value.
    async fn commit_session_transition(
        &self,
        events: Vec<SessionEventRecord>,
        session_end: Option<SessionEnd>,
    ) -> Result<bool>;
    /// Atomically commits one non-game terminal transition and its participant results.
    async fn end_session(&self, event: SessionEventRecord, session_end: SessionEnd)
        -> Result<bool>;
    /// Reads a retained terminal result by the authenticated participant-session handle.
    async fn terminal_participant_state(
        &self,
        experiment_id: &str,
        participant_session_id: &str,
    ) -> Result<Option<StoredTerminalParticipantState>>;
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
    /// Physically removes a participant and every session in which they appeared.
    async fn delete_participant_data(
        &self,
        experiment_id: &str,
        participant_id: i64,
    ) -> Result<ParticipantDataPreview>;
}

/// Shared trait-object handle for the configured experiment store backend.
pub type SharedExperimentStore = Arc<dyn ExperimentStore>;

/// Returns whether SQLite names the expected unique column in its constraint error.
fn sqlite_unique_constraint_for(error: &sqlx::Error, column: &str) -> bool {
    error.as_database_error().is_some_and(|database_error| {
        database_error.is_unique_violation() && database_error.message().contains(column)
    })
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

/// SQLite implementation of the evaluation-oriented experiment store.
pub struct SqliteExperimentStore {
    pool: SqlitePool,
    database_path: Option<PathBuf>,
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
            };
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
        let store = Self {
            pool,
            database_path: Some(PathBuf::from(path)),
        };
        store.ensure_schema().await?;
        Ok(store)
    }

    /// Creates the relational evaluation schema.
    async fn ensure_schema(&self) -> Result<()> {
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
            create table if not exists administrator_sessions (
                token_digest text primary key,
                role text not null,
                csrf_token text not null,
                created_at integer not null,
                last_seen_at integer not null
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
                game_version text not null,
                config_json text not null,
                config_revision integer not null default 1,
                server_version text,
                version_manifest_json text,
                status text not null default 'inactive',
                notes text,
                pinned integer not null default 0
            )
            "#,
            r#"
            create table if not exists experiment_config_revisions (
                experiment_id text not null,
                revision integer not null,
                config_json text not null,
                created_at text not null,
                change_summary text,
                primary key (experiment_id, revision),
                foreign key (experiment_id) references experiments(experiment_id)
            )
            "#,
            r#"
            create table if not exists experiment_secrets (
                experiment_id text not null,
                secret_key text not null,
                secret_value text not null,
                updated_at text not null,
                primary key (experiment_id, secret_key),
                foreign key (experiment_id) references experiments(experiment_id)
            )
            "#,
            r#"
            create table if not exists game_settings (
                singleton integer primary key check (singleton = 1),
                institution text not null default '',
                admin_allowed_ip_ranges_json text not null default '[]',
                speechmatics_realtime_url text not null default 'wss://eu.rt.speechmatics.com/v2',
                tts_base_url text not null default 'wss://api.elevenlabs.io',
                prolific_api_base_url text not null default 'https://api.prolific.com',
                prolific_workspace_id text not null default '',
                prolific_workspace_title text not null default '',
                revision integer not null default 1,
                updated_at text not null
            )
            "#,
            r#"
            create table if not exists game_secrets (
                secret_key text primary key,
                secret_value text not null,
                updated_at text not null
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
                public_session_id text not null unique,
                dialogue_id text unique,
                mode text not null,
                lifecycle text not null,
                initialization_complete integer not null default 0,
                purpose text not null default 'research',
                created_at text not null,
                waiting_started_at text not null,
                waiting_deadline_at text not null,
                lifetime_deadline_at text not null,
                started_at text,
                last_meaningful_activity_at text,
                idle_deadline_at text,
                ended_at text,
                completion_json text,
                session_end_json text,
                config_revision integer not null default 1,
                game_version text not null,
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
                disconnected_at text,
                reconnect_deadline_at text,
                terminal_result_json text,
                primary key (experiment_id, session_id, participant_id),
                foreign key (experiment_id, session_id) references sessions(experiment_id, session_id),
                foreign key (participant_id) references participants(participant_id)
            )
            "#,
            r#"
            create table if not exists prolific_submissions (
                prolific_submission_id integer primary key autoincrement,
                experiment_id text not null,
                participant_id integer not null,
                prolific_participant_id text not null,
                prolific_study_id text not null,
                prolific_session_id text not null,
                participant_session_id text not null unique,
                received_at text not null,
                verification_method text not null,
                verified_at text not null,
                completion_path_key text,
                completion_code_presented_at text,
                completion_link_opened_at text,
                provider_status text,
                entered_completion_code text,
                return_requested_at text,
                reconciled_at text,
                unique (experiment_id, prolific_session_id),
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
                purpose text not null default 'research',
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
                game_time_ms integer not null,
                unique (experiment_id, session_id, event_index),
                foreign key (experiment_id, session_id) references sessions(experiment_id, session_id),
                foreign key (actor_participant_id) references participants(participant_id)
            )
            "#,
            "create index if not exists idx_session_events_session on session_events(experiment_id, session_id)",
            "create index if not exists idx_session_events_session_type on session_events(experiment_id, session_id, event_type)",
            "create index if not exists idx_session_events_actor_game_time on session_events(actor_participant_id, game_time_ms)",
            "create unique index if not exists idx_participants_experiment_provider_external on participants(experiment_id, identity_provider, external_id) where external_id is not null",
            "create unique index if not exists idx_participants_research_id on participants(research_id) where research_id is not null",
            "create unique index if not exists idx_sessions_dialogue_id on sessions(dialogue_id) where dialogue_id is not null",
        ] {
            sqlx::query(statement).execute(&self.pool).await?;
        }
        self.apply_pending_migrations().await?;
        sqlx::query("insert or ignore into game_settings (singleton, updated_at) values (1, ?)")
            .bind(now_iso())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Accepts only the current schema baseline or stamps a genuinely empty database.
    async fn apply_pending_migrations(&self) -> Result<()> {
        const CURRENT_SCHEMA_VERSION: i64 = 15;
        let version =
            sqlx::query_scalar::<_, Option<i64>>("select max(version) from schema_migrations")
                .fetch_one(&self.pool)
                .await?
                .unwrap_or(0);
        if version == CURRENT_SCHEMA_VERSION {
            return Ok(());
        }
        if version != 0 {
            bail!(
                "database schema version {version} is unsupported; export it with a compatible older Parlando release and import it into a new database"
            );
        }
        let stored_rows = sqlx::query_scalar::<_, i64>(
            "select (select count(*) from experiments) + (select count(*) from participants) + (select count(*) from sessions)",
        )
        .fetch_one(&self.pool)
        .await?;
        if stored_rows != 0 {
            bail!(
                "a populated pre-baseline database is unsupported; export it with a compatible older Parlando release and import it into a new database"
            );
        }
        sqlx::query("insert into schema_migrations (version, applied_at) values (?, ?)")
            .bind(CURRENT_SCHEMA_VERSION)
            .bind(now_iso())
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
    async fn health_check(&self) -> Result<()> {
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

    async fn storage_capacity(&self) -> Result<Option<StorageCapacity>> {
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

    async fn save_admin_session(&self, session: StoredAdminSession) -> Result<()> {
        sqlx::query(
            r#"
            insert into administrator_sessions
                (token_digest, role, csrf_token, created_at, last_seen_at)
            values (?, ?, ?, ?, ?)
            "#,
        )
        .bind(session.token_digest)
        .bind(session.role)
        .bind(session.csrf_token)
        .bind(session.created_at)
        .bind(session.last_seen_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn admin_session(&self, token_digest: &str) -> Result<Option<StoredAdminSession>> {
        Ok(sqlx::query_as::<_, (String, String, String, i64, i64)>(
            r#"
            select token_digest, role, csrf_token, created_at, last_seen_at
            from administrator_sessions
            where token_digest = ?
            "#,
        )
        .bind(token_digest)
        .fetch_optional(&self.pool)
        .await?
        .map(
            |(token_digest, role, csrf_token, created_at, last_seen_at)| StoredAdminSession {
                token_digest,
                role,
                csrf_token,
                created_at,
                last_seen_at,
            },
        ))
    }

    async fn touch_admin_session(&self, token_digest: &str, last_seen_at: i64) -> Result<()> {
        sqlx::query("update administrator_sessions set last_seen_at = ? where token_digest = ?")
            .bind(last_seen_at)
            .bind(token_digest)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn delete_admin_session(&self, token_digest: &str) -> Result<()> {
        sqlx::query("delete from administrator_sessions where token_digest = ?")
            .bind(token_digest)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn delete_expired_admin_sessions(
        &self,
        idle_before: i64,
        absolute_before: i64,
    ) -> Result<()> {
        sqlx::query(
            "delete from administrator_sessions where last_seen_at <= ? or created_at <= ?",
        )
        .bind(idle_before)
        .bind(absolute_before)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn ensure_experiment(&self, experiment: ExperimentRecord) -> Result<String> {
        let config_json = serde_json::to_string(&experiment.config)?;
        let created_at = now_iso();
        let status = sqlx::query_scalar::<_, String>(
            r#"
            insert into experiments
                (experiment_id, created_at, game_version, config_json, config_revision,
                 server_version, version_manifest_json, status, notes)
            values (?, ?, ?, ?, 1, ?, ?, ?, ?)
            on conflict(experiment_id) do update set
                server_version = excluded.server_version,
                version_manifest_json = excluded.version_manifest_json
            returning status
            "#,
        )
        .bind(&experiment.experiment_id)
        .bind(&created_at)
        .bind(&experiment.game_version)
        .bind(&config_json)
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
        sqlx::query(
            r#"
            insert or ignore into experiment_config_revisions
                (experiment_id, revision, config_json, created_at, change_summary)
            values (?, 1, ?, ?, 'Initial configuration')
            "#,
        )
        .bind(experiment.experiment_id)
        .bind(config_json)
        .bind(created_at)
        .execute(&self.pool)
        .await?;
        Ok(status)
    }

    async fn list_experiments(&self, limit: i64) -> Result<Vec<StoredExperimentSummary>> {
        let experiment_ids = sqlx::query_scalar::<_, String>(
            r#"
            select experiment_id
            from experiments
            order by case when status = 'active' then 0 else 1 end,
                     case when status = 'archived' then 1 else 0 end,
                     pinned desc, created_at desc
            limit ?
            "#,
        )
        .bind(limit.clamp(1, 1_000))
        .fetch_all(&self.pool)
        .await?;
        let mut experiments = Vec::with_capacity(experiment_ids.len());
        for experiment_id in experiment_ids {
            if let Some(experiment) = self.experiment_summary(&experiment_id).await? {
                experiments.push(experiment);
            }
        }
        Ok(experiments)
    }

    async fn experiment_definition(
        &self,
        experiment_id: &str,
    ) -> Result<Option<StoredExperimentDefinition>> {
        let row = sqlx::query_as::<_, (String, String, String, i64, String, Option<String>, bool)>(
            r#"
            select experiment_id, game_version, config_json, config_revision, status,
                   notes, pinned
            from experiments where experiment_id = ?
            "#,
        )
        .bind(experiment_id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| {
            Ok(StoredExperimentDefinition {
                experiment_id: row.0,
                game_version: row.1,
                config: serde_json::from_str(&row.2)?,
                config_revision: row.3,
                status: row.4,
                notes: row.5,
                pinned: row.6,
            })
        })
        .transpose()
    }

    async fn create_experiment(&self, experiment: ExperimentRecord) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        let created_at = now_iso();
        let config_json = serde_json::to_string(&experiment.config)?;
        let version_manifest = experiment
            .version_manifest
            .map(|value| serde_json::to_string(&value))
            .transpose()?;
        sqlx::query(
            r#"
            insert into experiments
                (experiment_id, created_at, game_version, config_json, config_revision,
                 server_version, version_manifest_json, status, notes, pinned)
            values (?, ?, ?, ?, 1, ?, ?, 'inactive', ?, 0)
            "#,
        )
        .bind(&experiment.experiment_id)
        .bind(&created_at)
        .bind(&experiment.game_version)
        .bind(&config_json)
        .bind(experiment.server_version)
        .bind(version_manifest)
        .bind(experiment.notes)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            r#"
            insert into experiment_config_revisions
                (experiment_id, revision, config_json, created_at, change_summary)
            values (?, 1, ?, ?, 'Initial configuration')
            "#,
        )
        .bind(experiment.experiment_id)
        .bind(config_json)
        .bind(created_at)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn experiment_secrets(&self, experiment_id: &str) -> Result<HashMap<String, String>> {
        Ok(sqlx::query_as::<_, (String, String)>(
            "select secret_key, secret_value from experiment_secrets where experiment_id = ?",
        )
        .bind(experiment_id)
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .collect())
    }

    async fn save_experiment_configuration(
        &self,
        experiment_id: &str,
        expected_revision: i64,
        config: Value,
        change_summary: Option<String>,
        secret_updates: HashMap<String, String>,
        secret_deletions: Vec<String>,
    ) -> Result<i64> {
        let mut tx = self.pool.begin().await?;
        let next_revision = expected_revision + 1;
        let config_json = serde_json::to_string(&config)?;
        let result = sqlx::query(
            "update experiments set config_json = ?, config_revision = ? where experiment_id = ? and config_revision = ? and status = 'inactive'",
        )
        .bind(&config_json)
        .bind(next_revision)
        .bind(experiment_id)
        .bind(expected_revision)
        .execute(&mut *tx)
        .await?;
        if result.rows_affected() != 1 {
            bail!("experiment configuration changed concurrently or experiment is not inactive");
        }
        sqlx::query(
            "insert into experiment_config_revisions (experiment_id, revision, config_json, created_at, change_summary) values (?, ?, ?, ?, ?)",
        )
        .bind(experiment_id)
        .bind(next_revision)
        .bind(&config_json)
        .bind(now_iso())
        .bind(change_summary)
        .execute(&mut *tx)
        .await?;
        for (key, value) in secret_updates {
            sqlx::query(
                "insert into experiment_secrets (experiment_id, secret_key, secret_value, updated_at) values (?, ?, ?, ?) on conflict(experiment_id, secret_key) do update set secret_value = excluded.secret_value, updated_at = excluded.updated_at",
            )
            .bind(experiment_id)
            .bind(key)
            .bind(value)
            .bind(now_iso())
            .execute(&mut *tx)
            .await?;
        }
        for key in secret_deletions {
            sqlx::query(
                "delete from experiment_secrets where experiment_id = ? and secret_key = ?",
            )
            .bind(experiment_id)
            .bind(key)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(next_revision)
    }

    async fn experiment_revisions(
        &self,
        experiment_id: &str,
    ) -> Result<Vec<StoredExperimentRevision>> {
        let rows = sqlx::query_as::<_, (String, i64, String, String, Option<String>)>(
            r#"
            select experiment_id, revision, config_json, created_at, change_summary
            from experiment_config_revisions
            where experiment_id = ? order by revision desc
            "#,
        )
        .bind(experiment_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok(StoredExperimentRevision {
                    experiment_id: row.0,
                    revision: row.1,
                    config: serde_json::from_str(&row.2)?,
                    created_at: row.3,
                    change_summary: row.4,
                })
            })
            .collect()
    }

    async fn update_experiment_catalogue(
        &self,
        experiment_id: &str,
        pinned: bool,
        notes: Option<String>,
    ) -> Result<()> {
        let result =
            sqlx::query("update experiments set pinned = ?, notes = ? where experiment_id = ?")
                .bind(pinned)
                .bind(notes)
                .bind(experiment_id)
                .execute(&self.pool)
                .await?;
        if result.rows_affected() != 1 {
            bail!("experiment not found");
        }
        Ok(())
    }

    async fn game_settings(&self) -> Result<StoredGameSettings> {
        let row = sqlx::query_as::<_, (String, String, String, String, String, String, String, i64)>(
            "select institution, admin_allowed_ip_ranges_json, speechmatics_realtime_url, tts_base_url, prolific_api_base_url, prolific_workspace_id, prolific_workspace_title, revision from game_settings where singleton = 1",
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(StoredGameSettings {
            institution: row.0,
            admin_allowed_ip_ranges: serde_json::from_str(&row.1)?,
            speechmatics_realtime_url: row.2,
            tts_base_url: row.3,
            prolific_api_base_url: row.4,
            prolific_workspace_id: row.5,
            prolific_workspace_title: row.6,
            revision: row.7,
        })
    }

    async fn reconcile_prolific_submission(
        &self,
        experiment_id: &str,
        prolific_session_id: &str,
        status: &str,
        entered_code: Option<String>,
        return_requested_at: Option<String>,
    ) -> Result<()> {
        sqlx::query(
            "update prolific_submissions set provider_status = ?, entered_completion_code = ?, return_requested_at = ?, reconciled_at = ? where experiment_id = ? and prolific_session_id = ?",
        )
        .bind(status)
        .bind(entered_code)
        .bind(return_requested_at)
        .bind(now_iso())
        .bind(experiment_id)
        .bind(prolific_session_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn game_secrets(&self) -> Result<HashMap<String, String>> {
        Ok(sqlx::query_as::<_, (String, String)>(
            "select secret_key, secret_value from game_secrets",
        )
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .collect())
    }

    async fn update_game_settings(
        &self,
        expected_revision: i64,
        institution: String,
        admin_allowed_ip_ranges: Vec<String>,
        speechmatics_realtime_url: String,
        tts_base_url: String,
        prolific_api_base_url: String,
        prolific_workspace_id: String,
        prolific_workspace_title: String,
        secret_updates: HashMap<String, String>,
        secret_deletions: Vec<String>,
    ) -> Result<i64> {
        let mut tx = self.pool.begin().await?;
        let next_revision = expected_revision + 1;
        let result = sqlx::query(
            r#"
            update game_settings set institution = ?, admin_allowed_ip_ranges_json = ?, speechmatics_realtime_url = ?, tts_base_url = ?, prolific_api_base_url = ?, prolific_workspace_id = ?, prolific_workspace_title = ?, revision = ?, updated_at = ?
            where singleton = 1 and revision = ?
            "#,
        )
        .bind(institution.trim())
        .bind(serde_json::to_string(&admin_allowed_ip_ranges)?)
        .bind(speechmatics_realtime_url.trim())
        .bind(tts_base_url.trim())
        .bind(prolific_api_base_url.trim())
        .bind(prolific_workspace_id.trim())
        .bind(prolific_workspace_title.trim())
        .bind(next_revision)
        .bind(now_iso())
        .bind(expected_revision)
        .execute(&mut *tx)
        .await?;
        if result.rows_affected() != 1 {
            bail!("game settings changed concurrently");
        }
        for (key, value) in secret_updates {
            sqlx::query("insert into game_secrets (secret_key, secret_value, updated_at) values (?, ?, ?) on conflict(secret_key) do update set secret_value = excluded.secret_value, updated_at = excluded.updated_at")
                .bind(key)
                .bind(value)
                .bind(now_iso())
                .execute(&mut *tx)
                .await?;
        }
        for key in secret_deletions {
            sqlx::query("delete from game_secrets where secret_key = ?")
                .bind(key)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(next_revision)
    }

    async fn experiment_summary(
        &self,
        experiment_id: &str,
    ) -> Result<Option<StoredExperimentSummary>> {
        let row = sqlx::query_as::<
            _,
            (String, String, String, String, Option<String>, Option<String>, String,
             Option<String>, bool, i64, i64, i64, Option<String>),
        >(
            r#"
            select e.experiment_id, e.game_version, e.created_at, e.config_json, e.server_version,
                   e.version_manifest_json, e.status, e.notes, e.pinned,
                   e.config_revision,
                   count(s.session_id) as session_count,
                   sum(case when s.lifecycle = 'ended' then 1 else 0 end) as completed_session_count,
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
                game_version: row.1,
                created_at: row.2,
                server_version: row.4,
                version_manifest: row
                    .5
                    .map(|raw| serde_json::from_str::<Value>(&raw))
                    .transpose()?,
                status: row.6,
                notes: row.7,
                pinned: row.8,
                config_revision: row.9,
                session_count: row.10,
                completed_session_count: row.11,
                last_session_at: row.12,
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

    async fn archive_experiment(&self, experiment_id: &str) -> Result<()> {
        let result = sqlx::query(
            "update experiments set status = 'archived' where experiment_id = ? and status in ('inactive', 'completed')",
        )
        .bind(experiment_id)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 1 {
            return Ok(());
        }
        let status = sqlx::query_scalar::<_, String>(
            "select status from experiments where experiment_id = ?",
        )
        .bind(experiment_id)
        .fetch_optional(&self.pool)
        .await?;
        match status.as_deref() {
            None => Err(anyhow!("Experiment not found.")),
            Some("archived") => Err(anyhow!("Experiment is already archived.")),
            Some(status) => Err(anyhow!(
                "Experiment in {status} status must stop intake before archival."
            )),
        }
    }

    async fn deactivate_open_experiments(&self) -> Result<u64> {
        let result = sqlx::query(
            "update experiments set status = 'inactive' where status in ('active', 'testing')",
        )
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
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

    async fn admit_prolific_submission(
        &self,
        submission: ProlificSubmissionRecord,
    ) -> Result<ProlificAdmission> {
        let mut tx = self.pool.begin().await?;
        let existing = sqlx::query_as::<_, (i64, String, String, String, String)>(
            r#"
            select ps.participant_id, p.research_id, ps.participant_session_id,
                   ps.prolific_participant_id, ps.prolific_study_id
            from prolific_submissions ps
            join participants p on p.participant_id = ps.participant_id
            where ps.experiment_id = ? and ps.prolific_session_id = ?
            "#,
        )
        .bind(&submission.experiment_id)
        .bind(&submission.prolific_session_id)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some((participant_id, research_id, participant_session_id, participant, study)) =
            existing
        {
            if participant != submission.prolific_participant_id
                || study != submission.prolific_study_id
            {
                bail!("Prolific submission identifiers conflict with an existing admission");
            }
            tx.commit().await?;
            return Ok(ProlificAdmission {
                participant_id,
                research_id,
                participant_session_id,
            });
        }

        let participant_record = ParticipantRecord {
            experiment_id: submission.experiment_id.clone(),
            participant_kind: "human".to_string(),
            identity_provider: "prolific".to_string(),
            external_id: None,
            metadata: Value::Null,
        };
        let (participant_id, research_id) = {
            let mut identifier_attempt = 1;
            loop {
                let research_id =
                    participant_identifier_candidate(&participant_record, identifier_attempt);
                let result = sqlx::query(
                    r#"
                    insert into participants
                    (research_id, experiment_id, participant_kind, identity_provider, external_id, metadata_json, created_at)
                    values (?, ?, 'human', 'prolific', null, ?, ?)
                    "#,
                )
                .bind(&research_id)
                .bind(&submission.experiment_id)
                .bind(serde_json::to_string(&Value::Null)?)
                .bind(now_iso())
                .execute(&mut *tx)
                .await;
                match result {
                    Ok(result) => break (result.last_insert_rowid(), research_id),
                    Err(error)
                        if sqlite_unique_constraint_for(&error, "participants.research_id") =>
                    {
                        identifier_attempt += 1;
                    }
                    Err(error) => return Err(error.into()),
                }
            }
        };
        let participant_session_id = new_id("ps");
        let verified_at = now_iso();
        sqlx::query(
            r#"
            insert into prolific_submissions
                (experiment_id, participant_id, prolific_participant_id, prolific_study_id,
                 prolific_session_id, participant_session_id, received_at,
                 verification_method, verified_at)
            values (?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&submission.experiment_id)
        .bind(participant_id)
        .bind(&submission.prolific_participant_id)
        .bind(&submission.prolific_study_id)
        .bind(&submission.prolific_session_id)
        .bind(&participant_session_id)
        .bind(&verified_at)
        .bind(&submission.verification_method)
        .bind(&verified_at)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(ProlificAdmission {
            participant_id,
            research_id,
            participant_session_id,
        })
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

    async fn session_timing(&self, experiment_id: &str, session_id: i64) -> Result<SessionTiming> {
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

    async fn complete_session_initialization(
        &self,
        experiment_id: &str,
        session_id: i64,
    ) -> Result<bool> {
        let result = sqlx::query(
            "update sessions set initialization_complete = 1 where experiment_id = ? and session_id = ? and lifecycle = 'forming' and initialization_complete = 0",
        )
        .bind(experiment_id)
        .bind(session_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    async fn fail_session_initialization(
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

    async fn start_session(
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

    async fn touch_session_activity(
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

    async fn session_game_time_ms(
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

    async fn record_consent_declaration(
        &self,
        declaration: ConsentDeclarationRecord,
    ) -> Result<()> {
        sqlx::query(
            r#"
            insert into consent_declarations
            (experiment_id, session_id, participant_id, consent_item_id, accepted, purpose, declared_at, consent_text_hash, metadata_json)
            values (?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(declaration.experiment_id)
        .bind(declaration.session_id)
        .bind(declaration.participant_id)
        .bind(declaration.consent_item_id)
        .bind(if declaration.accepted { 1 } else { 0 })
        .bind(declaration.purpose)
        .bind(now_iso())
        .bind(declaration.consent_text_hash)
        .bind(serde_json::to_string(&declaration.metadata)?)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn append_session_event(&self, event: SessionEventRecord) -> Result<i64> {
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

    async fn commit_session_transition(
        &self,
        events: Vec<SessionEventRecord>,
        session_end: Option<SessionEnd>,
    ) -> Result<bool> {
        let Some(first) = events.first() else {
            return Ok(false);
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
        let mut tx = self.pool.begin().await?;
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
            return Ok(false);
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
        tx.commit().await?;
        Ok(true)
    }

    async fn end_session(
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

    async fn terminal_participant_state(
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

    async fn session_events(
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

    async fn recent_sessions(
        &self,
        experiment_id: &str,
        limit: i64,
    ) -> Result<Vec<StoredSessionSummary>> {
        let limit = limit.clamp(1, 10_000);
        let sessions = sqlx::query(
            r#"
            select s.session_id, s.public_session_id, s.dialogue_id, s.mode, s.lifecycle, s.purpose,
                   s.config_revision, s.game_version, s.created_at,
                   s.waiting_started_at, s.waiting_deadline_at, s.lifetime_deadline_at,
                   s.last_meaningful_activity_at, s.idle_deadline_at, s.started_at,
                   s.ended_at, s.completion_json, s.session_end_json,
                   count(distinct sp.participant_id) as participant_count,
                   count(distinct se.event_id) as event_count,
                   max(se.game_time_ms) as last_event_game_time_ms
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

    async fn session_participants(
        &self,
        experiment_id: &str,
        session_id: i64,
    ) -> Result<Vec<StoredSessionParticipant>> {
        let participants = sqlx::query(
            r#"
            select sp.experiment_id, sp.session_id, sp.participant_id,
                   sp.participant_session_id, sp.role, sp.joined_at, sp.left_at,
                   sp.connection_status, sp.disconnected_at, sp.reconnect_deadline_at,
                   p.research_id, p.participant_kind, p.identity_provider,
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
        if preview.has_non_terminal_session {
            bail!("affected sessions are live or non-terminal");
        }
        let mut tx = self.pool.begin().await?;
        let non_terminal = sqlx::query_scalar::<_, i64>(
            r#"
            select count(*) from sessions s
            join session_participants sp
              on sp.experiment_id = s.experiment_id and sp.session_id = s.session_id
            where sp.experiment_id = ? and sp.participant_id = ?
              and s.lifecycle != 'ended'
            "#,
        )
        .bind(experiment_id)
        .bind(participant_id)
        .fetch_one(&mut *tx)
        .await?;
        if non_terminal > 0 {
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
    .fetch_all(pool)
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
    .fetch_all(pool)
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
    i64,
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
        game_time_ms: row.9,
    })
}

/// Exports SQLite rows as JSON objects for the current admin/evaluation export.
async fn export_rows(pool: &SqlitePool, scope: Option<(&str, Option<i64>)>) -> Result<Value> {
    let (experiment_id, session_id) = scope.unwrap_or(("", None));
    let experiment = if experiment_id.is_empty() {
        json!(null)
    } else {
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
        .fetch_optional(pool)
        .await?;
        json!(row.map(
            |(id, game_version, config_revision, created_at, config_json, server_version, version_manifest_json, status, notes, pinned)| json!({
                "experiment_id": id,
                "game_version": game_version,
                "config_revision": config_revision,
                "created_at": created_at,
                "config": serde_json::from_str::<Value>(&config_json).unwrap_or(Value::Null),
                "server_version": server_version,
                "version_manifest": version_manifest_json.and_then(|raw| serde_json::from_str::<Value>(&raw).ok()),
                "status": status,
                "notes": notes,
                "pinned": pinned,
            })
        ))
    };
    let sessions_sql = if session_id.is_some() {
        "select experiment_id, session_id, public_session_id, dialogue_id, mode, lifecycle, purpose, config_revision, game_version, created_at, started_at, ended_at, completion_json, session_end_json from sessions where experiment_id = ? and session_id = ? order by session_id"
    } else {
        "select experiment_id, session_id, public_session_id, dialogue_id, mode, lifecycle, purpose, config_revision, game_version, created_at, started_at, ended_at, completion_json, session_end_json from sessions where experiment_id = ? order by session_id"
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
            i64,
            String,
            String,
            Option<String>,
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
                "public_session_id": row.2,
                "dialogue_id": row.3,
                "mode": row.4,
                "lifecycle": row.5,
                "purpose": row.6,
                "config_revision": row.7,
                "game_version": row.8,
                "created_at": row.9,
                "started_at": row.10,
                "ended_at": row.11,
                "completion": row.12.and_then(|raw| serde_json::from_str::<Value>(&raw).ok()),
                "session_end": row.13.and_then(|raw| serde_json::from_str::<Value>(&raw).ok()),
            })
        })
        .collect::<Vec<_>>();
    let event_sql = if session_id.is_some() {
        "select event_id, experiment_id, session_id, event_index, event_type, actor_participant_id, actor_role, payload_json, game_state_json, game_time_ms from session_events where experiment_id = ? and session_id = ? order by session_id, event_index"
    } else {
        "select event_id, experiment_id, session_id, event_index, event_type, actor_participant_id, actor_role, payload_json, game_state_json, game_time_ms from session_events where experiment_id = ? order by session_id, event_index"
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
        "select consent_id, experiment_id, session_id, participant_id, consent_item_id, accepted, purpose, declared_at, consent_text_hash, metadata_json from consent_declarations where experiment_id = ? and session_id = ? order by declared_at, consent_id"
    } else {
        "select consent_id, experiment_id, session_id, participant_id, consent_item_id, accepted, purpose, declared_at, consent_text_hash, metadata_json from consent_declarations where experiment_id = ? order by declared_at, consent_id"
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
                "purpose": row.6,
                "declared_at": row.7,
                "consent_text_hash": row.8,
                "metadata": row.9.and_then(|raw| serde_json::from_str::<Value>(&raw).ok()),
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
    /// Immutable data-use purpose selected when participant intake opened.
    pub purpose: String,
    pub status: String,
    pub consent_decisions: HashMap<String, bool>,
    pub created_at: String,
    pub updated_at: String,
}

/// Runtime record for one participant's membership in a session.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SessionParticipant {
    pub participant_session_id: String,
    pub participant_id: i64,
    /// Runtime participant source, used to distinguish humans from agents.
    pub source: String,
    pub role: Seat,
    pub connected: bool,
    /// Whether the browser declared its game channel ready at least once.
    pub ready: bool,
    /// Whether this participant has completed required audio/STT setup for this session.
    pub audio_ready: bool,
    pub consent_decisions: HashMap<String, bool>,
    pub joined_at: String,
    pub updated_at: String,
    /// Most recent disconnection instant for restart-safe grace handling.
    pub disconnected_at: Option<String>,
    /// Fixed reconnect deadline derived when the connection was lost.
    pub reconnect_deadline_at: Option<String>,
}

/// Runtime session record parameterized by a concrete game state type.
pub struct LiveSession<G: Game> {
    pub id: String,
    pub experiment_id: String,
    pub session_id: i64,
    pub mode: String,
    /// Immutable data-use purpose shared by every participant in this session.
    pub purpose: String,
    /// Session-local behavior object retaining session-scoped capabilities.
    pub game: G,
    /// Authoritative serializable mechanics state.
    pub state: G::State,
    /// Base game-scoped handle for this session's log.
    pub log_writer: Option<SessionLogWriter>,
    /// Authoritative shared lifecycle for this forming, running, or ended session.
    pub lifecycle: SessionLifecycle,
    /// Temporary interaction pause while a required role may reconnect.
    pub pause: Option<crate::protocol::ParticipantPauseReason>,
    pub participants: HashMap<String, SessionParticipant>,
    /// Fixed beginning of the unmatched waiting phase.
    pub waiting_started_at: String,
    /// Fixed server-owned deadline for finding a partner.
    pub waiting_deadline_at: String,
    /// Fixed absolute infrastructure lifetime deadline.
    pub lifetime_deadline_at: String,
    /// Last accepted message or action; heartbeats never advance this value.
    pub last_meaningful_activity_at: Option<String>,
    /// Fixed idle deadline derived from the last meaningful activity.
    pub idle_deadline_at: Option<String>,
    pub updated_at: String,
}

/// Typed shared session lifecycle used by every runtime transition.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "state", content = "end", rename_all = "snake_case")]
pub enum SessionLifecycle {
    /// Participants or required services are still assembling.
    Forming,
    /// The game has started and may be active or temporarily paused.
    Running,
    /// One immutable shared terminal value has been committed.
    Ended(SessionEnd),
}

impl SessionLifecycle {
    /// Returns the stable database lifecycle value.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Forming => "forming",
            Self::Running => "running",
            Self::Ended(_) => "ended",
        }
    }

    /// Reports whether the session has reached its absorbing state.
    pub fn is_ended(&self) -> bool {
        matches!(self, Self::Ended(_))
    }
}

/// Stored transcript segment received from the browser transcription flow.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TranscriptSegment {
    pub id: String,
    pub public_session_id: String,
    pub participant_session_id: String,
    pub player: String,
    /// Speech onset on the session's authoritative game clock.
    pub start_game_time_ms: i64,
    /// Speech endpoint on the session's authoritative game clock.
    pub end_game_time_ms: i64,
    pub text: String,
    pub metadata: Value,
}

/// In-memory active-session cache used for live WebSocket and game execution state.
pub struct MemoryState<G: Game> {
    pub participants: HashMap<String, ParticipantSession>,
    pub sessions: HashMap<String, LiveSession<G>>,
}

impl<G: Game> Default for MemoryState<G> {
    fn default() -> Self {
        Self {
            participants: HashMap::new(),
            sessions: HashMap::new(),
        }
    }
}

impl<G: Game> MemoryState<G> {
    /// Creates and stores an active participant session after durable identity creation.
    pub fn create_participant(
        &mut self,
        participant_id: i64,
        research_id: String,
        source: String,
        purpose: String,
    ) -> ParticipantSession {
        self.create_participant_with_id(new_id("ps"), participant_id, research_id, source, purpose)
    }

    /// Restores or creates a participant session with a durable admission subject.
    pub fn create_participant_with_id(
        &mut self,
        participant_session_id: String,
        participant_id: i64,
        research_id: String,
        source: String,
        purpose: String,
    ) -> ParticipantSession {
        if let Some(existing) = self.participants.get(&participant_session_id) {
            return existing.clone();
        }
        let now = now_iso();
        let participant = ParticipantSession {
            id: participant_session_id,
            participant_id,
            research_id,
            source,
            purpose,
            status: "created".to_string(),
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
