//! Reads and revises the configuration of a selected experiment. The editor submits its
//! expected revision, game YAML and explicit secret changes. A successful save invalidates
//! the cached mount so its successor uses the new revision. Ordinary configuration reads
//! describe secret availability. Revealing a value has a separate authenticated handler.

use crate::app::{
    apply_experiment_secrets, apply_game_provider_secrets, apply_provider_endpoint_defaults,
    dashboard_agent_definitions, experiment_config_from_json,
    experiment_config_from_json_unvalidated, invalidate_runtime_router, json,
    local_testing_issues_for_config, lock_runtime_router_build, parse_game_config,
    persistable_config_json, prolific_configuration_complete,
    run_and_cache_prolific_activation_preflight, validate_game_config_contains_no_secrets,
    AppError, Arc, Deserialize, HashMap, HeaderValue, IntoResponse, Json, Path, Response, Result,
    ServerContext, State, StatusCode, Value, CACHE_CONTROL,
};

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::app) struct AdminSaveExperimentConfigRequest {
    expected_revision: i64,
    config: Value,
    #[serde(default)]
    game_yaml: Option<String>,
    #[serde(default)]
    secret_updates: HashMap<String, String>,
    #[serde(default)]
    secret_deletions: Vec<String>,
    change_summary: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::app) struct AdminRevealExperimentSecretRequest {
    key: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::app) struct AdminValidateGameConfigRequest {
    game_yaml: String,
}

