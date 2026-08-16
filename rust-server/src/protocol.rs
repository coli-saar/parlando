use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ParticipantCreateRequest {}

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
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConsentRequest {
    pub room_id: Option<String>,
    #[serde(default)]
    pub decisions: std::collections::HashMap<String, bool>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CreateRoomRequest {}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RoomResponse {
    pub room_id: String,
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presence: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observation: Option<Value>,
    /// `None` means the game does not enumerate actions; an empty vector means none are available.
    pub available_actions: Option<Vec<Value>>,
}

pub type CreateRoomResponse = RoomResponse;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AudioSessionRequest {}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AudioSessionPlanResponse {
    /// Whether room audio is enabled for this experiment.
    pub enabled: bool,
    /// Authenticated Parlando audio WebSocket URL without its credential.
    pub websocket_url: Option<String>,
    /// Short-lived credential bound to the room, participant, and current role.
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
    /// Short-lived, one-use ticket bound to this room and participant role.
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
pub struct ConversationMessageIn {
    pub text: String,
    #[serde(default = "default_typed")]
    pub origin: String,
    pub source_message_id: Option<String>,
    #[serde(default)]
    pub metadata: Value,
}

fn default_typed() -> String {
    "typed".to_string()
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ConversationMessageResponse {
    pub id: String,
    pub room_id: String,
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
    /// Intentionally leaves and abandons the session.
    Leave,
}

/// One exact payload variant in the versioned participant WebSocket protocol.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerPayload {
    /// Starts active rendering with the first complete role-specific observation.
    SessionStarted {
        room_id: String,
        role: String,
        observation: Value,
        available_actions: Option<Vec<Value>>,
    },
    /// Reports one accepted action and the receiving role's resulting observation.
    Transition {
        room_id: String,
        actor: String,
        action: Value,
        observation: Value,
        available_actions: Option<Vec<Value>>,
    },
    /// Delivers one player-to-player message.
    Message {
        room_id: String,
        message: PlayerMessageResponse,
    },
    /// Reports current role connectivity and narrow readiness capabilities.
    Presence { room_id: String, presence: Value },
    /// Reports current audio and transcription readiness.
    VoiceStatus { room_id: String, voice: Value },
    /// Reports that the game reached a terminal state with its shared game-specific result.
    Completed { room_id: String, completion: Value },
    /// Reports that a player intentionally ended the session.
    Abandoned { room_id: String, code: String },
    /// Reports an expected game-rule rejection without ending the session.
    ActionRejected { room_id: String, code: String },
    /// Reports a transport or runtime failure using presentation-neutral fields.
    Error {
        room_id: String,
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
    /// Creates a broadcast message for every player connected to the room bus.
    pub fn broadcast(payload: ServerPayload) -> Self {
        Self {
            protocol_version: 1,
            payload,
            recipient: None,
        }
    }

    /// Creates a message routed only to one authenticated participant session.
    pub fn targeted(recipient: impl Into<String>, payload: ServerPayload) -> Self {
        Self {
            protocol_version: 1,
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
    use super::ClientMessage;

    /// Confirms the game channel accepts only the current five operation names.
    #[test]
    fn client_message_protocol_has_no_legacy_consent_variant() {
        for payload in [
            r#"{"type":"ready"}"#,
            r#"{"type":"action","action":{"move":1}}"#,
            r#"{"type":"message","text":"hello"}"#,
            r#"{"type":"heartbeat"}"#,
            r#"{"type":"leave"}"#,
        ] {
            serde_json::from_str::<ClientMessage>(payload).unwrap();
        }

        assert!(serde_json::from_str::<ClientMessage>(
            r#"{"type":"consentUpdated","consent":{"decisions":{}}}"#
        )
        .is_err());
    }
}
