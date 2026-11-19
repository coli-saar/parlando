//! Participant lifecycle snapshots, presence, and protocol responses.

use super::{
    abandon_session, authenticated_participant_id, json, protocol_json, session_ready_for_game,
    AppError, AppState, Arc, Extension, Game, HashSet, Json, LiveSession, ParticipantPrincipal,
    ParticipantState, ParticipantStateResponse, Path, PlayerRole, Result, ServerMessage,
    ServerPayload, SessionLifecycle, SessionParticipant, State, StatusCode, Value,
};

/// Returns the authoritative lifecycle snapshot for the authenticated participant.
pub(super) async fn get_participant_state<A: Game>(
    State(state): State<Arc<AppState<A>>>,
    principal: Option<Extension<ParticipantPrincipal>>,
) -> Result<Json<ParticipantStateResponse>, AppError> {
    let participant_session_id = authenticated_participant_id(principal)?;
    Ok(Json(
        existing_assignment(&state, &participant_session_id)
            .await?
            .unwrap_or(ParticipantStateResponse {
                participant_state: ParticipantState::Registered,
            }),
    ))
}

/// Reuses the participant's published assignment or their durable terminal result.
pub(super) async fn existing_assignment<A: Game>(
    state: &Arc<AppState<A>>,
    participant_session_id: &str,
) -> Result<Option<ParticipantStateResponse>, AppError> {
    let existing = {
        let memory = state.memory.read().await;
        memory.sessions.values().find_map(|session| {
            session
                .participants
                .get(participant_session_id)
                .map(|participant| (session.id.clone(), participant.role))
        })
    };
    if let Some((public_session_id, role)) = existing {
        return Ok(Some(
            participant_state_response(state, &public_session_id, role).await?,
        ));
    }
    if let Some(stored) = state
        .context
        .installation
        .store
        .terminal_participant_state(&state.experiment_id, participant_session_id)
        .await?
    {
        return Ok(Some(ParticipantStateResponse {
            participant_state: ParticipantState::Ended {
                public_session_id: stored.public_session_id,
                role: stored.role,
                result: stored.result,
                voice_ends_at: None,
            },
        }));
    }
    Ok(None)
}

/// Idempotently records an explicit participant leave and returns the terminal snapshot.
pub(super) async fn leave_session<A: Game>(
    State(state): State<Arc<AppState<A>>>,
    principal: Option<Extension<ParticipantPrincipal>>,
    Path(public_session_id): Path<String>,
) -> Result<Json<ParticipantStateResponse>, AppError> {
    let participant_session_id = authenticated_participant_id(principal)?;
    let role = {
        let memory = state.memory.read().await;
        memory
            .sessions
            .get(&public_session_id)
            .and_then(|session| session.participants.get(&participant_session_id))
            .map(|participant| participant.role)
    };
    if let Some(role) = role {
        abandon_session(
            &state,
            &public_session_id,
            &participant_session_id,
            "participant_left",
        )
        .await?;
        let response = participant_state_response(&state, &public_session_id, role).await?;
        broadcast_participant_states(&state, &public_session_id).await;
        return Ok(Json(response));
    }
    if let Some(stored) = state
        .context
        .installation
        .store
        .terminal_participant_state(&state.experiment_id, &participant_session_id)
        .await?
    {
        return Ok(Json(ParticipantStateResponse {
            participant_state: ParticipantState::Ended {
                public_session_id: stored.public_session_id,
                role: stored.role,
                result: stored.result,
                voice_ends_at: None,
            },
        }));
    }
    Err(AppError::not_found("Session not found."))
}

/// Role-specific game view used to assemble snapshots and transition deltas.
struct ParticipantView {
    presence: Value,
    observation: Option<Value>,
    available_actions: Option<Vec<Value>>,
}

/// Computes a participant view from the same session snapshot as its lifecycle.
fn participant_view_from_session<A: Game>(
    session: &LiveSession<A>,
    role: PlayerRole,
) -> Result<ParticipantView, AppError> {
    let presence = session_presence(session);
    if session.lifecycle() == &SessionLifecycle::Forming {
        return Ok(ParticipantView {
            presence,
            observation: None,
            available_actions: None,
        });
    }
    Ok(ParticipantView {
        presence,
        observation: Some(protocol_json(
            &session.game.observation(session.state(), role),
        )?),
        available_actions: available_actions(&session.game, session.state(), role)?,
    })
}

