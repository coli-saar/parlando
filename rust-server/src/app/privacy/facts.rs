//! Reads the configurations covered by a privacy request and projects the fields used by
//! the report. An unknown requested experiment yields an error. For installation scope, an
//! empty catalogue uses the installation configuration. Historical records can supply facts
//! without passing current activation checks.

use crate::{
    app::{persistable_config_json, AppError},
    config::ExperimentConfig,
    storage::SqliteExperimentStore,
};
use serde_json::Value;

/// Privacy-relevant projection which remains readable across configuration schema versions.
pub(super) struct PrivacyConfigFacts {
    pub(super) contract_version: String,
    pub(super) transcription_enabled: bool,
    pub(super) transcription_provider: String,
    pub(super) transcription_endpoint: String,
    pub(super) tts_enabled: bool,
    pub(super) tts_provider: String,
    pub(super) tts_endpoint: String,
    pub(super) voice_enabled: bool,
    pub(super) human_vs_agent: bool,
    pub(super) consent_items: usize,
    pub(super) participant_information_version: String,
    pub(super) participant_information_url: String,
}

/// Reads one boolean from a nested normalized experiment configuration.
fn privacy_config_bool(config: &Value, section: &str, key: &str) -> bool {
    config
        .get(section)
        .and_then(|value| value.get(key))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// Reads one string from a nested normalized experiment configuration.
fn privacy_config_string(config: &Value, section: &str, key: &str) -> String {
    config
        .get(section)
        .and_then(|value| value.get(key))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Projects privacy facts without requiring an old configuration to validate under today's schema.
pub(super) fn privacy_config_facts(config: &Value) -> PrivacyConfigFacts {
    let information_version =
        privacy_config_string(config, "direct", "participant_information_version");
    let information_url = privacy_config_string(config, "direct", "participant_information_url");
    PrivacyConfigFacts {
        contract_version: privacy_config_string(config, "privacy", "contract_version"),
        transcription_enabled: privacy_config_bool(config, "transcription", "enabled"),
        transcription_provider: privacy_config_string(config, "transcription", "provider"),
        transcription_endpoint: privacy_config_string(config, "speechmatics", "realtime_url"),
        tts_enabled: privacy_config_bool(config, "tts", "enabled"),
        tts_provider: privacy_config_string(config, "tts", "provider"),
        tts_endpoint: privacy_config_string(config, "tts", "base_url"),
        voice_enabled: privacy_config_bool(config, "voice", "enabled"),
        human_vs_agent: privacy_config_string(config, "agents", "mode") == "human_vs_agent",
        consent_items: config
            .get("direct")
            .and_then(|value| value.get("consents"))
            .and_then(Value::as_array)
            .map_or(0, Vec::len),
        participant_information_version: information_version,
        participant_information_url: information_url,
    }
}

/// Resolves configuration records in report order, falling back to the installation configuration for an empty catalogue.
pub(super) async fn load(
    store: &SqliteExperimentStore,
    fallback: &ExperimentConfig,
    experiment_id: Option<&str>,
) -> Result<Vec<PrivacyConfigFacts>, AppError> {
    let mut facts = Vec::new();
    if let Some(experiment_id) = experiment_id {
        let definition = store
            .experiment_definition(experiment_id)
            .await?
            .ok_or_else(|| AppError::not_found("Experiment not found."))?;
        facts.push(privacy_config_facts(&definition.config));
    } else {
        let summaries = store.list_experiments(1_000).await?;
        facts.reserve(summaries.len());
        for summary in &summaries {
            if let Some(definition) = store.experiment_definition(&summary.experiment_id).await? {
                facts.push(privacy_config_facts(&definition.config));
            }
        }
    }
    if facts.is_empty() {
        facts.push(privacy_config_facts(&persistable_config_json(fallback)?));
    }
    Ok(facts)
}
