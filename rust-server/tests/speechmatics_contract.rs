use std::sync::{Arc, Mutex};

use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use parlando_server::{
    audio::{AudioFrame, AUDIO_FRAME_BYTES},
    config::SpeechmaticsConfig,
    transcription::{
        SpeechmaticsTranscriptionProvider, TranscriptionEvent, TranscriptionInput,
        TranscriptionProvider, TranscriptionSessionContext,
    },
};
use serde_json::{json, Value};
use tokio::{net::TcpListener, sync::oneshot, time::Duration};
use tokio_tungstenite::{
    accept_hdr_async,
    tungstenite::{
        handshake::server::{Request, Response},
        Message,
    },
};

/// Captures the client request and drives the subset of the provider protocol Parlando uses.
#[allow(clippy::result_large_err)]
async fn run_fake_speechmatics(
    listener: TcpListener,
    authorization: Arc<Mutex<Option<String>>>,
    completion: oneshot::Sender<(Value, Vec<u8>, Value)>,
) {
    let (stream, _) = listener.accept().await.unwrap();
    let observed_authorization = authorization.clone();
    let mut socket = accept_hdr_async(stream, move |request: &Request, response: Response| {
        *observed_authorization.lock().unwrap() = request
            .headers()
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        Ok(response)
    })
    .await
    .unwrap();

    let start = read_json(&mut socket).await;
    socket
        .send(Message::Text(
            json!({"message":"RecognitionStarted"}).to_string(),
        ))
        .await
        .unwrap();
    let pcm = read_binary(&mut socket).await;
    socket
        .send(Message::Text(
            json!({
                "message":"AddPartialTranscript",
                "metadata":{"transcript":"hello"}
            })
            .to_string(),
        ))
        .await
        .unwrap();
    socket
        .send(Message::Text(
            json!({
                "message":"AddTranscript",
                "id":"result-1",
                "metadata":{"transcript":"hello world", "start_time":0.25, "end_time":0.75}
            })
            .to_string(),
        ))
        .await
        .unwrap();
    socket
        .send(Message::Text(
            json!({"message":"EndOfUtterance"}).to_string(),
        ))
        .await
        .unwrap();
    let end = read_json(&mut socket).await;
    socket
        .send(Message::Text(
            json!({"message":"EndOfTranscript"}).to_string(),
        ))
        .await
        .unwrap();
    completion.send((start, pcm, end)).unwrap();
}

/// Reads the next JSON text message from a fake provider socket.
async fn read_json<S>(socket: &mut S) -> Value
where
    S: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    loop {
        let message = socket.next().await.unwrap().unwrap();
        if let Message::Text(text) = message {
            return serde_json::from_str(&text).unwrap();
        }
    }
}

/// Reads the next binary PCM payload from a fake provider socket.
async fn read_binary<S>(socket: &mut S) -> Vec<u8>
where
    S: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    loop {
        let message = socket.next().await.unwrap().unwrap();
        if let Message::Binary(bytes) = message {
            return bytes.to_vec();
        }
    }
}

/// Exercises the production Speechmatics adapter against a local credential-free protocol peer.
#[tokio::test]
async fn speechmatics_adapter_obeys_streaming_protocol_without_external_service() -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let authorization = Arc::new(Mutex::new(None));
    let (completion_tx, completion_rx) = oneshot::channel();
    tokio::spawn(run_fake_speechmatics(
        listener,
        authorization.clone(),
        completion_tx,
    ));

    let provider = SpeechmaticsTranscriptionProvider::new(SpeechmaticsConfig {
        api_key: "test-api-key".to_string(),
        realtime_url: format!("ws://{address}/v2"),
        max_delay: 1.5,
        enable_partials: true,
        end_of_utterance_silence_trigger: 0.8,
    })?;
    let mut session = provider
        .start_session(TranscriptionSessionContext {
            room_id: "room-contract".to_string(),
            participant_session_id: "speaker-contract".to_string(),
            role: "A".to_string(),
            language: "en-US".to_string(),
            model: "enhanced".to_string(),
        })
        .await?;

    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(2), session.events.recv()).await?,
        Some(TranscriptionEvent::Ready)
    ));
    let pcm = vec![7; AUDIO_FRAME_BYTES];
    session
        .input
        .send(TranscriptionInput::Audio(AudioFrame {
            sequence: 0,
            timestamp_ms: 0,
            pcm: pcm.clone(),
        }))
        .await?;
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(2), session.events.recv()).await?,
        Some(TranscriptionEvent::Partial(text)) if text == "hello"
    ));
    let final_utterance = tokio::time::timeout(Duration::from_secs(2), session.events.recv())
        .await?
        .unwrap();
    let TranscriptionEvent::FinalUtterance(final_utterance) = final_utterance else {
        panic!("expected final utterance");
    };
    assert_eq!(final_utterance.text, "hello world");
    assert_eq!(final_utterance.start_time_ms, 250);
    assert_eq!(final_utterance.end_time_ms, 750);
    assert_eq!(final_utterance.result_ids, ["result-1"]);
    session.input.send(TranscriptionInput::Finish).await?;

    let (start, observed_pcm, end) =
        tokio::time::timeout(Duration::from_secs(2), completion_rx).await??;
    assert_eq!(
        authorization.lock().unwrap().as_deref(),
        Some("Bearer test-api-key")
    );
    assert_eq!(start["message"], "StartRecognition");
    assert_eq!(start["audio_format"]["encoding"], "pcm_s16le");
    assert_eq!(start["audio_format"]["sample_rate"], 24_000);
    assert_eq!(start["transcription_config"]["language"], "en");
    assert_eq!(start["transcription_config"]["model"], "enhanced");
    assert_eq!(start["transcription_config"]["enable_partials"], true);
    assert_eq!(
        start["transcription_config"]["conversation_config"]["end_of_utterance_silence_trigger"],
        0.8
    );
    assert_eq!(observed_pcm, pcm);
    assert_eq!(end, json!({"message":"EndOfStream", "last_seq_no":1}));
    Ok(())
}