/// Builds the canonical participant lifecycle snapshot from authoritative live-session facts.
pub(super) async fn participant_state_response<A: Game>(
    state: &Arc<AppState<A>>,
    public_session_id: &str,
    role: PlayerRole,
) -> Result<ParticipantStateResponse, AppError> {
    let memory = state.memory.read().await;
    let session = memory
        .sessions
        .get(public_session_id)
        .ok_or_else(|| AppError::not_found("Session not found."))?;
    let view = participant_view_from_session(session, role)?;
    let presence = view.presence;
    let participant_state = match session.lifecycle() {
        SessionLifecycle::Forming => ParticipantState::Waiting {
            public_session_id: public_session_id.to_string(),
            role: role.as_str().to_string(),
            waiting_started_at: session.timing().waiting_started_at.clone(),
            waiting_deadline_at: session.timing().waiting_deadline_at.clone(),
            presence,
        },
        SessionLifecycle::Running => match session.pause() {
            Some(reason) => ParticipantState::Paused {
                public_session_id: public_session_id.to_string(),
                role: role.as_str().to_string(),
                reason: reason.clone(),
                observation: view.observation.ok_or_else(|| {
                    AppError::new(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "Running session has no observation",
                    )
                })?,
                available_actions: view.available_actions,
                presence,
                idle_deadline_at: session.timing().idle_deadline_at.clone().ok_or_else(|| {
                    AppError::new(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "Running session has no idle deadline",
                    )
                })?,
            },
            None => ParticipantState::Active {
                public_session_id: public_session_id.to_string(),
                role: role.as_str().to_string(),
                observation: view.observation.ok_or_else(|| {
                    AppError::new(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "Running session has no observation",
                    )
                })?,
                available_actions: view.available_actions,
                presence,
                idle_deadline_at: session.timing().idle_deadline_at.clone().ok_or_else(|| {
                    AppError::new(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "Running session has no idle deadline",
                    )
                })?,
            },
        },
        SessionLifecycle::Ended(session_end) => {
            let result = session_end
                .participant_results
                .get(role.as_str())
                .cloned()
                .ok_or_else(|| {
                    AppError::new(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "Terminal participant result is missing",
                    )
                })?;
            ParticipantState::Ended {
                public_session_id: public_session_id.to_string(),
                role: role.as_str().to_string(),
                result,
                voice_ends_at: session.timing().voice_ends_at.clone(),
            }
        }
    };
    Ok(ParticipantStateResponse { participant_state })
}

