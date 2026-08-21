use std::{collections::HashMap, sync::Arc};

use anyhow::{bail, Result};
use tokio::sync::{mpsc, RwLock};

/// Maximum number of outbound audio messages buffered for one connected participant.
///
/// The relay deliberately uses a bounded queue so a slow browser cannot accumulate
/// unbounded stale speech or memory. Once this limit is reached, newly relayed audio
/// is dropped in favor of keeping the session live.
pub const AUDIO_OUTBOUND_QUEUE_CAPACITY: usize = 64;

/// Wire protocol version for Parlando PCM audio frames.
pub const AUDIO_PROTOCOL_VERSION: u8 = 1;
/// Canonical audio sample rate used by browser, relay, STT, and TTS.
pub const AUDIO_SAMPLE_RATE: u32 = 24_000;
/// Canonical number of mono audio channels.
pub const AUDIO_CHANNELS: u16 = 1;
/// Duration represented by one complete browser audio frame.
pub const AUDIO_FRAME_DURATION_MS: u16 = 20;
/// Number of PCM payload bytes in one complete canonical audio frame.
pub const AUDIO_FRAME_BYTES: usize = 960;
/// Number of bytes preceding PCM in a binary WebSocket message.
pub const AUDIO_HEADER_BYTES: usize = 13;

/// One validated canonical PCM frame with sender timing metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioFrame {
    /// Monotonically increasing sequence number within a connection generation.
    pub sequence: u32,
    /// Capture time in milliseconds relative to the browser stream start.
    pub timestamp_ms: u64,
    /// Signed 16-bit little-endian mono PCM payload.
    pub pcm: Vec<u8>,
}

impl AudioFrame {
    /// Decodes and validates one binary WebSocket audio message.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != AUDIO_HEADER_BYTES + AUDIO_FRAME_BYTES {
            bail!(
                "audio frame must contain {} bytes",
                AUDIO_HEADER_BYTES + AUDIO_FRAME_BYTES
            );
        }
        if bytes[0] != AUDIO_PROTOCOL_VERSION {
            bail!("unsupported audio protocol version {}", bytes[0]);
        }
        Ok(Self {
            sequence: u32::from_be_bytes(bytes[1..5].try_into().expect("fixed sequence slice")),
            timestamp_ms: u64::from_be_bytes(
                bytes[5..13].try_into().expect("fixed timestamp slice"),
            ),
            pcm: bytes[AUDIO_HEADER_BYTES..].to_vec(),
        })
    }

    /// Encodes this frame for delivery to a browser.
    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(AUDIO_HEADER_BYTES + self.pcm.len());
        bytes.push(AUDIO_PROTOCOL_VERSION);
        bytes.extend_from_slice(&self.sequence.to_be_bytes());
        bytes.extend_from_slice(&self.timestamp_ms.to_be_bytes());
        bytes.extend_from_slice(&self.pcm);
        bytes
    }
}

#[derive(Default)]
struct AudioSession {
    peers: HashMap<String, AudioPeer>,
}

struct AudioPeer {
    generation: String,
    sender: mpsc::Sender<AudioOutbound>,
}

/// One server-to-browser audio transport message.
pub enum AudioOutbound {
    /// Versioned binary PCM frame.
    Binary(Vec<u8>),
    /// JSON control or status message.
    Text(String),
}

/// In-memory registry for active, process-local audio session connections.
#[derive(Default)]
pub struct AudioSessionRegistry {
    sessions: RwLock<HashMap<String, AudioSession>>,
}

impl AudioSessionRegistry {
    /// Registers the latest browser connection for a role and returns its outbound queue.
    pub async fn connect(
        &self,
        public_session_id: &str,
        role: &str,
    ) -> (String, mpsc::Receiver<AudioOutbound>) {
        let (sender, receiver) = mpsc::channel(AUDIO_OUTBOUND_QUEUE_CAPACITY);
        let generation = uuid::Uuid::new_v4().to_string();
        self.sessions
            .write()
            .await
            .entry(public_session_id.to_string())
            .or_default()
            .peers
            .insert(
                role.to_string(),
                AudioPeer {
                    generation: generation.clone(),
                    sender,
                },
            );
        (generation, receiver)
    }

