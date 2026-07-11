use anyhow::{bail, Result};
use async_trait::async_trait;
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use serde_json::json;

use crate::config::TtsConfig;

#[derive(Clone, Debug)]
pub struct AudioChunk {
    pub data: Vec<u8>,
    pub sample_rate: u32,
    pub channels: u16,
    pub final_chunk: bool,
}

#[async_trait]
pub trait StreamingTtsProvider: Send + Sync {
    async fn synthesize(&self, text: &str, message_id: &str) -> Result<Vec<AudioChunk>>;
}

pub struct ElevenLabsStreamingTtsProvider {
    config: TtsConfig,
}

impl ElevenLabsStreamingTtsProvider {
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
            "wss://api.elevenlabs.io/v1/text-to-speech/{}/stream-input?model_id={}&output_format={}",
            self.config.voice_id, self.config.model, self.config.output_format
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
            .send(tokio_tungstenite::tungstenite::Message::Text(json!({"text": ""}).to_string()))
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
                        final_chunk: false,
                    });
                }
            }
            if payload.get("isFinal").and_then(|value| value.as_bool()).unwrap_or(false) {
                chunks.push(AudioChunk { data: vec![], sample_rate, channels: 1, final_chunk: true });
                break;
            }
        }
        Ok(chunks)
    }
}

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
