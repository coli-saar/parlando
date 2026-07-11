use std::{env, fs, path::PathBuf, sync::Once, time::Duration};

use anyhow::{anyhow, bail, Context, Result};
use futures_util::{SinkExt, StreamExt};
use parlando_server::{
    config::ExperimentConfig,
    livekit::create_livekit_token,
    speechmatics::create_speechmatics_temporary_key,
    tts::{ElevenLabsStreamingTtsProvider, StreamingTtsProvider},
};
use serde_json::{json, Value};
use tokio_tungstenite::{connect_async, tungstenite::Message};

const LIVE_TEST_FLAG: &str = "PARLANDO_RUN_LIVE_TESTS";
const LIVE_CONFIG_ENV: &str = "PARLANDO_LIVE_CONFIG";
const LIVE_AUDIO_ENV: &str = "PARLANDO_LIVE_AUDIO_PCM";
const DEFAULT_LIVE_CONFIG: &str = "config/experiment.livekit.private.yaml";
const DEFAULT_LIVE_AUDIO: &str = "tests/resources/elevenlabs-speechmatics-test.pcm";
static RUSTLS_PROVIDER: Once = Once::new();

fn install_rustls_provider() {
    RUSTLS_PROVIDER.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

fn live_tests_enabled() -> bool {
    env::var(LIVE_TEST_FLAG).ok().as_deref() == Some("1")
}

fn skip_if_live_tests_not_enabled() -> bool {
    if live_tests_enabled() {
        false
    } else {
        eprintln!(
            "skipping live service test; set {LIVE_TEST_FLAG}=1 and run this ignored test explicitly"
        );
        true
    }
}

fn live_config_path() -> PathBuf {
    env::var(LIVE_CONFIG_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(DEFAULT_LIVE_CONFIG))
}

fn live_audio_path() -> PathBuf {
    env::var(LIVE_AUDIO_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(DEFAULT_LIVE_AUDIO))
}

fn load_live_config() -> Result<ExperimentConfig> {
    let path = live_config_path();
    ExperimentConfig::from_yaml(&path)
        .with_context(|| format!("failed to load live config from {}", path.display()))
}

fn require_tts_and_speechmatics(config: &ExperimentConfig) -> Result<()> {
    if !config.speechmatics.enabled || config.speechmatics.api_key.is_empty() {
        bail!("live Speechmatics test requires speechmatics.enabled and speechmatics.api_key");
    }
    if !config.tts.enabled || config.tts.api_key.is_empty() || config.tts.voice_id.is_empty() {
        bail!("live Speechmatics audio test requires enabled ElevenLabs TTS credentials");
    }
    Ok(())
}

#[tokio::test]
#[ignore = "live paid-service test; set PARLANDO_RUN_LIVE_TESTS=1 and run explicitly"]
async fn livekit_credentials_mint_join_token_from_private_config() -> Result<()> {
    if skip_if_live_tests_not_enabled() {
        return Ok(());
    }
    let config = load_live_config()?;
    if !config.livekit.enabled {
        bail!("live LiveKit test requires livekit.enabled = true");
    }

    let token = create_livekit_token(
        &config.livekit,
        "parlando-live-test-room",
        "worker",
        "parlando-live-test-worker",
        60,
    )?;

    assert!(!token.is_empty());
    assert!(config.livekit.url.starts_with("wss://"));
    Ok(())
}

#[tokio::test]
#[ignore = "live paid-service test; set PARLANDO_RUN_LIVE_TESTS=1 and run explicitly"]
async fn speechmatics_temporary_key_mints_with_live_credentials() -> Result<()> {
    if skip_if_live_tests_not_enabled() {
        return Ok(());
    }
    let config = load_live_config()?;
    if !config.speechmatics.enabled || config.speechmatics.api_key.is_empty() {
        bail!("live Speechmatics test requires speechmatics.enabled and speechmatics.api_key");
    }

    let key = create_speechmatics_temporary_key(&config.speechmatics).await?;

    assert!(!key.is_empty());
    Ok(())
}

#[tokio::test]
#[ignore = "live paid-service test; set PARLANDO_RUN_LIVE_TESTS=1 and run explicitly"]
async fn elevenlabs_generates_temp_audio_file() -> Result<()> {
    if skip_if_live_tests_not_enabled() {
        return Ok(());
    }
    install_rustls_provider();
    let config = load_live_config()?;
    if !config.tts.enabled || config.tts.api_key.is_empty() || config.tts.voice_id.is_empty() {
        bail!("live ElevenLabs audio generation requires enabled TTS credentials");
    }

    let audio = generate_elevenlabs_test_audio(&config).await?;
    let file = tempfile::NamedTempFile::new()?;
    fs::write(file.path(), audio)?;
    assert!(file.path().is_file());
    Ok(())
}

#[tokio::test]
#[ignore = "live paid-service test; set PARLANDO_RUN_LIVE_TESTS=1 and run explicitly"]
async fn speechmatics_transcribes_saved_test_resource() -> Result<()> {
    if skip_if_live_tests_not_enabled() {
        return Ok(());
    }
    install_rustls_provider();
    let config = load_live_config()?;
    if !config.speechmatics.enabled || config.speechmatics.api_key.is_empty() {
        bail!("live Speechmatics test requires speechmatics.enabled and speechmatics.api_key");
    }

    let path = live_audio_path();
    let audio = fs::read(&path).with_context(|| {
        format!(
            "failed to read bundled STT test resource from {}; add the resource or set {LIVE_AUDIO_ENV}",
            path.display()
        )
    })?;
    assert_transcribes_expected_words(&config, audio).await
}

#[tokio::test]
#[ignore = "live paid-service test; set PARLANDO_RUN_LIVE_TESTS=1 and run explicitly"]
async fn speechmatics_transcribes_elevenlabs_generated_audio() -> Result<()> {
    if skip_if_live_tests_not_enabled() {
        return Ok(());
    }
    install_rustls_provider();
    let config = load_live_config()?;
    require_tts_and_speechmatics(&config)?;

    let audio = generate_elevenlabs_test_audio(&config).await?;
    assert_transcribes_expected_words(&config, audio).await
}

async fn generate_elevenlabs_test_audio(config: &ExperimentConfig) -> Result<Vec<u8>> {
    let mut tts_config = config.tts.clone();
    tts_config.output_format = "pcm_16000".to_string();
    let provider = ElevenLabsStreamingTtsProvider::new(tts_config)?;
    let chunks = provider
        .synthesize(
            "Parlando live speech recognition test. The launch beacon is ready.",
            "live-speechmatics-test",
        )
        .await?;
    let audio = chunks
        .into_iter()
        .filter(|chunk| !chunk.final_chunk)
        .flat_map(|chunk| chunk.data)
        .collect::<Vec<_>>();
    if audio.is_empty() {
        bail!("ElevenLabs returned no audio bytes");
    }
    Ok(audio)
}

async fn assert_transcribes_expected_words(
    config: &ExperimentConfig,
    audio: Vec<u8>,
) -> Result<()> {
    let transcript = transcribe_pcm_with_speechmatics(&config, audio).await?;
    let normalized = transcript.to_lowercase();
    if !normalized.contains("parlando") && !normalized.contains("beacon") {
        bail!("Speechmatics transcript did not contain expected words: {transcript:?}");
    }
    Ok(())
}

async fn transcribe_pcm_with_speechmatics(
    config: &ExperimentConfig,
    audio: Vec<u8>,
) -> Result<String> {
    let temporary_key = create_speechmatics_temporary_key(&config.speechmatics).await?;
    let url = format!(
        "{}?jwt={}",
        config.speechmatics.realtime_url.trim_end_matches('/'),
        temporary_key
    );
    let (mut socket, _) = connect_async(url).await?;
    let start = json!({
        "message": "StartRecognition",
        "audio_format": {
            "type": "raw",
            "encoding": "pcm_s16le",
            "sample_rate": 16000
        },
        "transcription_config": {
            "language": speechmatics_language(&config.transcription.language),
            "model": config.transcription.model,
            "enable_partials": config.speechmatics.enable_partials,
            "max_delay": config.speechmatics.max_delay,
            "conversation_config": {
                "end_of_utterance_silence_trigger": config.speechmatics.end_of_utterance_silence_trigger
            }
        }
    });
    socket.send(Message::Text(start.to_string())).await?;
    wait_for_message_type(&mut socket, "RecognitionStarted").await?;

    let mut last_seq_no = 0usize;
    for (index, chunk) in audio.chunks(4096).enumerate() {
        last_seq_no = index + 1;
        socket.send(Message::Binary(chunk.to_vec())).await?;
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    socket
        .send(Message::Text(
            json!({"message": "EndOfStream", "last_seq_no": last_seq_no}).to_string(),
        ))
        .await?;

    let mut transcript = String::new();
    loop {
        let payload = read_json_message(&mut socket).await?;
        match payload.get("message").and_then(Value::as_str) {
            Some("AddTranscript") => {
                transcript.push_str(&transcript_text(&payload));
                transcript.push(' ');
            }
            Some("EndOfTranscript") => break,
            Some("Error") => bail!("Speechmatics realtime error: {payload}"),
            _ => {}
        }
    }
    Ok(transcript.trim().to_string())
}

async fn wait_for_message_type(
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    expected: &str,
) -> Result<Value> {
    loop {
        let payload = read_json_message(socket).await?;
        if payload.get("message").and_then(Value::as_str) == Some(expected) {
            return Ok(payload);
        }
        if payload.get("message").and_then(Value::as_str) == Some("Error") {
            bail!("Speechmatics realtime error: {payload}");
        }
    }
}

async fn read_json_message(
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) -> Result<Value> {
    let message = socket
        .next()
        .await
        .ok_or_else(|| anyhow!("Speechmatics WebSocket closed before final transcript"))??;
    if !message.is_text() {
        return Ok(json!({}));
    }
    Ok(serde_json::from_str(message.to_text()?)?)
}

fn transcript_text(payload: &Value) -> String {
    payload
        .get("metadata")
        .and_then(|metadata| metadata.get("transcript"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            payload
                .get("results")
                .and_then(Value::as_array)
                .map(|results| {
                    results
                        .iter()
                        .filter_map(|result| {
                            result
                                .get("alternatives")
                                .and_then(Value::as_array)
                                .and_then(|alternatives| alternatives.first())
                                .and_then(|alternative| alternative.get("content"))
                                .and_then(Value::as_str)
                        })
                        .collect::<Vec<_>>()
                        .join(" ")
                })
        })
        .unwrap_or_default()
}

fn speechmatics_language(language: &str) -> String {
    language.split('-').next().unwrap_or(language).to_string()
}