    /// Removes a browser connection only when it is still the current generation.
    pub async fn disconnect(&self, public_session_id: &str, role: &str, generation: &str) {
        let mut sessions = self.sessions.write().await;
        if let Some(session) = sessions.get_mut(public_session_id) {
            if session
                .peers
                .get(role)
                .is_some_and(|peer| peer.generation == generation)
            {
                session.peers.remove(role);
            }
            if session.peers.is_empty() {
                sessions.remove(public_session_id);
            }
        }
    }

    /// Reports whether a connection generation still owns its session role.
    pub async fn is_current(&self, public_session_id: &str, role: &str, generation: &str) -> bool {
        self.sessions
            .read()
            .await
            .get(public_session_id)
            .and_then(|session| session.peers.get(role))
            .is_some_and(|peer| peer.generation == generation)
    }

    /// Relays a frame to the other human role without waiting on a slow browser.
    pub async fn relay_partner(&self, public_session_id: &str, sender_role: &str, bytes: Vec<u8>) {
        let partner_role = match sender_role {
            "A" => "B",
            "B" => "A",
            _ => return,
        };
        let target = {
            let sessions = self.sessions.read().await;
            sessions
                .get(public_session_id)
                .and_then(|session| session.peers.get(partner_role))
                .map(|peer| peer.sender.clone())
        };
        if let Some(target) = target {
            let _ = target.try_send(AudioOutbound::Binary(bytes));
        }
    }

    /// Sends a provider-neutral control message to one connected role.
    pub async fn send_control(&self, public_session_id: &str, role: &str, text: String) {
        let target = self
            .sessions
            .read()
            .await
            .get(public_session_id)
            .and_then(|session| session.peers.get(role))
            .map(|peer| peer.sender.clone());
        if let Some(target) = target {
            let _ = target.try_send(AudioOutbound::Text(text));
        }
    }

