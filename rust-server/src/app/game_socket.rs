//! Authenticated game transport and connection lifecycle.
//!
//! The upgrade route verifies origin, consumes the game ticket, and resolves the
//! participant's role. The socket registers its generation, forwards decoded input to the
//! message dispatcher, and delivers role-addressed server messages. Teardown consults the
//! registry before changing room presence so an old connection cannot disconnect its
//! replacement.

use super::{
    broadcast_participant_states, depart_session_locked, due_live_deadline, error_message,
    expire_if_due_locked, expire_live_session_locked, flush_rejected_input_aggregates,
    handle_client_message, maybe_start_game, now_iso, participant_role, participant_state_response,
    persist_event, persist_rejected_input, persist_session_event, presence_message,
    session_has_started, session_transition_lock, validate_websocket_origin, voice_message,
    websocket_path, AppError, AppState, Arc, ClientMessage, ConnectionLiveness, Duration,
    Extension, FuturesStreamExt, GameSessionPlanResponse, HashMap, HeaderMap, Json, Message,
    ParticipantPrincipal, Path, PlayerRole, Query, Response, Result, ServerMessage, ServerPayload,
    SinkExt, State, TokenBucket, UpgradePurpose, UpgradeTicketClaims, Value, WebSocket,
    WebSocketUpgrade,
};

/// Mints a purpose-bound, one-use game WebSocket upgrade ticket.
pub(super) async fn game_session(
    State(state): State<Arc<AppState>>,
    Path(public_session_id): Path<String>,
    Extension(principal): Extension<ParticipantPrincipal>,
) -> Result<Json<GameSessionPlanResponse>, AppError> {
    let role = participant_role(
        &state,
        &public_session_id,
        &principal.participant_session_id,
    )
    .await?;
    let token = state
        .upgrade_tickets
        .issue(UpgradeTicketClaims {
            public_session_id: public_session_id.clone(),
            participant_session_id: principal.participant_session_id,
            role: role.as_str().to_string(),
            generation: principal.generation,
            purpose: UpgradePurpose::Game,
            expires_at: 0,
        })
        .await;
    Ok(Json(GameSessionPlanResponse {
        websocket_url: websocket_path(
            &state.context.config,
            &format!("ws/game/{public_session_id}"),
        ),
        token,
    }))
}

