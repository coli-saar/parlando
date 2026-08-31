use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ParticipantCreateRequest {
    /// Prolific correlation values captured from the external-study URL.
    pub prolific: Option<ProlificParticipantRequest>,
}

/// Prolific correlation values which never enter participant-visible session data or exports.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProlificParticipantRequest {
    pub participant_id: String,
    pub study_id: String,
    pub session_id: String,
    /// Short-lived Secure external URL token, when enabled for the linked study.
    pub prolific_token: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ParticipantCreateResponse {
    /// Opaque participant bearer credential; clients must keep it out of URLs and logs.
    pub participant_credential: String,
    /// Human-readable random identifier scoped to the current experiment.
    pub participant_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ConsentItemResponse {
    pub id: String,
    pub title: String,
    /// Plain-text consent copy safe to render directly.
    pub body: String,
    pub required: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PublicConfigResponse {
    /// Human-readable name of the compiled game shown during participant startup.
    pub game_name: String,
    /// Lifecycle state of the experiment selected by the participant route.
    pub experiment_status: String,
    /// Institution displayed with the Parlando platform identity, when configured.
    pub institution: Option<String>,
    pub participant_information_version: Option<String>,
    pub participant_information_url: Option<String>,
    pub consents: Vec<ConsentItemResponse>,
    pub voice: Value,
    /// Narrow intake capability; study ids and completion codes remain private.
    pub recruitment: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConsentRequest {
    pub public_session_id: Option<String>,
    #[serde(default)]
    pub decisions: std::collections::HashMap<String, bool>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CreateSessionRequest {}

/// One authoritative participant lifecycle snapshot returned by HTTP.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ParticipantStateResponse {
    pub participant_state: ParticipantState,
}

pub type CreateSessionResponse = ParticipantStateResponse;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AudioSessionRequest {}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AudioSessionPlanResponse {
    /// Whether session audio is enabled for this experiment.
    pub enabled: bool,
    /// Authenticated Parlando audio WebSocket URL without its credential.
    pub websocket_url: Option<String>,
    /// Short-lived credential bound to the session, participant, and current role.
    pub token: Option<String>,
    /// Binary audio protocol version understood by the server.
    pub protocol_version: u8,
    /// Required PCM sample rate.
    pub sample_rate_hz: u32,
    /// Required PCM channel count.
    pub channels: u16,
    /// Duration represented by each complete binary frame.
    pub frame_duration_ms: u16,
    /// Recommended browser playback buffer target.
    pub jitter_buffer_ms: u16,
}

/// One-use authenticated game WebSocket upgrade plan.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GameSessionPlanResponse {
    /// Game WebSocket URL without embedded participant identifiers.
    pub websocket_url: String,
    /// Short-lived, one-use ticket bound to this session and participant role.
    pub token: String,
}

impl AudioSessionPlanResponse {
    /// Creates a credential-free plan for experiments with voice disabled.
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            websocket_url: None,
            token: None,
            protocol_version: 1,
            sample_rate_hz: 24_000,
            channels: 1,
            frame_duration_ms: 20,
            jitter_buffer_ms: 100,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VoiceDiagnosticIn {
    pub event: String,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ConversationMessageResponse {
    pub id: String,
    pub public_session_id: String,
    pub sender_participant_session_id: Option<String>,
    pub sender_role: Option<String>,
    pub text: String,
    pub origin: String,
    pub source_message_id: Option<String>,
    #[serde(default)]
    pub metadata: Value,
    pub created_at: String,
}

/// Player-visible input channel for one message.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlayerMessageInput {
    /// Text supplied directly by a player or text-producing agent.
    Text,
    /// Final text transcribed from a player's speech.
    VoiceTranscript,
}

/// Minimal player-to-player message carried by the participant protocol.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PlayerMessageResponse {
    /// Stable message identifier used for client-side deduplication.
    pub id: String,
    /// Player role which sent the message.
    pub sender: String,
    /// Message text delivered to the other player.
    pub text: String,
    /// Input channel which produced the text, without revealing controller kind.
    pub input: PlayerMessageInput,
    /// Server timestamp at which the message was accepted.
    pub created_at: String,
}

impl ConversationMessageResponse {
    /// Projects an internal conversation record onto the participant protocol.
    pub fn player_message(&self) -> Option<PlayerMessageResponse> {
        let sender = self.sender_role.clone()?;
        let input = if self.origin == "voice_transcript" {
            PlayerMessageInput::VoiceTranscript
        } else {
            PlayerMessageInput::Text
        };
        Some(PlayerMessageResponse {
            id: self.id.clone(),
            sender,
            text: self.text.clone(),
            input,
            created_at: self.created_at.clone(),
        })
    }
}

/// Provider-neutral result experienced by one participant when their session ends.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ParticipantOutcomeKind {
    Completed,
    LeftWaitingRoom,
    LeftGame,
    ConnectionLost,
    PartnerLeft,
    PartnerUnavailable,
    IdleLimitReached,
    TechnicalFailure,
    LifetimeLimitReached,
}

/// Why one shared session reached its single terminal lifecycle state.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionEndCause {
    /// The game produced its normal shared completion value.
    GameCompleted,
    /// One role explicitly left a forming or running session.
    ParticipantLeft { actor: String },
    /// Matchmaking ended before another required participant became available.
    PartnerUnavailable,
    /// One disconnected role did not return before its reconnect deadline.
    ReconnectTimedOut { disconnected_role: String },
    /// A running session exceeded its meaningful-activity deadline.
    IdleTimedOut,
    /// A forming or running session exceeded its absolute lifetime.
    LifetimeTimedOut,
    /// Infrastructure prevented the session from continuing.
    TechnicalFailure,
}

