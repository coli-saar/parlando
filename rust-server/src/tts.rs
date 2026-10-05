use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::json;

use crate::transcription::TranscriptToken;

use crate::config::TtsConfig;

/// One ordered chunk of synthesized PCM, retained only for audio publication.
#[derive(Clone, Debug)]
pub struct AudioChunk {
    /// Raw mono PCM bytes decoded from the TTS provider.
    pub data: Vec<u8>,
    /// Sample rate for this audio chunk.
    pub sample_rate: u32,
    /// Number of audio channels.
    pub channels: u16,
}

/// Synthesized speech and its word alignment in the concatenated PCM clock.
/// Tokens describe generated speech rather than recognition hypotheses; they have
/// no ASR confidence. Providers without alignment leave the token array empty.
#[derive(Clone, Debug)]
pub struct SynthesizedSpeech {
    /// Ordered PCM chunks to publish without storing raw audio.
    pub chunks: Vec<AudioChunk>,
    /// Spoken words and punctuation, in milliseconds from the first PCM sample.
    pub tokens: Vec<TranscriptToken>,
}

/// Character boundaries returned alongside one synchronized provider audio chunk.
#[derive(Deserialize)]
struct CharacterAlignment {
    chars: Vec<String>,
    #[serde(alias = "charStartTimesMs")]
    char_start_times_ms: Vec<f64>,
    #[serde(alias = "charDurationsMs")]
    char_durations_ms: Vec<f64>,
}

/// One normalized spoken character in the concatenated audio clock.
struct TimedCharacter {
    character: char,
    start_ms: i64,
    end_ms: i64,
}

/// Validates alignment offsets, which remain cumulative within one synthesis request.
/// Chunk boundaries do not reset this clock; adding PCM offsets would double-count time.
fn append_alignment(value: &serde_json::Value, output: &mut Vec<TimedCharacter>) -> Result<()> {
    let alignment: CharacterAlignment = serde_json::from_value(value.clone())?;
    if alignment.chars.len() != alignment.char_start_times_ms.len()
        || alignment.chars.len() != alignment.char_durations_ms.len()
    {
        bail!("TTS alignment array lengths differ");
    }
    for ((text, start), duration) in alignment
        .chars
        .iter()
        .zip(alignment.char_start_times_ms)
        .zip(alignment.char_durations_ms)
    {
        if !start.is_finite()
            || !duration.is_finite()
            || start < 0.0
            || duration < 0.0
            || start + duration >= i64::MAX as f64
        {
            bail!("invalid TTS character interval");
        }
        let start_ms = start.round() as i64;
        let end_ms = (start + duration).round() as i64;
        if output
            .last()
            .is_some_and(|previous| start_ms < previous.start_ms)
        {
            bail!("TTS alignment clock moved backwards");
        }
        for character in text.chars() {
            output.push(TimedCharacter {
                character,
                start_ms,
                end_ms,
            });
        }
    }
    Ok(())
}

/// Groups normalized characters into spoken words, including words split across chunks.
fn alignment_tokens(characters: &[TimedCharacter]) -> Vec<TranscriptToken> {
    let mut tokens: Vec<TranscriptToken> = Vec::new();
    let mut in_word = false;
    for (index, item) in characters.iter().enumerate() {
        let character = item.character;
        if character.is_whitespace() {
            in_word = false;
            continue;
        }
        let connector = matches!(character, '\'' | '’' | '-')
            && in_word
            && characters
                .get(index + 1)
                .is_some_and(|next| next.character.is_alphanumeric());
        if character.is_alphanumeric() || connector {
            if in_word {
                let token = tokens.last_mut().expect("a word was started");
                token.text.push(character);
                token.end_time_ms = token.end_time_ms.max(item.end_ms);
            } else {
                tokens.push(TranscriptToken {
                    kind: "word".into(),
                    text: character.to_string(),
                    start_time_ms: item.start_ms,
                    end_time_ms: item.end_ms,
                    confidence: None,
                });
                in_word = true;
            }
        } else {
            in_word = false;
            // Punctuation is a written boundary, not a separate span of speech.
            tokens.push(TranscriptToken {
                kind: "punctuation".into(),
                text: character.to_string(),
                start_time_ms: item.start_ms,
                end_time_ms: item.start_ms,
                confidence: None,
            });
        }
    }
    tokens
}

