//! Reconstructs a recorded game for the administrator replay viewer. It starts from the
//! stored initial state, applies accepted actions in event order and asks the game for each
//! role’s observation. Frames carry the event index and game clock
//! used by the browser player.

use crate::app::{
    admin_experiment_id, json, protocol_json, AdminExperimentScope, AppError, AppState, Arc,
    ComponentValue, Extension, GameFrame, Json, Path, PlayerRole, Result, ServerContext, State,
    StatusCode, Value,
};

/// Builds one role-specific replay frame from an authoritative reconstructed state.
fn replay_frame(
    state: &GameFrame,
    role: PlayerRole,
    event_index: i64,
    game_time_ms: i64,
    transition: Option<Value>,
    conversation: &[Value],
) -> Result<Value> {
    Ok(json!({
        "event_index": event_index,
        "game_time_ms": game_time_ms,
        "observation": protocol_json(&state.observation(role))?,
        "available_actions": Value::Null,
        "transition": transition,
        "conversation": conversation,
        "completion": state.completion().as_ref().map(protocol_json).transpose()?,
    }))
}

/// Reconstructs a recorded session by applying accepted actions to its one stored initial state.
pub(in crate::app) async fn admin_session_replay(
    State(state): State<Arc<AppState>>,
    scope: Option<Extension<AdminExperimentScope>>,
    Path(session_id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let experiment_id = admin_experiment_id(&state, scope.as_ref());
    experiment_session_replay(&state.context, &experiment_id, session_id).await
}

/// Produces the same authorized replay for the dashboard and a scoped hosted replay grant.
pub(in crate::app) async fn experiment_session_replay(
    state: &ServerContext,
    experiment_id: &str,
    session_id: i64,
) -> Result<Json<Value>, AppError> {
    let events = state
        .installation
        .store
        .session_events(experiment_id, session_id, None)
        .await?;
    let created = events
        .iter()
        .find(|event| event.event_type == "session_created")
        .ok_or_else(|| AppError::not_found("Session not found."))?;
    let recorded = created.payload.get("replay_compatibility").ok_or_else(|| {
        AppError::new(
            StatusCode::CONFLICT,
            "This session predates replay recording.",
        )
    })?;
    let expected = json!({
        "state": "1",
        "action": "1",
        "rules": "1",
    });
    if recorded != &expected {
        return Err(AppError::new(
            StatusCode::CONFLICT,
            format!(
                "Replay compatibility mismatch: recorded {}, current {}.",
                recorded, expected
            ),
        ));
    }
    let initial_state = created.game_state.clone().ok_or_else(|| {
        AppError::new(
            StatusCode::CONFLICT,
            "The recorded session has no initial authoritative state.",
        )
    })?;
    let mut game_state: GameFrame = serde_json::from_value(initial_state).map_err(|error| {
        AppError::new(
            StatusCode::CONFLICT,
            format!("The recorded initial state is incompatible: {error}"),
        )
    })?;
    let game = state.installation.loaded_game.session();
    game.validate_replay_state(&game_state)
        .map_err(|error| AppError::new(StatusCode::CONFLICT, error.to_string()))?;
    let mut conversation = Vec::<Value>::new();
    let mut role_a = vec![replay_frame(
        &game_state,
        PlayerRole::A,
        created.event_index,
        created.game_time_ms,
        None,
        &conversation,
    )?];
    let mut role_b = vec![replay_frame(
        &game_state,
        PlayerRole::B,
        created.event_index,
        created.game_time_ms,
        None,
        &conversation,
    )?];
    for event in events.iter().filter(|event| {
        matches!(
            event.event_type.as_str(),
            "game_action_accepted" | "conversation_message"
        )
    }) {
        if event.event_type == "conversation_message" {
            let input = if event.payload["origin"] == "voice_transcript" {
                "voice_transcript"
            } else {
                "text"
            };
            let sender = event.payload["sender_role"].as_str().ok_or_else(|| {
                AppError::new(
                    StatusCode::CONFLICT,
                    format!(
                        "Conversation event #{} has no sender role.",
                        event.event_index
                    ),
                )
            })?;
            conversation.push(json!({
                "id": event.payload["id"],
                "sender": sender,
                "text": event.payload["text"],
                "input": input,
                "createdAt": "",
            }));
            role_a.push(replay_frame(
                &game_state,
                PlayerRole::A,
                event.event_index,
                event.game_time_ms,
                None,
                &conversation,
            )?);
            role_b.push(replay_frame(
                &game_state,
                PlayerRole::B,
                event.event_index,
                event.game_time_ms,
                None,
                &conversation,
            )?);
            continue;
        }
        let actor = match event.actor_role.as_deref() {
            Some("A") => PlayerRole::A,
            Some("B") => PlayerRole::B,
            _ => {
                return Err(AppError::new(
                    StatusCode::CONFLICT,
                    format!(
                        "Accepted action #{} has no valid actor role.",
                        event.event_index
                    ),
                ))
            }
        };
        let action_value = event.payload.get("action").cloned().ok_or_else(|| {
            AppError::new(
                StatusCode::CONFLICT,
                format!(
                    "Accepted action #{} has no action value.",
                    event.event_index
                ),
            )
        })?;
        let action: ComponentValue =
            serde_json::from_value(action_value.clone()).map_err(|error| {
                AppError::new(
                    StatusCode::CONFLICT,
                    format!(
                        "Accepted action #{} is incompatible: {error}",
                        event.event_index
                    ),
                )
            })?;
        game_state = if event.payload["recovery_checkpoint"] == true {
            let checkpoint = event.game_state.as_ref().ok_or_else(|| {
                AppError::new(
                    StatusCode::CONFLICT,
                    format!(
                        "Missing recovery checkpoint at action #{}",
                        event.event_index
                    ),
                )
            })?;
            let restored: GameFrame =
                serde_json::from_value(checkpoint.clone()).map_err(|error| {
                    AppError::new(
                        StatusCode::CONFLICT,
                        format!(
                            "Invalid checkpoint at action #{}: {error}",
                            event.event_index
                        ),
                    )
                })?;
            game.validate_replay_state(&restored)
                .map_err(|error| AppError::new(StatusCode::CONFLICT, error.to_string()))?;
            restored
        } else {
            game.step(&game_state, &action, actor)
                .map_err(|error| {
                    AppError::new(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!(
                            "Game failed while reconstructing action #{}: {error}",
                            event.event_index
                        ),
                    )
                })?
                .map_err(|rejection| {
                    AppError::new(
                        StatusCode::CONFLICT,
                        format!(
                            "Accepted action #{} no longer reconstructs: {}",
                            event.event_index, rejection.code
                        ),
                    )
                })?
        };
        let transition = Some(json!({"actor": actor.as_str(), "action": action_value}));
        role_a.push(replay_frame(
            &game_state,
            PlayerRole::A,
            event.event_index,
            event.game_time_ms,
            transition.clone(),
            &conversation,
        )?);
        role_b.push(replay_frame(
            &game_state,
            PlayerRole::B,
            event.event_index,
            event.game_time_ms,
            transition,
            &conversation,
        )?);
    }
    let communication = state
        .installation
        .store
        .session_communication(experiment_id, session_id)
        .await?
        .ok_or_else(|| {
            AppError::new(
                StatusCode::CONFLICT,
                "The session configuration revision is missing.",
            )
        })?;
    Ok(Json(json!({
        "experiment_id": experiment_id,
        "session_id": session_id,
        "public_session_id": created.payload.get("public_session_id"),
        "compatibility": expected,
        "communication": communication,
        "roles": {"A": role_a, "B": role_b},
    })))
}
