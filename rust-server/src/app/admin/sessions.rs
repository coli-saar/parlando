use crate::app::{
    admin_event_bundles, admin_experiment_id, important_admin_events, json, AdminExperimentScope,
    AppError, AppState, Arc, Deserialize, Extension, Game, Json, Path, Query, Result,
    SessionLifecycle, State, Value,
};

#[derive(Clone, Debug, Deserialize)]
pub(in crate::app) struct AdminSessionsQuery {
    limit: Option<i64>,
    lifecycle: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub(in crate::app) struct AdminEventsQuery {
    after: Option<i64>,
}

/// Returns recent database sessions for the experiment dashboard.
pub(in crate::app) async fn admin_sessions<A: Game>(
    State(state): State<Arc<AppState<A>>>,
    scope: Option<Extension<AdminExperimentScope>>,
    Query(query): Query<AdminSessionsQuery>,
) -> Result<Json<Value>, AppError> {
    let experiment_id = admin_experiment_id(&state, scope.as_ref());
    if query
        .lifecycle
        .as_deref()
        .is_some_and(|lifecycle| !matches!(lifecycle, "forming" | "running" | "ended"))
    {
        return Err(AppError::bad_request(
            "Session lifecycle must be forming, running, or ended.",
        ));
    }
    let sessions = state
        .context
        .installation
        .store
        .recent_sessions(&experiment_id, query.limit.unwrap_or(50))
        .await?;
    let sessions = sessions
        .into_iter()
        .filter(|session| {
            query
                .lifecycle
                .as_ref()
                .is_none_or(|lifecycle| session.lifecycle == *lifecycle)
        })
        .collect::<Vec<_>>();
    Ok(Json(json!({
        "experiment_id": experiment_id,
        "sessions": sessions,
    })))
}

/// Returns one database session's metadata and important event timeline.
pub(in crate::app) async fn admin_session_detail<A: Game>(
    State(state): State<Arc<AppState<A>>>,
    scope: Option<Extension<AdminExperimentScope>>,
    Path(session_id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let experiment_id = admin_experiment_id(&state, scope.as_ref());
    let session = state
        .context
        .installation
        .store
        .session_detail(&experiment_id, session_id)
        .await?
        .ok_or_else(|| AppError::not_found("Session not found."))?;
    let public_session_id = &session.public_session_id;
    let durable_lifecycle = session.lifecycle.as_str();
    let live_participant_state = {
        let memory = state.memory.read().await;
        memory
            .sessions
            .get(public_session_id)
            .map(|live| match live.lifecycle() {
                SessionLifecycle::Forming => json!({"state": "waiting"}),
                SessionLifecycle::Running => match live.pause() {
                    Some(reason) => json!({"state": "paused", "reason": reason}),
                    None => json!({"state": "active"}),
                },
                SessionLifecycle::Ended(_) => json!({"state": "ended"}),
            })
    };
    let mut participants = state
        .context
        .installation
        .store
        .session_participants(&experiment_id, session_id)
        .await?;
    let mut reconciliation_issue = None;
    if let Some(client) = state.prolific_client.read().await.clone() {
        for participant in participants
            .iter()
            .filter(|row| row.prolific_session_id.is_some())
        {
            let prolific_session_id = participant
                .prolific_session_id
                .as_deref()
                .expect("filtered Prolific participant has a submission id");
            match client.submission(prolific_session_id).await {
                Ok(submission) => {
                    if let Err(error) = state
                        .context
                        .installation
                        .store
                        .reconcile_prolific_submission(
                            &experiment_id,
                            prolific_session_id,
                            &submission.status,
                            submission.entered_code,
                            submission.return_requested,
                        )
                        .await
                    {
                        reconciliation_issue = Some(error.to_string());
                    }
                }
                Err(error) => reconciliation_issue = Some(error.to_string()),
            }
        }
        participants = state
            .context
            .installation
            .store
            .session_participants(&experiment_id, session_id)
            .await?;
    }
    let participants = participants
        .into_iter()
        .map(|participant| {
            let mut value = serde_json::to_value(&participant).map_err(anyhow::Error::from)?;
            value["research_id"] = json!(participant.research_id);
            value["participant_state"] = if let Some(result) = &participant.terminal_result {
                json!({"state": "ended", "result": result})
            } else if let Some(live_state) = &live_participant_state {
                live_state.clone()
            } else if durable_lifecycle == "forming" {
                json!({"state": "waiting"})
            } else if durable_lifecycle == "ended" {
                json!({"state": "ended"})
            } else {
                Value::Null
            };
            Ok::<Value, anyhow::Error>(value)
        })
        .collect::<Result<Vec<_>>>()?;
    let events = important_admin_events(
        state
            .context
            .installation
            .store
            .session_events(&experiment_id, session_id, None)
            .await?,
    );
    let event_bundles = admin_event_bundles(&events);
    Ok(Json(json!({
        "experiment_id": experiment_id,
        "session": session,
        "participants": participants,
        "events": events,
        "event_bundles": event_bundles,
        "prolific_reconciliation_issue": reconciliation_issue,
    })))
}

/// Returns important database events after an optional event index.
pub(in crate::app) async fn admin_session_events<A: Game>(
    State(state): State<Arc<AppState<A>>>,
    scope: Option<Extension<AdminExperimentScope>>,
    Path(session_id): Path<i64>,
    Query(query): Query<AdminEventsQuery>,
) -> Result<Json<Value>, AppError> {
    let after = query.after.unwrap_or(0);
    let experiment_id = admin_experiment_id(&state, scope.as_ref());
    let all_events = state
        .context
        .installation
        .store
        .session_events(&experiment_id, session_id, None)
        .await?;
    let events = all_events
        .clone()
        .into_iter()
        .filter(|event| event.event_index > after)
        .collect::<Vec<_>>();
    let visible_events = important_admin_events(events);
    let event_bundles = admin_event_bundles(&important_admin_events(all_events));
    Ok(Json(json!({
        "experiment_id": experiment_id,
        "session_id": session_id,
        "events": visible_events,
        "event_bundles": event_bundles,
    })))
}