    /// Publishes server-generated agent audio to all connected human browsers in a session.
    pub async fn publish_agent(&self, public_session_id: &str, bytes: Vec<u8>) {
        let targets = self
            .sessions
            .read()
            .await
            .get(public_session_id)
            .map(|session| {
                session
                    .peers
                    .values()
                    .map(|peer| peer.sender.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for target in targets {
            let _ = target.try_send(AudioOutbound::Binary(bytes.clone()));
        }
    }
}

/// Shared handle used by the application and agent TTS publisher.
pub type SharedAudioSessions = Arc<AudioSessionRegistry>;

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds one valid test frame.
    fn frame() -> AudioFrame {
        AudioFrame {
            sequence: 7,
            timestamp_ms: 140,
            pcm: vec![0; AUDIO_FRAME_BYTES],
        }
    }

    #[test]
    fn frame_round_trips() {
        assert_eq!(AudioFrame::decode(&frame().encode()).unwrap(), frame());
    }

    /// Keeps Rust framing byte-for-byte aligned with the browser implementation.
    #[test]
    fn shared_pcm_wire_fixtures_match() {
        let fixtures: serde_json::Value = serde_json::from_str(include_str!(
            "../../proto/pcm_frame_v1.fixtures.json"
        ))
        .unwrap();
        for fixture in fixtures["cases"].as_array().unwrap() {
            let frame = AudioFrame {
                sequence: fixture["sequence"].as_u64().unwrap() as u32,
                timestamp_ms: fixture["timestamp_ms"].as_u64().unwrap(),
                pcm: vec![0; AUDIO_FRAME_BYTES],
            };
            let expected: Vec<u8> = fixture["header_bytes"]
                .as_array()
                .unwrap()
                .iter()
                .map(|byte| byte.as_u64().unwrap() as u8)
                .collect();
            assert_eq!(&frame.encode()[..AUDIO_HEADER_BYTES], expected);
            assert_eq!(AudioFrame::decode(&frame.encode()).unwrap(), frame);
        }
    }

    #[test]
    fn frame_rejects_wrong_version_and_size() {
        let mut encoded = frame().encode();
        encoded[0] = 2;
        assert!(AudioFrame::decode(&encoded).is_err());
        assert!(AudioFrame::decode(&encoded[..encoded.len() - 1]).is_err());
    }

    #[tokio::test]
    async fn relay_routes_only_to_partner() {
        let sessions = AudioSessionRegistry::default();
        let (_, mut a) = sessions.connect("session", "A").await;
        let (_, mut b) = sessions.connect("session", "B").await;
        sessions.relay_partner("session", "A", vec![1]).await;
        assert!(matches!(b.recv().await, Some(AudioOutbound::Binary(bytes)) if bytes == vec![1]));
        assert!(a.try_recv().is_err());
    }

    #[tokio::test]
    async fn replacement_connection_owns_role_until_its_own_disconnect() {
        let sessions = AudioSessionRegistry::default();
        let (old_generation, _old_receiver) = sessions.connect("session", "A").await;
        let (new_generation, _new_receiver) = sessions.connect("session", "A").await;

        assert!(!sessions.is_current("session", "A", &old_generation).await);
        assert!(sessions.is_current("session", "A", &new_generation).await);
        sessions.disconnect("session", "A", &old_generation).await;
        assert!(sessions.is_current("session", "A", &new_generation).await);
        sessions.disconnect("session", "A", &new_generation).await;
        assert!(!sessions.is_current("session", "A", &new_generation).await);
    }

    /// Confirms a saturated peer queue drops new frames without blocking or harming another session.
    #[tokio::test]
    async fn saturated_partner_queue_is_bounded_and_room_local() {
        let sessions = AudioSessionRegistry::default();
        let (_, _a) = sessions.connect("busy", "A").await;
        let (_, mut busy_b) = sessions.connect("busy", "B").await;
        let (_, _other_a) = sessions.connect("other", "A").await;
        let (_, mut other_b) = sessions.connect("other", "B").await;

        for index in 0..AUDIO_OUTBOUND_QUEUE_CAPACITY + 5 {
            sessions.relay_partner("busy", "A", vec![index as u8]).await;
        }
        sessions.relay_partner("other", "A", vec![99]).await;

        let mut delivered = Vec::new();
        while let Ok(AudioOutbound::Binary(bytes)) = busy_b.try_recv() {
            delivered.push(bytes[0]);
        }
        assert_eq!(delivered.len(), AUDIO_OUTBOUND_QUEUE_CAPACITY);
        assert_eq!(delivered[0], 0);
        assert_eq!(delivered.last().copied(), Some(63));
        assert!(matches!(
            other_b.try_recv(),
            Ok(AudioOutbound::Binary(bytes)) if bytes == vec![99]
        ));
    }

    /// Verifies control messages target one role and agent audio fans out to every current peer.
    #[tokio::test]
    async fn control_is_targeted_and_agent_audio_fans_out() {
        let sessions = AudioSessionRegistry::default();
        let (_, mut a) = sessions.connect("session", "A").await;
        let (_, mut b) = sessions.connect("session", "B").await;

        sessions.send_control("session", "A", "ready".into()).await;
        assert!(matches!(a.recv().await, Some(AudioOutbound::Text(text)) if text == "ready"));
        assert!(b.try_recv().is_err());

        sessions.publish_agent("session", vec![4, 2]).await;
        assert!(
            matches!(a.recv().await, Some(AudioOutbound::Binary(bytes)) if bytes == vec![4, 2])
        );
        assert!(
            matches!(b.recv().await, Some(AudioOutbound::Binary(bytes)) if bytes == vec![4, 2])
        );
    }

    /// Confirms missing sessions and invalid role names are safe no-ops.
    #[tokio::test]
    async fn missing_rooms_and_unknown_roles_are_noops() {
        let sessions = AudioSessionRegistry::default();
        let (_, mut a) = sessions.connect("session", "A").await;
        sessions.relay_partner("missing", "A", vec![1]).await;
        sessions
            .relay_partner("session", "spectator", vec![2])
            .await;
        sessions
            .send_control("missing", "A", "ignored".into())
            .await;
        sessions.publish_agent("missing", vec![3]).await;
        assert!(a.try_recv().is_err());
    }
}
