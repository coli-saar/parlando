use std::time::Duration;

use anyhow::{bail, Result};
use async_trait::async_trait;
use tokio::time::{sleep_until, Instant};

use crate::{
    audio::{AudioFrame, SharedAudioRooms, AUDIO_CHANNELS, AUDIO_FRAME_BYTES, AUDIO_SAMPLE_RATE},
    tts::AudioChunk,
};

/// Summary returned after publishing one synthesized agent message.
#[derive(Clone, Debug)]
pub struct AudioPublishSummary {
    /// Number of non-empty canonical frames published.
    pub chunks_published: usize,
    /// Number of PCM bytes published.
    pub bytes_published: usize,
    /// Canonical sample rate used for playback.
    pub sample_rate: u32,
    /// Canonical channel count used for playback.
    pub channels: u16,
}

/// Publishes synthesized agent audio into a room-specific audio transport.
#[async_trait]
pub trait AgentAudioPublisher: Send + Sync {
    /// Publishes one synthesized message to connected human browsers.
    async fn publish(
        &self,
        room_id: &str,
        message_id: &str,
        chunks: &[AudioChunk],
    ) -> Result<AudioPublishSummary>;
}

/// Agent audio publisher backed by the process-local Parlando audio room registry.
pub(crate) struct RoomAgentAudioPublisher {
    rooms: SharedAudioRooms,
    prebuffer_frames: usize,
}

impl RoomAgentAudioPublisher {
    /// Creates a publisher using the browser's configured initial jitter-buffer target.
    pub(crate) fn new(rooms: SharedAudioRooms, jitter_buffer_ms: u16) -> Self {
        Self {
            rooms,
            prebuffer_frames: usize::from(jitter_buffer_ms)
                .div_ceil(usize::from(crate::audio::AUDIO_FRAME_DURATION_MS))
                .max(1),
        }
    }
}

#[async_trait]
impl AgentAudioPublisher for RoomAgentAudioPublisher {
    async fn publish(
        &self,
        room_id: &str,
        _message_id: &str,
        chunks: &[AudioChunk],
    ) -> Result<AudioPublishSummary> {
        let mut pcm = vec![];
        for chunk in chunks.iter().filter(|chunk| !chunk.data.is_empty()) {
            if chunk.sample_rate != AUDIO_SAMPLE_RATE || chunk.channels != AUDIO_CHANNELS {
                bail!("agent TTS must return 24000 Hz mono PCM");
            }
            pcm.extend_from_slice(&chunk.data);
        }
        if pcm.is_empty() {
            bail!("cannot publish empty agent audio");
        }
        let mut count = 0usize;
        let started_at = Instant::now();
        for (index, payload) in pcm.chunks(AUDIO_FRAME_BYTES).enumerate() {
            sleep_until(started_at + frame_send_offset(index, self.prebuffer_frames)).await;
            let mut padded = vec![0; AUDIO_FRAME_BYTES];
            padded[..payload.len()].copy_from_slice(payload);
            let frame = AudioFrame {
                sequence: index as u32,
                timestamp_ms: index as u64 * 20,
                pcm: padded,
            };
            self.rooms.publish_agent(room_id, frame.encode()).await;
            count += 1;
        }
        Ok(AudioPublishSummary {
            chunks_published: count,
            bytes_published: pcm.len(),
            sample_rate: AUDIO_SAMPLE_RATE,
            channels: AUDIO_CHANNELS,
        })
    }
}

/// Returns an absolute send offset that maintains a fixed browser playout lead.
fn frame_send_offset(frame_index: usize, prebuffer_frames: usize) -> Duration {
    let paced_index = frame_index.saturating_sub(prebuffer_frames.saturating_sub(1));
    Duration::from_millis(paced_index as u64 * u64::from(crate::audio::AUDIO_FRAME_DURATION_MS))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_schedule_prebuffers_then_uses_absolute_twenty_ms_deadlines() {
        let offsets = (0..8)
            .map(|index| frame_send_offset(index, 5))
            .collect::<Vec<_>>();

        assert_eq!(offsets[..5], [Duration::ZERO; 5]);
        assert_eq!(offsets[5], Duration::from_millis(20));
        assert_eq!(offsets[6], Duration::from_millis(40));
        assert_eq!(offsets[7], Duration::from_millis(60));
    }
}
