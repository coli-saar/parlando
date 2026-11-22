//! Experiment activation checks and Prolific study preflight.
//!
//! Catalogue and launch handlers supply the effective configuration and registered
//! capabilities. They receive the issues that block intake. A Prolific preflight checks
//! the linked study and caches the result against the local inputs that were checked, so
//! an edited configuration cannot silently reuse an unrelated result.

use super::{
    hydrated_experiment_config, now_iso, prolific_setup_url, validate_agent_configuration,
    AppError, AppState, Arc, Digest, Duration, ExperimentConfig, HashMap, HashSet, Instant,
    ProlificPreflightCacheEntry, Result, ServerContext, Sha256, VerifiedProlificStudy,
};

/// Computes intake blockers while honoring providers injected by an embedding application.
pub(super) async fn experiment_activation_issues(
    state: &Arc<AppState>,
    experiment_id: &str,
) -> Result<Vec<String>, AppError> {
    let mut config = hydrated_experiment_config(&state.context, experiment_id).await?;
    let mut issues = activation_issues_for_config(state, &mut config);
    let prolific_api_base_url = state
        .context
        .installation
        .game_settings
        .read()
        .await
        .prolific_api_base_url
        .clone();
    let (prolific_issues, verified_study) = if issues.is_empty() {
        cached_prolific_activation_preflight(
            &state.context,
            experiment_id,
            &config,
            &prolific_api_base_url,
        )
        .await
    } else {
        (Vec::new(), None)
    };
    issues.extend(prolific_issues);
    if issues.is_empty() {
        *state.prolific_study.write().await = verified_study;
        *state.prolific_client.write().await = if config.recruitment.prolific.enabled {
            Some(crate::prolific::ProlificClient::with_base_url(
                config.recruitment.prolific.api_token.clone(),
                prolific_api_base_url,
            )?)
        } else {
            None
        };
    }
    Ok(issues)
}

/// Reuses a recent provider check when its complete local input contract is unchanged.
pub(super) async fn cached_prolific_activation_preflight(
    state: &ServerContext,
    experiment_id: &str,
    config: &ExperimentConfig,
    prolific_api_base_url: &str,
) -> (Vec<String>, Option<VerifiedProlificStudy>) {
    let fingerprint = prolific_preflight_fingerprint(config, prolific_api_base_url);
    if let Some(cached) = state
        .installation
        .prolific_preflight_cache
        .read()
        .await
        .get(experiment_id)
        .filter(|cached| {
            cached.fingerprint == fingerprint
                && cached.checked_at.elapsed() < Duration::from_secs(30)
        })
        .cloned()
    {
        return (cached.issues, cached.study);
    }
    run_and_cache_prolific_activation_preflight(state, experiment_id, config, prolific_api_base_url)
        .await
}

/// Performs a fresh provider check and publishes its result for badges and Start.
pub(super) async fn run_and_cache_prolific_activation_preflight(
    state: &ServerContext,
    experiment_id: &str,
    config: &ExperimentConfig,
    prolific_api_base_url: &str,
) -> (Vec<String>, Option<VerifiedProlificStudy>) {
    let fingerprint = prolific_preflight_fingerprint(config, prolific_api_base_url);
    let (issues, study) = prolific_activation_preflight(config, prolific_api_base_url).await;
    state
        .installation
        .prolific_preflight_cache
        .write()
        .await
        .insert(
            experiment_id.to_string(),
            ProlificPreflightCacheEntry {
                fingerprint,
                checked_at: Instant::now(),
                issues: issues.clone(),
                study: study.clone(),
            },
        );
    (issues, study)
}

/// Hashes every local value that can change the result of a Prolific study preflight.
fn prolific_preflight_fingerprint(
    config: &ExperimentConfig,
    prolific_api_base_url: &str,
) -> String {
    let prolific = &config.recruitment.prolific;
    let paths = &prolific.completion_paths;
    let values = [
        prolific_api_base_url,
        prolific.api_token.as_str(),
        prolific.study_id.as_str(),
        config.server.public_base_url.as_str(),
        paths.completed.as_str(),
        paths.partner_left.as_str(),
        paths.game_did_not_start.as_str(),
        paths.participation_ended_early.as_str(),
        paths.technical_failure.as_str(),
        paths.no_consent.as_str(),
    ];
    let mut digest = Sha256::new();
    digest.update([u8::from(prolific.enabled)]);
    for value in values {
        digest.update((value.len() as u64).to_be_bytes());
        digest.update(value.as_bytes());
    }
    digest.update(config.session.waiting_session_timeout_seconds.to_be_bytes());
    digest.update(config.session.session_max_lifetime_seconds.to_be_bytes());
    format!("{:x}", digest.finalize())
}

