//! Changes the intake state of a mounted experiment. Under the mount lock, the handler
//! checks the current revision, activation requirements and allowed lifecycle transition.
//! It persists the new status before exposing it to admission and returning the selected
//! participant URL to the dashboard.

use crate::app::{
    experiment_activation_issues, hydrated_experiment_config, invalidate_runtime_router, json,
    local_testing_issues_for_config, lock_runtime_router_build, new_id, AdminSession, AppError,
    AppState, Arc, Deserialize, ExperimentConfig, ExperimentLifecycle, Extension, Json, Result,
    RunningParticipantUrl, State, StatusCode, Value,
};

#[derive(Clone, Debug, Deserialize)]
pub(in crate::app) struct AdminUpdateExperimentStatusRequest {
    status: String,
    #[serde(default)]
    verify_prolific: bool,
    participant_url_kind: Option<String>,
}

/// Chooses and materializes the participant URL that remains fixed for one intake run.
fn running_participant_url(
    config: &ExperimentConfig,
    lifecycle: ExperimentLifecycle,
    requested_kind: Option<&str>,
) -> Result<RunningParticipantUrl, AppError> {
    if !config.recruitment.prolific.enabled {
        if requested_kind.is_some_and(|kind| kind == "prolific") {
            return Err(AppError::bad_request(
                "A Direct experiment cannot use a Prolific participant URL",
            ));
        }
        return Ok(RunningParticipantUrl {
            kind: "direct".to_string(),
            participant_id: None,
            session_id: None,
        });
    }
    let kind = requested_kind.unwrap_or(if lifecycle == ExperimentLifecycle::Testing {
        "local"
    } else {
        "prolific"
    });
    match kind {
        "local" if lifecycle == ExperimentLifecycle::Testing => Ok(RunningParticipantUrl {
            kind: "local".to_string(),
            participant_id: Some(new_id("TEST").replace('_', "").to_uppercase()),
            session_id: Some(new_id("TEST").replace('_', "").to_uppercase()),
        }),
        "prolific" => Ok(RunningParticipantUrl {
            kind: "prolific".to_string(),
            participant_id: None,
            session_id: None,
        }),
        "local" => Err(AppError::bad_request(
            "Local Preview is available only while testing",
        )),
        _ => Err(AppError::bad_request(
            "participant_url_kind must be local or prolific",
        )),
    }
}

/// Returns the process-owned experiment with dashboard aggregates.
pub(in crate::app) async fn admin_experiment(
    State(state): State<Arc<AppState>>,
    Extension(admin_session): Extension<AdminSession>,
) -> Result<Json<Value>, AppError> {
    let experiment = state
        .context
        .installation
        .store
        .experiment_summary(&state.experiment_id)
        .await?
        .ok_or_else(|| AppError::not_found("Configured experiment not found."))?;
    Ok(Json(json!({
        "experiment": experiment,
        "game": state.context.installation.game_descriptor,
        "game_settings": state.context.installation.game_settings.read().await.clone(),
        "version_manifest": state.context.installation.version_manifest,
        "csrf_token": admin_session.csrf_token,
    })))
}

/// Updates a durable experiment lifecycle status.
pub(in crate::app) async fn admin_update_experiment_status(
    State(state): State<Arc<AppState>>,
    Json(request): Json<AdminUpdateExperimentStatusRequest>,
) -> Result<Json<Value>, AppError> {
    let _mount_guard = lock_runtime_router_build(&state.context, &state.experiment_id).await;
    if state.retired.load(std::sync::atomic::Ordering::Acquire) {
        return Err(AppError::new(
            StatusCode::CONFLICT,
            "Experiment configuration changed; reload the experiment.",
        ));
    }
    let lifecycle = ExperimentLifecycle::parse(&request.status)?;
    let stored = state
        .context
        .installation
        .store
        .experiment_definition(&state.experiment_id)
        .await?
        .ok_or_else(|| AppError::not_found("Configured experiment not found."))?;
    if lifecycle.allows_intake()
        && stored.game_version
            != state
                .context
                .installation
                .game_descriptor
                .version
                .to_string()
    {
        return Err(AppError::new(
            StatusCode::CONFLICT,
            "This experiment belongs to another game version; clone it before activation",
        ));
    }
    let next_participant_url = lifecycle
        .allows_intake()
        .then(|| {
            running_participant_url(
                &state.context.config,
                lifecycle,
                request.participant_url_kind.as_deref(),
            )
        })
        .transpose()?;
    if lifecycle.allows_intake() {
        let verify_prolific =
            request.verify_prolific || request.participant_url_kind.as_deref() == Some("prolific");
        let issues = if lifecycle == ExperimentLifecycle::Testing && !verify_prolific {
            let mut config =
                hydrated_experiment_config(&state.context, &state.experiment_id).await?;
            local_testing_issues_for_config(&state.context, &mut config)
        } else {
            experiment_activation_issues(&state, &state.experiment_id).await?
        };
        if !issues.is_empty() {
            return Err(AppError::new(
                StatusCode::CONFLICT,
                format!("Experiment cannot start: {}", issues.join(" ")),
            ));
        }
    }
    let mut current_lifecycle = state.experiment_lifecycle.write().await;
    if !current_lifecycle.can_transition_to(lifecycle) {
        return Err(AppError::new(
            StatusCode::CONFLICT,
            format!(
                "Experiment cannot transition directly from {} to {}",
                current_lifecycle.as_str(),
                lifecycle.as_str()
            ),
        ));
    }
    state
        .context
        .installation
        .store
        .update_experiment_status(&state.experiment_id, lifecycle.as_str())
        .await?;
    *current_lifecycle = lifecycle;
    if lifecycle == ExperimentLifecycle::Inactive {
        invalidate_runtime_router(&state.context, &state.experiment_id).await;
    }
    *state.participant_url.write().await = next_participant_url;
    tracing::info!(experiment_id = %state.experiment_id, status = lifecycle.as_str(), "administrator updated experiment status");
    let participant_url = state.participant_url.read().await.clone();
    Ok(Json(json!({
        "experiment_id": state.experiment_id,
        "status": lifecycle.as_str(),
        "participant_url": participant_url,
    })))
}
