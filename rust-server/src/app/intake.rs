//! Participant registration, consent declarations, and recruitment handoff.
//!
//! Registration verifies the selected recruitment path and issues the participant
//! identity used by admission. Consent records bind the participant's declarations to the
//! configured items. Terminal projection uses the recorded participant outcome and
//! recruitment configuration to select a Prolific handoff.

use super::{
    authenticated_participant_id, consent_configuration_hash, enforce_creation_rate,
    experiment_occupancy, json, nonempty_string, now_iso, require_open_experiment, AppError,
    AppState, Arc, ConsentDeclarationRecord, ConsentRequest, ExperimentConfig, ExperimentLifecycle,
    Extension, Game, HashMap, Json, ParticipantCreateRequest, ParticipantCreateResponse,
    ParticipantOutcomeKind, ParticipantPrincipal, ParticipantRecord, ProlificParticipantRequest,
    ProlificSubmissionRecord, RecruitmentHandoff, Result, RunningParticipantUrl, State, StatusCode,
    Value,
};

/// Builds the no-consent handoff for an active run without ever emitting an empty code.
pub(super) fn prolific_no_consent_url(
    config: &ExperimentConfig,
    participant_url: Option<&RunningParticipantUrl>,
) -> Option<String> {
    if config.direct.consents.is_empty() || participant_url.is_none() {
        return None;
    }
    let code = config
        .recruitment
        .prolific
        .completion_paths
        .no_consent
        .trim();
    (!code.is_empty()).then(|| prolific_completion_url(code))
}

/// Builds Prolific's documented participant completion redirect for one validated code.
fn prolific_completion_url(code: &str) -> String {
    format!("https://app.prolific.com/submissions/complete?cc={code}")
}

/// Registers a participant using the configured recruitment policy.
pub(super) async fn create_participant<A: Game>(
    State(state): State<Arc<AppState<A>>>,
    Json(request): Json<ParticipantCreateRequest>,
) -> Result<Json<ParticipantCreateResponse>, AppError> {
    create_participant_inner(state, request).await.map(Json)
}

/// Recognizes dashboard-generated Prolific identities which are safe only during testing intake.
fn is_synthetic_prolific_testing_launch(
    lifecycle: ExperimentLifecycle,
    prolific: &ProlificParticipantRequest,
) -> bool {
    lifecycle == ExperimentLifecycle::Testing
        && prolific.prolific_token.is_none()
        && prolific.participant_id.starts_with("TEST")
        && prolific.session_id.starts_with("TEST")
        && prolific
            .participant_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric())
        && prolific
            .session_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric())
}

