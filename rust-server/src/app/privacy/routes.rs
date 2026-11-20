//! Exposes installation-wide and experiment-specific privacy reports to authenticated
//! administrators. Each endpoint resolves the same configuration facts and report model
//! before selecting the HTML, JSON or Markdown representation.

use super::super::{attachment_response, AppError, Game, ServerContext};
use super::{
    facts, html::render_privacy_status_html, markdown::render_privacy_status_markdown, report,
    PrivacyStatus,
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{Html, Response},
    Json,
};
use std::sync::Arc;

/// Serves the installation-wide privacy status inside the protected administrator area.
pub(in crate::app) async fn admin_privacy_page<A: Game>(
    State(state): State<Arc<ServerContext<A>>>,
) -> Result<Html<String>, AppError> {
    Ok(Html(render_privacy_status_html(
        &privacy_status(&state, None).await?,
    )))
}

/// Returns the installation-wide privacy status as structured JSON for administrative tooling.
pub(in crate::app) async fn admin_privacy_json<A: Game>(
    State(state): State<Arc<ServerContext<A>>>,
) -> Result<Json<PrivacyStatus>, AppError> {
    Ok(Json(privacy_status(&state, None).await?))
}

/// Downloads the installation-wide privacy status as a pretty-printed JSON attachment.
pub(in crate::app) async fn admin_privacy_json_download<A: Game>(
    State(state): State<Arc<ServerContext<A>>>,
) -> Result<Response, AppError> {
    let body = serde_json::to_string_pretty(&privacy_status(&state, None).await?)
        .map_err(|error| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    Ok(attachment_response(
        body,
        "application/json; charset=utf-8",
        "parlando-privacy-status.json",
    ))
}

/// Downloads the installation-wide privacy status as a Markdown attachment for review records.
pub(in crate::app) async fn admin_privacy_markdown_download<A: Game>(
    State(state): State<Arc<ServerContext<A>>>,
) -> Result<Response, AppError> {
    Ok(attachment_response(
        render_privacy_status_markdown(&privacy_status(&state, None).await?),
        "text/markdown; charset=utf-8",
        "parlando-privacy-status.md",
    ))
}
/// Returns the privacy status for one experiment selected in the dashboard workspace.
pub(in crate::app) async fn admin_experiment_privacy_json<A: Game>(
    State(state): State<Arc<ServerContext<A>>>,
    Path(experiment_id): Path<String>,
) -> Result<Json<PrivacyStatus>, AppError> {
    Ok(Json(privacy_status(&state, Some(&experiment_id)).await?))
}

/// Downloads one experiment's privacy status as a pretty-printed JSON attachment.
pub(in crate::app) async fn admin_experiment_privacy_json_download<A: Game>(
    State(state): State<Arc<ServerContext<A>>>,
    Path(experiment_id): Path<String>,
) -> Result<Response, AppError> {
    let body =
        serde_json::to_string_pretty(&privacy_status(&state, Some(&experiment_id)).await?)
            .map_err(|error| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    Ok(attachment_response(
        body,
        "application/json; charset=utf-8",
        &format!("{experiment_id}-privacy-status.json"),
    ))
}

/// Downloads one experiment's privacy status as Markdown for review records.
pub(in crate::app) async fn admin_experiment_privacy_markdown_download<A: Game>(
    State(state): State<Arc<ServerContext<A>>>,
    Path(experiment_id): Path<String>,
) -> Result<Response, AppError> {
    Ok(attachment_response(
        render_privacy_status_markdown(&privacy_status(&state, Some(&experiment_id)).await?),
        "text/markdown; charset=utf-8",
        &format!("{experiment_id}-privacy-status.md"),
    ))
}

/// Resolves the requested scope before passing its facts to the report builder.
async fn privacy_status<A: Game>(
    state: &ServerContext<A>,
    experiment_id: Option<&str>,
) -> Result<PrivacyStatus, AppError> {
    let facts = facts::load(&state.installation.store, &state.config, experiment_id).await?;
    Ok(report::build_report(&facts, experiment_id))
}
