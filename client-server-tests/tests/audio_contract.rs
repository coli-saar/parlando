use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use parlando::test_support::{
    ServeOptions, TranscriptionEvent, TranscriptionInput, TranscriptionProvider,
    TranscriptionSessionContext, TranscriptionSessionHandle,
};
use parlando_client_server_contract_tests::{
    contract_config, enable_voice, run_node_driver, spawn_server,
};
use serde_json::json;

/// Deterministic transcription peer that records the marker byte of every received frame.
#[derive(Clone)]
struct RecordingTranscriptionProvider {
    frames: Arc<Mutex<Vec<(String, u8)>>>,
}

#[async_trait]
impl TranscriptionProvider for RecordingTranscriptionProvider {
    /// Starts one role-bound recorder and reports immediate provider readiness.
    async fn start_session(
        &self,
        context: TranscriptionSessionContext,
    ) -> Result<TranscriptionSessionHandle> {
        let (input, mut inputs) = tokio::sync::mpsc::channel(32);
        let (events, event_receiver) = tokio::sync::mpsc::channel(4);
        let frames = self.frames.clone();
        tokio::spawn(async move {
            let _ = events.send(TranscriptionEvent::Ready).await;
            while let Some(message) = inputs.recv().await {
                match message {
                    TranscriptionInput::Audio(frame) => frames.lock().unwrap().push((
                        context.role.clone(),
                        frame.pcm.first().copied().unwrap_or(0),
                    )),
                    TranscriptionInput::Finish => break,
                }
            }
        });
        Ok(TranscriptionSessionHandle {
            input,
            events: event_receiver,
        })
    }
}

/// Waits for transcription to observe exactly the expected role-A capture markers.
async fn wait_for_markers(frames: &Arc<Mutex<Vec<(String, u8)>>>, expected: &[u8]) -> Result<()> {
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let observed = frames
                .lock()
                .unwrap()
                .iter()
                .filter_map(|(role, marker)| (role == "A").then_some(*marker))
                .collect::<Vec<_>>();
            if observed == expected {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .map_err(|_| anyhow!("transcription did not observe markers {expected:?}"))?;
    Ok(())
}

/// Runs both production JavaScript participant clients and the audio sink against the live server.
#[tokio::test]
async fn javascript_mute_blocks_capture_but_not_partner_playback() -> Result<()> {
    let frames = Arc::new(Mutex::new(Vec::new()));
    let mut config = contract_config();
    enable_voice(&mut config);
    config.transcription.enabled = true;
    config.speechmatics.api_key = "contract-test-key".to_string();
    let server = spawn_server(
        config,
        ServeOptions {
            transcription_provider: Some(Arc::new(RecordingTranscriptionProvider {
                frames: frames.clone(),
            })),
            ..ServeOptions::default()
        },
    )
    .await?;

    let result = run_node_driver(&server, "audio-driver.mjs", json!({})).await?;
    assert_eq!(
        result,
        json!({"status": "passed", "relayedMarkers": [1, 3]})
    );
    wait_for_markers(&frames, &[1, 3]).await?;
    Ok(())
}