/// Validates intake and publishes a new or recovered participant credential.
async fn create_participant_inner<A: Game>(
    state: Arc<AppState<A>>,
    request: ParticipantCreateRequest,
) -> Result<ParticipantCreateResponse, AppError> {
    let intake_guard = require_open_experiment(&state).await?;
    enforce_creation_rate(&state).await?;
    if !state.context.config.direct.enabled && !state.context.config.recruitment.prolific.enabled {
        return Err(AppError::not_found("Direct mode is disabled."));
    }
    let prolific = if state.context.config.recruitment.prolific.enabled {
        let prolific = request.prolific.ok_or_else(|| {
            AppError::bad_request("Prolific participant parameters are required.")
        })?;
        if prolific.study_id != state.context.config.recruitment.prolific.study_id {
            return Err(AppError::bad_request(
                "This Prolific study id is not accepted.",
            ));
        }
        for value in [&prolific.participant_id, &prolific.session_id] {
            if value.is_empty() || value.len() > 128 || !value.is_ascii() {
                return Err(AppError::bad_request(
                    "Invalid Prolific participant parameters.",
                ));
            }
        }
        let verification_method = if is_synthetic_prolific_testing_launch(*intake_guard, &prolific)
        {
            "synthetic_testing"
        } else {
            let client = state.prolific_client.read().await.clone().ok_or_else(|| {
                AppError::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Prolific intake is not connected",
                )
            })?;
            let verified_study = state.prolific_study.read().await.clone().ok_or_else(|| {
                AppError::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Prolific study preflight has not completed",
                )
            })?;
            if verified_study.study.is_external_study_url_secure {
                let token = prolific.prolific_token.as_deref().ok_or_else(|| {
                    AppError::bad_request("The secure Prolific launch token is missing.")
                })?;
                let verified = client
                    .verify_signed_launch(
                        token,
                        crate::prolific::SignedLaunchExpectation {
                            audience: &verified_study.study.external_study_url,
                            study_id: &state.context.config.recruitment.prolific.study_id,
                            workspace_id: &verified_study.workspace_id,
                        },
                    )
                    .await
                    .map_err(|error| {
                        AppError::bad_request(format!("Invalid Prolific launch: {error}"))
                    })?;
                if verified.participant_id != prolific.participant_id
                    || verified.study_id != prolific.study_id
                    || verified.session_id != prolific.session_id
                {
                    return Err(AppError::bad_request(
                        "The signed Prolific launch does not match its URL parameters.",
                    ));
                }
                "signed_url"
            } else {
                client
                    .verify_unsigned_launch(
                        &prolific.participant_id,
                        &prolific.study_id,
                        &prolific.session_id,
                    )
                    .await
                    .map_err(|error| {
                        AppError::new(
                            StatusCode::SERVICE_UNAVAILABLE,
                            format!("Could not verify the Prolific submission: {error}"),
                        )
                    })?;
                "submission_api"
            }
        };
        Some((prolific, verification_method))
    } else {
        None
    };
    let _admission_guard = state.session_admission.reserve().await;
    let usage = experiment_occupancy(&state).await;
    let capacity_available =
        usage.unattached < state.context.config.capacity.max_unattached_participants;
    let intake_full = || {
        AppError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "Participant intake is temporarily full",
        )
    };
    let participant = if let Some((prolific, verification_method)) = prolific {
        let admission = state
            .context
            .installation
            .store
            .admit_prolific_submission(
                ProlificSubmissionRecord {
                    experiment_id: state.experiment_id.clone(),
                    prolific_participant_id: prolific.participant_id,
                    prolific_study_id: prolific.study_id,
                    prolific_session_id: prolific.session_id,
                    verification_method: verification_method.to_string(),
                },
                capacity_available,
            )
            .await?
            .ok_or_else(intake_full)?;
        // A verified returning submission keeps its live room even when intake has reopened.
        if let Some(cache) = state.context.installation.runtime_router_cache.upgrade() {
            let previous = cache
                .read()
                .await
                .get(&state.experiment_id)
                .map(|mounts| mounts.draining_states())
                .unwrap_or_default();
            for owner in previous {
                let assigned = owner.memory.read().await.sessions.values().any(|room| {
                    !room.lifecycle().is_ended()
                        && room
                            .participants
                            .contains_key(&admission.participant_session_id)
                });
                if assigned {
                    return Ok(ParticipantCreateResponse {
                        participant_credential: state
                            .participant_auth
                            .issue(admission.participant_session_id, owner.mount_id)
                            .await,
                        participant_id: admission.research_id,
                    });
                }
            }
        }
        if !capacity_available
            && !state
                .memory
                .read()
                .await
                .participants
                .contains_key(&admission.participant_session_id)
        {
            return Err(intake_full());
        }
        state.memory.write().await.create_participant_with_id(
            admission.participant_session_id,
            admission.participant_id,
            admission.research_id,
            "prolific".to_string(),
            intake_guard.data_purpose().to_string(),
        )
    } else {
        if !capacity_available {
            return Err(intake_full());
        }
        let participant_id = state
            .context
            .installation
            .store
            .upsert_participant(ParticipantRecord {
                experiment_id: state.experiment_id.clone(),
                participant_kind: "human".to_string(),
                identity_provider: "direct".to_string(),
                external_id: None,
                metadata: Value::Null,
            })
            .await?;
        let research_id = state
            .context
            .installation
            .store
            .participant_research_id(participant_id)
            .await?
            .ok_or_else(|| {
                AppError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Participant identifier is missing",
                )
            })?;
        state.memory.write().await.create_participant(
            participant_id,
            research_id,
            "direct".to_string(),
            intake_guard.data_purpose().to_string(),
        )
    };
    drop(_admission_guard);
    let participant_credential = state
        .participant_auth
        .issue(participant.id.clone(), state.mount_id)
        .await;
    Ok(ParticipantCreateResponse {
        participant_credential,
        participant_id: participant.research_id,
    })
}

