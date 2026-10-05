use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub(crate) mod clock;
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::client::IntoClientRequest};

use crate::{audio::AudioFrame, config::SpeechmaticsConfig};

/// Context that identifies one authenticated speaker transcription stream.
#[derive(Clone, Debug)]
pub struct TranscriptionSessionContext {
    /// Authoritative player role for transcript attribution.
    #[allow(dead_code)]
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

/// One recognized word or punctuation mark with independently estimated timing.
/// Times are milliseconds in the provider's supplied-audio clock until the
/// server maps them into the capture clock. Entity tokens use spoken forms.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TranscriptToken {
    /// Provider result category: word or punctuation.
    pub kind: String,
    /// Recognized spoken spelling, or punctuation glyph.
    pub text: String,
    /// Inclusive onset in the current clock domain.
    pub start_time_ms: i64,
    /// End boundary in the current clock domain; punctuation may have zero duration.
    pub end_time_ms: i64,
    /// Provider confidence when supplied, between zero and one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
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
    /// Ordered timed words and punctuation; formatted text remains separate.
    pub tokens: Vec<TranscriptToken>,
}

/// Normalized output emitted by any streaming or utterance STT provider.
#[derive(Clone, Debug)]
pub enum TranscriptionEvent {
    /// Provider accepted configuration and is ready for audio.
    Ready,
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
                "enable_entities": true,
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
            // Reading remains independent of socket writes under network backpressure.
            let writer_events = events.clone();
            let writer = tokio::spawn(async move {
                let mut sequence = 0usize;
                while let Some(input) = inputs.recv().await {
                    let message = match input {
                        TranscriptionInput::Audio(frame) => {
                            sequence += 1;
                            tokio_tungstenite::tungstenite::Message::Binary(frame.pcm)
                        }
                        TranscriptionInput::Finish => {
                            let _ = write
                                .send(tokio_tungstenite::tungstenite::Message::Text(
                                    json!({"message":"EndOfStream","last_seq_no":sequence})
                                        .to_string(),
                                ))
                                .await;
                            return;
                        }
                    };
                    if write.send(message).await.is_err() {
                        let _ = writer_events
                            .send(TranscriptionEvent::Failed(
                                "Speechmatics audio write failed".into(),
                            ))
                            .await;
                        return;
                    }
                }
                let _ = write
                    .send(tokio_tungstenite::tungstenite::Message::Text(
                        json!({"message":"EndOfStream","last_seq_no":sequence}).to_string(),
                    ))
                    .await;
            });
            let mut pending = Vec::new();
            loop {
                let message = tokio::select! {
                    _ = events.closed() => break,
                    message = read.next() => message,
                };
                let Some(Ok(message)) = message else {
                    break;
                };
                if !message.is_text() {
                    continue;
                }
                let Ok(payload) = serde_json::from_str::<Value>(message.to_text().unwrap_or(""))
                else {
                    continue;
                };
                match payload["message"].as_str() {
                    Some("AddTranscript") => match parse_fragment(&payload) {
                        Ok(Some(fragment)) => {
                            pending.push(fragment);
                            if config.end_of_utterance_silence_trigger <= 0.0 {
                                flush_utterance(&mut pending, &events).await;
                            }
                        }
                        Ok(None) => {}
                        Err(error) => {
                            let _ = events
                                .send(TranscriptionEvent::Failed(format!(
                                    "Invalid Speechmatics final: {error}"
                                )))
                                .await;
                            break;
                        }
                    },
                    Some("EndOfUtterance") => flush_utterance(&mut pending, &events).await,
                    Some("EndOfTranscript") => {
                        flush_utterance(&mut pending, &events).await;
                        writer.abort();
                        return;
                    }
                    Some("Error") => {
                        let _ = events
                            .send(TranscriptionEvent::Failed(payload.to_string()))
                            .await;
                        break;
                    }
                    _ => {}
                }
            }
            writer.abort();
            let _ = events
                .send(TranscriptionEvent::Failed(
                    "Speechmatics closed before EndOfTranscript".into(),
                ))
                .await;
        });
        Ok(TranscriptionSessionHandle {
            input,
            events: event_receiver,
        })
    }
}

/// Converts a required, finite, nonnegative provider timestamp without guessing.
fn seconds_ms(value: &Value) -> Result<i64> {
    let seconds = value.as_f64().context("missing timestamp")?;
    if !seconds.is_finite() || seconds < 0.0 || seconds * 1000.0 >= i64::MAX as f64 {
        bail!("invalid timestamp");
    }
    Ok((seconds * 1000.0).round() as i64)
}

/// Extracts spoken words recursively; entity written forms stay in utterance text.
fn parse_tokens(results: &[Value], tokens: &mut Vec<TranscriptToken>) -> Result<()> {
    for result in results {
        if result["type"] == "entity" {
            let spoken = result["spoken_form"]
                .as_array()
                .context("entity lacks spoken form")?;
            parse_tokens(spoken, tokens)?;
            continue;
        }
        let kind = result["type"].as_str().context("missing result type")?;
        if !matches!(kind, "word" | "punctuation") {
            bail!("unsupported result type: {kind}");
        }
        let alternative = result["alternatives"]
            .as_array()
            .and_then(|items| items.first())
            .context("missing alternative")?;
        let text = alternative["content"]
            .as_str()
            .context("missing token text")?;
        let start_time_ms = seconds_ms(&result["start_time"])?;
        let end_time_ms = seconds_ms(&result["end_time"])?;
        if end_time_ms < start_time_ms {
            bail!("reversed token interval");
        }
        let confidence = alternative["confidence"].as_f64();
        if confidence.is_some_and(|value| !(0.0..=1.0).contains(&value)) {
            bail!("invalid confidence");
        }
        tokens.push(TranscriptToken {
            kind: kind.into(),
            text: text.into(),
            start_time_ms,
            end_time_ms,
            confidence,
        });
    }
    Ok(())
}

