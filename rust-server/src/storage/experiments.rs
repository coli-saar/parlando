//! Experiment catalogue, versioned configuration, lifecycle and secrets.
//!
//! Catalogue and activation handlers read experiment definitions and update their lifecycle.
//! Configuration saves create revisions and store secret values separately from the
//! configuration shown to readers. The host uses the definition and revision when
//! constructing a runtime.

use super::{now_iso, SqliteExperimentStore};
use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// Input for creating or updating the durable experiment row.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ExperimentRecord {
    pub experiment_id: String,
    /// Semantic version of the game at experiment creation.
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
    /// Semantic version of the game when the experiment was created.
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
    /// Game version recorded at experiment creation.
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

impl SqliteExperimentStore {
    /// Resolves the durable experiment identifier, creating its definition if necessary.
    pub(crate) async fn ensure_experiment(&self, experiment: ExperimentRecord) -> Result<String> {
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

    /// Lists experiment catalogue entries up to the requested limit.
    pub(crate) async fn list_experiments(
        &self,
        limit: i64,
    ) -> Result<Vec<StoredExperimentSummary>> {
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

    /// Loads the current experiment configuration and metadata.
    pub(crate) async fn experiment_definition(
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

    /// Creates an experiment and its initial configuration revision.
    pub(crate) async fn create_experiment(&self, experiment: ExperimentRecord) -> Result<()> {
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

    /// Loads the secrets belonging to one experiment.
    pub(crate) async fn experiment_secrets(
        &self,
        experiment_id: &str,
    ) -> Result<HashMap<String, String>> {
        Ok(sqlx::query_as::<_, (String, String)>(
            "select secret_key, secret_value from experiment_secrets where experiment_id = ?",
        )
        .bind(experiment_id)
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .collect())
    }

    /// Saves a configuration revision together with its secret updates.
    pub(crate) async fn save_experiment_configuration(
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

    /// Reads the revisioned communication channel of the experiment that created the session.
    pub(crate) async fn session_communication(
        &self,
        experiment_id: &str,
        session_id: i64,
    ) -> Result<Option<crate::config::CommunicationConfig>> {
        let config = sqlx::query_scalar::<_, String>(
            "select r.config_json from sessions s
             join experiment_config_revisions r
             on r.experiment_id = s.experiment_id and r.revision = s.config_revision
             where s.experiment_id = ? and s.session_id = ?",
        )
        .bind(experiment_id)
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await?;
        config
            .map(|json| {
                let config: crate::config::StoredExperimentConfig = serde_json::from_str(&json)?;
                Ok(config.communication)
            })
            .transpose()
    }

    /// Lists the saved configuration revisions for one experiment.
    pub(crate) async fn experiment_revisions(
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

    /// Updates the catalogue metadata for one experiment.
    pub(crate) async fn update_experiment_catalogue(
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
    /// Reads the catalogue summary for an experiment.
    pub(crate) async fn experiment_summary(
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

    /// Records the current lifecycle status of an experiment.
    pub(crate) async fn update_experiment_status(
        &self,
        experiment_id: &str,
        status: &str,
    ) -> Result<()> {
        sqlx::query("update experiments set status = ? where experiment_id = ?")
            .bind(status)
            .bind(experiment_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Archives a durable experiment definition.
    pub(crate) async fn archive_experiment(&self, experiment_id: &str) -> Result<()> {
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

    /// Checks whether any experiment still allows participant intake.
    pub(crate) async fn has_open_experiments(&self) -> Result<bool> {
        Ok(sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM experiments WHERE status IN ('active', 'testing'))",
        )
        .fetch_one(&self.pool)
        .await?)
    }

    /// Returns open experiments to inactive status.
    pub(crate) async fn deactivate_open_experiments(&self) -> Result<u64> {
        let result = sqlx::query(
            "update experiments set status = 'inactive' where status in ('active', 'testing')",
        )
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }
}
