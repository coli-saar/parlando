//! Exercises both provider clocks through real sockets, publication, and read projections.
use super::*;
use base64::Engine as _;
use chrono::Utc;
use tokio::sync::oneshot;

/// Sends two cumulative alignment chunks with a word crossing their PCM boundary.
async fn timed_tts_peer(listener: TcpListener, ready: oneshot::Sender<i64>) {
    let (stream, _) = listener.accept().await.unwrap();
    let mut socket = tokio_tungstenite::accept_hdr_async(
        stream,
        |request: &tokio_tungstenite::tungstenite::handshake::server::Request,
         response: tokio_tungstenite::tungstenite::handshake::server::Response| {
            assert!(request
                .uri()
                .query()
                .unwrap()
                .contains("sync_alignment=true"));
            Ok(response)
        },
    )
    .await
    .unwrap();
    for _ in 0..3 {
        socket.next().await.unwrap().unwrap();
    }
    // Text is already committed; synthesis latency must not become the audio origin.
    tokio::time::sleep(Duration::from_millis(150)).await;
    ready.send(Utc::now().timestamp_millis()).unwrap();
    for (text, start, bytes, fingerprint) in
        [("Hel", 0, 14400, 7), ("lo seventeen.", 300, 62400, 9)]
    {
        let payload = json!({
            "audio":base64::engine::general_purpose::STANDARD.encode(vec![fingerprint;bytes]),
            "normalizedAlignment":{"chars":text.chars().map(|c| c.to_string()).collect::<Vec<_>>(),
                "charStartTimesMs":(0..text.len()).map(|i|start+i*100).collect::<Vec<_>>(),
                "charDurationsMs":vec![100;text.len()]}
        });
        socket
            .send(TungsteniteMessage::Text(payload.to_string()))
            .await
            .unwrap();
    }
    socket
        .send(TungsteniteMessage::Text(
            json!({"isFinal":true}).to_string(),
        ))
        .await
        .unwrap();
}

/// Recognizes two turns on the sample clock after receiving a capture-clock pause.
async fn timed_asr_peer(listener: TcpListener, first_audio: oneshot::Sender<i64>) {
    let (stream, _) = listener.accept().await.unwrap();
    let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
    let start = socket.next().await.unwrap().unwrap();
    let start: Value = serde_json::from_str(start.to_text().unwrap()).unwrap();
    assert_eq!(start["message"], "StartRecognition");
    socket
        .send(TungsteniteMessage::Text(
            json!({"message":"RecognitionStarted"}).to_string(),
        ))
        .await
        .unwrap();
    let mut first_audio = Some(first_audio);
    for index in 0..4 {
        let message = socket.next().await.unwrap().unwrap();
        assert_eq!(
            message.into_data(),
            vec![index + 1; crate::audio::AUDIO_FRAME_BYTES]
        );
        if let Some(sender) = first_audio.take() {
            sender.send(Utc::now().timestamp_millis()).unwrap();
        }
    }
    // Arrival time is deliberately unrelated to either token's audio position.
    tokio::time::sleep(Duration::from_millis(150)).await;
    for (index, text, start, end) in [(1, "before", 0.0, 0.04), (2, "after", 0.04, 0.08)] {
        let payload = json!({"message":"AddTranscript","id":format!("turn-{index}"),
            "metadata":{"transcript":format!("{text}."),"start_time":start,"end_time":end},
            "results":[{"type":"word","start_time":start,"end_time":end,
                "alternatives":[{"content":text,"confidence":0.9}]},
                {"type":"punctuation","start_time":end,"end_time":end,"alternatives":[{"content":"."}]}]});
        socket
            .send(TungsteniteMessage::Text(payload.to_string()))
            .await
            .unwrap();
        socket
            .send(TungsteniteMessage::Text(
                json!({"message":"EndOfUtterance"}).to_string(),
            ))
            .await
            .unwrap();
    }
    while let Some(Ok(message)) = socket.next().await {
        if message.is_text() && message.to_text().unwrap().contains("EndOfStream") {
            socket
                .send(TungsteniteMessage::Text(
                    json!({"message":"EndOfTranscript"}).to_string(),
                ))
                .await
                .unwrap();
            break;
        }
    }
}

