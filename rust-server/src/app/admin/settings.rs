//! Manages the installation settings shared by experiments of the installed game. Updates
//! carry an expected revision and explicit provider secret changes. After persistence, the
//! handler refreshes the installation settings used by the host and invalidates inactive
//! runtime mounts.

use crate::app::{
    invalidate_inactive_runtime_routers, json, AppError, Arc, Deserialize, GameSettingsUpdate,
    HashMap, HeaderValue, IntoResponse, Json, Response, Result, ServerContext, State, StatusCode,
    StoredGameSettings, Value, CACHE_CONTROL,
};

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::app) struct AdminGameSettingsRequest {
    expected_revision: i64,
    institution: String,
    admin_allowed_ip_ranges: Vec<String>,
    speechmatics_realtime_url: Option<String>,
    tts_base_url: Option<String>,
    prolific_api_base_url: Option<String>,
    #[serde(default)]
    secret_updates: HashMap<String, String>,
    #[serde(default)]
    secret_deletions: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::app) struct AdminRevealGameSecretRequest {
    key: String,
}

/// Returns settings shared by every experiment of the installed game.
pub(in crate::app) async fn admin_game_settings(
    State(state): State<Arc<ServerContext>>,
) -> Json<StoredGameSettings> {
    Json(state.installation.game_settings.read().await.clone())
}

/// Updates the shared institution with optimistic concurrency.
pub(in crate::app) async fn admin_update_game_settings(
    State(state): State<Arc<ServerContext>>,
    Json(request): Json<AdminGameSettingsRequest>,
) -> Result<Json<Value>, AppError> {
    let institution = request.institution.trim().to_string();
    let admin_allowed_ip_ranges = request
        .admin_allowed_ip_ranges
        .into_iter()
        .map(|range| range.trim().to_string())
        .filter(|range| !range.is_empty())
        .collect::<Vec<_>>();
    for range in &admin_allowed_ip_ranges {
        range.parse::<ipnet::IpNet>().map_err(|error| {
            AppError::bad_request(format!("Invalid administrator IP range {range:?}: {error}"))
        })?;
    }
    let current_realtime_url = state
        .installation
        .game_settings
        .read()
        .await
        .speechmatics_realtime_url
        .clone();
    let speechmatics_realtime_url = request
        .speechmatics_realtime_url
        .unwrap_or(current_realtime_url)
        .trim()
        .to_string();
    crate::config::validate_websocket_url("Speechmatics realtime URL", &speechmatics_realtime_url)
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    let current_tts_base_url = state
        .installation
        .game_settings
        .read()
        .await
        .tts_base_url
        .clone();
    let tts_base_url = request
        .tts_base_url
        .unwrap_or(current_tts_base_url)
        .trim()
        .to_string();
    crate::config::validate_websocket_url("ElevenLabs base URL", &tts_base_url)
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    let current_prolific_api_base_url = state
        .installation
        .game_settings
        .read()
        .await
        .prolific_api_base_url
        .clone();
    let prolific_api_base_url = request
        .prolific_api_base_url
        .unwrap_or(current_prolific_api_base_url)
        .trim_end_matches('/')
        .to_string();
    crate::config::validate_provider_http_url("Prolific API base URL", &prolific_api_base_url)
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    for (key, value) in &request.secret_updates {
        validate_game_provider_secret(key, Some(value))?;
    }
    for key in &request.secret_deletions {
        validate_game_provider_secret(key, None)?;
    }
    let revision = state
        .installation
        .store
        .update_game_settings(GameSettingsUpdate {
            expected_revision: request.expected_revision,
            institution: institution.clone(),
            admin_allowed_ip_ranges: admin_allowed_ip_ranges.clone(),
            speechmatics_realtime_url: speechmatics_realtime_url.clone(),
            tts_base_url: tts_base_url.clone(),
            prolific_api_base_url: prolific_api_base_url.clone(),
            secret_updates: request.secret_updates,
            secret_deletions: request.secret_deletions,
        })
        .await
        .map_err(|error| AppError::new(StatusCode::CONFLICT, error.to_string()))?;
    *state.installation.game_settings.write().await = StoredGameSettings {
        institution,
        admin_allowed_ip_ranges,
        speechmatics_realtime_url,
        tts_base_url,
        prolific_api_base_url,
        revision,
    };
    invalidate_inactive_runtime_routers(&state).await;
    Ok(Json(json!({ "revision": revision })))
}

/// Validates one game-wide hosted-provider credential change.
fn validate_game_provider_secret(key: &str, value: Option<&str>) -> Result<(), AppError> {
    if !matches!(
        key,
        "speechmatics.api_key" | "tts.api_key" | "prolific.api_token"
    ) {
        return Err(AppError::bad_request(
            "Unknown game-wide provider credential",
        ));
    }
    if value.is_some_and(|value| value.is_empty() || value.len() > 65_536) {
        return Err(AppError::bad_request(
            "Provider credentials must contain 1 to 65536 bytes",
        ));
    }
    Ok(())
}

/// Reveals one game-wide provider credential after an explicit administrator action.
pub(in crate::app) async fn admin_reveal_game_secret(
    State(state): State<Arc<ServerContext>>,
    Json(request): Json<AdminRevealGameSecretRequest>,
) -> Result<Response, AppError> {
    validate_game_provider_secret(&request.key, None)?;
    let stored = state.installation.store.game_secrets().await?;
    let value = stored
        .get(&request.key)
        .cloned()
        .ok_or_else(|| AppError::not_found("Provider credential is not configured."))?;
    tracing::warn!(
        secret_key = request.key,
        "administrator revealed game provider credential"
    );
    let mut response = Json(json!({"key": request.key, "value": value})).into_response();
    response.headers_mut().insert(
        CACHE_CONTROL,
        HeaderValue::from_static("no-store, max-age=0"),
    );
    Ok(response)
}
