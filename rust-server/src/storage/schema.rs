//! Schema initialization and validation for the SQLite experiment database.
//!
//! Opening the SQLite store initializes an empty database in a transaction. For a populated
//! database it checks the supported schema before normal reads or writes. All other storage
//! modules rely on the resulting tables and constraints.

use super::{now_iso, SqliteExperimentStore};
use anyhow::{bail, Result};

impl SqliteExperimentStore {
    /// Validates an existing schema before writes, or initializes a new database atomically.
    pub(crate) async fn initialize_or_validate_schema(&self) -> Result<()> {
        const CURRENT_SCHEMA_VERSION: i64 = 17;
        let existing_tables = sqlx::query_scalar::<_, String>(
            "select name from sqlite_master where type = 'table' and name not like 'sqlite_%' order by name",
        )
        .fetch_all(&self.pool)
        .await?;
        if !existing_tables.is_empty() {
            if !existing_tables
                .iter()
                .any(|table| table == "schema_migrations")
            {
                bail!("database has no Parlando schema version; convert it explicitly before opening it with this release");
            }
            let version =
                sqlx::query_scalar::<_, Option<i64>>("select max(version) from schema_migrations")
                    .fetch_one(&self.pool)
                    .await?
                    .unwrap_or(0);
            if version != CURRENT_SCHEMA_VERSION {
                bail!("database schema version {version} is unsupported; convert it explicitly to version {CURRENT_SCHEMA_VERSION} before opening it");
            }
            return Ok(());
        }

        let mut transaction = self.pool.begin().await?;
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
            sqlx::query(statement).execute(&mut *transaction).await?;
        }
        sqlx::query("insert into schema_migrations (version, applied_at) values (?, ?)")
            .bind(CURRENT_SCHEMA_VERSION)
            .bind(now_iso())
            .execute(&mut *transaction)
            .await?;
        sqlx::query("insert or ignore into game_settings (singleton, updated_at) values (1, ?)")
            .bind(now_iso())
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;
        Ok(())
    }

    /// Returns SQLite user table names for schema tests.
    #[cfg(test)]
    pub(crate) async fn table_names(&self) -> Result<Vec<String>> {
        Ok(sqlx::query_scalar::<_, String>(
            "select name from sqlite_master where type = 'table' and name not like 'sqlite_%' order by name",
        )
        .fetch_all(&self.pool)
        .await?)
    }
}