/// Parses one final fragment, keeping absolute stream offsets and provider formatting.
fn parse_fragment(payload: &Value) -> Result<Option<FinalTranscriptUtterance>> {
    let metadata = &payload["metadata"];
    let text = metadata["transcript"]
        .as_str()
        .context("missing final text")?;
    if text.trim().is_empty() {
        return Ok(None);
    }
    let start_time_ms = seconds_ms(&metadata["start_time"])?;
    let end_time_ms = seconds_ms(&metadata["end_time"])?;
    if end_time_ms < start_time_ms {
        bail!("reversed utterance interval");
    }
    let mut tokens = Vec::new();
    if let Some(results) = payload["results"].as_array() {
        parse_tokens(results, &mut tokens)?;
    }
    Ok(Some(FinalTranscriptUtterance {
        start_time_ms,
        end_time_ms,
        text: text.into(),
        tokens,
        result_ids: payload["id"]
            .as_str()
            .map(|id| vec![id.into()])
            .unwrap_or_default(),
    }))
}

/// Combines final fragments into one parent utterance without altering formatting.
fn take_utterance(pending: &mut Vec<FinalTranscriptUtterance>) -> Option<FinalTranscriptUtterance> {
    if pending.is_empty() {
        return None;
    }
    let mut utterance = FinalTranscriptUtterance {
        start_time_ms: pending.iter().map(|part| part.start_time_ms).min()?,
        end_time_ms: pending.iter().map(|part| part.end_time_ms).max()?,
        text: String::new(),
        result_ids: Vec::new(),
        tokens: Vec::new(),
    };
    for part in pending.drain(..) {
        utterance.text.push_str(&part.text);
        utterance.result_ids.extend(part.result_ids);
        utterance.tokens.extend(part.tokens);
    }
    utterance.text = utterance.text.trim().to_string();
    Some(utterance)
}

/// Emits the accumulated final fragments at the provider's utterance boundary.
async fn flush_utterance(
    pending: &mut Vec<FinalTranscriptUtterance>,
    events: &mpsc::Sender<TranscriptionEvent>,
) {
    if let Some(utterance) = take_utterance(pending) {
        let _ = events
            .send(TranscriptionEvent::FinalUtterance(utterance))
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parses saved real provider finals without network access or credentials.
    fn replay(source: &str) -> Vec<FinalTranscriptUtterance> {
        let messages: Vec<Value> = serde_json::from_str(source).unwrap();
        let mut pending = Vec::new();
        let mut utterances = Vec::new();
        for message in messages {
            match message["message"].as_str() {
                Some("AddTranscript") => {
                    if let Some(fragment) = parse_fragment(&message).unwrap() {
                        pending.push(fragment);
                    }
                }
                Some("EndOfUtterance" | "EndOfTranscript") => {
                    if let Some(utterance) = take_utterance(&mut pending) {
                        utterances.push(utterance);
                    }
                }
                _ => {}
            }
        }
        utterances
    }

    /// The experiment's sentence remains fourteen words within one formatted utterance.
    #[test]
    fn saved_sentence_has_independent_words() {
        let utterances = replay(include_str!("../tests/fixtures/speechmatics/sentence.json"));
        assert_eq!(utterances.len(), 1);
        let utterance = &utterances[0];
        assert_eq!(
            utterance.text,
            "So I think you should do two long presses because the stripe is blue."
        );
        assert_eq!(
            utterance
                .tokens
                .iter()
                .filter(|token| token.kind == "word")
                .count(),
            14
        );
        assert_eq!(utterance.tokens[2].start_time_ms, 360);
    }

    /// Normalized currency keeps six timed spoken words instead of one written entity.
    #[test]
    fn saved_entities_keep_spoken_forms() {
        let utterances = replay(include_str!("../tests/fixtures/speechmatics/entities.json"));
        let tokens: Vec<_> = utterances
            .iter()
            .flat_map(|utterance| &utterance.tokens)
            .collect();
        assert!(utterances
            .iter()
            .any(|utterance| utterance.text.contains("£17.25")));
        assert!(!tokens.iter().any(|token| token.text == "£17.25"));
        let start = tokens
            .iter()
            .position(|token| token.text == "Seventeen")
            .unwrap();
        assert_eq!(
            tokens[start..start + 6]
                .iter()
                .map(|token| token.text.as_str())
                .collect::<Vec<_>>(),
            ["Seventeen", "pounds", "and", "twenty", "five", "pence"]
        );
        assert_eq!(tokens[start].start_time_ms, 1400);
    }

    /// Metadata onset must never be added a second time to absolute result timestamps.
    #[test]
    fn saved_anchor_offsets_are_absolute() {
        let utterances = replay(include_str!("../tests/fixtures/speechmatics/anchors.json"));
        assert_eq!(utterances[0].start_time_ms, 2720);
        assert_eq!(utterances[0].tokens[0].start_time_ms, 3000);
    }

    /// Malformed times are rejected rather than silently converted to zero.
    #[test]
    fn invalid_times_fail() {
        assert!(seconds_ms(&Value::Null).is_err());
        assert!(seconds_ms(&json!(-1)).is_err());
    }
}
