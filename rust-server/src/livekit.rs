use anyhow::{bail, Result};
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use serde::Serialize;

use crate::config::LiveKitConfig;

/// Builds the LiveKit identity string used for browser and worker participants.
pub fn livekit_identity(room_id: &str, role: &str, participant_session_id: &str) -> String {
    format!("{room_id}:{role}:{participant_session_id}")
}

/// Parses a LiveKit identity string into `(room_id, role, participant_session_id)`.
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

/// Creates an HS256 LiveKit access token with a room join grant.
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

#[cfg(test)]
mod tests {
    use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};
    use serde::Deserialize;

    use super::*;

    #[derive(Debug, Deserialize)]
    struct TestClaims {
        iss: String,
        sub: String,
        exp: i64,
        nbf: i64,
        video: TestVideoGrant,
    }

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct TestVideoGrant {
        room: String,
        room_join: bool,
        can_publish: bool,
        can_subscribe: bool,
        can_publish_data: bool,
    }

    #[test]
    fn livekit_identity_round_trips_and_rejects_malformed_values() {
        let identity = livekit_identity("room-1", "A", "participant-1");

        assert_eq!(
            parse_livekit_identity(&identity).unwrap(),
            (
                "room-1".to_string(),
                "A".to_string(),
                "participant-1".to_string()
            )
        );
        assert!(parse_livekit_identity("room:A").is_err());
        assert!(parse_livekit_identity("room::participant").is_err());
    }

    #[test]
    fn livekit_token_contains_expected_hs256_claims() {
        let config = LiveKitConfig {
            enabled: true,
            url: "wss://livekit.example.test".to_string(),
            api_key: "api-key".to_string(),
            api_secret: "secret".to_string(),
        };
        let token = create_livekit_token(&config, "room-1", "B", "session-1", 3600).unwrap();
        let claims = decode::<TestClaims>(
            &token,
            &DecodingKey::from_secret(config.api_secret.as_bytes()),
            &Validation::new(Algorithm::HS256),
        )
        .unwrap()
        .claims;

        assert_eq!(claims.iss, "api-key");
        assert_eq!(claims.sub, "room-1:B:session-1");
        assert!(claims.exp > claims.nbf);
        assert_eq!(claims.video.room, "room-1");
        assert!(claims.video.room_join);
        assert!(claims.video.can_publish);
        assert!(claims.video.can_subscribe);
        assert!(claims.video.can_publish_data);
    }

    #[test]
    fn livekit_token_requires_credentials() {
        let config = LiveKitConfig {
            enabled: true,
            url: "wss://livekit.example.test".to_string(),
            api_key: String::new(),
            api_secret: String::new(),
        };

        assert!(create_livekit_token(&config, "room", "A", "session", 3600).is_err());
    }
}
