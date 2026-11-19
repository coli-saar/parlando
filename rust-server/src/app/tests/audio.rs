use super::*;

#[tokio::test]
async fn audio_session_is_disabled_when_voice_is_disabled() {
    let router = build_router(TinyAdapter, step_five_config(), ServeOptions::default())
        .await
        .unwrap();
    let (a, _b, public_session_id) = create_joined_room(router.clone()).await;

    let (audio_status, audio) = participant_json_request(
        router,
        &a,
        http::Method::POST,
        &format!("/api/sessions/{public_session_id}/audio-session"),
        json!({}),
    )
    .await;
    assert_eq!(audio_status, StatusCode::OK);
    assert_eq!(audio["enabled"], false);
    assert!(audio["token"].is_null());
}

#[tokio::test]
async fn enabled_audio_session_returns_parlando_websocket_contract() {
    let router = build_router(TinyAdapter, voice_enabled_config(), ServeOptions::default())
        .await
        .unwrap();
    let (a, _b, public_session_id) = create_joined_room(router.clone()).await;

    let (audio_status, audio) = participant_json_request(
        router,
        &a,
        http::Method::POST,
        &format!("/api/sessions/{public_session_id}/audio-session"),
        json!({}),
    )
    .await;
    assert_eq!(audio_status, StatusCode::OK);
    assert_eq!(audio["enabled"], true);
    assert_eq!(audio["protocol_version"], 1);
    assert_eq!(audio["sample_rate_hz"], 24000);
    assert_eq!(
        audio["websocket_url"],
        format!("/ws/audio/{public_session_id}"),
        "the browser must resolve the path against its actual origin"
    );
    assert!(audio["token"]
        .as_str()
        .is_some_and(|value| !value.is_empty()));
}

#[tokio::test]
/// Forming audio is excluded, but the first frame after public activation is transcribed.
async fn audio_readiness_first_active_frame_is_transcribed() {
    let (finished, mut recordings) = mpsc::unbounded_channel();
    let mut config = voice_enabled_config();
    config.transcription.enabled = true;
    config.speechmatics.api_key = "readiness-test-key".into();
    let router = build_router(
        TinyAdapter,
        config,
        ServeOptions {
            transcription_provider: Some(Arc::new(ReadinessTranscriptionProvider { finished })),
            ..ServeOptions::default()
        },
    )
    .await
    .unwrap();
    let (a, b, room) = create_joined_room(router.clone()).await;
    let (base_url, server) = spawn_test_server(router.clone()).await;
    let host = base_url.trim_start_matches("http://");
    let plan_b = request_audio_plan(router.clone(), &room, &b).await;
    let (mut audio_b, _) = connect_async(format!(
        "ws://{host}/ws/audio/{room}?token={}",
        plan_b["token"].as_str().unwrap()
    ))
    .await
    .unwrap();
    let preparing_b = read_ws_type(&mut audio_b, "transcriptionStatus").await;
    assert_eq!(
        preparing_b["ready"], false,
        "forming audio advertised transcription readiness"
    );
    let plan_a = request_audio_plan(router.clone(), &room, &a).await;
    let (mut audio_a, _) = connect_async(format!(
        "ws://{host}/ws/audio/{room}?token={}",
        plan_a["token"].as_str().unwrap()
    ))
    .await
    .unwrap();
    let preparing_a = read_ws_type(&mut audio_a, "transcriptionStatus").await;
    assert_eq!(
        preparing_a["ready"], false,
        "forming audio advertised transcription readiness"
    );
    // No game sockets have connected: activation cannot complete regardless of scheduling.
    let (_, forming) = participant_json_request(
        router.clone(),
        &a,
        http::Method::GET,
        "/api/participant-state",
        json!({}),
    )
    .await;
    assert_eq!(forming["participant_state"]["state"], "waiting");
    let pregame = AudioFrame {
        sequence: 0,
        timestamp_ms: 0,
        pcm: vec![1; crate::audio::AUDIO_FRAME_BYTES],
    }
    .encode();
    audio_a
        .send(TungsteniteMessage::Binary(pregame.clone()))
        .await
        .unwrap();
    assert_eq!(read_audio_binary(&mut audio_b).await, pregame);
    audio_a.close(None).await.unwrap();
    assert_eq!(
        readiness_finished_audio(&mut recordings).await,
        ("A".into(), vec![])
    );

    let plan_a = request_audio_plan(router.clone(), &room, &a).await;
    let (mut audio_a, _) = connect_async(format!(
        "ws://{host}/ws/audio/{room}?token={}",
        plan_a["token"].as_str().unwrap()
    ))
    .await
    .unwrap();
    wait_for_audio_control(&mut audio_a).await;
    let (mut game_a, _) = connect_async(game_socket_url(&router, &base_url, &room, &a).await)
        .await
        .unwrap();
    let (mut game_b, _) = connect_async(game_socket_url(&router, &base_url, &room, &b).await)
        .await
        .unwrap();
    let _ = read_participant_state(&mut game_a, "active").await;
    let _ = read_participant_state(&mut game_b, "active").await;
    let listening = read_ws_type(&mut audio_a, "transcriptionStatus").await;
    assert_eq!(
        listening["ready"], true,
        "activation did not publish transcription readiness"
    );
    let first = AudioFrame {
        sequence: 0,
        timestamp_ms: 0,
        pcm: vec![3; crate::audio::AUDIO_FRAME_BYTES],
    }
    .encode();
    audio_a
        .send(TungsteniteMessage::Binary(first.clone()))
        .await
        .unwrap();
    assert_eq!(read_audio_binary(&mut audio_b).await, first);
    // Finish is ordered after the frame on this socket and the transcription input queue.
    audio_a.close(None).await.unwrap();
    let recorded = readiness_finished_audio(&mut recordings).await;
    server.abort();
    assert_eq!(recorded, ("A".into(), vec![3]));
}

