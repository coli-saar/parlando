//! Commitment of typed, transcribed, and agent-authored messages.
//!
//! Browser chat, final transcripts, and live agent responses share a commit operation. It
//! resolves the speaker from room membership, checks interaction under the transition
//! lock, and reserves delivery to the observing agent when required. The durable message
//! precedes browser publication and agent observation.

use super::{
    authenticated_participant_id, expire_if_due_locked, json, new_id, now_iso, participant_role,
    persist_session_event_required, require_session, reserve_agent_delivery,
    session_transition_lock, touch_session_activity, AgentObservation, AppError, AppState, Arc,
    ConversationMessageResponse, Extension, FinalTranscriptUtterance, Json, ParticipantPrincipal,
    Path, Result, ServerMessage, ServerPayload, State, StatusCode, SubmissionRejection, Value,
    VoiceDiagnosticIn,
};

/// Commits one final provider utterance to storage, conversation, and agents.
pub(super) async fn commit_final_transcript(
    state: &Arc<AppState>,
    public_session_id: &str,
    participant_session_id: &str,
    stream_started_at: &str,
    utterance: FinalTranscriptUtterance,
) -> Result<(), AppError> {
    participant_role(state, public_session_id, participant_session_id).await?;
    ensure_participant_interaction_active(state, public_session_id).await?;
    let (experiment_id, session_id) = {
        let memory = state.memory.read().await;
        let session = memory
            .sessions
            .get(public_session_id)
            .ok_or_else(|| AppError::not_found("Session not found."))?;
        (session.experiment_id.clone(), session.session_id)
    };
    let stream_start_game_time_ms = state
        .context
        .installation
        .store
        .session_game_time_ms(&experiment_id, session_id, stream_started_at)
        .await
        .map_err(AppError::from)?;
    let start_game_time_ms = stream_start_game_time_ms
        .checked_add(utterance.start_time_ms)
        .ok_or_else(|| {
            AppError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Transcript start time overflowed",
            )
        })?;
    let end_game_time_ms = stream_start_game_time_ms
        .checked_add(utterance.end_time_ms)
        .ok_or_else(|| {
            AppError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Transcript end time overflowed",
            )
        })?;
    let provider_identity = if utterance.result_ids.is_empty() {
        format!(
            "{}:{}:{}",
            utterance.start_time_ms, utterance.end_time_ms, utterance.text
        )
    } else {
        utterance.result_ids.join(",")
    };
    let idempotency_key = format!(
        "{public_session_id}:{participant_session_id}:{stream_started_at}:{provider_identity}"
    );
    let tokens = utterance
        .tokens
        .iter()
        .map(|token| {
            let start_ms = stream_start_game_time_ms
                .checked_add(token.start_time_ms)
                .ok_or_else(|| {
                    AppError::new(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "Token start time overflowed",
                    )
                })?;
            let end_ms = stream_start_game_time_ms
                .checked_add(token.end_time_ms)
                .ok_or_else(|| {
                    AppError::new(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "Token end time overflowed",
                    )
                })?;
            let mut value =
                json!({"kind":token.kind,"text":token.text,"start_ms":start_ms,"end_ms":end_ms});
            if let Some(confidence) = token.confidence {
                value["confidence"] = json!(confidence);
            }
            Ok(value)
        })
        .collect::<Result<Vec<Value>, AppError>>()?;
    if !state
        .committed_transcripts
        .write()
        .await
        .insert(idempotency_key.clone())
    {
        return Ok(());
    }
    let message = commit_conversation_message(
        state,
        public_session_id,
        participant_session_id,
        ConversationOrigin::VoiceTranscript,
        utterance.text,
        Some(new_id("tr")),
        json!({
            "start_game_time_ms": start_game_time_ms,
            "end_game_time_ms": end_game_time_ms,
            "tokens": tokens,
            "client_metadata": {
                "provider": state.context.config.transcription.provider,
                "result_ids": utterance.result_ids,
            },
        }),
    )
    .await;
    if let Err(error) = message {
        state
            .committed_transcripts
            .write()
            .await
            .remove(&idempotency_key);
        return Err(error);
    }
    Ok(())
}

/// Removes live-protocol wall time before a conversation message enters research storage.
fn conversation_message_event_payload(message: &ConversationMessageResponse) -> Value {
    let mut payload = serde_json::to_value(message).expect("conversation messages serialize");
    if let Some(object) = payload.as_object_mut() {
        object.remove("created_at");
    }
    payload
}

/// Accepts a bounded operational voice metric without retaining it in research storage.
pub(super) async fn add_voice_diagnostic(
    State(state): State<Arc<AppState>>,
    Path(public_session_id): Path<String>,
    principal: Option<Extension<ParticipantPrincipal>>,
    Json(diagnostic): Json<VoiceDiagnosticIn>,
) -> Result<Json<Value>, AppError> {
    let participant_session_id = authenticated_participant_id(principal)?;
    participant_role(&state, &public_session_id, &participant_session_id).await?;
    if diagnostic.event.is_empty()
        || diagnostic.event.len() > 64
        || !diagnostic
            .event
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(AppError::bad_request("Invalid voice diagnostic event name"));
    }
    let _ = minimized_voice_diagnostic_metadata(&diagnostic.metadata);
    Ok(Json(json!({"stored": false})))
}

