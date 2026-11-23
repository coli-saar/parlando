//! Handles administrator downloads and participant-data operations for the selected
//! experiment. The corpus projector formats a storage snapshot. Deletion resolves the
//! participant’s research identifier within the experiment and delegates the final
//! affected-session check and transaction to SQLite. A preview describes the impact at the
//! time of that read.

#[cfg(test)]
use crate::app::corpus::retain_export_sessions;
use crate::app::{
    admin_experiment_id, corpus_experiment_export, export_csv, json, redact_secret_fields,
    AdminExperimentScope, AppError, AppState, Arc, Deserialize, ExperimentSnapshot, Extension,
    HeaderValue, IntoResponse, Json, Path, Query, Response, Result, State, StatusCode, Value,
    CONTENT_DISPOSITION, CONTENT_TYPE, CORPUS_EXPORT_SCHEMA_V1,
};

#[derive(Clone, Debug, Deserialize)]
pub(in crate::app) struct AdminExportQuery {
    format: Option<String>,
    variant: Option<String>,
}

/// Downloads the exact JSON Schema enforced by the corpus export projector.
pub(in crate::app) async fn admin_corpus_export_schema() -> Response {
    attachment_response(
        CORPUS_EXPORT_SCHEMA_V1.to_string(),
        "application/schema+json; charset=utf-8",
        "parlando.corpus.v1.schema.json",
    )
}

/// Creates a download response with an experiment-id-safe filename and explicit media type.
pub(in crate::app) fn attachment_response(
    body: String,
    content_type: &'static str,
    filename: &str,
) -> Response {
    let mut response = body.into_response();
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
    response.headers_mut().insert(
        CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("attachment; filename=\"{filename}\""))
            .expect("validated experiment ids produce a valid attachment filename"),
    );
    response
}

/// Exports the selected experiment snapshot in the requested representation.
pub(in crate::app) async fn admin_export(
    State(state): State<Arc<AppState>>,
    scope: Option<Extension<AdminExperimentScope>>,
    Query(query): Query<AdminExportQuery>,
) -> Result<Response, AppError> {
    let experiment_id = admin_experiment_id(&state, scope.as_ref());
    let raw = load_export(&state, &experiment_id).await?;
    let mut value = match query.variant.as_deref().unwrap_or("corpus") {
        "corpus" => corpus_experiment_export(raw, &state.context.config.privacy.contract_version)
            .map_err(|error| AppError::new(StatusCode::CONFLICT, error.to_string()))?,
        #[cfg(test)]
        "full" => {
            let mut snapshot = raw;
            retain_export_sessions(&mut snapshot, false);
            serde_json::to_value(snapshot).map_err(anyhow::Error::from)?
        }
        other => {
            return Err(AppError::bad_request(format!(
                "Unsupported export variant {other:?}."
            )))
        }
    };
    tracing::info!(experiment_id, "administrator requested corpus export");
    redact_secret_fields(&mut value);
    if value
        .get("data_inventory")
        .and_then(|inventory| inventory.get("events"))
        .and_then(Value::as_u64)
        .is_some_and(|events| events > 100_000)
    {
        return Err(AppError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "Export exceeds the 100,000 event limit.",
        ));
    }
    let (content_type, body) = match query.format.as_deref().unwrap_or("json") {
        "json" => (
            "application/json; charset=utf-8",
            serde_json::to_string(&value).map_err(|error| {
                AppError::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
            })?,
        ),
        "yaml" | "yml" => (
            "application/yaml; charset=utf-8",
            serde_yaml::to_string(&value).map_err(|error| {
                AppError::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
            })?,
        ),
        "csv" => ("text/csv; charset=utf-8", export_csv(&value)),
        other => {
            return Err(AppError::bad_request(format!(
                "Unsupported export format {other:?}."
            )))
        }
    };
    if body.len() > 32 * 1024 * 1024 {
        return Err(AppError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "Export exceeds the 32 MiB response limit.",
        ));
    }
    let extension = match query.format.as_deref().unwrap_or("json") {
        "yaml" | "yml" => "yaml",
        "csv" => "csv",
        _ => "json",
    };
    Ok((
        [
            ("content-type", content_type),
            ("cache-control", "no-store"),
            ("x-content-type-options", "nosniff"),
            (
                "content-disposition",
                &format!(
                    "attachment; filename=\"{}-corpus-v1.{extension}\"",
                    experiment_id
                ),
            ),
        ],
        body,
    )
        .into_response())
}

/// Previews the bounded records affected by manual participant-data deletion.
pub(in crate::app) async fn admin_participant_deletion_preview(
    State(state): State<Arc<AppState>>,
    scope: Option<Extension<AdminExperimentScope>>,
    Path(requested): Path<String>,
) -> Result<Json<Value>, AppError> {
    let experiment_id = admin_experiment_id(&state, scope.as_ref());
    let participant_id = state
        .context
        .installation
        .store
        .participant_id_for_research_id(&experiment_id, &requested)
        .await?
        .ok_or_else(|| AppError::not_found("Participant not found."))?;
    let preview = state
        .context
        .installation
        .store
        .participant_data_preview(&experiment_id, participant_id)
        .await?;
    Ok(Json(json!({"research_id": requested, "preview": preview})))
}

/// Executes confirmed participant-data deletion and returns its audit counts.
pub(in crate::app) async fn admin_delete_participant_data(
    State(state): State<Arc<AppState>>,
    scope: Option<Extension<AdminExperimentScope>>,
    Path(requested): Path<String>,
) -> Result<Json<Value>, AppError> {
    let experiment_id = admin_experiment_id(&state, scope.as_ref());
    let Some(participant_id) = state
        .context
        .installation
        .store
        .participant_id_for_research_id(&experiment_id, &requested)
        .await?
    else {
        return Ok(Json(json!({
            "research_id": requested,
            "deleted": true,
            "already_absent": true,
        })));
    };
    let preview = state
        .context
        .installation
        .store
        .participant_data_preview(&experiment_id, participant_id)
        .await?;
    if preview.has_non_terminal_session {
        return Err(AppError::new(
            StatusCode::CONFLICT,
            "Participant deletion is blocked while an affected session is live or non-terminal.",
        ));
    }
    let deleted = state
        .context
        .installation
        .store
        .delete_participant_data(&experiment_id, participant_id)
        .await?;
    tracing::info!(
        experiment_id,
        research_id = requested,
        "administrator deleted participant data"
    );
    Ok(Json(json!({
        "research_id": requested,
        "deleted": true,
        "affected": deleted,
    })))
}

/// Loads the administrator export and checks its lifecycle at the same snapshot.
async fn load_export(
    state: &Arc<AppState>,
    experiment_id: &str,
) -> Result<ExperimentSnapshot, AppError> {
    let snapshot = state
        .context
        .installation
        .store
        .export_snapshot(experiment_id)
        .await?;
    let experiment = snapshot
        .experiment
        .as_ref()
        .ok_or_else(|| AppError::not_found("Configured experiment not found."))?;
    if experiment["status"] == "archived" {
        return Err(AppError::new(
            StatusCode::CONFLICT,
            "Archived experiments must be restored before export",
        ));
    }
    Ok(snapshot)
}
