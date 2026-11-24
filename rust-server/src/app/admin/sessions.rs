//! Answers session-list, detail and event-poll requests in the selected experiment. Detail
//! combines durable session and participant records with the current room state when
//! available. Event reads follow indexed cursors and group the selected window for display.

use crate::app::{
    admin_experiment_id, important_admin_events, json, AdminExperimentScope, AppError, AppState,
    Arc, Deserialize, Extension, Json, Path, Query, Result, SessionLifecycle, State, Value,
};

#[derive(Clone, Debug, Deserialize)]
pub(in crate::app) struct AdminSessionsQuery {
    limit: Option<i64>,
    lifecycle: Option<String>,
}

/// Filters the session list by a literal suffix of a retained Prolific participant ID.
#[derive(Deserialize)]
pub(in crate::app) struct AdminSessionSearch {
    prolific_pid_suffix: String,
}

#[derive(Clone, Debug, Deserialize)]
pub(in crate::app) struct AdminEventsQuery {
    after: Option<i64>,
    before: Option<i64>,
    since: Option<i64>,
}

/// Returns recent database sessions for the experiment dashboard.
pub(in crate::app) async fn admin_sessions(
    State(state): State<Arc<AppState>>,
    scope: Option<Extension<AdminExperimentScope>>,
    Query(query): Query<AdminSessionsQuery>,
) -> Result<Json<Value>, AppError> {
    list_admin_sessions(&state, scope.as_ref(), query, None).await
}

/// Searches through the experiment's sessions using a request body for the private ID.
pub(in crate::app) async fn admin_search_sessions(
    State(state): State<Arc<AppState>>,
    scope: Option<Extension<AdminExperimentScope>>,
    Query(query): Query<AdminSessionsQuery>,
    Json(search): Json<AdminSessionSearch>,
) -> Result<Json<Value>, AppError> {
    let suffix = search.prolific_pid_suffix.trim();
    if suffix.is_empty() || suffix.chars().count() > 128 {
        return Err(AppError::bad_request(
            "Enter a Prolific participant ID suffix of 1 to 128 characters.",
        ));
    }
    list_admin_sessions(&state, scope.as_ref(), query, Some(suffix)).await
}

/// Validates the lifecycle filter and returns only session summaries for the current scope.
async fn list_admin_sessions(
    state: &AppState,
    scope: Option<&Extension<AdminExperimentScope>>,
    query: AdminSessionsQuery,
    prolific_pid_suffix: Option<&str>,
) -> Result<Json<Value>, AppError> {
    let experiment_id = admin_experiment_id(state, scope);
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
        .recent_sessions(
            &experiment_id,
            query.lifecycle.as_deref(),
            prolific_pid_suffix,
            query.limit.unwrap_or(50),
        )
        .await?;
    Ok(Json(json!({
        "experiment_id": experiment_id,
        "sessions": sessions,
    })))
}

/// Returns one database session's metadata and important event timeline.
pub(in crate::app) async fn admin_session_detail(
    State(state): State<Arc<AppState>>,
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
    let mut detail = session_timeline_page(&state, &experiment_id, session_id, None, None).await?;
    detail["experiment_id"] = json!(experiment_id);
    detail["session"] = json!(session);
    detail["participants"] = json!(participants);
    detail["prolific_reconciliation_issue"] = json!(reconciliation_issue);
    Ok(Json(detail))
}

/// Projects a bounded event window, enriching speech from the entire session and
/// optionally limiting the raw event summaries to rows newer than a polling cursor.
async fn session_timeline_page(
    state: &AppState,
    experiment_id: &str,
    session_id: i64,
    before: Option<i64>,
    after: Option<i64>,
) -> Result<Value, AppError> {
    let page = state
        .context
        .installation
        .store
        .timeline_page(experiment_id, session_id, before, after)
        .await?;
    let events = important_admin_events(page.events);
    let bundles = crate::app::timeline::page_event_bundles(&events, page.after_terminal);
    Ok(
        json!({"events": events, "event_bundles": bundles, "older_before": page.older_before, "last_index": page.last_index}),
    )
}

/// Returns important database events after an optional event index.
pub(in crate::app) async fn admin_session_events(
    State(state): State<Arc<AppState>>,
    scope: Option<Extension<AdminExperimentScope>>,
    Path(session_id): Path<i64>,
    Query(query): Query<AdminEventsQuery>,
) -> Result<Json<Value>, AppError> {
    let experiment_id = admin_experiment_id(&state, scope.as_ref());
    if [query.after, query.before, query.since]
        .into_iter()
        .flatten()
        .count()
        > 1
    {
        return Err(AppError::bad_request("Specify one timeline cursor."));
    }
    if let Some(since) = query.since {
        if state
            .context
            .installation
            .store
            .latest_event_index(&experiment_id, session_id)
            .await?
            == since
        {
            return Ok(Json(json!({"unchanged": true})));
        }
    }
    let mut page = session_timeline_page(
        &state,
        &experiment_id,
        session_id,
        query.before,
        query.after,
    )
    .await?;
    page["experiment_id"] = json!(experiment_id);
    page["session_id"] = json!(session_id);
    Ok(Json(page))
}