/// Authenticates a single-use game ticket and upgrades its connection.
pub(super) async fn game_socket(
    State(state): State<Arc<AppState>>,
    Path(public_session_id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Result<Response, AppError> {
    validate_websocket_origin(&state.context.config, &headers)?;
    let ticket = query
        .get("token")
        .ok_or_else(|| AppError::bad_request("token query parameter is required"))?;
    let claims = state
        .upgrade_tickets
        .consume(ticket, UpgradePurpose::Game, &public_session_id)
        .await
        .ok_or_else(|| AppError::forbidden("Invalid or expired game ticket"))?;
    if !state
        .participant_auth
        .generation_is_active(
            &claims.participant_session_id,
            claims.generation,
            state.mount_id,
        )
        .await
    {
        return Err(AppError::forbidden("Game ticket credential was revoked"));
    }
    let participant_session_id = claims.participant_session_id;
    let claimed_role = Some(claims.role);
    let role = participant_role(&state, &public_session_id, &participant_session_id).await?;
    if claimed_role
        .as_deref()
        .is_some_and(|claimed| claimed != role.as_str())
    {
        return Err(AppError::forbidden(
            "Game ticket role no longer matches participant",
        ));
    }
    Ok(ws
        .max_frame_size(32 * 1024)
        .max_message_size(64 * 1024)
        .on_upgrade(move |socket| async move {
            websocket_loop(
                state,
                socket,
                public_session_id,
                participant_session_id,
                role,
            )
            .await;
        }))
}

/// Registers a game socket and owns its input, publication, and disconnection lifecycle.
async fn websocket_loop(
    state: Arc<AppState>,
    socket: WebSocket,
    public_session_id: String,
    participant_session_id: String,
    role: PlayerRole,
) {
    let connection_key = format!("{public_session_id}:{}", role.as_str());
    let liveness = Arc::new(ConnectionLiveness::new());
    let Some(transition_lock) = session_transition_lock(&state, &public_session_id).await else {
        return;
    };
    let connection_guard = transition_lock.lock().await;
    if let Err(error) = expire_if_due_locked(&state, &public_session_id).await {
        tracing::error!(%error, public_session_id, "could not evaluate reconnect deadline");
        return;
    }
    let (connection_generation, mut shutdown, replaced) = state
        .game_connections
        .register(connection_key.clone(), liveness.clone())
        .await;
    if replaced {
        state.context.installation.telemetry.record_reconnection();
    }
    {
        let mut memory = state.memory.write().await;
        if let Some(session) = memory.sessions.get_mut(&public_session_id) {
            session.reconnect(&participant_session_id);
        }
    }
    persist_event(
        "participant_connected",
        state
            .context
            .installation
            .store
            .update_session_participant_connection(
                &participant_session_id,
                "connected",
                None,
                None,
            ),
    )
    .await;
    persist_session_event(
        &state,
        &public_session_id,
        Some(&participant_session_id),
        "participant_connected",
        Value::Null,
        None,
    )
    .await;
    let bus = state.session_bus(&public_session_id).await;
    let mut receiver = bus.subscribe();
    if let Ok(response) = participant_state_response(&state, &public_session_id, role).await {
        let _ = bus.send(ServerMessage::targeted(
            participant_session_id.clone(),
            ServerPayload::ParticipantState {
                participant_state: response.participant_state,
            },
        ));
    }
    if let Some(message) = presence_message(&state, &public_session_id).await {
        let _ = bus.send(message);
    }
    let _ = bus.send(voice_message(&state, &public_session_id).await);
    drop(connection_guard);
    if session_has_started(&state, &public_session_id).await {
        broadcast_participant_states(&state, &public_session_id).await;
    } else {
        maybe_start_game(state.clone(), &public_session_id).await;
    }
    let (mut sender, mut incoming) = socket.split();
    let outbound_participant_session_id = participant_session_id.clone();
    let send_task = tokio::spawn(async move {
        while let Ok(message) = receiver.recv().await {
            if message
                .recipient()
                .is_some_and(|target| target != outbound_participant_session_id)
            {
                continue;
            }
            if let Ok(text) = serde_json::to_string(&message) {
                if sender.send(Message::Text(text)).await.is_err() {
                    break;
                }
            }
        }
    });
    let mut transport_budget = TokenBucket::new(200.0, 100.0);
    loop {
        let next_message = tokio::select! {
            _ = shutdown.recv() => break,
            _ = tokio::time::sleep(Duration::from_secs(5)) => break,
            message = FuturesStreamExt::next(&mut incoming) => message,
        };
        let Some(Ok(message)) = next_message else {
            break;
        };
        let Message::Text(text) = message else {
            continue;
        };
        state.context.installation.telemetry.record_game_message();
        if text.len() > 64 * 1024 {
            let _ = bus.send(error_message(
                &participant_session_id,
                &public_session_id,
                "message_too_large",
                true,
            ));
            break;
        }
        if !transport_budget.consume() {
            state
                .context
                .installation
                .telemetry
                .record_transport_rejected();
            persist_rejected_input(
                &state,
                &public_session_id,
                &participant_session_id,
                "client_message_rejected",
                "transport_pacing",
                "Game channel exceeded 100 messages per second after a burst of 200",
                Some(text.as_bytes()),
            )
            .await;
            continue;
        }
        let Ok(client_message) = serde_json::from_str::<ClientMessage>(&text) else {
            let _ = bus.send(error_message(
                &participant_session_id,
                &public_session_id,
                "invalid_message",
                false,
            ));
            continue;
        };
        if matches!(&client_message, ClientMessage::Heartbeat) {
            liveness.touch_heartbeat();
            state.context.installation.telemetry.record_heartbeat();
        } else {
            liveness.touch_message(true);
        }
        handle_client_message(
            state.clone(),
            &bus,
            &public_session_id,
            &participant_session_id,
            role,
            client_message,
        )
        .await;
    }
    send_task.abort();
    let disconnect_guard = transition_lock.lock().await;
    let still_owner = state
        .game_connections
        .unregister(&connection_key, &connection_generation)
        .await;
    if !still_owner {
        return;
    }
    flush_rejected_input_aggregates(&state, &public_session_id, &participant_session_id).await;
    let disconnected_at = now_iso();
    let grace_seconds = state.context.config.session.reconnect_grace_seconds.max(0) as u64;
    let reconnect_deadline_at =
        (chrono::Utc::now() + chrono::Duration::seconds(grace_seconds as i64)).to_rfc3339();
    {
        let mut memory = state.memory.write().await;
        if let Some(session) = memory.sessions.get_mut(&public_session_id) {
            session.disconnect(
                &participant_session_id,
                &disconnected_at,
                &reconnect_deadline_at,
            );
        }
    }
    persist_event(
        "participant_disconnected",
        state
            .context
            .installation
            .store
            .update_session_participant_connection(
                &participant_session_id,
                "disconnected",
                Some(disconnected_at),
                Some(reconnect_deadline_at.clone()),
            ),
    )
    .await;
    persist_session_event(
        &state,
        &public_session_id,
        Some(&participant_session_id),
        "participant_disconnected",
        Value::Null,
        None,
    )
    .await;
    let _ = bus.send(
        presence_message(&state, &public_session_id)
            .await
            .unwrap_or_else(|| {
                ServerMessage::broadcast(ServerPayload::Presence {
                    public_session_id: public_session_id.clone(),
                    presence: Value::Null,
                })
            }),
    );
    {
        let mut memory = state.memory.write().await;
        if let Some(session) = memory.sessions.get_mut(&public_session_id) {
            session.pause_for_reconnect(&reconnect_deadline_at);
        }
    }
    if grace_seconds == 0 {
        if matches!(
            depart_session_locked(
                &state,
                &public_session_id,
                &participant_session_id,
                "reconnect_timeout"
            )
            .await,
            Ok(true)
        ) {
            broadcast_participant_states(&state, &public_session_id).await;
        }
        return;
    }
    broadcast_participant_states(&state, &public_session_id).await;
    drop(disconnect_guard);
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(grace_seconds)).await;
        let _guard = transition_lock.lock().await;
        let expired = state
            .memory
            .read()
            .await
            .sessions
            .get(&public_session_id)
            .is_some_and(|room| {
                room.participants
                    .get(&participant_session_id)
                    .is_some_and(|participant| {
                        !participant.connected
                            && participant.reconnect_deadline_at.as_deref()
                                == Some(&reconnect_deadline_at)
                    })
                    && due_live_deadline(room, chrono::Utc::now()) == Some("reconnect_timeout")
            });
        if expired
            && matches!(
                expire_live_session_locked(&state, &public_session_id, "reconnect_timeout").await,
                Ok(true)
            )
        {
            broadcast_participant_states(&state, &public_session_id).await;
        }
    });
}
