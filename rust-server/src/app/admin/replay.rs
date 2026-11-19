use crate::app::{
    admin_experiment_id, json, protocol_json, AdminExperimentScope, AppError, AppState, Arc,
    Extension, Game, GameSessionContext, Json, Path, PlayerRole, Result, SessionLogger, State,
    StatusCode, Value,
};

/// Builds one role-specific replay frame from an authoritative reconstructed state.
fn replay_frame<A: Game>(
    game: &A,
    state: &A::State,
    role: PlayerRole,
    event_index: i64,
    game_time_ms: i64,
    transition: Option<Value>,
    conversation: &[Value],
) -> Result<Value> {
    Ok(json!({
        "event_index": event_index,
        "game_time_ms": game_time_ms,
        "observation": protocol_json(&game.observation(state, role))?,
        "available_actions": game
            .available_actions(state, role)
            .as_ref()
            .map(protocol_json)
            .transpose()?,
        "transition": transition,
        "conversation": conversation,
        "completion": game.completion(state).as_ref().map(protocol_json).transpose()?,
    }))
}

/// Reconstructs a recorded session by applying accepted actions to its one stored initial state.
pub(in crate::app) async fn admin_session_replay<A: Game>(
    State(state): State<Arc<AppState<A>>>,
    scope: Option<Extension<AdminExperimentScope>>,
    Path(session_id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let experiment_id = admin_experiment_id(&state, scope.as_ref());
    let events = state
        .context
        .installation
        .store
        .session_events(&experiment_id, session_id, None)
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
        "state": A::STATE_REPRESENTATION_VERSION,
        "action": A::ACTION_REPRESENTATION_VERSION,
        "rules": A::RULES_VERSION,
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
    let mut game_state: A::State = serde_json::from_value(initial_state).map_err(|error| {
        AppError::new(
            StatusCode::CONFLICT,
            format!("The recorded initial state is incompatible: {error}"),
        )
    })?;
    let game = state
        .context
        .installation
        .game_factory
        .create(GameSessionContext {
            logger: SessionLogger::headless(),
        })
        .map_err(|error| {
            AppError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Could not construct the game for replay: {error}"),
            )
        })?;
    let mut conversation = Vec::<Value>::new();
    let mut role_a = vec![replay_frame(
        &game,
        &game_state,
        PlayerRole::A,
        created.event_index,
        created.game_time_ms,
        None,
        &conversation,
    )?];
    let mut role_b = vec![replay_frame(
        &game,
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
                &game,
                &game_state,
                PlayerRole::A,
                event.event_index,
                event.game_time_ms,
                None,
                &conversation,
            )?);
            role_b.push(replay_frame(
                &game,
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
        let action: A::Action = serde_json::from_value(action_value.clone()).map_err(|error| {
            AppError::new(
                StatusCode::CONFLICT,
                format!(
                    "Accepted action #{} is incompatible: {error}",
                    event.event_index
                ),
            )
        })?;
        game_state = game
            .apply_action(&game_state, &action, actor)
            .map_err(|rejection| {
                AppError::new(
                    StatusCode::CONFLICT,
                    format!(
                        "Accepted action #{} no longer reconstructs: {}",
                        event.event_index, rejection.code
                    ),
                )
            })?;
        let transition = Some(json!({"actor": actor.as_str(), "action": action_value}));
        role_a.push(replay_frame(
            &game,
            &game_state,
            PlayerRole::A,
            event.event_index,
            event.game_time_ms,
            transition.clone(),
            &conversation,
        )?);
        role_b.push(replay_frame(
            &game,
            &game_state,
            PlayerRole::B,
            event.event_index,
            event.game_time_ms,
            transition,
            &conversation,
        )?);
    }
    Ok(Json(json!({
        "experiment_id": experiment_id,
        "session_id": session_id,
        "public_session_id": created.payload.get("public_session_id"),
        "compatibility": expected,
        "roles": {"A": role_a, "B": role_b},
    })))
}
