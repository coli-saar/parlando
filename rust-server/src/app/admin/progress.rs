//! Builds dashboard progress from durable experiment-wide session counts and the configured
//! or provider-derived target. Prolific supplies participant places, which are converted to
//! sessions using the experiment’s pairing mode. The refresh operation updates the provider
//! facts used by the progress view.

use crate::app::{
    admin_experiment_id, hydrated_experiment_config, json,
    run_and_cache_prolific_activation_preflight, AdminExperimentScope, AgentsMode, AppError,
    AppState, Arc, ExperimentConfig, Extension, Json, Result, State, StatusCode, Value,
    VerifiedProlificStudy,
};

/// Derives a whole-session target from Prolific's participant-place count.
pub(in crate::app) fn prolific_planned_sessions(
    config: &ExperimentConfig,
    study: &crate::prolific::Study,
) -> Result<i64, String> {
    let places = study.total_available_places;
    if !places.is_finite() || places <= 0.0 || places.fract() != 0.0 {
        return Err("Prolific's available-place count is not a positive whole number.".to_string());
    }
    let participants_per_session = match config.agents.mode {
        AgentsMode::HumanVsHuman => 2_i64,
        AgentsMode::HumanVsAgent => 1_i64,
    };
    let places = places as i64;
    if places % participants_per_session != 0 {
        return Err(format!(
            "Prolific has {places} available places, which cannot form complete {participants_per_session}-participant sessions."
        ));
    }
    Ok(places / participants_per_session)
}

/// Refreshes the provider-owned study facts used by the progress target.
pub(in crate::app) async fn refresh_prolific_progress(
    state: &Arc<AppState>,
    experiment_id: &str,
) -> Result<VerifiedProlificStudy, AppError> {
    let config = hydrated_experiment_config(&state.context, experiment_id).await?;
    if !config.recruitment.prolific.enabled {
        return Err(AppError::bad_request(
            "This experiment does not use Prolific intake.",
        ));
    }
    let prolific_api_base_url = state
        .context
        .installation
        .game_settings
        .read()
        .await
        .prolific_api_base_url
        .clone();
    let (issues, verified) = run_and_cache_prolific_activation_preflight(
        &state.context,
        experiment_id,
        &config,
        &prolific_api_base_url,
    )
    .await;
    if !issues.is_empty() {
        return Err(AppError::new(StatusCode::CONFLICT, issues.join(" ")));
    }
    let verified = verified.ok_or_else(|| {
        AppError::new(
            StatusCode::BAD_GATEWAY,
            "Prolific did not return the linked study.",
        )
    })?;
    *state.prolific_study.write().await = Some(verified.clone());
    Ok(verified)
}

/// Adds the configured or provider-derived target to durable progress data.
async fn admin_progress_value(
    state: &Arc<AppState>,
    experiment_id: &str,
) -> Result<Value, AppError> {
    let progress = state
        .context
        .installation
        .store
        .session_progress(experiment_id)
        .await?;
    let config = hydrated_experiment_config(&state.context, experiment_id).await?;
    let mut value = serde_json::to_value(progress).map_err(anyhow::Error::from)?;
    let object = value
        .as_object_mut()
        .expect("stored progress serializes as an object");
    object.insert(
        "prolific_enabled".to_string(),
        json!(config.recruitment.prolific.enabled),
    );
    if let Some(planned_sessions) = config.progress.planned_sessions {
        object.insert("planned_sessions".to_string(), json!(planned_sessions));
        object.insert("plan_source".to_string(), json!("configured"));
    } else if config.recruitment.prolific.enabled {
        if let Some(verified) = state
            .prolific_study
            .read()
            .await
            .clone()
            .filter(|verified| verified.study.id == config.recruitment.prolific.study_id)
        {
            object.insert(
                "prolific_total_available_places".to_string(),
                json!(verified.study.total_available_places),
            );
            object.insert(
                "plan_refreshed_at".to_string(),
                json!(verified.refreshed_at),
            );
            match prolific_planned_sessions(&config, &verified.study) {
                Ok(planned_sessions) => {
                    object.insert("planned_sessions".to_string(), json!(planned_sessions));
                    object.insert("plan_source".to_string(), json!("prolific"));
                }
                Err(issue) => {
                    object.insert("plan_issue".to_string(), json!(issue));
                }
            }
        }
    }
    Ok(value)
}

/// Returns complete experiment-wide session counts and the chronological session log.
pub(in crate::app) async fn admin_progress(
    State(state): State<Arc<AppState>>,
    scope: Option<Extension<AdminExperimentScope>>,
) -> Result<Json<Value>, AppError> {
    let experiment_id = admin_experiment_id(&state, scope.as_ref());
    let progress = admin_progress_value(&state, &experiment_id).await?;
    Ok(Json(json!({
        "experiment_id": experiment_id,
        "progress": progress,
    })))
}

/// Performs an explicit Prolific refresh and returns the updated progress projection.
pub(in crate::app) async fn admin_refresh_progress(
    State(state): State<Arc<AppState>>,
    scope: Option<Extension<AdminExperimentScope>>,
) -> Result<Json<Value>, AppError> {
    let experiment_id = admin_experiment_id(&state, scope.as_ref());
    refresh_prolific_progress(&state, &experiment_id).await?;
    let progress = admin_progress_value(&state, &experiment_id).await?;
    Ok(Json(json!({
        "experiment_id": experiment_id,
        "progress": progress,
    })))
}
