//! Manages the dashboard catalogue of experiment definitions. Listing combines stored
//! revisions with game and provider readiness information. Creation and cloning produce
//! inactive experiments for the loaded game version. Activation belongs to the mounted
//! experiment.

use crate::app::{
    apply_provider_endpoint_defaults, cached_prolific_activation_preflight,
    configured_provider_secret_statuses, experiment_config_from_json, experiment_public_base_url,
    hydrated_experiment_config, invalidate_runtime_router, json, local_testing_issues_for_config,
    lock_runtime_router_build, parse_game_config, persistable_config_json,
    prolific_local_activation_issues, prolific_setup_url, validate_agent_configuration,
    AdminSession, AppError, Arc, Deserialize, ExperimentRecord, Extension, Json, Path, Result,
    ServerContext, State, StatusCode, Value, Weak,
};

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::app) struct AdminCreateExperimentRequest {
    experiment_id: String,
    config: Option<Value>,
    notes: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::app) struct AdminCloneExperimentRequest {
    experiment_id: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::app) struct AdminCatalogueRequest {
    pinned: bool,
    notes: Option<String>,
}

/// Returns the catalogue of experiments owned by this game installation.
pub(in crate::app) async fn admin_experiments(
    State(state): State<Arc<ServerContext>>,
    Extension(admin_session): Extension<AdminSession>,
) -> Result<Json<Value>, AppError> {
    let experiments = state.installation.store.list_experiments(1_000).await?;
    let mut catalogue = Vec::with_capacity(experiments.len());
    for experiment in experiments {
        let readiness = experiment_catalogue_readiness(
            &state,
            &experiment.experiment_id,
            &experiment.game_version,
        )
        .await;
        let runtime = state
            .installation
            .runtime_registry
            .read()
            .await
            .get(&experiment.experiment_id)
            .and_then(|runtimes| {
                runtimes
                    .iter()
                    .rev()
                    .filter_map(Weak::upgrade)
                    .find(|state| !state.retired.load(std::sync::atomic::Ordering::Acquire))
            });
        let participant_url = if let Some(runtime) = runtime {
            runtime.participant_url.read().await.clone()
        } else {
            None
        };
        let mut value = serde_json::to_value(&experiment)
            .map_err(|error| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
        let object = value
            .as_object_mut()
            .expect("serialized experiment summaries are JSON objects");
        object.insert(
            "configuration_valid".to_string(),
            json!(readiness.configuration_valid),
        );
        object.insert(
            "configuration_error".to_string(),
            json!(readiness.configuration_error),
        );
        object.insert(
            "local_testing_runnable".to_string(),
            json!(readiness.local_testing_issues.is_empty() && readiness.configuration_valid),
        );
        object.insert(
            "local_testing_issues".to_string(),
            json!(readiness.local_testing_issues),
        );
        object.insert(
            "runnable".to_string(),
            json!(readiness.prolific_issues.is_empty() && readiness.configuration_valid),
        );
        object.insert(
            "runnable_issues".to_string(),
            json!(readiness.prolific_issues),
        );
        object.insert(
            "prolific_setup_url".to_string(),
            json!(prolific_setup_url(&experiment_public_base_url(
                &state.config.server.public_base_url,
                &experiment.experiment_id,
            ))),
        );
        object.insert("participant_url".to_string(), json!(participant_url));
        catalogue.push(value);
    }
    let game_settings = state.installation.game_settings.read().await.clone();
    let game_secrets = state.installation.store.game_secrets().await?;
    Ok(Json(json!({
        "game": state.installation.game_descriptor,
        "version_manifest": state.installation.version_manifest,
        "game_settings": game_settings,
        "game_provider_secrets": configured_provider_secret_statuses(&game_secrets),
        "experiments": catalogue,
        "csrf_token": admin_session.csrf_token,
    })))
}

/// Mode-specific readiness facts displayed beside experiment launch actions.
struct ExperimentCatalogueReadiness {
    configuration_valid: bool,
    configuration_error: Option<String>,
    local_testing_issues: Vec<String>,
    prolific_issues: Vec<String>,
}

/// Assesses structural validity and intake readiness without constructing a runtime router.
async fn experiment_catalogue_readiness(
    state: &ServerContext,
    experiment_id: &str,
    game_version: &str,
) -> ExperimentCatalogueReadiness {
    let mut config = match hydrated_experiment_config(state, experiment_id).await {
        Ok(config) => config,
        Err(error) => {
            return ExperimentCatalogueReadiness {
                configuration_valid: false,
                configuration_error: Some(error.message),
                local_testing_issues: Vec::new(),
                prolific_issues: Vec::new(),
            }
        }
    };
    if let Err(error) = config.validate() {
        return ExperimentCatalogueReadiness {
            configuration_valid: false,
            configuration_error: Some(error.to_string()),
            local_testing_issues: Vec::new(),
            prolific_issues: Vec::new(),
        };
    }
    if let Err(error) = parse_game_config(&state.installation.loaded_game, &config.game) {
        return ExperimentCatalogueReadiness {
            configuration_valid: false,
            configuration_error: Some(error.to_string()),
            local_testing_issues: Vec::new(),
            prolific_issues: Vec::new(),
        };
    }
    let mut local_testing_issues = Vec::new();
    if game_version != state.installation.game_descriptor.version.to_string() {
        local_testing_issues.push(
            "This experiment belongs to another game version; clone it before activation."
                .to_string(),
        );
    }
    local_testing_issues.extend(local_testing_issues_for_config(state, &mut config));
    let mut prolific_issues = local_testing_issues.clone();
    prolific_issues.extend(prolific_local_activation_issues(&config));
    if prolific_issues.is_empty() {
        let prolific_api_base_url = state
            .installation
            .game_settings
            .read()
            .await
            .prolific_api_base_url
            .clone();
        let (provider_issues, _) = cached_prolific_activation_preflight(
            state,
            experiment_id,
            &config,
            &prolific_api_base_url,
        )
        .await;
        prolific_issues.extend(provider_issues);
    }
    ExperimentCatalogueReadiness {
        configuration_valid: true,
        configuration_error: None,
        local_testing_issues,
        prolific_issues,
    }
}

/// Creates one inactive experiment for the exact installed game version.
pub(in crate::app) async fn admin_create_experiment(
    State(state): State<Arc<ServerContext>>,
    Json(request): Json<AdminCreateExperimentRequest>,
) -> Result<Json<Value>, AppError> {
    validate_new_experiment_id(&request.experiment_id)?;
    let game_settings = state.installation.game_settings.read().await.clone();
    let mut config = if let Some(mut value) = request.config {
        apply_provider_endpoint_defaults(&mut value, &game_settings)
            .map_err(|error| AppError::bad_request(error.to_string()))?;
        experiment_config_from_json(value, &state.config, &request.experiment_id)
            .map_err(|error| AppError::bad_request(error.to_string()))?
    } else {
        let mut config = state.config.clone();
        config.speechmatics.realtime_url = game_settings.speechmatics_realtime_url;
        config.tts.base_url = game_settings.tts_base_url;
        config
    };
    config.experiment.id = Some(request.experiment_id.clone());
    config
        .validate()
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    validate_agent_configuration(&state.agent_definitions, &config, false)
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    let persisted = persistable_config_json(&config)?;
    state
        .installation
        .store
        .create_experiment(ExperimentRecord {
            experiment_id: request.experiment_id.clone(),
            game_version: state.installation.game_descriptor.version.to_string(),
            config: persisted,
            server_version: Some(env!("CARGO_PKG_VERSION").to_string()),
            version_manifest: Some(state.installation.version_manifest.clone()),
            status: "inactive".to_string(),
            notes: request.notes,
        })
        .await
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    Ok(Json(json!({
        "experiment_id": request.experiment_id,
        "game_version": state.installation.game_descriptor.version,
        "status": "inactive",
    })))
}

/// Clones a historical configuration into a new current-version inactive experiment.
pub(in crate::app) async fn admin_clone_experiment(
    State(state): State<Arc<ServerContext>>,
    Path(source_experiment_id): Path<String>,
    Json(request): Json<AdminCloneExperimentRequest>,
) -> Result<Json<Value>, AppError> {
    validate_new_experiment_id(&request.experiment_id)?;
    let source = state
        .installation
        .store
        .experiment_definition(&source_experiment_id)
        .await?
        .ok_or_else(|| AppError::not_found("Source experiment not found."))?;
    let mut config =
        experiment_config_from_json(source.config, &state.config, &request.experiment_id).map_err(
            |error| {
                AppError::bad_request(format!(
                    "Source configuration is not valid for the current game version: {error}"
                ))
            },
        )?;
    config.experiment.id = Some(request.experiment_id.clone());
    config
        .validate()
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    state
        .installation
        .store
        .create_experiment(ExperimentRecord {
            experiment_id: request.experiment_id.clone(),
            game_version: state.installation.game_descriptor.version.to_string(),
            config: persistable_config_json(&config)?,
            server_version: Some(env!("CARGO_PKG_VERSION").to_string()),
            version_manifest: Some(state.installation.version_manifest.clone()),
            status: "inactive".to_string(),
            notes: None,
        })
        .await
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    Ok(Json(json!({
        "experiment_id": request.experiment_id,
        "source_experiment_id": source_experiment_id,
        "game_version": state.installation.game_descriptor.version,
        "status": "inactive",
    })))
}

/// Updates pinning, obsolescence, and notes without changing runtime configuration.
pub(in crate::app) async fn admin_update_experiment_catalogue(
    State(state): State<Arc<ServerContext>>,
    Path(experiment_id): Path<String>,
    Json(request): Json<AdminCatalogueRequest>,
) -> Result<Json<Value>, AppError> {
    state
        .installation
        .store
        .update_experiment_catalogue(&experiment_id, request.pinned, request.notes)
        .await
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    Ok(Json(json!({ "experiment_id": experiment_id, "ok": true })))
}

/// Archives catalogue state without constructing the selected experiment's runtime.
pub(in crate::app) async fn admin_archive_experiment(
    State(state): State<Arc<ServerContext>>,
    Path(experiment_id): Path<String>,
) -> Result<Json<Value>, AppError> {
    let runtime_guard = lock_runtime_router_build(&state, &experiment_id).await;
    state
        .installation
        .store
        .archive_experiment(&experiment_id)
        .await
        .map_err(|error| {
            let message = error.to_string();
            if message == "Experiment not found." {
                AppError::not_found(message)
            } else {
                AppError::new(StatusCode::CONFLICT, message)
            }
        })?;
    invalidate_runtime_router(&state, &experiment_id).await;
    drop(runtime_guard);
    Ok(Json(
        json!({ "experiment_id": experiment_id, "status": "archived" }),
    ))
}

/// Validates a dashboard-supplied experiment id using the public configuration contract.
fn validate_new_experiment_id(experiment_id: &str) -> Result<(), AppError> {
    if experiment_id.is_empty()
        || experiment_id.chars().count() > 128
        || !experiment_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(AppError::bad_request(
            "experiment_id must contain 1 to 128 letters, digits, dots, dashes, or underscores",
        ));
    }
    Ok(())
}
