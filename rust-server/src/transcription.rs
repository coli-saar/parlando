use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::client::IntoClientRequest};

use crate::{audio::AudioFrame, config::SpeechmaticsConfig};

/// Context that identifies one authenticated speaker transcription stream.
#[derive(Clone, Debug)]
pub struct TranscriptionSessionContext {
    /// Room that owns the audio stream.
    pub room_id: String,
    /// Participant session that owns the microphone.
    pub participant_session_id: String,
    /// Authoritative player role for transcript attribution.
    pub role: String,
    /// BCP-47-like configured language identifier.
    pub language: String,
    /// Provider model selection.
    pub model: String,
}

/// Input accepted by a running provider session.
#[derive(Debug)]
pub enum TranscriptionInput {
    /// One canonical PCM frame.
    Audio(AudioFrame),
    /// Gracefully finish the stream and flush final output.
    Finish,
}

/// One provider-independent final spoken utterance.
#[derive(Clone, Debug)]
pub struct FinalTranscriptUtterance {
    /// Start time in the provider stream.
    pub start_time_ms: i64,
    /// End time in the provider stream.
    pub end_time_ms: i64,
    /// Final normalized text.
    pub text: String,
    /// Stable provider result identifiers where available.
    pub result_ids: Vec<String>,
}

/// Normalized output emitted by any streaming or utterance STT provider.
#[derive(Clone, Debug)]
pub enum TranscriptionEvent {
    /// Provider accepted configuration and is ready for audio.
    Ready,
    /// Replaceable partial hypothesis intended only for status/UI.
    Partial(String),
    /// Durable utterance safe to persist and send to agents.
    FinalUtterance(FinalTranscriptUtterance),
    /// Provider session failed; partner audio can continue independently.
    Failed(String),
}

/// Channels owned by one running provider task.
pub struct TranscriptionSessionHandle {
    /// Bounded audio input queue.
    pub input: mpsc::Sender<TranscriptionInput>,
    /// Provider output stream.
    pub events: mpsc::Receiver<TranscriptionEvent>,
}

/// Factory for provider-owned per-speaker transcription sessions.
#[async_trait]
pub trait TranscriptionProvider: Send + Sync {
    /// Starts one isolated transcription stream.
    async fn start_session(
        &self,
        context: TranscriptionSessionContext,
    ) -> Result<TranscriptionSessionHandle>;
}

/// Server-side Speechmatics realtime transcription provider.
pub struct SpeechmaticsTranscriptionProvider {
    config: SpeechmaticsConfig,
}

impl SpeechmaticsTranscriptionProvider {
    /// Creates a provider that keeps its permanent API key on the server.
    pub fn new(config: SpeechmaticsConfig) -> Result<Self> {
        if config.api_key.is_empty() {
            bail!("Speechmatics transcription requires an API key");
        }
        Ok(Self { config })
    }
}

