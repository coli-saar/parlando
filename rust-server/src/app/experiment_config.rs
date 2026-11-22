//! Loads revisioned experiment configuration and hydrates installation secrets.
//!
//! Storage supplies the experiment definition and installation settings. Hydration
//! combines them with provider defaults and resolved secrets for the runtime. Editing and
//! persistence use separate projections so stored secret references and public
//! configuration can be inspected without revealing credentials.

use super::{
    anyhow, bail, json, AppError, Digest, ExperimentConfig, HashMap, Result, ServerContext, Sha256,
    StoredExperimentConfig, StoredGameSettings, Value,
};

/// Produces the only configuration representation allowed to enter durable storage.
pub(super) fn persistable_config_json(config: &ExperimentConfig) -> Result<Value> {
    let mut value = serde_json::to_value(StoredExperimentConfig::from(config))?;
    redact_secret_fields(&mut value);
    Ok(value)
}

/// Rejects credentials embedded in revisioned game options instead of silently redacting them.
pub(super) fn validate_game_config_contains_no_secrets(value: &Value) -> Result<()> {
    /// Traverses nested game options while maintaining the path for validation errors.
    fn visit(value: &Value, path: &mut Vec<String>) -> Result<()> {
        match value {
            Value::Object(object) => {
                for (key, child) in object {
                    path.push(key.clone());
                    let normalized = key.to_ascii_lowercase();
                    let secret = normalized == "private_key"
                        || normalized == "client_secret"
                        || normalized == "access_token"
                        || normalized == "auth_token"
                        || normalized.ends_with("_api_key")
                        || normalized.ends_with("_password")
                        || normalized.ends_with("_secret")
                        || normalized.ends_with("_token")
                        || matches!(
                            normalized.as_str(),
                            "api_key" | "apikey" | "password" | "token" | "secret"
                        );
                    if secret {
                        bail!(
                            "game configuration field {} looks like a credential; add it under Game secrets instead",
                            path.join(".")
                        );
                    }
                    visit(child, path)?;
                    path.pop();
                }
            }
            Value::Array(items) => {
                for item in items {
                    visit(item, path)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    visit(value, &mut Vec::new())
}

/// Restores bootstrap-only fields before strict experiment configuration validation.
pub(super) fn experiment_config_from_json(
    value: Value,
    bootstrap: &ExperimentConfig,
    experiment_id: &str,
) -> Result<ExperimentConfig> {
    let config = experiment_config_from_json_unvalidated(value, bootstrap, experiment_id)?;
    config.validate()?;
    Ok(config)
}

/// Restores bootstrap-only fields while preserving invalid revisions for dashboard inspection.
pub(super) fn experiment_config_from_json_unvalidated(
    value: Value,
    bootstrap: &ExperimentConfig,
    experiment_id: &str,
) -> Result<ExperimentConfig> {
    if !value.is_object() {
        bail!("experiment configuration must be a JSON object");
    }
    let stored: StoredExperimentConfig = serde_json::from_value(value)?;
    Ok(stored.into_runtime(
        bootstrap,
        experiment_id,
        experiment_public_base_url(&bootstrap.server.public_base_url, experiment_id),
    ))
}

/// Builds the canonical externally visible participant base for one experiment.
pub(super) fn experiment_public_base_url(public_origin: &str, experiment_id: &str) -> String {
    let base = public_origin.trim_end_matches('/');
    let experiment_suffix = format!("/e/{experiment_id}");
    if base.ends_with(&experiment_suffix) {
        base.to_string()
    } else {
        format!("{base}{experiment_suffix}")
    }
}

/// Builds the exact external-study URL template shown to and verified against Prolific.
pub(super) fn prolific_setup_url(experiment_public_base_url: &str) -> String {
    format!(
        "{}/?PROLIFIC_PID={{{{%PROLIFIC_PID%}}}}&STUDY_ID={{{{%STUDY_ID%}}}}&SESSION_ID={{{{%SESSION_ID%}}}}",
        experiment_public_base_url.trim_end_matches('/')
    )
}

/// Applies write-only experiment credentials after loading revisioned non-secret settings.
pub(super) fn apply_experiment_secrets(
    config: &mut ExperimentConfig,
    secrets: &HashMap<String, String>,
) {
    config.game_secrets = secrets
        .iter()
        .filter_map(|(key, value)| {
            key.strip_prefix("game.")
                .map(|key| (key.to_string(), value.clone()))
        })
        .collect();
}

/// Applies explicit game-wide provider credentials to one experiment runtime.
pub(super) fn apply_game_provider_secrets(
    config: &mut ExperimentConfig,
    stored_secrets: &HashMap<String, String>,
) {
    config.speechmatics.api_key = if config.transcription_active() {
        stored_secrets
            .get("speechmatics.api_key")
            .cloned()
            .unwrap_or_default()
    } else {
        String::new()
    };
    config.tts.api_key = if config.tts_active() {
        stored_secrets
            .get("tts.api_key")
            .cloned()
            .unwrap_or_default()
    } else {
        String::new()
    };
    config.recruitment.prolific.api_token = stored_secrets
        .get("prolific.api_token")
        .cloned()
        .unwrap_or_default();
}

/// Materializes game endpoint defaults into a new experiment revision without overriding choices.
pub(super) fn apply_provider_endpoint_defaults(
    value: &mut Value,
    settings: &StoredGameSettings,
) -> Result<()> {
    let object = value
        .as_object_mut()
        .ok_or_else(|| anyhow!("experiment configuration must be a JSON object"))?;
    for (section, key, default) in [
        (
            "speechmatics",
            "realtime_url",
            settings.speechmatics_realtime_url.as_str(),
        ),
        ("tts", "base_url", settings.tts_base_url.as_str()),
    ] {
        let provider = object
            .entry(section)
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .ok_or_else(|| anyhow!("{section} configuration must be an object"))?;
        provider
            .entry(key)
            .or_insert_with(|| Value::String(default.to_string()));
    }
    Ok(())
}

/// Loads one stored revision with process bootstrap settings and effective credentials applied.
pub(super) async fn hydrated_experiment_config(
    state: &ServerContext,
    experiment_id: &str,
) -> Result<ExperimentConfig, AppError> {
    let experiment = state
        .installation
        .store
        .experiment_definition(experiment_id)
        .await?
        .ok_or_else(|| AppError::not_found("Experiment not found."))?;
    let mut config = experiment_config_from_json(experiment.config, &state.config, experiment_id)
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    let experiment_secrets = state
        .installation
        .store
        .experiment_secrets(experiment_id)
        .await?;
    apply_experiment_secrets(&mut config, &experiment_secrets);
    let game_secrets = state.installation.store.game_secrets().await?;
    apply_game_provider_secrets(&mut config, &game_secrets);
    Ok(config)
}

/// Returns the stable hidden key for one dashboard-managed agent credential.
pub(super) fn derived_agent_secret_key(factory_id: &str, path: &str) -> String {
    format!("agent_{factory_id}_{path}")
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '_' | '-') {
                character
            } else {
                '_'
            }
        })
        .take(128)
        .collect()
}

/// Recursively removes credential-shaped fields as a defense-in-depth serialization boundary.
pub(super) fn redact_secret_fields(value: &mut Value) {
    match value {
        Value::Object(object) => {
            for (key, child) in object.iter_mut() {
                let normalized = key.to_ascii_lowercase();
                if normalized == "private_key"
                    || normalized == "client_secret"
                    || normalized == "access_token"
                    || normalized == "auth_token"
                    || normalized.ends_with("_api_key")
                    || normalized.ends_with("_password")
                    || normalized.ends_with("_secret")
                    || normalized.ends_with("_token")
                    || matches!(
                        normalized.as_str(),
                        "api_key" | "apikey" | "password" | "password_hash" | "token" | "secret"
                    )
                {
                    if !child
                        .as_str()
                        .is_some_and(is_experiment_game_secret_reference)
                    {
                        *child = Value::String(String::new());
                    }
                } else {
                    redact_secret_fields(child);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(redact_secret_fields),
        _ => {}
    }
}

/// Recognizes the non-secret identifier stored by semantic secret-reference fields.
fn is_experiment_game_secret_reference(value: &str) -> bool {
    value.strip_prefix("game.").is_some_and(|key| {
        !key.is_empty()
            && key.len() <= 128
            && key
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    })
}

/// Encodes a SHA-256 digest as lowercase hexadecimal without adding another dependency.
pub(super) fn hex_sha256(value: &str) -> String {
    Sha256::digest(value.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Returns a cloned configuration string only when it contains a non-whitespace value.
pub(super) fn nonempty_string(value: &str) -> Option<String> {
    (!value.trim().is_empty()).then(|| value.to_string())
}