/// Compares token boundaries and utterance duration against an independent audio oracle.
fn assert_speech_offsets(value: &Value, origin: i64, duration: i64, expected: &[(&str, i64, i64)]) {
    assert_eq!(value["utterance_timing"]["start_ms"], origin);
    assert_eq!(value["utterance_timing"]["end_ms"], origin + duration);
    let tokens = value["tokens"].as_array().unwrap();
    assert_eq!(tokens.len(), expected.len());
    for (token, (text, start, end)) in tokens.iter().zip(expected) {
        assert_eq!(token["text"], *text);
        assert_eq!(token["start_ms"], origin + start);
        assert_eq!(token["end_ms"], origin + end);
    }
}

/// Validates generated B and recognized A against PCM and capture clocks in one live room.
#[tokio::test]
async fn generated_and_recognized_speech_keep_audio_clock_timings_end_to_end() {
    let tts_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let asr_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut config = human_vs_agent_config();
    config.voice.enabled = true;
    config.transcription.enabled = true;
    config.speechmatics.api_key = "local-asr-key".into();
    config.speechmatics.realtime_url = format!("ws://{}", asr_listener.local_addr().unwrap());
    config.tts.enabled = true;
    config.tts.api_key = "local-tts-key".into();
    config.tts.voice_id = "local-voice".into();
    config.tts.output_format = "pcm_24000".into();
    config.tts.base_url = format!("ws://{}", tts_listener.local_addr().unwrap());
    let (tts_ready_tx, tts_ready_rx) = oneshot::channel();
    let (asr_first_tx, asr_first_rx) = oneshot::channel();
    let tts_task = tokio::spawn(timed_tts_peer(tts_listener, tts_ready_tx));
    let asr_task = tokio::spawn(timed_asr_peer(asr_listener, asr_first_tx));
    let factory = Arc::new(ScriptedAgentFactory::new(vec![vec![scripted_response(
        Some("Hello £17."),
        None,
    )]]));
    let router = build_router(
        TinyAdapter,
        config,
        ServeOptions {
            agent_factory: Some(factory),
            ..ServeOptions::default()
        },
    )
    .await
    .unwrap();
    let (human, room) = create_human_vs_agent_room(router.clone(), "Timing human").await;
    let (base_url, server) = spawn_test_server(router.clone()).await;
    let plan = request_audio_plan(router.clone(), &room, &human).await;
    let (mut audio, _) = connect_async(format!(
        "{}/ws/audio/{room}?token={}",
        base_url.replacen("http://", "ws://", 1),
        plan["token"].as_str().unwrap()
    ))
    .await
    .unwrap();
    wait_for_audio_control(&mut audio).await;
    let (mut game, _) = connect_async(game_socket_url(&base_url, &room, &human).await)
        .await
        .unwrap();
    let _ = read_participant_state(&mut game, "active").await;
    // Capture starts at a nonzero client offset and omits 960 ms between frames.
    let sent_at = Utc::now().timestamp_millis();
    for (index, capture) in [9000, 9020, 10000, 10020].into_iter().enumerate() {
        audio
            .send(TungsteniteMessage::Binary(
                AudioFrame {
                    sequence: index as u32,
                    timestamp_ms: capture,
                    pcm: vec![index as u8 + 1; crate::audio::AUDIO_FRAME_BYTES],
                }
                .encode(),
            ))
            .await
            .unwrap();
    }
    let first_asr_received_at = asr_first_rx.await.unwrap();
    let tts_ready_at = tts_ready_rx.await.unwrap();
    let mut first_tts_received_at = None;
    for index in 0..80 {
        let bytes = read_audio_binary(&mut audio).await;
        first_tts_received_at.get_or_insert_with(|| Utc::now().timestamp_millis());
        let frame = AudioFrame::decode(&bytes).unwrap();
        assert_eq!(frame.sequence, index);
        assert_eq!(frame.timestamp_ms, u64::from(index) * 20);
        assert_eq!(
            frame.pcm,
            vec![if index < 15 { 7 } else { 9 }; crate::audio::AUDIO_FRAME_BYTES]
        );
    }
    let export = wait_for_tts_diagnostic(router, "tts_message_completed").await;
    let started_at =
        chrono::DateTime::parse_from_rfc3339(export["sessions"][0]["started_at"].as_str().unwrap())
            .unwrap()
            .timestamp_millis();
    let events: Vec<crate::storage::StoredSessionEvent> =
        serde_json::from_value(export["session_events"].clone()).unwrap();
    let messages = events
        .iter()
        .filter(|event| event.event_type == "conversation_message")
        .collect::<Vec<_>>();
    assert_eq!(
        messages.len(),
        3,
        "one agent turn and both recognized turns must survive"
    );
    let first = messages
        .iter()
        .find(|event| event.payload["text"] == "before.")
        .unwrap();
    let human_origin = first.payload["metadata"]["start_game_time_ms"]
        .as_i64()
        .unwrap();
    assert!(
        (sent_at - started_at..=first_asr_received_at - started_at).contains(&human_origin),
        "ASR origin must be first accepted audio, not final response time"
    );
    assert!(
        first.game_time_ms >= human_origin + 100,
        "fixture must exercise delayed ASR delivery"
    );
    let timing = events
        .iter()
        .find(|event| event.event_type == "conversation_speech_timing")
        .unwrap();
    let agent_origin = timing.payload["metadata"]["start_game_time_ms"]
        .as_i64()
        .unwrap();
    assert!(
        (tts_ready_at - started_at..=first_tts_received_at.unwrap() - started_at)
            .contains(&agent_origin),
        "TTS origin must bracket actual publication, not text commit"
    );
    let agent = messages
        .iter()
        .find(|event| event.payload["origin"] == "agent")
        .unwrap();
    assert!(
        agent_origin >= agent.game_time_ms + 100,
        "fixture must exercise delayed synthesis"
    );
    let bundles = admin_event_bundles(&important_admin_events(events));
    let corpus = corpus_experiment_export(export, "2").unwrap();
    let corpus_messages = corpus["experiment"]["sessions"][0]["events"]
        .as_array()
        .unwrap();
    for values in [&bundles, corpus_messages] {
        let agent = values
            .iter()
            .find(|value| value["origin"] == "agent")
            .unwrap();
        assert_eq!(agent["role"], "B");
        assert_eq!(agent["text"], "Hello £17.");
        assert_speech_offsets(
            agent,
            agent_origin,
            1600,
            &[
                ("Hello", 0, 500),
                ("seventeen", 600, 1500),
                (".", 1500, 1500),
            ],
        );
        assert!(agent["tokens"]
            .as_array()
            .unwrap()
            .iter()
            .all(|token| token.get("confidence").is_none()));
        let before = values
            .iter()
            .find(|value| value["text"] == "before.")
            .unwrap();
        assert_eq!(before["role"], "A");
        assert_speech_offsets(
            before,
            human_origin,
            40,
            &[("before", 0, 40), (".", 40, 40)],
        );
        let after = values
            .iter()
            .find(|value| value["text"] == "after.")
            .unwrap();
        assert_eq!(after["role"], "A");
        assert_speech_offsets(
            after,
            human_origin + 1000,
            40,
            &[("after", 0, 40), (".", 40, 40)],
        );
        assert_eq!(after["tokens"][0]["confidence"], 0.9);
    }
    audio.close(None).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), asr_task)
        .await
        .unwrap()
        .unwrap();
    tts_task.await.unwrap();
    server.abort();
}
