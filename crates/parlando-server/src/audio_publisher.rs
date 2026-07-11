use std::time::Duration;

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use livekit::{
    options::TrackPublishOptions,
    prelude::{
        LocalAudioTrack, LocalTrack, Room, RoomEvent, RoomOptions, RtcAudioSource, TrackSource,
    },
    webrtc::{
        audio_source::native::NativeAudioSource,
        prelude::{AudioFrame, AudioSourceOptions},
    },
};
use tokio::time::{sleep, timeout};

use crate::{config::LiveKitConfig, livekit::create_livekit_token, tts::AudioChunk};

/// Summary returned after publishing one synthesized agent message.
#[derive(Clone, Debug)]
pub struct AudioPublishSummary {
    /// Number of non-empty audio chunks submitted to the RTC source.
    pub chunks_published: usize,
    /// Number of PCM bytes submitted to the RTC source.
    pub bytes_published: usize,
    /// Sample rate used for the published audio track.
    pub sample_rate: u32,
    /// Number of audio channels used for the published audio track.
    pub channels: u16,
}

/// Publishes synthesized agent audio into a room-specific audio transport.
#[async_trait]
pub trait AgentAudioPublisher: Send + Sync {
    /// Publishes one synthesized agent message for the given room.
    async fn publish(
        &self,
        room_id: &str,
        message_id: &str,
        chunks: &[AudioChunk],
    ) -> Result<AudioPublishSummary>;
}

/// LiveKit implementation of agent audio publishing.
pub struct LiveKitAgentAudioPublisher {
    config: LiveKitConfig,
}

impl LiveKitAgentAudioPublisher {
    /// Creates a LiveKit audio publisher from server LiveKit config.
    pub fn new(config: LiveKitConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl AgentAudioPublisher for LiveKitAgentAudioPublisher {
    /// Publishes PCM chunks into a short-lived `agent-voice` LiveKit track.
    async fn publish(
        &self,
        room_id: &str,
        message_id: &str,
        chunks: &[AudioChunk],
    ) -> Result<AudioPublishSummary> {
        let Some(first_audio) = chunks.iter().find(|chunk| !chunk.data.is_empty()) else {
            bail!("cannot publish empty agent audio");
        };
        let sample_rate = first_audio.sample_rate;
        let channels = first_audio.channels;
        if channels == 0 {
            bail!("cannot publish agent audio with zero channels");
        }
        let token = create_livekit_token(
            &self.config,
            room_id,
            "agent-voice",
            &format!("agent-audio-{message_id}"),
            300,
        )?;
        let (room, mut events) = Room::connect(&self.config.url, &token, RoomOptions::default())
            .await
            .context("failed to connect LiveKit agent audio publisher")?;
        let source = NativeAudioSource::new(
            AudioSourceOptions::default(),
            sample_rate,
            channels as u32,
            1000,
        );
        let track = LocalAudioTrack::create_audio_track(
            "agent-voice",
            RtcAudioSource::Native(source.clone()),
        );
        room.local_participant()
            .publish_track(
                LocalTrack::Audio(track.clone()),
                TrackPublishOptions {
                    source: TrackSource::Microphone,
                    ..TrackPublishOptions::default()
                },
            )
            .await
            .context("failed to publish LiveKit agent voice track")?;
        wait_for_subscriber(&mut events)
            .await
            .context("failed while waiting for LiveKit subscriber before agent audio publish")?;

        let mut chunks_published = 0usize;
        let mut bytes_published = 0usize;
        for chunk in chunks.iter().filter(|chunk| !chunk.data.is_empty()) {
            if chunk.sample_rate != sample_rate || chunk.channels != channels {
                bail!("all agent audio chunks must share one sample rate and channel count");
            }
            publish_pcm_chunk(&source, chunk).await?;
            chunks_published += 1;
            bytes_published += chunk.data.len();
        }

        sleep(Duration::from_millis(250)).await;
        room.local_participant()
            .unpublish_track(&track.sid())
            .await
            .context("failed to unpublish LiveKit agent voice track")?;
        let _ = room.close().await;
        Ok(AudioPublishSummary {
            chunks_published,
            bytes_published,
            sample_rate,
            channels,
        })
    }
}

/// Waits until LiveKit confirms that at least one remote participant subscribed to the track.
async fn wait_for_subscriber(
    events: &mut tokio::sync::mpsc::UnboundedReceiver<RoomEvent>,
) -> Result<()> {
    timeout(Duration::from_secs(10), async {
        loop {
            let event = events
                .recv()
                .await
                .context("LiveKit room event stream closed before subscriber attached")?;
            match event {
                RoomEvent::LocalTrackSubscribed { .. } => return Ok(()),
                RoomEvent::TrackSubscriptionFailed { error, .. } => {
                    bail!("LiveKit track subscription failed: {error}");
                }
                _ => {}
            }
        }
    })
    .await
    .context("timed out waiting for LiveKit subscriber")?
}

/// Submits one raw PCM chunk to a LiveKit native audio source.
async fn publish_pcm_chunk(source: &NativeAudioSource, chunk: &AudioChunk) -> Result<()> {
    let samples = pcm_bytes_to_i16_samples(&chunk.data)?;
    let channels = chunk.channels as u32;
    if samples.len() % channels as usize != 0 {
        bail!("PCM sample count is not divisible by channel count");
    }
    let frame = AudioFrame {
        data: samples.as_slice().into(),
        sample_rate: chunk.sample_rate,
        num_channels: channels,
        samples_per_channel: samples.len() as u32 / channels,
    };
    source.capture_frame(&frame).await?;
    Ok(())
}

/// Converts little-endian signed 16-bit PCM bytes to samples.
pub fn pcm_bytes_to_i16_samples(bytes: &[u8]) -> Result<Vec<i16>> {
    if bytes.len() % 2 != 0 {
        bail!("PCM byte length must be even");
    }
    Ok(bytes
        .chunks_exact(2)
        .map(|sample| i16::from_le_bytes([sample[0], sample[1]]))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pcm_bytes_to_i16_samples_decodes_little_endian_values() {
        let samples = pcm_bytes_to_i16_samples(&[0, 0, 255, 127, 0, 128]).unwrap();
        assert_eq!(samples, vec![0, 32767, -32768]);
    }
}