/// Keeps only non-identifying scalar metrics used to diagnose voice transport quality.
fn minimized_voice_diagnostic_metadata(metadata: &Value) -> Value {
    const ALLOWED: &[&str] = &[
        "buffered_samples",
        "channels",
        "count",
        "enabled",
        "frame_duration_ms",
        "jitter_buffer_ms",
        "protocol_version",
        "sample_rate_hz",
    ];
    let mut minimized = serde_json::Map::new();
    if let Some(values) = metadata.as_object() {
        for key in ALLOWED {
            if let Some(value) = values
                .get(*key)
                .filter(|value| value.is_boolean() || value.is_number() || value.is_null())
            {
                minimized.insert((*key).to_string(), value.clone());
            }
        }
    }
    Value::Object(minimized)
}

/// Trusted source of one accepted conversation message.
#[derive(Clone, Copy)]
pub(super) enum ConversationOrigin {
    Typed,
    VoiceTranscript,
    Agent,
}

impl ConversationOrigin {
    /// Returns the stable research-record origin label.
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Typed => "typed",
            Self::VoiceTranscript => "voice_transcript",
            Self::Agent => "agent",
        }
    }
}

/// Commits and publishes one already-attributed player message through a single pipeline.
pub(super) async fn commit_conversation_message(
    state: &Arc<AppState>,
    public_session_id: &str,
    sender_participant_session_id: &str,
    origin: ConversationOrigin,
    text: String,
    source_message_id: Option<String>,
    metadata: Value,
) -> Result<ConversationMessageResponse, AppError> {
    let Some(lock) = session_transition_lock(state, public_session_id).await else {
        return Err(AppError::not_found("Session not found."));
    };
    let _guard = lock.lock().await;
    if expire_if_due_locked(state, public_session_id).await? {
        return Err(AppError::new(StatusCode::CONFLICT, "Session has ended."));
    }
    require_session(state, public_session_id).await?;
    ensure_participant_interaction_active(state, public_session_id).await?;
    if text.chars().count() > 4_000 {
        return Err(AppError::bad_request("Conversation message is too long"));
    }
    if serde_json::to_vec(&metadata)
        .map_err(|error| AppError::bad_request(error.to_string()))?
        .len()
        > 8 * 1024
    {
        return Err(AppError::bad_request("Conversation metadata is too large"));
    }
    let sender_role =
        participant_role(state, public_session_id, sender_participant_session_id).await?;
    let sender = state
        .memory
        .read()
        .await
        .sessions
        .get(public_session_id)
        .filter(|room| {
            room.participants
                .values()
                .any(|p| p.source == "agent" && p.role != sender_role)
        })
        .and_then(|room| room.agent_inbox.clone());
    let delivery = match sender {
        Some(sender) => Some(
            reserve_agent_delivery(state, public_session_id, sender)
                .await
                .map_err(|error| {
                    if error.is::<SubmissionRejection>() {
                        AppError::new(StatusCode::CONFLICT, "Session has ended.")
                    } else {
                        AppError::from(error)
                    }
                })?,
        ),
        None => None,
    };
    let message = ConversationMessageResponse {
        id: new_id("msg"),
        public_session_id: public_session_id.to_string(),
        sender_participant_session_id: Some(sender_participant_session_id.to_string()),
        sender_role: Some(sender_role.as_str().to_string()),
        text,
        origin: origin.as_str().to_string(),
        source_message_id,
        metadata,
        created_at: now_iso(),
    };
    persist_session_event_required(
        state,
        public_session_id,
        message.sender_participant_session_id.as_deref(),
        "conversation_message",
        conversation_message_event_payload(&message),
        None,
    )
    .await?;
    touch_session_activity(state, public_session_id).await;
    if let Some(player_message) = message.player_message() {
        let _ = state
            .session_bus(public_session_id)
            .await
            .send(ServerMessage::broadcast(ServerPayload::Message {
                public_session_id: public_session_id.to_string(),
                message: player_message,
            }));
    }
    if matches!(origin, ConversationOrigin::Typed) {
        state.context.installation.telemetry.record_chat_accepted();
    }
    if let Some(permit) = delivery {
        permit.send(AgentObservation::Message {
            speaker: sender_role,
            text: message.text.clone(),
        });
    }
    Ok(message)
}

/// Rejects meaningful participant input unless the projected lifecycle is exactly `Active`.
async fn ensure_participant_interaction_active(
    state: &Arc<AppState>,
    public_session_id: &str,
) -> Result<(), AppError> {
    let active = {
        let memory = state.memory.read().await;
        let session = memory
            .sessions
            .get(public_session_id)
            .ok_or_else(|| AppError::not_found("Session not found."))?;
        session.accepts_interaction()
    };
    if !active {
        return Err(AppError::forbidden(
            "Participant is not active and cannot send meaningful game input.",
        ));
    }
    Ok(())
}