/// Persists the participant's consent declarations and resulting eligibility.
pub(super) async fn consent<A: Game>(
    State(state): State<Arc<AppState<A>>>,
    principal: Option<Extension<ParticipantPrincipal>>,
    Json(request): Json<ConsentRequest>,
) -> Result<Json<Value>, AppError> {
    let participant_session_id = authenticated_participant_id(principal)?;
    let configured_items = state
        .context
        .config
        .direct
        .consents
        .iter()
        .map(|item| (item.id.as_str(), item))
        .collect::<HashMap<_, _>>();
    if request
        .decisions
        .keys()
        .any(|item_id| !configured_items.contains_key(item_id.as_str()))
    {
        return Err(AppError::bad_request("Unknown consent item"));
    }
    let (participant_id, purpose, current_decisions) = {
        let memory = state.memory.read().await;
        let participant = memory
            .participants
            .get(&participant_session_id)
            .ok_or_else(|| AppError::not_found("Participant session not found."))?;
        (
            participant.participant_id,
            participant.purpose.clone(),
            participant.consent_decisions.clone(),
        )
    };
    let changed_decisions = request
        .decisions
        .iter()
        .filter(|(item_id, accepted)| current_decisions.get(*item_id) != Some(*accepted))
        .map(|(item_id, accepted)| (item_id.clone(), *accepted))
        .collect::<HashMap<_, _>>();
    if changed_decisions.is_empty() {
        return Ok(Json(json!({"ok": true})));
    }
    let session_id = if let Some(public_session_id) = request.public_session_id.as_ref() {
        state
            .memory
            .read()
            .await
            .sessions
            .get(public_session_id)
            .map(|session| session.session_id)
    } else {
        None
    };
    let consent_text_hash = Some(consent_configuration_hash(&state.context.config)?);
    let consent_metadata = json!({
        "participant_information_version": nonempty_string(&state.context.config.direct.participant_information_version),
        "participant_information_url": nonempty_string(&state.context.config.direct.participant_information_url),
    });
    for (consent_item_id, accepted) in &changed_decisions {
        state
            .context
            .installation
            .store
            .record_consent_declaration(ConsentDeclarationRecord {
                experiment_id: state.experiment_id.clone(),
                session_id,
                participant_id,
                purpose: purpose.clone(),
                consent_item_id: consent_item_id.clone(),
                accepted: *accepted,
                consent_text_hash: consent_text_hash.clone(),
                metadata: consent_metadata.clone(),
            })
            .await?;
    }
    let mut memory = state.memory.write().await;
    let participant = memory
        .participants
        .get_mut(&participant_session_id)
        .ok_or_else(|| AppError::not_found("Participant session not found."))?;
    participant
        .consent_decisions
        .extend(changed_decisions.clone());
    participant.updated_at = now_iso();
    for session in memory.sessions.values_mut() {
        if request
            .public_session_id
            .as_ref()
            .is_some_and(|public_session_id| public_session_id != &session.id)
        {
            continue;
        }
        if let Some(session_participant) = session.participants.get_mut(&participant_session_id) {
            session_participant
                .consent_decisions
                .extend(changed_decisions.clone());
            session_participant.updated_at = now_iso();
        }
    }
    Ok(Json(json!({"ok": true})))
}

/// Checks required declarations before a participant can acquire a seat.
pub(super) async fn require_consent<A: Game>(
    state: &Arc<AppState<A>>,
    participant_session_id: &str,
) -> Result<(), AppError> {
    let memory = state.memory.read().await;
    let participant = memory
        .participants
        .get(participant_session_id)
        .ok_or_else(|| AppError::not_found("Participant session not found."))?;
    let missing = state
        .context
        .config
        .direct
        .consents
        .iter()
        .filter(|item| item.required)
        .filter(|item| {
            !participant
                .consent_decisions
                .get(&item.id)
                .copied()
                .unwrap_or(false)
        })
        .map(|item| item.title.clone())
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(AppError::forbidden(format!(
            "Consent is required before entering the game: {}.",
            missing.join(", ")
        )));
    }
    Ok(())
}

/// Selects a configured Prolific code only after a durable provider-neutral outcome exists.
pub(super) fn prolific_handoff(
    config: &ExperimentConfig,
    outcome: &ParticipantOutcomeKind,
    game_started: bool,
) -> Option<RecruitmentHandoff> {
    let paths = &config.recruitment.prolific.completion_paths;
    let code =
        match outcome {
            ParticipantOutcomeKind::Completed => &paths.completed,
            ParticipantOutcomeKind::PartnerLeft => &paths.partner_left,
            ParticipantOutcomeKind::LeftWaitingRoom
            | ParticipantOutcomeKind::PartnerUnavailable => &paths.game_did_not_start,
            ParticipantOutcomeKind::ConnectionLost if !game_started => &paths.game_did_not_start,
            ParticipantOutcomeKind::LeftGame
            | ParticipantOutcomeKind::ConnectionLost
            | ParticipantOutcomeKind::IdleLimitReached => &paths.participation_ended_early,
            ParticipantOutcomeKind::TechnicalFailure
            | ParticipantOutcomeKind::LifetimeLimitReached => &paths.technical_failure,
        };
    if code.is_empty() {
        return None;
    }
    Some(RecruitmentHandoff {
        provider: "prolific".to_string(),
        code: code.clone(),
        url: prolific_completion_url(code),
    })
}