#[tokio::test]
async fn audio_websocket_relays_pcm_and_commits_one_final_utterance() {
    let mut config = voice_enabled_config();
    config.transcription.enabled = true;
    config.speechmatics.api_key = "server-only-test-key".to_string();
    let router = build_router(
        TinyAdapter,
        config,
        ServeOptions {
            transcription_provider: Some(Arc::new(DuplicateFinalTranscriptionProvider)),
            ..ServeOptions::default()
        },
    )
    .await
    .unwrap();
    let (a, b, public_session_id) = create_joined_room(router.clone()).await;
    let (_, plan_a) = participant_json_request(
        router.clone(),
        &a,
        http::Method::POST,
        &format!("/api/sessions/{public_session_id}/audio-session"),
        json!({}),
    )
    .await;
    let (_, plan_b) = participant_json_request(
        router.clone(),
        &b,
        http::Method::POST,
        &format!("/api/sessions/{public_session_id}/audio-session"),
        json!({}),
    )
    .await;
    let (base_url, server) = spawn_test_server(router.clone()).await;
    let host = base_url.trim_start_matches("http://");
    let (mut socket_b, _) = connect_async(format!(
        "ws://{host}/ws/audio/{public_session_id}?token={}",
        plan_b["token"].as_str().unwrap()
    ))
    .await
    .unwrap();
    let (mut socket_a, _) = connect_async(format!(
        "ws://{host}/ws/audio/{public_session_id}?token={}",
        plan_a["token"].as_str().unwrap()
    ))
    .await
    .unwrap();
    wait_for_audio_control(&mut socket_a).await;
    wait_for_audio_control(&mut socket_b).await;
    let waiting_frame = AudioFrame {
        sequence: 0,
        timestamp_ms: 0,
        pcm: vec![0; crate::audio::AUDIO_FRAME_BYTES],
    }
    .encode();
    socket_a
        .send(TungsteniteMessage::Binary(waiting_frame.clone()))
        .await
        .unwrap();
    assert_eq!(read_audio_binary(&mut socket_b).await, waiting_frame);
    tokio::time::sleep(Duration::from_millis(50)).await;
    let (_, waiting_export) = admin_json_request(
        router.clone(),
        http::Method::GET,
        "/api/admin/export?variant=full",
        Value::Null,
    )
    .await;
    assert!(!waiting_export["session_events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|event| event["event_type"] == "conversation_message"));
    let (mut game_a, _) =
        connect_async(game_socket_url(&router, &base_url, &public_session_id, &a).await)
            .await
            .unwrap();
    let (mut game_b, _) =
        connect_async(game_socket_url(&router, &base_url, &public_session_id, &b).await)
            .await
            .unwrap();
    let _ = read_participant_state(&mut game_a, "active").await;
    let _ = read_participant_state(&mut game_b, "active").await;
    let frame = AudioFrame {
        sequence: 1,
        timestamp_ms: 20,
        pcm: vec![0; crate::audio::AUDIO_FRAME_BYTES],
    }
    .encode();
    socket_a
        .send(TungsteniteMessage::Binary(frame.clone()))
        .await
        .unwrap();
    loop {
        let message = tokio::time::timeout(Duration::from_secs(2), socket_b.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        if matches!(message, TungsteniteMessage::Binary(bytes) if bytes == frame) {
            break;
        }
    }
    let export = wait_for_export_event(router, "conversation_message").await;
    assert_eq!(
        export["session_events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|event| {
                event["event_type"] == "conversation_message"
                    && event["payload"]["origin"] == "voice_transcript"
            })
            .count(),
        1
    );
    assert!(!export["session_events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|event| event["event_type"] == "transcript_segment"));
    let message = export["session_events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|event| event["event_type"] == "conversation_message")
        .unwrap();
    let metadata = &message["payload"]["metadata"];
    assert_eq!(metadata["tokens"][0]["text"], "relay");
    assert_eq!(
        metadata["tokens"][0]["start_ms"],
        metadata["start_game_time_ms"]
    );
    assert_eq!(
        metadata["tokens"][0]["end_ms"],
        metadata["end_game_time_ms"]
    );
    server.abort();
}

/// Normal completion records its result immediately while retaining live human audio relay.
#[tokio::test]
async fn completed_human_session_keeps_audio_relay_during_farewell_period() {
    let mut config = voice_enabled_config();
    config.voice.post_completion_seconds = 2;
    config.transcription.enabled = true;
    config.speechmatics.api_key = "server-only-test-key".to_string();
    let router = build_router(
        TinyAdapter,
        config,
        ServeOptions {
            transcription_provider: Some(Arc::new(DuplicateFinalTranscriptionProvider)),
            ..ServeOptions::default()
        },
    )
    .await
    .unwrap();
    let (a, b, public_session_id) = create_joined_room(router.clone()).await;
    let (_, plan_a) = participant_json_request(
        router.clone(),
        &a,
        http::Method::POST,
        &format!("/api/sessions/{public_session_id}/audio-session"),
        json!({}),
    )
    .await;
    let (_, plan_b) = participant_json_request(
        router.clone(),
        &b,
        http::Method::POST,
        &format!("/api/sessions/{public_session_id}/audio-session"),
        json!({}),
    )
    .await;
    let (base_url, server) = spawn_test_server(router.clone()).await;
    let host = base_url.trim_start_matches("http://");
    let (mut audio_a, _) = connect_async(format!(
        "ws://{host}/ws/audio/{public_session_id}?token={}",
        plan_a["token"].as_str().unwrap()
    ))
    .await
    .unwrap();
    let (mut audio_b, _) = connect_async(format!(
        "ws://{host}/ws/audio/{public_session_id}?token={}",
        plan_b["token"].as_str().unwrap()
    ))
    .await
    .unwrap();
    wait_for_audio_control(&mut audio_a).await;
    wait_for_audio_control(&mut audio_b).await;
    let (mut game_a, _) =
        connect_async(game_socket_url(&router, &base_url, &public_session_id, &a).await)
            .await
            .unwrap();
    let (mut game_b, _) =
        connect_async(game_socket_url(&router, &base_url, &public_session_id, &b).await)
            .await
            .unwrap();
    let _ = read_participant_state(&mut game_a, "active").await;
    let _ = read_participant_state(&mut game_b, "active").await;

    send_ws_json(
        &mut game_a,
        json!({"type": "action", "action": {"finish": true}}),
    )
    .await;
    let completed_a = read_participant_state(&mut game_a, "ended").await;
    let completed_b = read_participant_state(&mut game_b, "ended").await;
    assert_eq!(completed_a["result"]["outcome"], "completed");
    assert!(completed_a["voice_ends_at"].as_str().is_some());
    assert_eq!(completed_b["voice_ends_at"], completed_a["voice_ends_at"]);

    let farewell_frame = AudioFrame {
        sequence: 0,
        timestamp_ms: 0,
        pcm: vec![5; crate::audio::AUDIO_FRAME_BYTES],
    }
    .encode();
    audio_a
        .send(TungsteniteMessage::Binary(farewell_frame.clone()))
        .await
        .unwrap();
    assert_eq!(read_audio_binary(&mut audio_b).await, farewell_frame);
    tokio::time::sleep(Duration::from_millis(50)).await;
    let (_, export) = admin_json_request(
        router.clone(),
        http::Method::GET,
        "/api/admin/export?variant=full",
        Value::Null,
    )
    .await;
    assert!(!export["session_events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|event| event["event_type"] == "conversation_message"));
    // Farewell closes audio even while both game transports still own their room.
    tokio::time::timeout(Duration::from_secs(4), async {
        while let Some(Ok(message)) = audio_b.next().await {
            if matches!(message, TungsteniteMessage::Close(_)) {
                break;
            }
        }
    })
    .await
    .expect("server did not close farewell audio");
    let (_, load) =
        admin_json_request(router, http::Method::GET, "/api/admin/load", Value::Null).await;
    assert_eq!(
        load["current"]["capacity"]["completed_retained_sessions"],
        1
    );
    server.abort();
}

/// Proves that two simultaneous player pairs receive audio only inside their own rooms.
#[tokio::test]
async fn audio_websockets_isolate_two_simultaneous_player_pairs() {
    let router = build_router(TinyAdapter, voice_enabled_config(), ServeOptions::default())
        .await
        .unwrap();
    let (a_one, b_one, room_one) = create_joined_room(router.clone()).await;
    let (a_two, b_two, room_two) = create_joined_room(router.clone()).await;
    assert_ne!(room_one, room_two);

    let plan_a_one = request_audio_plan(router.clone(), &room_one, &a_one).await;
    let plan_b_one = request_audio_plan(router.clone(), &room_one, &b_one).await;
    let plan_a_two = request_audio_plan(router.clone(), &room_two, &a_two).await;
    let plan_b_two = request_audio_plan(router.clone(), &room_two, &b_two).await;
    let tokens = [
        plan_a_one["token"].as_str().unwrap(),
        plan_b_one["token"].as_str().unwrap(),
        plan_a_two["token"].as_str().unwrap(),
        plan_b_two["token"].as_str().unwrap(),
    ];
    for (index, token) in tokens.iter().enumerate() {
        assert!(tokens[..index].iter().all(|existing| existing != token));
    }

    let (base_url, server) = spawn_test_server(router.clone()).await;
    let host = base_url.trim_start_matches("http://");
    let connect = |public_session_id: &str, token: &str| {
        connect_async(format!(
            "ws://{host}/ws/audio/{public_session_id}?token={token}"
        ))
    };
    let (mut socket_a_one, _) = connect(&room_one, tokens[0]).await.unwrap();
    let (mut socket_b_one, _) = connect(&room_one, tokens[1]).await.unwrap();
    let (mut socket_a_two, _) = connect(&room_two, tokens[2]).await.unwrap();
    let (mut socket_b_two, _) = connect(&room_two, tokens[3]).await.unwrap();

    let frame_one = AudioFrame {
        sequence: 11,
        timestamp_ms: 220,
        pcm: vec![1; crate::audio::AUDIO_FRAME_BYTES],
    }
    .encode();
    let frame_two = AudioFrame {
        sequence: 22,
        timestamp_ms: 440,
        pcm: vec![2; crate::audio::AUDIO_FRAME_BYTES],
    }
    .encode();
    socket_a_one
        .send(TungsteniteMessage::Binary(frame_one.clone()))
        .await
        .unwrap();
    assert_eq!(read_audio_binary(&mut socket_b_one).await, frame_one);
    assert_no_audio_binary(&mut socket_a_two).await;
    assert_no_audio_binary(&mut socket_b_two).await;

    socket_a_two
        .send(TungsteniteMessage::Binary(frame_two.clone()))
        .await
        .unwrap();
    assert_eq!(read_audio_binary(&mut socket_b_two).await, frame_two);
    assert_no_audio_binary(&mut socket_a_one).await;
    assert_no_audio_binary(&mut socket_b_one).await;

    server.abort();
}

/// Ensures a replacement connection cannot leave an older socket injecting room audio.
#[tokio::test]
async fn replacement_audio_connection_revokes_the_older_socket_generation() {
    let router = build_router(TinyAdapter, voice_enabled_config(), ServeOptions::default())
        .await
        .unwrap();
    let (a, b, public_session_id) = create_joined_room(router.clone()).await;
    let old_a = request_audio_plan(router.clone(), &public_session_id, &a).await;
    let plan_b = request_audio_plan(router.clone(), &public_session_id, &b).await;
    let (base_url, server) = spawn_test_server(router.clone()).await;
    let host = base_url.trim_start_matches("http://");
    let (mut socket_b, _) = connect_async(format!(
        "ws://{host}/ws/audio/{public_session_id}?token={}",
        plan_b["token"].as_str().unwrap()
    ))
    .await
    .unwrap();
    let (mut old_socket_a, _) = connect_async(format!(
        "ws://{host}/ws/audio/{public_session_id}?token={}",
        old_a["token"].as_str().unwrap()
    ))
    .await
    .unwrap();
    wait_for_audio_control(&mut old_socket_a).await;
    let new_a = request_audio_plan(router, &public_session_id, &a).await;
    let (mut new_socket_a, _) = connect_async(format!(
        "ws://{host}/ws/audio/{public_session_id}?token={}",
        new_a["token"].as_str().unwrap()
    ))
    .await
    .unwrap();
    wait_for_audio_control(&mut new_socket_a).await;

    let stale_frame = AudioFrame {
        sequence: 1,
        timestamp_ms: 20,
        pcm: vec![3; crate::audio::AUDIO_FRAME_BYTES],
    }
    .encode();
    old_socket_a
        .send(TungsteniteMessage::Binary(stale_frame))
        .await
        .unwrap();
    assert_no_audio_binary(&mut socket_b).await;

    let current_frame = AudioFrame {
        sequence: 2,
        timestamp_ms: 40,
        pcm: vec![4; crate::audio::AUDIO_FRAME_BYTES],
    }
    .encode();
    new_socket_a
        .send(TungsteniteMessage::Binary(current_frame.clone()))
        .await
        .unwrap();
    assert_eq!(read_audio_binary(&mut socket_b).await, current_frame);
    server.abort();
}

/// Verifies that audio credentials are single-use and cannot cross room boundaries.
#[tokio::test]
async fn audio_tokens_are_single_use_and_room_bound() {
    let router = build_router(TinyAdapter, voice_enabled_config(), ServeOptions::default())
        .await
        .unwrap();
    let (a_one, _b_one, room_one) = create_joined_room(router.clone()).await;
    let (_a_two, _b_two, room_two) = create_joined_room(router.clone()).await;
    let wrong_room_plan = request_audio_plan(router.clone(), &room_one, &a_one).await;
    let reusable_plan = request_audio_plan(router.clone(), &room_one, &a_one).await;
    let (base_url, server) = spawn_test_server(router.clone()).await;
    let host = base_url.trim_start_matches("http://");

    let wrong_room = connect_async(format!(
        "ws://{host}/ws/audio/{room_two}?token={}",
        wrong_room_plan["token"].as_str().unwrap()
    ))
    .await;
    assert!(matches!(
        wrong_room,
        Err(tokio_tungstenite::tungstenite::Error::Http(response))
            if response.status() == StatusCode::FORBIDDEN
    ));

    let url = format!(
        "ws://{host}/ws/audio/{room_one}?token={}",
        reusable_plan["token"].as_str().unwrap()
    );
    let (_socket, _) = connect_async(url.clone()).await.unwrap();
    let replay = connect_async(url).await;
    assert!(matches!(
        replay,
        Err(tokio_tungstenite::tungstenite::Error::Http(response))
            if response.status() == StatusCode::FORBIDDEN
    ));
    server.abort();
}

/// Deterministic provider that finalizes the first received audio frame twice.
struct DuplicateFinalTranscriptionProvider;

/// Reports all accepted PCM markers when each stream is explicitly finished.
struct ReadinessTranscriptionProvider {
    finished: mpsc::UnboundedSender<(String, Vec<u8>)>,
}

#[async_trait]
impl crate::transcription::TranscriptionProvider for ReadinessTranscriptionProvider {
    /// Records frames and signals completion after the ordered input queue is drained.
    async fn start_session(
        &self,
        context: crate::transcription::TranscriptionSessionContext,
    ) -> Result<crate::transcription::TranscriptionSessionHandle> {
        use crate::transcription::{
            TranscriptionEvent, TranscriptionInput, TranscriptionSessionHandle,
        };
        let (input, mut inputs) = mpsc::channel(32);
        let (events, receiver) = mpsc::channel(4);
        let finished = self.finished.clone();
        tokio::spawn(async move {
            events.send(TranscriptionEvent::Ready).await.unwrap();
            let mut markers = Vec::new();
            while let Some(message) = inputs.recv().await {
                match message {
                    TranscriptionInput::Audio(frame) => markers.push(frame.pcm[0]),
                    TranscriptionInput::Finish => {
                        let _ = finished.send((context.role, markers));
                        break;
                    }
                }
            }
        });
        Ok(TranscriptionSessionHandle {
            input,
            events: receiver,
        })
    }
}

/// Reads a completed stream's markers; the timeout only bounds failure diagnosis.
async fn readiness_finished_audio(
    finished: &mut mpsc::UnboundedReceiver<(String, Vec<u8>)>,
) -> (String, Vec<u8>) {
    tokio::time::timeout(Duration::from_secs(5), finished.recv())
        .await
        .expect("audio stream did not finish")
        .expect("transcription recorder closed before stream finished")
}

#[async_trait]
impl TranscriptionProvider for DuplicateFinalTranscriptionProvider {
    /// Starts a test session whose duplicate finals exercise idempotent persistence.
    async fn start_session(
        &self,
        _context: TranscriptionSessionContext,
    ) -> Result<crate::transcription::TranscriptionSessionHandle> {
        let (input, mut inputs) = mpsc::channel(4);
        let (events, event_receiver) = mpsc::channel(4);
        tokio::spawn(async move {
            let _ = events.send(TranscriptionEvent::Ready).await;
            while let Some(message) = inputs.recv().await {
                if matches!(message, TranscriptionInput::Audio(_)) {
                    let utterance = FinalTranscriptUtterance {
                        start_time_ms: 0,
                        end_time_ms: 20,
                        text: "relay transcript".to_string(),
                        result_ids: vec!["stable-result".to_string()],
                        tokens: vec![crate::transcription::TranscriptToken {
                            kind: "word".into(),
                            text: "relay".into(),
                            start_time_ms: 0,
                            end_time_ms: 20,
                            confidence: Some(1.0),
                        }],
                    };
                    let _ = events
                        .send(TranscriptionEvent::FinalUtterance(utterance.clone()))
                        .await;
                    let _ = events
                        .send(TranscriptionEvent::FinalUtterance(utterance))
                        .await;
                    break;
                }
            }
        });
        Ok(crate::transcription::TranscriptionSessionHandle {
            input,
            events: event_receiver,
        })
    }
}