/// Recruitment-provider handoff selected after durable participant outcome derivation.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RecruitmentHandoff {
    pub provider: String,
    pub code: String,
    pub url: String,
}

/// Immutable recipient-specific result derived from one shared session ending.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ParticipantResult {
    pub outcome: ParticipantOutcomeKind,
    pub reason: String,
    pub completion: Option<Value>,
    pub final_observation: Option<Value>,
    pub handoff: Option<RecruitmentHandoff>,
}

/// Complete terminal session value committed atomically with all human results.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SessionEnd {
    pub cause: SessionEndCause,
    pub completion: Option<Value>,
    /// Recipient results keyed by the stable two-player role names `A` and `B`.
    pub participant_results: HashMap<String, ParticipantResult>,
}

/// The participant lifecycle inventory shared by the Rust runtime and clients.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ParticipantState {
    /// The authenticated participant has not joined a session.
    Registered,
    /// The participant belongs to a session which has not started.
    Waiting {
        public_session_id: String,
        role: String,
        waiting_started_at: String,
        waiting_deadline_at: String,
        presence: Value,
    },
    /// The running session currently accepts meaningful participant input.
    Active {
        public_session_id: String,
        role: String,
        observation: Value,
        available_actions: Option<Vec<Value>>,
        presence: Value,
        idle_deadline_at: String,
    },
    /// The running session preserves its projection while interaction is unavailable.
    Paused {
        public_session_id: String,
        role: String,
        reason: ParticipantPauseReason,
        observation: Value,
        available_actions: Option<Vec<Value>>,
        presence: Value,
        idle_deadline_at: String,
    },
    /// The participant has one immutable recipient-specific terminal result.
    Ended {
        public_session_id: String,
        role: String,
        result: ParticipantResult,
    },
}

/// Lifecycle-only participant phase used to validate transitions independently of payload data.
#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParticipantPhase {
    Registered,
    Waiting,
    Active,
    Paused,
    Ended,
}

/// Reports whether two distinct participant phases form an allowed lifecycle transition.
#[cfg(test)]
pub fn participant_transition_allowed(from: ParticipantPhase, to: ParticipantPhase) -> bool {
    matches!(
        (from, to),
        (ParticipantPhase::Registered, ParticipantPhase::Waiting)
            | (
                ParticipantPhase::Waiting,
                ParticipantPhase::Active | ParticipantPhase::Ended
            )
            | (
                ParticipantPhase::Active,
                ParticipantPhase::Paused | ParticipantPhase::Ended
            )
            | (
                ParticipantPhase::Paused,
                ParticipantPhase::Active | ParticipantPhase::Ended
            )
    )
}

/// The deliberately closed participant pause-reason vocabulary.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ParticipantPauseReason {
    /// A required role may reclaim its assignment until the supplied deadline.
    PartnerReconnecting { deadline_at: String },
}

/// One participant operation accepted by the game WebSocket.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    /// Declares that the participant's game channel is ready.
    Ready,
    /// Proposes one game-specific action.
    Action { action: Value },
    /// Sends text to the other player without changing game state.
    Message { text: String },
    /// Maintains transport liveness without recording research activity.
    Heartbeat,
}

