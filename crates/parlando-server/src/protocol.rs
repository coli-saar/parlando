use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ParticipantCreateRequest {
    #[serde(default = "default_direct")]
    pub source: String,
    pub display_name: Option<String>,
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
    pub display_name: Option<String>,
    pub study_id: Option<String>,
    pub external_id: Option<String>,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DirectEnterRequest {
    pub display_name: Option<String>,
    pub study_id: Option<String>,
    pub external_id: Option<String>,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ParticipantCreateResponse {
    pub participant_session_id: String,
    pub source: String,
    pub display_name: Option<String>,
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
    pub consents: Vec<ConsentItemResponse>,
    pub livekit: Value,
    pub transcription: Value,
    pub tts: Value,
    pub conversation: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ConsentRequest {
    pub participant_session_id: String,
    pub room_id: Option<String>,
    #[serde(default)]
    pub decisions: std::collections::HashMap<String, bool>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CreateRoomRequest {
    pub participant_session_id: String,
    #[serde(default = "default_direct")]
    pub mode: String,
    pub force_role: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct JoinRoomRequest {
    pub participant_session_id: String,
    pub force_role: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct MatchmakingJoinRequest {
    pub participant_session_id: String,
    pub study_id: Option<String>,
    pub queue: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RoomResponse {
    pub room_id: String,
    pub participant_session_id: String,
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observation: Option<Value>,
    #[serde(default)]
    pub available_actions: Vec<Value>,
    #[serde(default)]
    pub events: Vec<Value>,
    #[serde(default)]
    pub conversation: Vec<ConversationMessageResponse>,
}

pub type CreateRoomResponse = RoomResponse;
pub type JoinRoomResponse = RoomResponse;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct MatchmakingJoinResponse {
    pub status: String,
    pub participant_session_id: String,
    pub source: Option<String>,
    pub room_id: Option<String>,
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observation: Option<Value>,
    #[serde(default)]
    pub available_actions: Vec<Value>,
    #[serde(default)]
    pub events: Vec<Value>,
    #[serde(default)]
    pub conversation: Vec<ConversationMessageResponse>,
}

pub type DirectEnterResponse = MatchmakingJoinResponse;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LiveKitTokenRequest {
    pub participant_session_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LiveKitWorkerTokenRequest {
    pub role: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LiveKitTokenResponse {
    pub enabled: bool,
    pub url: Option<String>,
    pub token: Option<String>,
    pub identity: Option<String>,
}

impl LiveKitTokenResponse {
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            url: None,
            token: None,
            identity: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AudioSessionRequest {
    pub participant_session_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AudioSinkPlan {
    pub id: String,
    pub provider: String,
    pub purposes: Vec<String>,
    pub transport: String,
    #[serde(default)]
    pub credentials: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AudioSessionPlanResponse {
    pub enabled: bool,
    #[serde(default = "default_capture")]
    pub capture: Value,
    #[serde(default)]
    pub sinks: Vec<AudioSinkPlan>,
}

fn default_capture() -> Value {
    json!({"audio": true})
}

impl AudioSessionPlanResponse {
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            capture: default_capture(),
            sinks: vec![],
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TranscriptSegmentIn {
    pub participant_session_id: String,
    pub player: String,
    #[serde(default)]
    pub start_time_ms: i64,
    #[serde(default)]
    pub end_time_ms: i64,
    pub move_count: Option<i64>,
    pub text: String,
    #[serde(default)]
    pub metadata: Value,
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
    #[serde(default)]
    pub available_actions: Vec<Value>,
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
            available_actions: vec![],
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