#[async_trait]
impl TranscriptionProvider for SpeechmaticsTranscriptionProvider {
    async fn start_session(
        &self,
        context: TranscriptionSessionContext,
    ) -> Result<TranscriptionSessionHandle> {
        let mut request = self.config.realtime_url.clone().into_client_request()?;
        request.headers_mut().insert(
            "Authorization",
            format!("Bearer {}", self.config.api_key).parse()?,
        );
        let (socket, _) = connect_async(request)
            .await
            .context("failed to connect Speechmatics realtime STT")?;
        let (input, mut inputs) = mpsc::channel(250);
        let (events, event_receiver) = mpsc::channel(64);
        let config = self.config.clone();
        tokio::spawn(async move {
            let (mut write, mut read) = socket.split();
            let language = context
                .language
                .split('-')
                .next()
                .unwrap_or(&context.language);
            let mut transcription_config = json!({
                "language": language,
                "enable_partials": config.enable_partials,
                "max_delay": config.max_delay
            });
            if config.end_of_utterance_silence_trigger > 0.0 {
                transcription_config["conversation_config"] = json!({
                    "end_of_utterance_silence_trigger": config.end_of_utterance_silence_trigger
                });
            }
            if !context.model.is_empty() && context.model != "default" {
                transcription_config["model"] = Value::String(context.model.clone());
            }
            let start = json!({
                "message": "StartRecognition",
                "audio_format": {"type": "raw", "encoding": "pcm_s16le", "sample_rate": 24000},
                "transcription_config": transcription_config
            });
            if write
                .send(tokio_tungstenite::tungstenite::Message::Text(
                    start.to_string(),
                ))
                .await
                .is_err()
            {
                let _ = events
                    .send(TranscriptionEvent::Failed(
                        "could not start Speechmatics".into(),
                    ))
                    .await;
                return;
            }
            loop {
                let Some(Ok(message)) = read.next().await else {
                    let _ = events
                        .send(TranscriptionEvent::Failed(
                            "Speechmatics closed before recognition started".into(),
                        ))
                        .await;
                    return;
                };
                if !message.is_text() {
                    continue;
                }
                let Ok(payload) = serde_json::from_str::<Value>(message.to_text().unwrap_or(""))
                else {
                    continue;
                };
                match payload["message"].as_str() {
                    Some("RecognitionStarted") => {
                        let _ = events.send(TranscriptionEvent::Ready).await;
                        break;
                    }
                    Some("Error") => {
                        let _ = events
                            .send(TranscriptionEvent::Failed(payload.to_string()))
                            .await;
                        return;
                    }
                    _ => {}
                }
            }
            let mut pending: Vec<(String, i64, i64, Option<String>)> = vec![];
            let mut sequence = 0usize;
            loop {
                tokio::select! {
                    input = inputs.recv() => match input {
                        Some(TranscriptionInput::Audio(frame)) => {
                            sequence += 1;
                            if write.send(tokio_tungstenite::tungstenite::Message::Binary(frame.pcm)).await.is_err() { break; }
                        }
                        Some(TranscriptionInput::Finish) | None => {
                            let _ = write.send(tokio_tungstenite::tungstenite::Message::Text(json!({"message":"EndOfStream","last_seq_no":sequence}).to_string())).await;
                            break;
                        }
                    },
                    message = read.next() => {
                        let Some(Ok(message)) = message else { break; };
                        if !message.is_text() { continue; }
                        let Ok(payload) = serde_json::from_str::<Value>(message.to_text().unwrap_or("")) else { continue; };
                        match payload["message"].as_str() {
                            Some("AddPartialTranscript") => {
                                if let Some(text) = payload["metadata"]["transcript"].as_str().filter(|text| !text.is_empty()) {
                                    let _ = events.send(TranscriptionEvent::Partial(text.to_string())).await;
                                }
                            }
                            Some("AddTranscript") => {
                                let metadata = &payload["metadata"];
                                if let Some(text) = metadata["transcript"].as_str().filter(|text| !text.trim().is_empty()) {
                                    pending.push((text.to_string(), seconds_ms(metadata["start_time"].as_f64()), seconds_ms(metadata["end_time"].as_f64()), payload["id"].as_str().map(str::to_string)));
                                    if config.end_of_utterance_silence_trigger <= 0.0 { flush_utterance(&mut pending, &events).await; }
                                }
                            }
                            Some("EndOfUtterance") => flush_utterance(&mut pending, &events).await,
                            Some("EndOfTranscript") => { flush_utterance(&mut pending, &events).await; return; }
                            Some("Error") => { let _ = events.send(TranscriptionEvent::Failed(payload.to_string())).await; return; }
                            _ => {}
                        }
                    }
                }
            }
            while let Some(Ok(message)) = read.next().await {
                if !message.is_text() {
                    continue;
                }
                let Ok(payload) = serde_json::from_str::<Value>(message.to_text().unwrap_or(""))
                else {
                    continue;
                };
                match payload["message"].as_str() {
                    Some("AddTranscript") => {
                        let metadata = &payload["metadata"];
                        if let Some(text) = metadata["transcript"]
                            .as_str()
                            .filter(|text| !text.trim().is_empty())
                        {
                            pending.push((
                                text.to_string(),
                                seconds_ms(metadata["start_time"].as_f64()),
                                seconds_ms(metadata["end_time"].as_f64()),
                                payload["id"].as_str().map(str::to_string),
                            ));
                        }
                    }
                    Some("EndOfUtterance") => flush_utterance(&mut pending, &events).await,
                    Some("EndOfTranscript") => break,
                    _ => {}
                }
            }
            flush_utterance(&mut pending, &events).await;
        });
        Ok(TranscriptionSessionHandle {
            input,
            events: event_receiver,
        })
    }
}

/// Converts optional provider seconds into non-negative milliseconds.
fn seconds_ms(seconds: Option<f64>) -> i64 {
    (seconds.unwrap_or_default().max(0.0) * 1000.0).round() as i64
}

/// Emits one normalized utterance from accumulated final provider fragments.
async fn flush_utterance(
    pending: &mut Vec<(String, i64, i64, Option<String>)>,
    events: &mpsc::Sender<TranscriptionEvent>,
) {
    if pending.is_empty() {
        return;
    }
    let utterance = FinalTranscriptUtterance {
        start_time_ms: pending
            .iter()
            .map(|entry| entry.1)
            .min()
            .unwrap_or_default(),
        end_time_ms: pending
            .iter()
            .map(|entry| entry.2)
            .max()
            .unwrap_or_default(),
        text: pending
            .iter()
            .map(|entry| entry.0.trim())
            .collect::<Vec<_>>()
            .join(" "),
        result_ids: pending.iter().filter_map(|entry| entry.3.clone()).collect(),
    };
    pending.clear();
    let _ = events
        .send(TranscriptionEvent::FinalUtterance(utterance))
        .await;
}
