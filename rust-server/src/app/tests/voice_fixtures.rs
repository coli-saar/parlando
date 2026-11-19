use super::*;

pub(super) struct MockTtsProvider {
    pub(super) calls: AtomicUsize,
    pub(super) fail_first: bool,
}

pub(super) struct MockAudioPublisher {
    pub(super) calls: AtomicUsize,
}

#[async_trait]
impl StreamingTtsProvider for MockTtsProvider {
    async fn synthesize(
        &self,
        _text: &str,
        _message_id: &str,
    ) -> Result<crate::tts::SynthesizedSpeech> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail_first && call == 0 {
            return Err(anyhow!("mock tts failure"));
        }
        Ok(crate::tts::SynthesizedSpeech {
            chunks: vec![crate::tts::AudioChunk {
                data: vec![0; 48000],
                sample_rate: 24000,
                channels: 1,
            }],
            tokens: vec![crate::transcription::TranscriptToken {
                kind: "word".into(),
                text: _text.into(),
                start_time_ms: 100,
                end_time_ms: 800,
                confidence: None,
            }],
        })
    }
}

#[async_trait]
impl crate::audio_publisher::AgentAudioPublisher for MockAudioPublisher {
    async fn publish(
        &self,
        _room_id: &str,
        _message_id: &str,
        chunks: &[crate::tts::AudioChunk],
    ) -> Result<crate::audio_publisher::AudioPublishSummary> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(crate::audio_publisher::AudioPublishSummary {
            chunks_published: chunks.iter().filter(|chunk| !chunk.data.is_empty()).count(),
            bytes_published: chunks.iter().map(|chunk| chunk.data.len()).sum(),
            sample_rate: 24000,
            channels: 1,
        })
    }
}
