//! Versioned installation settings and their associated secrets.

use super::{now_iso, SqliteExperimentStore};
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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
    /// Optimistic-concurrency revision for dashboard updates.
    pub revision: i64,
}

/// Complete validated update to installation-wide game settings and provider secrets.
pub(crate) struct GameSettingsUpdate {
    /// Revision which must still be current when the transaction commits.
    pub expected_revision: i64,
    /// Institution displayed by hosted experiments.
    pub institution: String,
    /// Administrator source networks accepted by the installation.
    pub admin_allowed_ip_ranges: Vec<String>,
    /// Default realtime transcription endpoint.
    pub speechmatics_realtime_url: String,
    /// Default text-to-speech endpoint.
    pub tts_base_url: String,
    /// Shared Prolific API endpoint.
    pub prolific_api_base_url: String,
    /// Secret values to insert or replace.
    pub secret_updates: HashMap<String, String>,
    /// Secret keys to remove.
    pub secret_deletions: Vec<String>,
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
            revision: 1,
        }
    }
}

impl SqliteExperimentStore {
    /// Loads the shared settings for the current installation.
    pub(crate) async fn game_settings(&self) -> Result<StoredGameSettings> {
        let row = sqlx::query_as::<_, (String, String, String, String, String, i64)>(
            "select institution, admin_allowed_ip_ranges_json, speechmatics_realtime_url, tts_base_url, prolific_api_base_url, revision from game_settings where singleton = 1",
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(StoredGameSettings {
            institution: row.0,
            admin_allowed_ip_ranges: serde_json::from_str(&row.1)?,
            speechmatics_realtime_url: row.2,
            tts_base_url: row.3,
            prolific_api_base_url: row.4,
            revision: row.5,
        })
    }
    /// Loads the secrets shared by the game installation.
    pub(crate) async fn game_secrets(&self) -> Result<HashMap<String, String>> {
        Ok(sqlx::query_as::<_, (String, String)>(
            "select secret_key, secret_value from game_secrets",
        )
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .collect())
    }

    /// Atomically replaces shared settings and applies the associated secret changes.
    pub(crate) async fn update_game_settings(&self, update: GameSettingsUpdate) -> Result<i64> {
        let mut tx = self.pool.begin().await?;
        let next_revision = update.expected_revision + 1;
        let result = sqlx::query(
            r#"
            update game_settings set institution = ?, admin_allowed_ip_ranges_json = ?, speechmatics_realtime_url = ?, tts_base_url = ?, prolific_api_base_url = ?, revision = ?, updated_at = ?
            where singleton = 1 and revision = ?
            "#,
        )
        .bind(update.institution.trim())
        .bind(serde_json::to_string(&update.admin_allowed_ip_ranges)?)
        .bind(update.speechmatics_realtime_url.trim())
        .bind(update.tts_base_url.trim())
        .bind(update.prolific_api_base_url.trim())
        .bind(next_revision)
        .bind(now_iso())
        .bind(update.expected_revision)
        .execute(&mut *tx)
        .await?;
        if result.rows_affected() != 1 {
            bail!("game settings changed concurrently");
        }
        for (key, value) in update.secret_updates {
            sqlx::query("insert into game_secrets (secret_key, secret_value, updated_at) values (?, ?, ?) on conflict(secret_key) do update set secret_value = excluded.secret_value, updated_at = excluded.updated_at")
                .bind(key)
                .bind(value)
                .bind(now_iso())
                .execute(&mut *tx)
                .await?;
        }
        for key in update.secret_deletions {
            sqlx::query("delete from game_secrets where secret_key = ?")
                .bind(key)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(next_revision)
    }
}
