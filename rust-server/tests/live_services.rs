use anyhow::{bail, Result};
use parlando_server::{
    config::ExperimentConfig,
    transcription::{
        SpeechmaticsTranscriptionProvider, TranscriptionInput, TranscriptionProvider,
        TranscriptionSessionContext,
    },
};

/// Loads the opt-in private configuration used for a real Speechmatics smoke test.
fn live_config() -> Result<ExperimentConfig> {
    let path = std::env::var("PARLANDO_LIVE_CONFIG")
        .unwrap_or_else(|_| "config/experiment.voice.private.yaml".to_string());
    ExperimentConfig::from_yaml(path)
}

#[tokio::test]
#[ignore = "requires PARLANDO_LIVE_CONFIG with Speechmatics credentials"]
async fn speechmatics_server_provider_connects_without_browser_credentials() -> Result<()> {
    let config = live_config()?;
    if config.speechmatics.api_key.is_empty() {
        bail!("Speechmatics API key is required");
    }
    let provider = SpeechmaticsTranscriptionProvider::new(config.speechmatics)?;
    let session = provider
        .start_session(TranscriptionSessionContext {
            room_id: "live-test".to_string(),
            participant_session_id: "speaker".to_string(),
            role: "A".to_string(),
            language: config.transcription.language,
            model: config.transcription.model,
        })
        .await?;
    session.input.send(TranscriptionInput::Finish).await?;
    Ok(())
}
