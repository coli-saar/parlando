use anyhow::{bail, Result};
use async_trait::async_trait;
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use serde_json::json;

use crate::config::TtsConfig;

#[derive(Clone, Debug)]
pub struct AudioChunk {
    /// Raw mono PCM bytes decoded from the TTS provider.
    pub data: Vec<u8>,
    /// Sample rate for this audio chunk.
    pub sample_rate: u32,
    /// Number of audio channels.
    pub channels: u16,
}

/// Streaming text-to-speech provider used by the agent voice runtime.
#[async_trait]
pub trait StreamingTtsProvider: Send + Sync {
    /// Synthesizes one conversation message into ordered audio chunks.
    async fn synthesize(&self, text: &str, message_id: &str) -> Result<Vec<AudioChunk>>;
}

/// ElevenLabs WebSocket streaming TTS provider.
pub struct ElevenLabsStreamingTtsProvider {
    config: TtsConfig,
}

impl ElevenLabsStreamingTtsProvider {
    /// Creates an ElevenLabs provider from validated TTS config.
    pub fn new(config: TtsConfig) -> Result<Self> {
        if config.api_key.is_empty() || config.voice_id.is_empty() {
            bail!("ElevenLabs TTS requires tts.api_key and tts.voice_id");
        }
        Ok(Self { config })
    }
}

#[async_trait]
impl StreamingTtsProvider for ElevenLabsStreamingTtsProvider {
    async fn synthesize(&self, text: &str, _message_id: &str) -> Result<Vec<AudioChunk>> {
        let sample_rate = sample_rate_from_output_format(&self.config.output_format);
        let url = format!(
            "{}/v1/text-to-speech/{}/stream-input?model_id={}&output_format={}",
            self.config.base_url.trim_end_matches('/'),
            self.config.voice_id,
            self.config.model,
            self.config.output_format
        );
        let (mut socket, _) = tokio_tungstenite::connect_async(url).await?;
        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                json!({
                    "text": " ",
                    "xi_api_key": self.config.api_key,
                    "voice_settings": {"stability": 0.5, "similarity_boost": 0.75}
                })
                .to_string(),
            ))
            .await?;
        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                json!({"text": text, "try_trigger_generation": true}).to_string(),
            ))
            .await?;
        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                json!({"text": ""}).to_string(),
            ))
            .await?;
        let mut chunks = vec![];
        while let Some(message) = socket.next().await {
            let message = message?;
            if !message.is_text() {
                continue;
            }
            let payload: serde_json::Value = serde_json::from_str(message.to_text()?)?;
            if let Some(audio) = payload.get("audio").and_then(|value| value.as_str()) {
                if !audio.is_empty() {
                    chunks.push(AudioChunk {
                        data: base64::engine::general_purpose::STANDARD.decode(audio)?,
                        sample_rate,
                        channels: 1,
                    });
                }
            }
            if payload
                .get("isFinal")
                .and_then(|value| value.as_bool())
                .unwrap_or(false)
            {
                break;
            }
        }
        Ok(chunks)
    }
}

/// Maps an ElevenLabs output format string to its PCM sample rate.
pub fn sample_rate_from_output_format(output_format: &str) -> u32 {
    if output_format.contains("44100") {
        44100
    } else if output_format.contains("24000") {
        24000
    } else if output_format.contains("22050") {
        22050
    } else if output_format.contains("16000") {
        16000
    } else {
        24000
    }
}

#[cfg(test)]
mod tests {
    use base64::Engine as _;
    use tokio::net::TcpListener;
    use tokio_tungstenite::{accept_async, tungstenite::Message};

    use super::*;

    // Creates a minimal TTS config suitable for provider tests.
    fn tts_config() -> TtsConfig {
        TtsConfig {
            enabled: true,
            provider: "elevenlabs".to_string(),
            model: "eleven_flash_v2_5".to_string(),
            voice_id: "voice-1".to_string(),
            voice_name: "Voice".to_string(),
            base_url: "wss://api.elevenlabs.io".to_string(),
            api_key: "api-key".to_string(),
            output_format: "pcm_16000".to_string(),
        }
    }

    #[test]
    fn sample_rate_mapping_uses_output_format_suffix() {
        assert_eq!(sample_rate_from_output_format("pcm_16000"), 16000);
        assert_eq!(sample_rate_from_output_format("pcm_22050"), 22050);
        assert_eq!(sample_rate_from_output_format("pcm_24000"), 24000);
        assert_eq!(sample_rate_from_output_format("pcm_44100"), 44100);
        assert_eq!(sample_rate_from_output_format("mp3_44100_128"), 44100);
        assert_eq!(sample_rate_from_output_format("unknown"), 24000);
    }

    #[test]
    fn elevenlabs_provider_requires_credentials() {
        let mut config = tts_config();
        config.api_key.clear();

        assert!(ElevenLabsStreamingTtsProvider::new(config).is_err());
    }

    #[tokio::test]
    async fn elevenlabs_streaming_client_decodes_audio_and_final_frame() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = accept_async(stream).await.unwrap();
            let first = socket.next().await.unwrap().unwrap().into_text().unwrap();
            assert!(first.contains("\"xi_api_key\":\"api-key\""));
            let second = socket.next().await.unwrap().unwrap().into_text().unwrap();
            assert!(second.contains("\"text\":\"hello\""));
            let third = socket.next().await.unwrap().unwrap().into_text().unwrap();
            assert!(third.contains("\"text\":\"\""));
            let audio = base64::engine::general_purpose::STANDARD.encode([1_u8, 2, 3]);
            socket
                .send(Message::Text(json!({"audio": audio}).to_string()))
                .await
                .unwrap();
            socket
                .send(Message::Text(json!({"isFinal": true}).to_string()))
                .await
                .unwrap();
        });
        let mut config = tts_config();
        config.base_url = format!("ws://{addr}");
        let provider = ElevenLabsStreamingTtsProvider::new(config).unwrap();

        let chunks = provider.synthesize("hello", "msg-1").await.unwrap();

        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].data, vec![1, 2, 3]);
        assert_eq!(chunks[0].sample_rate, 16000);
        assert_eq!(chunks[0].channels, 1);
        server.await.unwrap();
    }
}