/// One exact payload variant in the versioned participant WebSocket protocol.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerPayload {
    /// Replaces lifecycle inference with one complete authoritative participant snapshot.
    ParticipantState { participant_state: ParticipantState },
    /// Reports one accepted action and the receiving role's resulting observation.
    Transition {
        public_session_id: String,
        actor: String,
        action: Value,
        observation: Value,
        available_actions: Option<Vec<Value>>,
    },
    /// Delivers one player-to-player message.
    Message {
        public_session_id: String,
        message: PlayerMessageResponse,
    },
    /// Reports current role connectivity and narrow readiness capabilities.
    Presence {
        public_session_id: String,
        presence: Value,
    },
    /// Reports current audio and transcription readiness.
    VoiceStatus {
        public_session_id: String,
        voice: Value,
    },
    /// Reports an expected game-rule rejection without ending the session.
    ActionRejected {
        public_session_id: String,
        code: String,
    },
    /// Reports a transport or runtime failure using presentation-neutral fields.
    Error {
        public_session_id: String,
        code: String,
        fatal: bool,
    },
}

/// Versioned server message with an optional internal-only recipient selector.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ServerMessage {
    /// Version of the frontend-neutral JSON game protocol.
    pub protocol_version: u16,
    /// Participant payload serialized beside the version field.
    #[serde(flatten)]
    pub payload: ServerPayload,
    /// Participant session targeted by this broadcast message, when applicable.
    #[serde(skip)]
    recipient: Option<String>,
}

impl ServerMessage {
    /// Creates a broadcast message for every player connected to the session bus.
    pub fn broadcast(payload: ServerPayload) -> Self {
        Self {
            protocol_version: 2,
            payload,
            recipient: None,
        }
    }

    /// Creates a message routed only to one authenticated participant session.
    pub fn targeted(recipient: impl Into<String>, payload: ServerPayload) -> Self {
        Self {
            protocol_version: 2,
            payload,
            recipient: Some(recipient.into()),
        }
    }

    /// Returns the internal recipient selector without serializing it to clients.
    pub fn recipient(&self) -> Option<&str> {
        self.recipient.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::{participant_transition_allowed, ParticipantPhase};
    use super::{ClientMessage, ServerMessage};

    /// Locks the deliberately small participant transition graph, including its absorbing end.
    #[test]
    fn participant_transition_graph_is_strict() {
        use ParticipantPhase::*;
        let allowed = [
            (Registered, Waiting),
            (Waiting, Active),
            (Waiting, Ended),
            (Active, Paused),
            (Active, Ended),
            (Paused, Active),
            (Paused, Ended),
        ];
        for from in [Registered, Waiting, Active, Paused, Ended] {
            for to in [Registered, Waiting, Active, Paused, Ended] {
                assert_eq!(
                    participant_transition_allowed(from, to),
                    allowed.contains(&(from, to))
                );
            }
        }
    }

    /// Confirms the game channel accepts only the current four operation names.
    #[test]
    fn client_message_protocol_has_no_legacy_consent_variant() {
        for payload in [
            r#"{"type":"ready"}"#,
            r#"{"type":"action","action":{"move":1}}"#,
            r#"{"type":"message","text":"hello"}"#,
            r#"{"type":"heartbeat"}"#,
        ] {
            serde_json::from_str::<ClientMessage>(payload).unwrap();
        }

        assert!(serde_json::from_str::<ClientMessage>(
            r#"{"type":"consentUpdated","consent":{"decisions":{}}}"#
        )
        .is_err());
    }

    /// Keeps every version-two client and server JSON fixture round-trippable in Rust.
    #[test]
    fn shared_participant_protocol_fixtures_round_trip() {
        let fixtures: Value = serde_json::from_str(include_str!(
            "../../proto/participant_protocol_v2.fixtures.json"
        ))
        .unwrap();
        for fixture in fixtures["client_messages"].as_array().unwrap() {
            let decoded: ClientMessage = serde_json::from_value(fixture.clone()).unwrap();
            assert_eq!(serde_json::to_value(decoded).unwrap(), *fixture);
        }
        for fixture in fixtures["server_messages"].as_array().unwrap() {
            let decoded: ServerMessage = serde_json::from_value(fixture.clone()).unwrap();
            let encoded = serde_json::to_value(decoded).unwrap();
            assert_eq!(encoded, *fixture);
            assert!(encoded.get("recipient").is_none());
            assert!(encoded.get("room_id").is_none());
            assert!(encoded.get("participant_session_id").is_none());
        }
    }
}