/// Returns the current normalized configuration for one catalogue experiment.
pub(in crate::app) async fn admin_experiment_config(
    State(state): State<Arc<ServerContext>>,
    Path(experiment_id): Path<String>,
) -> Result<Json<Value>, AppError> {
    let mut experiment = state
        .installation
        .store
        .experiment_definition(&experiment_id)
        .await?
        .ok_or_else(|| AppError::not_found("Experiment not found."))?;
    let stored_secrets = state
        .installation
        .store
        .experiment_secrets(&experiment_id)
        .await?;
    let empty_game = Value::Object(Default::default());
    let game_yaml = serde_yaml::to_string(
        experiment
            .config
            .get("game")
            .filter(|game| !game.is_null())
            .unwrap_or(&empty_game),
    )
    .map_err(|error| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    let configured_secrets = configured_secret_statuses(&stored_secrets);
    let activation_issues =
        if experiment.game_version == state.installation.game_descriptor.version.to_string() {
            match experiment_config_from_json_unvalidated(
                experiment.config.clone(),
                &state.config,
                &experiment_id,
            ) {
                Ok(mut normalized) => {
                    experiment.config = persistable_config_json(&normalized).map_err(|error| {
                        AppError::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
                    })?;
                    match normalized.validate() {
                        Ok(()) => {
                            apply_experiment_secrets(&mut normalized, &stored_secrets);
                            let game_secrets = state.installation.store.game_secrets().await?;
                            apply_game_provider_secrets(&mut normalized, &game_secrets);
                            local_testing_issues_for_config(&state, &mut normalized)
                        }
                        Err(error) => vec![error.to_string()],
                    }
                }
                Err(error) => vec![error.to_string()],
            }
        } else {
            Vec::new()
        };
    Ok(Json(json!({
        "experiment": experiment,
        "game_yaml": game_yaml,
        "agent_factories": dashboard_agent_definitions(&state.agent_definitions)
            .map_err(|error| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?,
        "configured_secrets": configured_secrets,
        "activation_issues": activation_issues,
    })))
}

/// Describes secret availability without returning credential values.
pub(in crate::app) fn configured_provider_secret_statuses(
    stored: &HashMap<String, String>,
) -> Vec<Value> {
    ["speechmatics.api_key", "tts.api_key", "prolific.api_token"]
        .into_iter()
        .map(|key| {
            let configured = stored.contains_key(key);
            json!({
                "key": key,
                "configured": configured,
                "source": if configured { "game" } else { "missing" },
            })
        })
        .collect()
}

/// Describes configured experiment-owned game secrets without revealing values.
fn configured_secret_statuses(stored: &HashMap<String, String>) -> Vec<Value> {
    let mut game_keys = stored
        .keys()
        .filter(|key| key.starts_with("game."))
        .cloned()
        .collect::<Vec<_>>();
    game_keys.sort();
    game_keys
        .drain(..)
        .map(|key| json!({"key": key, "configured": true, "source": "experiment"}))
        .collect()
}

/// Validates one write-only secret name and value before opening a transaction.
fn validate_secret_update(key: &str, value: Option<&str>) -> Result<(), AppError> {
    let game = key
        .strip_prefix("game.")
        .is_some_and(|name| !name.is_empty());
    if key.chars().count() > 128
        || !game
        || !key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
    {
        return Err(AppError::bad_request(
            "Experiment secret keys must use game.<name> with letters, digits, dots, dashes, and underscores",
        ));
    }
    if value.is_some_and(|value| value.is_empty() || value.len() > 65_536) {
        return Err(AppError::bad_request(
            "Secret values must contain 1 to 65536 bytes",
        ));
    }
    Ok(())
}

/// Reveals one explicitly requested secret to an authenticated administrator.
pub(in crate::app) async fn admin_reveal_experiment_secret(
    State(state): State<Arc<ServerContext>>,
    Path(experiment_id): Path<String>,
    Json(request): Json<AdminRevealExperimentSecretRequest>,
) -> Result<Response, AppError> {
    validate_secret_update(&request.key, None)?;
    state
        .installation
        .store
        .experiment_definition(&experiment_id)
        .await?
        .ok_or_else(|| AppError::not_found("Experiment not found."))?;
    let stored = state
        .installation
        .store
        .experiment_secrets(&experiment_id)
        .await?;
    let value = stored.get(&request.key).cloned();
    let value = value.ok_or_else(|| AppError::not_found("Secret is not configured."))?;
    tracing::warn!(
        experiment_id,
        secret_key = request.key,
        "administrator revealed experiment secret"
    );
    let mut response = Json(json!({"key": request.key, "value": value})).into_response();
    response.headers_mut().insert(
        CACHE_CONTROL,
        HeaderValue::from_static("no-store, max-age=0"),
    );
    Ok(response)
}

/// Parses and validates game-owned YAML without saving a configuration revision.
pub(in crate::app) async fn admin_validate_game_config(
    State(state): State<Arc<ServerContext>>,
    Path(experiment_id): Path<String>,
    Json(request): Json<AdminValidateGameConfigRequest>,
) -> Result<Json<Value>, AppError> {
    state
        .installation
        .store
        .experiment_definition(&experiment_id)
        .await?
        .ok_or_else(|| AppError::not_found("Experiment not found."))?;
    let game = serde_yaml::from_str::<Value>(&request.game_yaml)
        .map_err(|error| AppError::bad_request(format!("Invalid game YAML: {error}")))?;
    validate_game_config_contains_no_secrets(&game)
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    parse_game_config(&state.installation.loaded_game, &game)
        .map_err(|error| AppError::bad_request(format!("Invalid game configuration: {error}")))?;
    Ok(Json(json!({"valid": true})))
}

/// Saves a structurally valid configuration revision before checking provider readiness.
pub(in crate::app) async fn admin_save_experiment_config(
    State(state): State<Arc<ServerContext>>,
    Path(experiment_id): Path<String>,
    Json(request): Json<AdminSaveExperimentConfigRequest>,
) -> Result<Json<Value>, AppError> {
    let runtime_guard = lock_runtime_router_build(&state, &experiment_id).await;
    let experiment = state
        .installation
        .store
        .experiment_definition(&experiment_id)
        .await?
        .ok_or_else(|| AppError::not_found("Experiment not found."))?;
    if experiment.status != "inactive" {
        return Err(AppError::new(
            StatusCode::CONFLICT,
            "Experiment configuration can only be edited while the experiment is inactive",
        ));
    }
    if experiment.game_version != state.installation.game_descriptor.version.to_string() {
        return Err(AppError::new(
            StatusCode::CONFLICT,
            "Clone this experiment before editing it under the current game version",
        ));
    }
    for (key, value) in &request.secret_updates {
        validate_secret_update(key, Some(value))?;
    }
    for key in &request.secret_deletions {
        validate_secret_update(key, None)?;
    }
    let mut config_value = request.config;
    if let Some(game_yaml) = request.game_yaml.as_deref() {
        let game = serde_yaml::from_str::<Value>(game_yaml)
            .map_err(|error| AppError::bad_request(format!("Invalid game YAML: {error}")))?;
        validate_game_config_contains_no_secrets(&game)
            .map_err(|error| AppError::bad_request(error.to_string()))?;
        config_value
            .as_object_mut()
            .ok_or_else(|| AppError::bad_request("configuration must be an object"))?
            .insert("game".to_string(), game);
    }
    let game_settings = state.installation.game_settings.read().await.clone();
    apply_provider_endpoint_defaults(&mut config_value, &game_settings)
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    let mut config = experiment_config_from_json(config_value, &state.config, &experiment_id)
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    let mut secrets = state
        .installation
        .store
        .experiment_secrets(&experiment_id)
        .await?;
    for key in &request.secret_deletions {
        secrets.remove(key);
    }
    secrets.extend(request.secret_updates.clone());
    apply_experiment_secrets(&mut config, &secrets);
    let game_secrets = state.installation.store.game_secrets().await?;
    apply_game_provider_secrets(&mut config, &game_secrets);
    config.experiment.id = Some(experiment_id.clone());
    config
        .validate()
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    validate_game_config_contains_no_secrets(&config.game)
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    parse_game_config(&state.installation.loaded_game, &config.game)
        .map_err(|error| AppError::bad_request(format!("Invalid game configuration: {error}")))?;
    let revision = state
        .installation
        .store
        .save_experiment_configuration(
            &experiment_id,
            request.expected_revision,
            persistable_config_json(&config)?,
            request.change_summary,
            request.secret_updates,
            request.secret_deletions,
        )
        .await
        .map_err(|error| AppError::new(StatusCode::CONFLICT, error.to_string()))?;
    invalidate_runtime_router(&state, &experiment_id).await;
    drop(runtime_guard);
    let prolific_issues = if prolific_configuration_complete(&config) {
        run_and_cache_prolific_activation_preflight(
            &state,
            &experiment_id,
            &config,
            &game_settings.prolific_api_base_url,
        )
        .await
        .0
    } else {
        Vec::new()
    };
    Ok(Json(json!({
        "experiment_id": experiment_id,
        "revision": revision,
        "prolific_issues": prolific_issues,
    })))
}

/// Lists immutable configuration revisions for one experiment.
pub(in crate::app) async fn admin_experiment_revisions(
    State(state): State<Arc<ServerContext>>,
    Path(experiment_id): Path<String>,
) -> Result<Json<Value>, AppError> {
    Ok(Json(json!({
        "experiment_id": experiment_id.clone(),
        "revisions": state.installation.store.experiment_revisions(&experiment_id).await?,
    })))
}
