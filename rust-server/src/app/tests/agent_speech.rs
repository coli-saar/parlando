use super::*;

#[tokio::test]
async fn agent_tts_records_diagnostics_for_agent_messages() {
    let factory = Arc::new(ScriptedAgentFactory::new(vec![vec![scripted_response(
        Some("speak this"),
        None,
    )]]));
    let tts_provider = Arc::new(MockTtsProvider {
        calls: AtomicUsize::new(0),
        fail_first: false,
    });
    let audio_publisher = Arc::new(MockAudioPublisher {
        calls: AtomicUsize::new(0),
    });
    let router = build_router(
        live_game(),
        human_vs_agent_config(),
        ServeOptions {
            agent_factory: Some(factory),
            tts_provider: Some(tts_provider),
            audio_publisher: Some(audio_publisher.clone()),
            ..ServeOptions::default()
        },
    )
    .await
    .unwrap();
    let (human, public_session_id) = create_human_vs_agent_room(router.clone(), "Human").await;
    let (base_url, server) = spawn_test_server(router.clone()).await;
    let (mut socket, _) =
        connect_async(game_socket_url(&router, &base_url, &public_session_id, &human).await)
            .await
            .unwrap();
    let _ = read_participant_state(&mut socket, "active").await;

    let export = wait_for_tts_diagnostic(router, "tts_message_completed").await;
    let diagnostics = export["session_events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| event["event_type"] == "tts_diagnostic")
        .map(|event| event["payload"]["event"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(diagnostics.contains(&"tts_message_started"));
    assert!(diagnostics.contains(&"tts_first_audio"));
    assert!(diagnostics.contains(&"tts_audio_summary"));
    assert!(diagnostics.contains(&"tts_publish_started"));
    assert!(diagnostics.contains(&"tts_publish_completed"));
    assert!(diagnostics.contains(&"tts_message_completed"));
    let timing = export["session_events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|event| event["event_type"] == "conversation_speech_timing")
        .expect("published speech has durable timing");
    assert_eq!(timing["actor_role"], "B");
    let metadata = &timing["payload"]["metadata"];
    let origin = metadata["start_game_time_ms"].as_i64().unwrap();
    assert_eq!(metadata["tokens"][0]["start_ms"], origin + 100);
    assert_eq!(metadata["tokens"][0]["end_ms"], origin + 800);
    assert!(metadata["tokens"][0].get("confidence").is_none());
    let raw_events: Vec<crate::storage::StoredSessionEvent> =
        serde_json::from_value(export["session_events"].clone()).unwrap();
    let historical_events = raw_events
        .iter()
        .filter(|event| event.event_type != "conversation_speech_timing")
        .cloned()
        .collect::<Vec<_>>();
    let historical_bundles =
        admin_event_bundles(&important_admin_events(historical_events.clone()));
    let historical = historical_bundles
        .iter()
        .find(|bundle| bundle["origin"] == "agent")
        .unwrap();
    assert_eq!(historical["spoken"], true);
    assert!(historical["tokens"].is_null());
    let typed_events = historical_events
        .into_iter()
        .filter(|event| event.event_type != "tts_diagnostic")
        .collect();
    let typed_bundles = admin_event_bundles(&important_admin_events(typed_events));
    assert_eq!(
        typed_bundles
            .iter()
            .find(|bundle| bundle["origin"] == "agent")
            .unwrap()["spoken"],
        false
    );
    let bundles = admin_event_bundles(&important_admin_events(raw_events));
    let spoken = bundles
        .iter()
        .find(|bundle| bundle["origin"] == "agent")
        .unwrap();
    assert_eq!(spoken["spoken"], true);
    assert_eq!(spoken["tokens"][0]["text"], "speak this");
    assert_eq!(spoken["utterance_timing"]["start_ms"], origin);
    let corpus = corpus_experiment_export(serde_json::from_value(export).unwrap(), "2").unwrap();
    let spoken = corpus["experiment"]["sessions"][0]["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|event| event["origin"] == "agent")
        .unwrap();
    assert_eq!(spoken["tokens"][0]["start_ms"], origin + 100);
    assert_eq!(audio_publisher.calls.load(Ordering::SeqCst), 1);
    server.abort();
}

#[tokio::test]
async fn agent_tts_continues_after_provider_failure() {
    let factory = Arc::new(ScriptedAgentFactory::new(vec![vec![
        scripted_response(Some("first fails"), None),
        scripted_response(Some("second succeeds"), None),
    ]]));
    let tts_provider = Arc::new(MockTtsProvider {
        calls: AtomicUsize::new(0),
        fail_first: true,
    });
    let router = build_router(
        live_game(),
        human_vs_agent_config(),
        ServeOptions {
            agent_factory: Some(factory),
            tts_provider: Some(tts_provider),
            ..ServeOptions::default()
        },
    )
    .await
    .unwrap();
    let (human, public_session_id) = create_human_vs_agent_room(router.clone(), "Human").await;
    let (base_url, server) = spawn_test_server(router.clone()).await;
    let (mut socket, _) =
        connect_async(game_socket_url(&router, &base_url, &public_session_id, &human).await)
            .await
            .unwrap();
    let _ = read_participant_state(&mut socket, "active").await;

    let _ = wait_for_tts_diagnostic(router.clone(), "tts_message_failed").await;
    send_ws_json(
        &mut socket,
        json!({"type": "message", "text": "please try again"}),
    )
    .await;
    let export = wait_for_tts_diagnostic(router, "tts_message_completed").await;
    let diagnostics = export["session_events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| event["event_type"] == "tts_diagnostic")
        .map(|event| event["payload"]["event"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(diagnostics.contains(&"tts_message_failed"));
    assert!(diagnostics.contains(&"tts_message_completed"));
    server.abort();
}
