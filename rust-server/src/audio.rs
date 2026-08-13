use std::{collections::HashMap, sync::Arc};

use anyhow::{bail, Result};
use tokio::sync::{mpsc, RwLock};

/// Maximum number of outbound audio messages buffered for one connected participant.
///
/// The relay deliberately uses a bounded queue so a slow browser cannot accumulate
/// unbounded stale speech or memory. Once this limit is reached, newly relayed audio
/// is dropped in favor of keeping the room live.
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
struct AudioRoom {
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

/// In-memory registry for active, process-local audio room connections.
#[derive(Default)]
pub struct AudioRoomRegistry {
    rooms: RwLock<HashMap<String, AudioRoom>>,
}

impl AudioRoomRegistry {
    /// Registers the latest browser connection for a role and returns its outbound queue.
    pub async fn connect(
        &self,
        room_id: &str,
        role: &str,
    ) -> (String, mpsc::Receiver<AudioOutbound>) {
        let (sender, receiver) = mpsc::channel(AUDIO_OUTBOUND_QUEUE_CAPACITY);
        let generation = uuid::Uuid::new_v4().to_string();
        self.rooms
            .write()
            .await
            .entry(room_id.to_string())
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
    pub async fn disconnect(&self, room_id: &str, role: &str, generation: &str) {
        let mut rooms = self.rooms.write().await;
        if let Some(room) = rooms.get_mut(room_id) {
            if room
                .peers
                .get(role)
                .is_some_and(|peer| peer.generation == generation)
            {
                room.peers.remove(role);
            }
            if room.peers.is_empty() {
                rooms.remove(room_id);
            }
        }
    }

    /// Reports whether a connection generation still owns its room role.
    pub async fn is_current(&self, room_id: &str, role: &str, generation: &str) -> bool {
        self.rooms
            .read()
            .await
            .get(room_id)
            .and_then(|room| room.peers.get(role))
            .is_some_and(|peer| peer.generation == generation)
    }

    /// Relays a frame to the other human role without waiting on a slow browser.
    pub async fn relay_partner(&self, room_id: &str, sender_role: &str, bytes: Vec<u8>) {
        let partner_role = match sender_role {
            "A" => "B",
            "B" => "A",
            _ => return,
        };
        let target = {
            let rooms = self.rooms.read().await;
            rooms
                .get(room_id)
                .and_then(|room| room.peers.get(partner_role))
                .map(|peer| peer.sender.clone())
        };
        if let Some(target) = target {
            let _ = target.try_send(AudioOutbound::Binary(bytes));
        }
    }

    /// Sends a provider-neutral control message to one connected role.
    pub async fn send_control(&self, room_id: &str, role: &str, text: String) {
        let target = self
            .rooms
            .read()
            .await
            .get(room_id)
            .and_then(|room| room.peers.get(role))
            .map(|peer| peer.sender.clone());
        if let Some(target) = target {
            let _ = target.try_send(AudioOutbound::Text(text));
        }
    }

    /// Publishes server-generated agent audio to all connected human browsers in a room.
    pub async fn publish_agent(&self, room_id: &str, bytes: Vec<u8>) {
        let targets = self
            .rooms
            .read()
            .await
            .get(room_id)
            .map(|room| {
                room.peers
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
pub type SharedAudioRooms = Arc<AudioRoomRegistry>;

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

    #[test]
    fn frame_rejects_wrong_version_and_size() {
        let mut encoded = frame().encode();
        encoded[0] = 2;
        assert!(AudioFrame::decode(&encoded).is_err());
        assert!(AudioFrame::decode(&encoded[..encoded.len() - 1]).is_err());
    }

    #[tokio::test]
    async fn relay_routes_only_to_partner() {
        let rooms = AudioRoomRegistry::default();
        let (_, mut a) = rooms.connect("room", "A").await;
        let (_, mut b) = rooms.connect("room", "B").await;
        rooms.relay_partner("room", "A", vec![1]).await;
        assert!(matches!(b.recv().await, Some(AudioOutbound::Binary(bytes)) if bytes == vec![1]));
        assert!(a.try_recv().is_err());
    }

    #[tokio::test]
    async fn replacement_connection_owns_role_until_its_own_disconnect() {
        let rooms = AudioRoomRegistry::default();
        let (old_generation, _old_receiver) = rooms.connect("room", "A").await;
        let (new_generation, _new_receiver) = rooms.connect("room", "A").await;

        assert!(!rooms.is_current("room", "A", &old_generation).await);
        assert!(rooms.is_current("room", "A", &new_generation).await);
        rooms.disconnect("room", "A", &old_generation).await;
        assert!(rooms.is_current("room", "A", &new_generation).await);
        rooms.disconnect("room", "A", &new_generation).await;
        assert!(!rooms.is_current("room", "A", &new_generation).await);
    }
}