/// Verifies the linked Prolific study without changing provider-owned configuration.
async fn prolific_activation_preflight(
    config: &ExperimentConfig,
    prolific_api_base_url: &str,
) -> (Vec<String>, Option<VerifiedProlificStudy>) {
    if !config.recruitment.prolific.enabled {
        return (Vec::new(), None);
    }
    let local_issues = prolific_local_activation_issues(config);
    if !local_issues.is_empty() {
        return (local_issues, None);
    }
    let client = match crate::prolific::ProlificClient::with_base_url(
        config.recruitment.prolific.api_token.clone(),
        prolific_api_base_url,
    ) {
        Ok(client) => client,
        Err(error) => return (vec![error.to_string()], None),
    };
    let study = match client.study(&config.recruitment.prolific.study_id).await {
        Ok(study) => study,
        Err(error) => {
            return (
                vec![format!(
                    "Could not verify the linked Prolific study: {error}"
                )],
                None,
            )
        }
    };
    let mut issues = Vec::new();
    if study.id != config.recruitment.prolific.study_id {
        issues.push("The Prolific API returned a different study id.".to_string());
    }
    let workspace_id = match study.project.as_deref() {
        Some(project_id) => match client.project(project_id).await {
            Ok(project) => Some(project.workspace),
            Err(error) => {
                issues.push(format!(
                    "Could not derive the Prolific study's workspace: {error}"
                ));
                None
            }
        },
        None => {
            issues.push("The Prolific study has no project workspace.".to_string());
            None
        }
    };
    if study.prolific_id_option != "url_parameters" {
        issues.push(
            "The Prolific study must record participant IDs through URL parameters.".to_string(),
        );
    }
    let expected_url = prolific_setup_url(&config.server.public_base_url);
    if study.external_study_url != expected_url {
        issues.push(format!(
            "The Prolific external study URL must exactly match {expected_url}."
        ));
    }
    let paths = &config.recruitment.prolific.completion_paths;
    let configured = HashMap::from([
        ("completed", paths.completed.as_str()),
        ("partner_left", paths.partner_left.as_str()),
        ("game_did_not_start", paths.game_did_not_start.as_str()),
        (
            "participation_ended_early",
            paths.participation_ended_early.as_str(),
        ),
        ("technical_failure", paths.technical_failure.as_str()),
        ("no_consent", paths.no_consent.as_str()),
    ]);
    issues.extend(crate::prolific::ProlificClient::completion_path_issues(
        &study,
        &configured,
    ));
    if study.estimated_completion_time.saturating_mul(60)
        < config.session.waiting_session_timeout_seconds
    {
        issues.push(
            "The Prolific estimated duration is shorter than the maximum partner wait.".to_string(),
        );
    }
    if study.maximum_allowed_time.is_some_and(|minutes| {
        (minutes * 60.0) < config.session.session_max_lifetime_seconds as f64
    }) {
        issues.push(
            "The Prolific maximum allowed time is shorter than Parlando's maximum session lifetime."
                .to_string(),
        );
    }
    (
        issues,
        workspace_id.map(|workspace_id| VerifiedProlificStudy {
            study,
            workspace_id,
            refreshed_at: now_iso(),
        }),
    )
}

/// Computes every runtime-readiness blocker from one hydrated configuration.
fn activation_issues_for_config(state: &AppState, config: &mut ExperimentConfig) -> Vec<String> {
    let mut issues = local_testing_issues_for_config(&state.context, config);
    issues.extend(prolific_local_activation_issues(config));
    issues
}

/// Computes readiness blockers shared by direct intake and synthetic local previews.
pub(super) fn local_testing_issues_for_config(
    state: &ServerContext,
    config: &mut ExperimentConfig,
) -> Vec<String> {
    if config.transcription_active() && state.has_transcription_provider {
        config.speechmatics.api_key = "provided-by-runtime".to_string();
    }
    if config.tts_active() && state.has_tts_provider {
        config.tts.api_key = "provided-by-runtime".to_string();
        config.tts.voice_id = "provided-by-runtime".to_string();
    }
    let mut issues = config.activation_issues();
    issues.extend(prolific_completion_path_issues(config));
    if let Err(error) = validate_agent_configuration(&state.agent_definitions, config, true) {
        issues.push(error.to_string());
    }
    issues
}

/// Validates the completion redirects exercised by every Prolific participant URL, including Local Preview.
pub(super) fn prolific_completion_path_issues(config: &ExperimentConfig) -> Vec<String> {
    if !config.recruitment.prolific.enabled {
        return Vec::new();
    }
    let paths = &config.recruitment.prolific.completion_paths;
    let codes = [
        paths.completed.as_str(),
        paths.partner_left.as_str(),
        paths.game_did_not_start.as_str(),
        paths.participation_ended_early.as_str(),
        paths.technical_failure.as_str(),
        paths.no_consent.as_str(),
    ];
    if codes.iter().any(|code| code.trim().is_empty()) {
        return vec![
            "Configure all six Prolific completion paths before starting this experiment."
                .to_string(),
        ];
    }
    if codes.iter().collect::<HashSet<_>>().len() != codes.len() {
        return vec!["Prolific completion path codes must be distinct.".to_string()];
    }
    Vec::new()
}

/// Lists missing installation-owned Prolific prerequisites without contacting Prolific.
pub(super) fn prolific_local_activation_issues(config: &ExperimentConfig) -> Vec<String> {
    if !config.recruitment.prolific.enabled {
        return Vec::new();
    }
    let mut issues = Vec::new();
    if config.recruitment.prolific.study_id.trim().is_empty() {
        issues.push(
            "Enter the linked Prolific study ID before starting this experiment.".to_string(),
        );
    }
    if config.recruitment.prolific.api_token.trim().is_empty() {
        issues.push(
            "Connect a Prolific API token in Game Settings before starting this experiment."
                .to_string(),
        );
    }
    if let Err(error) = crate::config::validate_provider_http_url(
        "The Prolific participant URL",
        &config.server.public_base_url,
    ) {
        issues.push(error.to_string());
    }
    issues
}

/// Reports whether Save has enough provider input to attempt remote study validation.
pub(super) fn prolific_configuration_complete(config: &ExperimentConfig) -> bool {
    if !config.recruitment.prolific.enabled
        || config.recruitment.prolific.study_id.trim().is_empty()
        || config.recruitment.prolific.api_token.trim().is_empty()
    {
        return false;
    }
    let paths = &config.recruitment.prolific.completion_paths;
    [
        paths.completed.as_str(),
        paths.partner_left.as_str(),
        paths.game_did_not_start.as_str(),
        paths.participation_ended_early.as_str(),
        paths.technical_failure.as_str(),
        paths.no_consent.as_str(),
    ]
    .iter()
    .all(|code| !code.trim().is_empty())
}
