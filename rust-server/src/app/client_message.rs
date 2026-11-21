//! Dispatches decoded browser messages to the live session operations.
//!
//! The game socket supplies a decoded message and the authenticated occupant's role.
//! Ready declarations update the seat, actions go to the action coordinator, and typed
//! messages go to conversation storage and delivery. The socket counts heartbeats before
//! dispatch. A heartbeat leaves the room's idle deadline untouched.

use super::{
    action_rejected_message, broadcast, commit_conversation_message, error_message,
    internal_error_message, json, persist_rejected_action, persist_rejected_input,
    persist_session_event_required, presence_message, submit_action, touch_transport_activity,
    voice_message, ActionRejection, AppState, Arc, ClientMessage, ConversationOrigin, PlayerRole,
    ServerMessage, SessionParticipant, SubmissionRejection, TokenBucket, Value,
};

/// Applies a decoded browser message according to session and input policy.
pub(super) async fn handle_client_message(
    state: Arc<AppState>,
    bus: &broadcast::Sender<ServerMessage>,
    public_session_id: &str,
    participant_session_id: &str,
    role: PlayerRole,
    message: ClientMessage,
) {
    match message {
        ClientMessage::Heartbeat => {
            // Transport liveness only: heartbeats intentionally cause no database write,
            // session-activity refresh, presence broadcast, or research event.
        }
        ClientMessage::Ready => {
            let first_declaration = {
                let mut memory = state.memory.write().await;
                memory
                    .sessions
                    .get_mut(public_session_id)
                    .and_then(|session| session.participants.get_mut(participant_session_id))
                    .is_some_and(SessionParticipant::declare_ready)
            };
            if !first_declaration {
                return;
            }
            if let Err(error) = persist_session_event_required(
                &state,
                public_session_id,
                Some(participant_session_id),
                "ready",
                Value::Null,
                None,
            )
            .await
            {
                if let Some(participant) = state
                    .memory
                    .write()
                    .await
                    .sessions
                    .get_mut(public_session_id)
                    .and_then(|session| session.participants.get_mut(participant_session_id))
                {
                    participant.rollback_ready_declaration();
                }
                tracing::error!(%error, public_session_id, "could not durably record readiness");
                let _ = bus.send(error_message(
                    participant_session_id,
                    public_session_id,
                    "readiness_failed",
                    true,
                ));
                return;
            }
            touch_transport_activity(&state, public_session_id).await;
            if let Some(message) = presence_message(&state, public_session_id).await {
                let _ = bus.send(message);
            }
            let _ = bus.send(voice_message(&state, public_session_id).await);
        }
        ClientMessage::Message { text } => {
            if text.chars().count() > 4_000 {
                persist_rejected_input(
                    &state,
                    public_session_id,
                    participant_session_id,
                    "chat_message_rejected",
                    "message_too_large",
                    "Chat message exceeds 4000 characters",
                    Some(text.as_bytes()),
                )
                .await;
                let _ = bus.send(error_message(
                    participant_session_id,
                    public_session_id,
                    "message_too_large",
                    false,
                ));
                return;
            }
            let chat_allowed = state
                .chat_submission_budgets
                .write()
                .await
                .entry(participant_session_id.to_string())
                .or_insert_with(|| TokenBucket::new(20.0, 1.0))
                .consume();
            if !chat_allowed {
                persist_rejected_input(
                    &state,
                    public_session_id,
                    participant_session_id,
                    "chat_message_rejected",
                    "human_pacing",
                    "Chat burst exceeded twenty messages; retry after one second",
                    Some(text.as_bytes()),
                )
                .await;
                let _ = bus.send(error_message(
                    participant_session_id,
                    public_session_id,
                    "message_rate_limited",
                    false,
                ));
                return;
            }
            if let Err(error) = commit_conversation_message(
                &state,
                public_session_id,
                participant_session_id,
                ConversationOrigin::Typed,
                text,
                None,
                json!({}),
            )
            .await
            {
                tracing::warn!(message = %error.message, public_session_id, "player message was rejected");
                let _ = bus.send(error_message(
                    participant_session_id,
                    public_session_id,
                    "message_rejected",
                    false,
                ));
            }
        }
        ClientMessage::Action { action: raw_action } => {
            let raw_action_bytes = match serde_json::to_vec(&raw_action) {
                Ok(bytes) => bytes,
                Err(error) => {
                    persist_rejected_action(
                        &state,
                        public_session_id,
                        participant_session_id,
                        "invalid_action_json",
                        &error.to_string(),
                        None,
                    )
                    .await;
                    let _ = bus.send(error_message(
                        participant_session_id,
                        public_session_id,
                        "invalid_action",
                        false,
                    ));
                    return;
                }
            };
            if raw_action_bytes.len() > 4 * 1024 {
                persist_rejected_action(
                    &state,
                    public_session_id,
                    participant_session_id,
                    "action_too_large",
                    "Action payload exceeds 4096 bytes",
                    Some(&raw_action_bytes),
                )
                .await;
                let _ = bus.send(error_message(
                    participant_session_id,
                    public_session_id,
                    "action_too_large",
                    false,
                ));
                return;
            }
            let action = match serde_json::from_value(raw_action) {
                Ok(action) => action,
                Err(error) => {
                    persist_rejected_action(
                        &state,
                        public_session_id,
                        participant_session_id,
                        "invalid_action",
                        &error.to_string(),
                        Some(&raw_action_bytes),
                    )
                    .await;
                    let _ = bus.send(error_message(
                        participant_session_id,
                        public_session_id,
                        "invalid_action",
                        false,
                    ));
                    return;
                }
            };
            match submit_action(
                state.clone(),
                public_session_id,
                participant_session_id,
                role,
                action,
            )
            .await
            {
                Ok(_) => {}
                Err(error) => {
                    let rejection_code = error
                        .downcast_ref::<ActionRejection>()
                        .map(|rejection| rejection.code.as_str())
                        .or_else(|| {
                            error
                                .downcast_ref::<SubmissionRejection>()
                                .map(|rejection| rejection.0)
                        });
                    let Some(rejection_code) = rejection_code else {
                        tracing::error!(%error, public_session_id, "action submission failed internally");
                        let _ = bus.send(internal_error_message(
                            participant_session_id,
                            public_session_id,
                        ));
                        return;
                    };
                    persist_rejected_action(
                        &state,
                        public_session_id,
                        participant_session_id,
                        rejection_code,
                        rejection_code,
                        Some(&raw_action_bytes),
                    )
                    .await;
                    let _ = bus.send(action_rejected_message(
                        participant_session_id,
                        public_session_id,
                        rejection_code,
                    ));
                }
            }
        }
    }
}
