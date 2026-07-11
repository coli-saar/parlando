use anyhow::{bail, Result};
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use serde::Serialize;

use crate::config::LiveKitConfig;

pub fn livekit_identity(room_id: &str, role: &str, participant_session_id: &str) -> String {
    format!("{room_id}:{role}:{participant_session_id}")
}

pub fn parse_livekit_identity(identity: &str) -> Result<(String, String, String)> {
    let parts = identity.splitn(3, ':').collect::<Vec<_>>();
    if parts.len() != 3 || parts.iter().any(|part| part.is_empty()) {
        bail!("invalid LiveKit participant identity: {identity:?}");
    }
    Ok((
        parts[0].to_string(),
        parts[1].to_string(),
        parts[2].to_string(),
    ))
}

#[derive(Serialize)]
struct LiveKitClaims<'a> {
    iss: &'a str,
    sub: String,
    nbf: i64,
    exp: i64,
    video: LiveKitVideoGrant<'a>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LiveKitVideoGrant<'a> {
    room: &'a str,
    room_join: bool,
    can_publish: bool,
    can_subscribe: bool,
    can_publish_data: bool,
}

pub fn create_livekit_token(
    config: &LiveKitConfig,
    room_id: &str,
    role: &str,
    participant_session_id: &str,
    ttl_seconds: i64,
) -> Result<String> {
    if config.api_key.is_empty() || config.api_secret.is_empty() {
        bail!("LiveKit API key and secret are required when LiveKit is enabled");
    }
    let now = chrono::Utc::now().timestamp();
    let claims = LiveKitClaims {
        iss: &config.api_key,
        sub: livekit_identity(room_id, role, participant_session_id),
        nbf: now,
        exp: now + ttl_seconds,
        video: LiveKitVideoGrant {
            room: room_id,
            room_join: true,
            can_publish: true,
            can_subscribe: true,
            can_publish_data: true,
        },
    };
    Ok(encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(config.api_secret.as_bytes()),
    )?)
}