/// Streaming text-to-speech provider used by the agent voice runtime.
#[async_trait]
pub trait StreamingTtsProvider: Send + Sync {
    /// Synthesizes one conversation message into ordered PCM and independently timed spoken tokens.
    async fn synthesize(&self, text: &str, message_id: &str) -> Result<SynthesizedSpeech>;
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
    async fn synthesize(&self, text: &str, _message_id: &str) -> Result<SynthesizedSpeech> {
        let sample_rate = sample_rate_from_output_format(&self.config.output_format);
        let url = format!(
            "{}/v1/text-to-speech/{}/stream-input?model_id={}&output_format={}&sync_alignment=true",
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
        let mut characters = Vec::new();
        let mut sample_offset = 0usize;
        let mut complete = false;
        while let Some(message) = socket.next().await {
            let message = message?;
            if !message.is_text() {
                continue;
            }
            let payload: serde_json::Value = serde_json::from_str(message.to_text()?)?;
            if let Some(error) = payload.get("error") {
                bail!("TTS provider error: {error}");
            }
            let alignment = payload
                .get("normalizedAlignment")
                .or_else(|| payload.get("normalized_alignment"))
                .filter(|value| !value.is_null())
                .or_else(|| payload.get("alignment").filter(|value| !value.is_null()));
            if let Some(alignment) = alignment {
                append_alignment(alignment, &mut characters)
                    .context("invalid ElevenLabs alignment")?;
            }
            if let Some(audio) = payload
                .get("audio")
                .and_then(|value| value.as_str())
                .filter(|audio| !audio.is_empty())
            {
                let data = base64::engine::general_purpose::STANDARD.decode(audio)?;
                if data.len() % 2 != 0 {
                    bail!("TTS returned an incomplete PCM sample");
                }
                sample_offset += data.len() / 2;
                chunks.push(AudioChunk {
                    data,
                    sample_rate,
                    channels: 1,
                });
            }
            if payload
                .get("isFinal")
                .or_else(|| payload.get("is_final"))
                .and_then(|value| value.as_bool())
                .unwrap_or(false)
            {
                complete = true;
                break;
            }
        }
        if !complete {
            bail!("TTS closed before final output");
        }
        let tokens = alignment_tokens(&characters);
        let audio_end_ms = (sample_offset as f64 * 1000.0 / f64::from(sample_rate)).round() as i64;
        if tokens
            .iter()
            .any(|token| token.start_time_ms > audio_end_ms || token.end_time_ms > audio_end_ms)
        {
            bail!("TTS alignment extends beyond synthesized PCM");
        }
        Ok(SynthesizedSpeech { chunks, tokens })
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

    /// Joins words spanning PCM chunks and uses normalized spoken entity text.
    #[tokio::test]
    async fn elevenlabs_alignment_keeps_absolute_offsets_and_joins_split_words() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_hdr_async(stream, |request: &tokio_tungstenite::tungstenite::handshake::server::Request, response: tokio_tungstenite::tungstenite::handshake::server::Response| {
                assert!(request.uri().query().unwrap().contains("sync_alignment=true"));
                Ok(response)
            }).await.unwrap();
            for _ in 0..3 {
                socket.next().await.unwrap().unwrap();
            }
            let first = json!({"audio":base64::engine::general_purpose::STANDARD.encode(vec![0;14400]),
                "normalizedAlignment":{"chars":["H","e","l"],"charStartTimesMs":[0,100,200],"charDurationsMs":[100,100,100]}});
            let text = "lo seventeen.";
            let second = json!({"audio":base64::engine::general_purpose::STANDARD.encode(vec![0;text.len()*4800]),
                "normalized_alignment":{"chars":text.chars().map(|c| c.to_string()).collect::<Vec<_>>(),
                    "char_start_times_ms":(0..text.len()).map(|i| 300+i*100).collect::<Vec<_>>(),
                    "char_durations_ms":vec![100;text.len()]},
                "alignment":{"chars":["£","1","7"],"char_start_times_ms":[0,100,200],"char_durations_ms":[100,100,100]}});
            socket.send(Message::Text(first.to_string())).await.unwrap();
            socket
                .send(Message::Text(second.to_string()))
                .await
                .unwrap();
            socket
                .send(Message::Text(json!({"is_final":true}).to_string()))
                .await
                .unwrap();
        });
        let mut config = tts_config();
        config.output_format = "pcm_24000".into();
        config.base_url = format!("ws://{addr}");
        let speech = ElevenLabsStreamingTtsProvider::new(config)
            .unwrap()
            .synthesize("Hello £17", "message")
            .await
            .unwrap();
        assert_eq!(
            speech
                .tokens
                .iter()
                .map(|token| token.text.as_str())
                .collect::<Vec<_>>(),
            ["Hello", "seventeen", "."]
        );
        assert_eq!(
            (speech.tokens[0].start_time_ms, speech.tokens[0].end_time_ms),
            (0, 500)
        );
        assert_eq!(
            (speech.tokens[1].start_time_ms, speech.tokens[1].end_time_ms),
            (600, 1500)
        );
        assert_eq!(
            (speech.tokens[2].start_time_ms, speech.tokens[2].end_time_ms),
            (1500, 1500)
        );
        assert!(speech.tokens.iter().all(|token| token.confidence.is_none()));
        server.await.unwrap();
    }

    /// Saved eleven-chunk provider output must keep cumulative offsets exactly once.
    #[test]
    fn saved_elevenlabs_alignment_stays_within_pcm_duration() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/elevenlabs/synchronized-chunks.json"
        ))
        .unwrap();
        let mut characters = Vec::new();
        let mut samples = 0usize;
        for chunk in fixture["chunks"].as_array().unwrap() {
            let alignment = chunk
                .get("normalizedAlignment")
                .or_else(|| chunk.get("normalized_alignment"))
                .unwrap();
            append_alignment(alignment, &mut characters).unwrap();
            samples += chunk["sample_count"].as_u64().unwrap() as usize;
        }
        let tokens = alignment_tokens(&characters);
        assert_eq!(fixture["chunks"].as_array().unwrap().len(), 11);
        assert_eq!(
            tokens.iter().filter(|token| token.kind == "word").count(),
            38
        );
        let end = (samples as f64 / 24.0).round() as i64;
        assert!(tokens.iter().all(|token| token.end_time_ms <= end));
        assert!(tokens
            .iter()
            .any(|token| token.text == "seventeen" && token.start_time_ms > 7000));
    }

    /// Invalid timing arrays fail validation rather than producing guessed offsets.
    #[test]
    fn malformed_character_alignment_is_rejected() {
        let mut characters = Vec::new();
        assert!(append_alignment(
            &json!({"chars":["a"],"char_start_times_ms":[],"char_durations_ms":[1]}),
            &mut characters
        )
        .is_err());
        assert!(append_alignment(
            &json!({"chars":["a"],"char_start_times_ms":[-1],"char_durations_ms":[1]}),
            &mut characters
        )
        .is_err());
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
            let audio = base64::engine::general_purpose::STANDARD.encode([1_u8, 2, 3, 4]);
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

        let speech = provider.synthesize("hello", "msg-1").await.unwrap();
        assert!(speech.tokens.is_empty());
        let chunks = speech.chunks;

        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].data, vec![1, 2, 3, 4]);
        assert_eq!(chunks[0].sample_rate, 16000);
        assert_eq!(chunks[0].channels, 1);
        server.await.unwrap();
    }
}