/// Broadcasts one authoritative lifecycle snapshot targeted to each human participant.
pub(super) async fn broadcast_participant_states<A: Game>(
    state: &Arc<AppState<A>>,
    public_session_id: &str,
) {
    let recipients = state
        .memory
        .read()
        .await
        .sessions
        .get(public_session_id)
        .map(|session| {
            session
                .participants
                .values()
                .filter(|participant| participant.source != "agent")
                .map(|participant| (participant.participant_session_id.clone(), participant.role))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let bus = state.session_bus(public_session_id).await;
    for (recipient, role) in recipients {
        if let Ok(response) = participant_state_response(state, public_session_id, role).await {
            let _ = bus.send(ServerMessage::targeted(
                recipient,
                ServerPayload::ParticipantState {
                    participant_state: response.participant_state,
                },
            ));
        }
    }
}

/// Sends the targeted authoritative snapshot after a participant becomes active.
pub(super) async fn send_role_assignment<A: Game>(
    state: &Arc<AppState<A>>,
    public_session_id: &str,
    participant_session_id: &str,
    role: PlayerRole,
) {
    if let Ok(response) = participant_state_response(state, public_session_id, role).await {
        let bus = state.session_bus(public_session_id).await;
        let _ = bus.send(ServerMessage::targeted(
            participant_session_id,
            ServerPayload::ParticipantState {
                participant_state: response.participant_state,
            },
        ));
    }
}

/// Builds the current presence update for a live session.
pub(super) async fn presence_message<A: Game>(
    state: &Arc<AppState<A>>,
    public_session_id: &str,
) -> Option<ServerMessage> {
    let memory = state.memory.read().await;
    let session = memory.sessions.get(public_session_id)?;
    Some(ServerMessage::broadcast(ServerPayload::Presence {
        public_session_id: public_session_id.to_string(),
        presence: session_presence(session),
    }))
}

/// Projects the session's occupied roles and connection status.
fn session_presence<G: Game>(session: &LiveSession<G>) -> Value {
    json!(session
        .participants
        .values()
        .map(|participant| {
            (
                participant.role.as_str().to_string(),
                json!({
                    "connected": participant.connected,
                    "audioReady": participant.audio_ready(),
                }),
            )
        })
        .collect::<serde_json::Map<_, _>>())
}

/// Builds the session's current voice availability and readiness update.
pub(super) async fn voice_message<A: Game>(
    state: &Arc<AppState<A>>,
    public_session_id: &str,
) -> ServerMessage {
    let (audio_ready, game_ready) = state
        .memory
        .read()
        .await
        .sessions
        .get(public_session_id)
        .map(|session| {
            let connected_roles = session
                .participants
                .values()
                .filter(|participant| participant.connected)
                .map(|participant| participant.role)
                .collect::<HashSet<_>>();
            (
                connected_roles == HashSet::from([PlayerRole::A, PlayerRole::B]),
                session.accepts_interaction()
                    && session_ready_for_game::<A>(&state.context.config, session),
            )
        })
        .unwrap_or((false, false));
    let transcription_ready = game_ready;
    ServerMessage::broadcast(ServerPayload::VoiceStatus {
        public_session_id: public_session_id.to_string(),
        voice: json!({
            "audioReady": audio_ready,
            "transcriptionReady": transcription_ready,
            "transcriptionStatus": if !state.context.config.transcription.enabled {
                "Disabled"
            } else if transcription_ready {
                "Ready"
            } else {
                "Initializing"
            },
        }),
    })
}

/// Builds one presentation-neutral runtime or transport failure.
pub(super) fn error_message(
    recipient: &str,
    public_session_id: &str,
    code: &str,
    fatal: bool,
) -> ServerMessage {
    ServerMessage::targeted(
        recipient,
        ServerPayload::Error {
            public_session_id: public_session_id.to_string(),
            code: code.to_string(),
            fatal,
        },
    )
}

/// Builds a machine-readable expected game-rule rejection.
pub(super) fn action_rejected_message(
    recipient: &str,
    public_session_id: &str,
    code: &str,
) -> ServerMessage {
    ServerMessage::targeted(
        recipient,
        ServerPayload::ActionRejected {
            public_session_id: public_session_id.to_string(),
            code: code.to_string(),
        },
    )
}

/// Builds a presentation-neutral internal failure without exposing diagnostics.
pub(super) fn internal_error_message(recipient: &str, public_session_id: &str) -> ServerMessage {
    error_message(recipient, public_session_id, "internal_error", true)
}

/// Serializes the game-defined action catalogue for a role at one revision.
fn available_actions<G: Game>(
    game: &G,
    state: &G::State,
    role: PlayerRole,
) -> Result<Option<Vec<Value>>> {
    game.available_actions(state, role)
        .map(|actions| actions.iter().map(protocol_json).collect())
        .transpose()
}

/// Constructs the browser delta for a prepared revision before the transition is committed.
pub(super) fn action_message<G: Game>(
    session: &LiveSession<G>,
    after: &G::State,
    participant: &SessionParticipant,
    actor: PlayerRole,
    action: &Value,
    observation: Value,
) -> Result<ServerMessage> {
    Ok(ServerMessage::targeted(
        participant.participant_session_id.clone(),
        ServerPayload::Transition {
            public_session_id: session.id.clone(),
            actor: actor.as_str().to_string(),
            action: action.clone(),
            observation,
            available_actions: available_actions(&session.game, after, participant.role)?,
        },
    ))
}
