//! Synthesizes and publishes agent speech with game-clock alignment.
//!
//! After the live agent commits a message, speech delivery passes its text to the
//! synthesis provider and the resulting audio to the session publisher. Recorded word
//! timings use the server publication anchor. Synthesis and publication failures are
//! reported as diagnostics for that message.

use super::{
    anyhow, json, now_iso, persist_session_event, session_event_record, AppState, Arc, Context,
    ConversationMessageResponse, Duration, Game, Result, Value,
};

/// Converts one trusted agent message to speech with a network timeout but no application quota.
pub(super) async fn speak_agent_message<A: Game>(
    state: &Arc<AppState<A>>,
    public_session_id: &str,
    message: &ConversationMessageResponse,
) {
    let Some(provider) = state.tts_provider.clone() else {
        return;
    };
    state.context.installation.telemetry.begin_tts();
    let mut failed = false;
    persist_tts_diagnostic(
        state,
        public_session_id,
        "tts_message_started",
        json!({"message_id": message.id}),
    )
    .await;
    match tokio::time::timeout(
        Duration::from_secs(60),
        provider.synthesize(&message.text, &message.id),
    )
    .await
    {
        Ok(Ok(speech)) => {
            let chunks = speech.chunks;
            let mut saw_audio = false;
            for chunk in &chunks {
                if !chunk.data.is_empty() && !saw_audio {
                    saw_audio = true;
                    persist_tts_diagnostic(
                        state,
                        public_session_id,
                        "tts_first_audio",
                        json!({
                            "message_id": message.id,
                            "sample_rate": chunk.sample_rate,
                            "channels": chunk.channels,
                        }),
                    )
                    .await;
                }
            }
            persist_tts_diagnostic(
                state,
                public_session_id,
                "tts_audio_summary",
                json!({
                    "message_id": message.id,
                    "chunks": chunks.len(),
                    "bytes": chunks.iter().map(|chunk| chunk.data.len()).sum::<usize>(),
                    "sample_rate": chunks.first().map(|chunk| chunk.sample_rate),
                    "channels": chunks.first().map(|chunk| chunk.channels),
                }),
            )
            .await;
            if let Some(publisher) = state.audio_publisher.clone() {
                persist_tts_diagnostic(
                    state,
                    public_session_id,
                    "tts_publish_started",
                    json!({"message_id": message.id}),
                )
                .await;
                let publication_started_at = now_iso();
                match publisher
                    .publish(public_session_id, &message.id, &chunks)
                    .await
                {
                    Ok(summary) => {
                        if let Err(error) = record_agent_speech_timing(
                            state,
                            public_session_id,
                            message,
                            &publication_started_at,
                            &speech.tokens,
                            &summary,
                        )
                        .await
                        {
                            failed = true;
                            tracing::warn!(%error, %public_session_id, "failed to record agent speech timing");
                        }
                        persist_tts_diagnostic(
                            state,
                            public_session_id,
                            "tts_publish_completed",
                            json!({
                                "message_id": message.id,
                                "chunks": summary.chunks_published,
                                "bytes": summary.bytes_published,
                                "sample_rate": summary.sample_rate,
                                "channels": summary.channels,
                            }),
                        )
                        .await;
                    }
                    Err(error) => {
                        failed = true;
                        persist_tts_diagnostic(
                            state,
                            public_session_id,
                            "tts_publish_failed",
                            json!({"message_id": message.id, "error": error.to_string()}),
                        )
                        .await;
                    }
                }
            }
            persist_tts_diagnostic(
                state,
                public_session_id,
                "tts_message_completed",
                json!({"message_id": message.id}),
            )
            .await;
        }
        Ok(Err(error)) => {
            failed = true;
            persist_tts_diagnostic(
                state,
                public_session_id,
                "tts_message_failed",
                json!({"message_id": message.id, "error": error.to_string()}),
            )
            .await;
        }
        Err(_) => {
            failed = true;
            persist_tts_diagnostic(
                state,
                public_session_id,
                "tts_message_failed",
                json!({"message_id": message.id, "error": "TTS network timeout"}),
            )
            .await;
        }
    }
    state.context.installation.telemetry.finish_tts(failed);
}

/// Records published agent speech on the game clock without duplicating conversation text.
/// The server publication anchor estimates playback onset; browser/network buffering
/// may shift what the listener hears. Word offsets come from the TTS alignment.
async fn record_agent_speech_timing<A: Game>(
    state: &Arc<AppState<A>>,
    public_session_id: &str,
    message: &ConversationMessageResponse,
    publication_started_at: &str,
    tokens: &[crate::transcription::TranscriptToken],
    summary: &crate::audio_publisher::AudioPublishSummary,
) -> Result<()> {
    let mut record = session_event_record(
        state,
        public_session_id,
        message.sender_participant_session_id.as_deref(),
        "conversation_speech_timing",
        Value::Null,
        None,
    )
    .await?;
    let origin_ms = state
        .context
        .installation
        .store
        .session_game_time_ms(
            &record.experiment_id,
            record.session_id,
            publication_started_at,
        )
        .await?;
    let denominator = i64::from(summary.sample_rate) * i64::from(summary.channels) * 2;
    if denominator == 0 {
        return Err(anyhow!("invalid published audio format"));
    }
    let duration_ms = i64::try_from(summary.bytes_published)?
        .checked_mul(1000)
        .and_then(|duration| duration.checked_add(denominator / 2))
        .context("speech duration overflow")?
        / denominator;
    let end_ms = origin_ms
        .checked_add(duration_ms)
        .context("speech endpoint overflow")?;
    let tokens = tokens
        .iter()
        .map(|token| -> Result<Value> {
            Ok(json!({"kind":token.kind,"text":token.text,
            "start_ms":origin_ms.checked_add(token.start_time_ms).context("token onset overflow")?,
            "end_ms":origin_ms.checked_add(token.end_time_ms).context("token endpoint overflow")?}))
        })
        .collect::<Result<Vec<_>>>()?;
    record.payload = json!({"message_id":message.id,"metadata":{
        "start_game_time_ms":origin_ms,"end_game_time_ms":end_ms,"tokens":tokens,"timing_source":"tts_alignment"
    }});
    state
        .context
        .installation
        .store
        .append_session_event(record)
        .await?;
    Ok(())
}

// Persists one TTS diagnostic event into the evaluation event stream.
/// Records bounded operational metadata for speech synthesis.
async fn persist_tts_diagnostic<A: Game>(
    state: &Arc<AppState<A>>,
    public_session_id: &str,
    event: &str,
    metadata: Value,
) {
    persist_session_event(
        state,
        public_session_id,
        None,
        "tts_diagnostic",
        json!({
            "event": event,
            "metadata": metadata,
        }),
        None,
    )
    .await;
}
