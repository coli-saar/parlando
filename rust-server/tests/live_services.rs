use std::{env, fs, path::PathBuf, sync::Once, time::Duration};

use anyhow::{anyhow, bail, Context, Result};
use axum::Router;
use futures_util::{SinkExt, StreamExt};
use livekit::{
    options::TrackPublishOptions,
    prelude::{
        LocalAudioTrack, LocalTrack, RemoteTrack, Room, RoomEvent, RoomOptions, RtcAudioSource,
        TrackSource,
    },
    webrtc::{
        audio_source::native::NativeAudioSource,
        audio_stream::native::NativeAudioStream,
        prelude::{AudioFrame, AudioSourceOptions},
    },
};
use parlando_server::{
    audio_publisher::{pcm_bytes_to_i16_samples, AgentAudioPublisher, LiveKitAgentAudioPublisher},
    build_router,
    config::{DatabaseConfig, DirectConfig, ExperimentConfig},
    game::{GameAdapter, PlayerRole},
    livekit::create_livekit_token,
    speechmatics::create_speechmatics_temporary_key,
    tts::{AudioChunk, ElevenLabsStreamingTtsProvider, StreamingTtsProvider},
    ServeOptions,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use uuid::Uuid;

const LIVE_TEST_FLAG: &str = "PARLANDO_RUN_LIVE_TESTS";
const LIVE_CONFIG_ENV: &str = "PARLANDO_LIVE_CONFIG";
const LIVE_AUDIO_ENV: &str = "PARLANDO_LIVE_AUDIO_PCM";
const DEFAULT_LIVE_CONFIG: &str = "config/experiment.livekit.private.yaml";
const DEFAULT_LIVE_AUDIO: &str = "tests/resources/elevenlabs-speechmatics-test.pcm";
static RUSTLS_PROVIDER: Once = Once::new();

#[derive(Clone, Debug, Deserialize, Serialize)]
struct LiveGameState {
    actions: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum LiveGameAction {
    Ping,
}

#[derive(Clone, Debug, Serialize)]
struct LiveGameObservation {
    role: String,
    actions: usize,
}

#[derive(Clone, Debug, Serialize)]
struct LiveGameEvent {
    kind: String,
}

#[derive(Clone, Debug, Serialize)]
struct LiveGameSummary {
    actions: usize,
}

#[derive(Clone)]
struct LiveGameAdapter;

impl GameAdapter for LiveGameAdapter {
    type State = LiveGameState;
    type Action = LiveGameAction;
    type Observation = LiveGameObservation;
    type Event = LiveGameEvent;
    type Summary = LiveGameSummary;

    /// Creates the initial state for the live-service dummy game.
    fn initial_state(&self) -> Self::State {
        LiveGameState { actions: 0 }
    }

    /// Accepts all dummy actions because the test focuses on room/audio integration.
    fn validate_action(
        &self,
        _state: &Self::State,
        _action: &Self::Action,
        _player: PlayerRole,
    ) -> Result<()> {
        Ok(())
    }

    /// Applies one dummy action by incrementing a counter.
    fn apply_action(&self, state: &Self::State, _action: &Self::Action) -> Result<Self::State> {
        Ok(LiveGameState {
            actions: state.actions + 1,
        })
    }

    /// Returns the player-facing observation used by the server protocol.
    fn observe_state(&self, state: &Self::State, player: PlayerRole) -> Self::Observation {
        LiveGameObservation {
            role: player.as_str().to_string(),
            actions: state.actions,
        }
    }

    /// Provides one optional action affordance for parity with ordinary game rooms.
    fn available_actions(
        &self,
        _state: &Self::State,
        _player: PlayerRole,
    ) -> Option<Vec<Self::Action>> {
        Some(vec![LiveGameAction::Ping])
    }

    /// Emits one event for each dummy action.
    fn events_for_action(
        &self,
        _before: &Self::State,
        _after: &Self::State,
        _action: &Self::Action,
        _player: PlayerRole,
    ) -> Vec<Self::Event> {
        vec![LiveGameEvent {
            kind: "ping".to_string(),
        }]
    }

    /// Keeps the dummy game open for the duration of the live audio test.
    fn is_complete(&self, _state: &Self::State) -> bool {
        false
    }

    /// Summarizes the dummy game state.
    fn completion_summary(&self, state: &Self::State) -> Self::Summary {
        LiveGameSummary {
            actions: state.actions,
        }
    }
}

struct LiveTestServer {
    base_url: String,
    _temp: TempDir,
    _task: tokio::task::JoinHandle<()>,
}

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

// Starts a real Parlando HTTP server with a test-only game adapter and live credentials.
async fn spawn_live_game_server(mut config: ExperimentConfig) -> Result<LiveTestServer> {
    let temp = TempDir::new()?;
    config.direct = DirectConfig {
        enabled: true,
        require_consent: false,
        ..config.direct
    };
    config.database = DatabaseConfig {
        url: format!("sqlite:///{}", temp.path().join("live-game.db").display()),
    };
    let router = build_router(
        LiveGameAdapter,
        config,
        ServeOptions::<LiveGameAdapter>::default(),
    )
    .await?;
    spawn_live_router(router, temp).await
}

// Binds the test router to an ephemeral localhost port.
async fn spawn_live_router(router: Router, temp: TempDir) -> Result<LiveTestServer> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    Ok(LiveTestServer {
        base_url: format!("http://{addr}"),
        _temp: temp,
        _task: task,
    })
}

// Creates a direct participant through the public Parlando HTTP API.
async fn create_live_participant(
    client: &reqwest::Client,
    base_url: &str,
    name: &str,
) -> Result<String> {
    let response = client
        .post(format!("{base_url}/api/participants"))
        .json(&json!({"source": "direct", "display_name": name}))
        .send()
        .await?
        .error_for_status()?
        .json::<Value>()
        .await?;
    Ok(response["participant_session_id"]
        .as_str()
        .context("participant response omitted participant_session_id")?
        .to_string())
}

// Creates a game room through the public Parlando HTTP API.
async fn create_live_room(
    client: &reqwest::Client,
    base_url: &str,
    participant_session_id: &str,
) -> Result<Value> {
    Ok(client
        .post(format!("{base_url}/api/rooms"))
        .json(&json!({"participant_session_id": participant_session_id, "mode": "direct"}))
        .send()
        .await?
        .error_for_status()?
        .json::<Value>()
        .await?)
}

// Joins a second player to a game room through the public Parlando HTTP API.
async fn join_live_room(
    client: &reqwest::Client,
    base_url: &str,
    room_id: &str,
    participant_session_id: &str,
) -> Result<Value> {
    Ok(client
        .post(format!("{base_url}/api/rooms/{room_id}/join"))
        .json(&json!({"participant_session_id": participant_session_id}))
        .send()
        .await?
        .error_for_status()?
        .json::<Value>()
        .await?)
}

// Requests the same LiveKit audio-session plan that the browser client uses.
async fn request_live_audio_session(
    client: &reqwest::Client,
    base_url: &str,
    room_id: &str,
    participant_session_id: &str,
) -> Result<Value> {
    Ok(client
        .post(format!("{base_url}/api/rooms/{room_id}/audio-session"))
        .json(&json!({"participant_session_id": participant_session_id}))
        .send()
        .await?
        .error_for_status()?
        .json::<Value>()
        .await?)
}

// Extracts the LiveKit sink credentials from a Parlando audio-session response.
fn livekit_credentials_from_audio_session(session: &Value) -> Result<(String, String)> {
    if session["enabled"] != true {
        bail!("audio-session response was disabled: {session}");
    }
    let sink = session["sinks"]
        .as_array()
        .and_then(|sinks| {
            sinks
                .iter()
                .find(|sink| sink["provider"] == "livekit" && sink["transport"] == "webrtc-room")
        })
        .context("audio-session response omitted LiveKit WebRTC sink")?;
    let credentials = &sink["credentials"];
    let url = credentials["url"]
        .as_str()
        .context("LiveKit sink omitted url")?
        .to_string();
    let token = credentials["token"]
        .as_str()
        .context("LiveKit sink omitted token")?
        .to_string();
    Ok((url, token))
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

#[tokio::test]
#[ignore = "live paid-service RTC test; set PARLANDO_RUN_LIVE_TESTS=1 and run explicitly"]
async fn livekit_streams_saved_pcm_between_participants_and_speechmatics_transcribes_it(
) -> Result<()> {
    if skip_if_live_tests_not_enabled() {
        return Ok(());
    }
    if cfg!(target_os = "macos")
        && std::env::var("PARLANDO_ALLOW_MACOS_LIVEKIT_RTC")
            .ok()
            .as_deref()
            != Some("1")
    {
        eprintln!(
            "skipping LiveKit RTC audio test on macOS by default; set PARLANDO_ALLOW_MACOS_LIVEKIT_RTC=1 to exercise the native WebRTC path"
        );
        return Ok(());
    }
    install_rustls_provider();
    let config = load_live_config()?;
    if !config.livekit.enabled {
        bail!("live LiveKit RTC test requires livekit.enabled = true");
    }
    if !config.speechmatics.enabled || config.speechmatics.api_key.is_empty() {
        bail!("live LiveKit RTC transcription test requires Speechmatics credentials");
    }
    let audio = fs::read(live_audio_path())?;
    let room_id = format!("parlando-live-rtc-{}", Uuid::new_v4());
    let publisher_token = create_livekit_token(&config.livekit, &room_id, "A", "publisher", 300)?;
    let subscriber_token = create_livekit_token(&config.livekit, &room_id, "B", "subscriber", 300)?;

    let (publisher_room, mut publisher_events) = Room::connect(
        &config.livekit.url,
        &publisher_token,
        RoomOptions::default(),
    )
    .await?;
    let (_subscriber_room, mut subscriber_events) = Room::connect(
        &config.livekit.url,
        &subscriber_token,
        RoomOptions::default(),
    )
    .await?;

    let source_audio_len = audio.len();
    let receive_task =
        tokio::spawn(
            async move { receive_livekit_pcm(&mut subscriber_events, source_audio_len).await },
        );
    publish_livekit_pcm(&publisher_room, &mut publisher_events, audio.clone()).await?;
    let received_audio = tokio::time::timeout(Duration::from_secs(45), receive_task)
        .await
        .context("timed out waiting for LiveKit audio")???;
    assert!(
        received_audio.len() >= audio.len() / 2,
        "received too little LiveKit audio: {} bytes from {} source bytes",
        received_audio.len(),
        audio.len()
    );

    assert_transcribes_expected_words(&config, received_audio).await
}

#[tokio::test]
#[ignore = "live paid-service game-path RTC test; set PARLANDO_RUN_LIVE_TESTS=1 and run explicitly"]
async fn livekit_audio_session_and_agent_voice_work_through_parlando_game_room() -> Result<()> {
    if skip_if_live_tests_not_enabled() {
        return Ok(());
    }
    if cfg!(target_os = "macos")
        && std::env::var("PARLANDO_ALLOW_MACOS_LIVEKIT_RTC")
            .ok()
            .as_deref()
            != Some("1")
    {
        eprintln!(
            "skipping LiveKit game-path RTC test on macOS by default; set PARLANDO_ALLOW_MACOS_LIVEKIT_RTC=1 to exercise the native WebRTC path"
        );
        return Ok(());
    }
    install_rustls_provider();
    let config = load_live_config()?;
    if !config.livekit.enabled {
        bail!("live game-path LiveKit test requires livekit.enabled = true");
    }
    if !config.speechmatics.enabled || config.speechmatics.api_key.is_empty() {
        bail!("live game-path audio verification requires Speechmatics credentials");
    }

    let server = spawn_live_game_server(config.clone()).await?;
    let client = reqwest::Client::new();
    let participant_a = create_live_participant(&client, &server.base_url, "Live A").await?;
    let participant_b = create_live_participant(&client, &server.base_url, "Live B").await?;
    let room = create_live_room(&client, &server.base_url, &participant_a).await?;
    let room_id = room["room_id"]
        .as_str()
        .context("room response omitted room_id")?
        .to_string();
    let joined = join_live_room(&client, &server.base_url, &room_id, &participant_b).await?;
    assert_eq!(joined["role"], "B");

    let audio_session =
        request_live_audio_session(&client, &server.base_url, &room_id, &participant_b).await?;
    let (livekit_url, participant_token) = livekit_credentials_from_audio_session(&audio_session)?;
    let (_subscriber_room, mut subscriber_events) =
        Room::connect(&livekit_url, &participant_token, RoomOptions::default()).await?;

    let source_audio = fs::read(live_audio_path())?;
    let mut chunks = pcm_chunks(source_audio.clone(), 16000, 1, 10)?;
    chunks.push(AudioChunk {
        data: vec![],
        sample_rate: 16000,
        channels: 1,
        final_chunk: true,
    });
    let source_audio_len = source_audio.len();
    let receive_task =
        tokio::spawn(
            async move { receive_livekit_pcm(&mut subscriber_events, source_audio_len).await },
        );

    let publisher = LiveKitAgentAudioPublisher::new(config.livekit.clone());
    let summary = publisher
        .publish(&room_id, "live-game-agent-message", &chunks)
        .await?;
    assert!(summary.bytes_published > 0);
    assert_eq!(summary.sample_rate, 16000);
    assert_eq!(summary.channels, 1);

    let received_audio = tokio::time::timeout(Duration::from_secs(45), receive_task)
        .await
        .context("timed out waiting for Parlando game-room LiveKit agent audio")???;
    assert!(
        received_audio.len() >= source_audio.len() / 2,
        "received too little game-room LiveKit audio: {} bytes from {} source bytes",
        received_audio.len(),
        source_audio_len
    );
    assert_transcribes_expected_words(&config, received_audio).await
}

// Generates a short PCM clip with ElevenLabs for paid live-service validation.
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

// Publishes a finite PCM clip to LiveKit after the remote subscriber has attached.
async fn publish_livekit_pcm(
    room: &Room,
    events: &mut tokio::sync::mpsc::UnboundedReceiver<RoomEvent>,
    audio: Vec<u8>,
) -> Result<()> {
    let chunks = pcm_chunks(audio, 16000, 1, 10)?;
    let source = NativeAudioSource::new(AudioSourceOptions::default(), 16000, 1, 1000);
    let track = LocalAudioTrack::create_audio_track(
        "parlando-test-pcm",
        RtcAudioSource::Native(source.clone()),
    );
    room.local_participant()
        .publish_track(
            LocalTrack::Audio(track.clone()),
            TrackPublishOptions {
                source: TrackSource::Microphone,
                ..TrackPublishOptions::default()
            },
        )
        .await?;
    wait_for_local_track_subscribed(events).await?;
    for chunk in &chunks {
        let samples = pcm_bytes_to_i16_samples(&chunk.data)?;
        let frame = AudioFrame {
            data: samples.as_slice().into(),
            sample_rate: chunk.sample_rate,
            num_channels: chunk.channels as u32,
            samples_per_channel: samples.len() as u32 / chunk.channels as u32,
        };
        source.capture_frame(&frame).await?;
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    room.local_participant()
        .unpublish_track(&track.sid())
        .await?;
    Ok(())
}

// Waits until LiveKit confirms that a remote participant subscribed to the local track.
async fn wait_for_local_track_subscribed(
    events: &mut tokio::sync::mpsc::UnboundedReceiver<RoomEvent>,
) -> Result<()> {
    loop {
        let event = events.recv().await.ok_or_else(|| {
            anyhow!("LiveKit room event stream closed before subscriber attached")
        })?;
        match event {
            RoomEvent::LocalTrackSubscribed { .. } => return Ok(()),
            RoomEvent::TrackSubscriptionFailed { error, .. } => {
                bail!("LiveKit track subscription failed before audio publish: {error}")
            }
            _ => {}
        }
    }
}

// Receives decoded PCM from the first subscribed LiveKit audio track.
async fn receive_livekit_pcm(
    events: &mut tokio::sync::mpsc::UnboundedReceiver<RoomEvent>,
    target_bytes: usize,
) -> Result<Vec<u8>> {
    let track = loop {
        let event = events
            .recv()
            .await
            .ok_or_else(|| anyhow!("LiveKit room event stream closed before audio subscribed"))?;
        if let RoomEvent::TrackSubscribed { track, .. } = event {
            break track;
        }
    };
    let RemoteTrack::Audio(track) = RemoteTrack::from(track) else {
        bail!("expected LiveKit audio track");
    };
    let mut stream = NativeAudioStream::new(track.rtc_track(), 16000, 1);
    let mut received = Vec::with_capacity(target_bytes);
    while received.len() < target_bytes {
        let frame = match tokio::time::timeout(Duration::from_secs(2), stream.next()).await {
            Ok(Some(frame)) => frame,
            Ok(None) => break,
            Err(_) if received.is_empty() => bail!("LiveKit audio stream produced no PCM frames"),
            Err(_) => break,
        };
        for sample in frame.data.iter() {
            received.extend_from_slice(&sample.to_le_bytes());
        }
    }
    Ok(received)
}

// Splits signed 16-bit little-endian PCM into fixed-duration chunks.
fn pcm_chunks(
    audio: Vec<u8>,
    sample_rate: u32,
    channels: u16,
    frame_ms: u32,
) -> Result<Vec<AudioChunk>> {
    let bytes_per_frame = (sample_rate / (1000 / frame_ms)) as usize * channels as usize * 2;
    if bytes_per_frame == 0 {
        bail!("invalid PCM frame size");
    }
    Ok(audio
        .chunks(bytes_per_frame)
        .map(|chunk| AudioChunk {
            data: chunk.to_vec(),
            sample_rate,
            channels,
            final_chunk: false,
        })
        .collect())
}

// Checks that Speechmatics recognized at least one stable word from the test clip.
async fn assert_transcribes_expected_words(
    config: &ExperimentConfig,
    audio: Vec<u8>,
) -> Result<()> {
    let transcript = transcribe_pcm_with_speechmatics(&config, audio).await?;
    let normalized = transcript.to_lowercase();
    if !normalized.contains("parlando")
        && !normalized.contains("pallando")
        && !normalized.contains("launch")
        && !normalized.contains("beacon")
    {
        bail!("Speechmatics transcript did not contain expected words: {transcript:?}");
    }
    Ok(())
}

// Streams raw PCM to Speechmatics realtime STT and returns the final transcript text.
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

// Reads Speechmatics messages until the expected message type appears.
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

// Reads one JSON text message from the Speechmatics WebSocket.
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

// Extracts transcript text from either modern metadata or token-level result fields.
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

// Maps the server config language into the value expected by Speechmatics realtime STT.
fn speechmatics_language(language: &str) -> String {
    language.split('-').next().unwrap_or(language).to_string()
}
