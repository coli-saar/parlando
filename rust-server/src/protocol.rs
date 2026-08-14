use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ParticipantCreateRequest {
    #[serde(default = "default_direct")]
    pub source: String,
    pub study_id: Option<String>,
    pub external_id: Option<String>,
    #[serde(default)]
    pub metadata: Value,
}

fn default_direct() -> String {
    "direct".to_string()
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DirectStartRequest {
    pub study_id: Option<String>,
    pub external_id: Option<String>,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ParticipantCreateResponse {
    /// Non-secret identifier used for correlation and user-interface state.
    pub participant_session_id: String,
    /// Opaque participant bearer credential; clients must keep it out of URLs and logs.
    pub participant_credential: String,
    pub source: String,
    /// Human-readable random identifier scoped to the current experiment.
    pub participant_id: String,
}

pub type DirectStartResponse = ParticipantCreateResponse;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ConsentItemResponse {
    pub id: String,
    pub title: String,
    pub body_html: String,
    pub required: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PublicConfigResponse {
    pub study_name: String,
    pub require_consent: bool,
    pub participant_information_version: Option<String>,
    pub participant_information_url: Option<String>,
    pub consents: Vec<ConsentItemResponse>,
    pub voice: Value,
    pub transcription: Value,
    pub tts: Value,
    pub conversation: Value,
    pub agents: Value,
    pub privacy: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ConsentRequest {
    pub participant_session_id: String,
    pub room_id: Option<String>,
    #[serde(default)]
    pub decisions: std::collections::HashMap<String, bool>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CreateRoomRequest {
    pub participant_session_id: String,
    #[serde(default = "default_direct")]
    pub mode: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct JoinRoomRequest {
    pub participant_session_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RoomResponse {
    pub room_id: String,
    pub participant_session_id: String,
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presence: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observation: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub available_actions: Option<Vec<Value>>,
    #[serde(default)]
    pub events: Vec<Value>,
    #[serde(default)]
    pub conversation: Vec<ConversationMessageResponse>,
}

pub type CreateRoomResponse = RoomResponse;
pub type JoinRoomResponse = RoomResponse;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AudioSessionRequest {
    pub participant_session_id: String,
}

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
pub struct VoiceDiagnosticIn {
    pub participant_session_id: String,
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

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ClientMessage {
    #[serde(rename = "type")]
    pub message_type: String,
    pub action: Option<Value>,
    pub consent: Option<ConsentRequest>,
    pub text: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ServerMessage {
    #[serde(rename = "type")]
    pub message_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub room_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub participant_session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observation: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub available_actions: Option<Vec<Value>>,
    #[serde(default)]
    pub events: Vec<Value>,
    #[serde(default)]
    pub conversation: Vec<ConversationMessageResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation_message: Option<ConversationMessageResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presence: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voice: Option<Value>,
}

impl ServerMessage {
    pub fn new(message_type: impl Into<String>) -> Self {
        Self {
            message_type: message_type.into(),
            room_id: None,
            participant_session_id: None,
            role: None,
            state: None,
            observation: None,
            available_actions: None,
            events: vec![],
            conversation: vec![],
            conversation_message: None,
            summary: None,
            message: None,
            presence: None,
            voice: None,
        }
    }
}
