use super::*;

#[tokio::test]
async fn agent_runtime_creates_one_fresh_agent_per_room() {
    let factory = Arc::new(ScriptedAgentFactory::new(vec![vec![], vec![]]));
    let router = build_router(
        live_game(),
        human_vs_agent_config(),
        ServeOptions {
            agent_factory: Some(factory.clone()),
            ..ServeOptions::default()
        },
    )
    .await
    .unwrap();
    let (human_1, room_1) = create_human_vs_agent_room(router.clone(), "Human 1").await;
    let (human_2, room_2) = create_human_vs_agent_room(router.clone(), "Human 2").await;
    let (base_url, server) = spawn_test_server(router.clone()).await;
    let (mut socket_1, _) =
        connect_async(game_socket_url(&router, &base_url, &room_1, &human_1).await)
            .await
            .unwrap();
    let (mut socket_2, _) =
        connect_async(game_socket_url(&router, &base_url, &room_2, &human_2).await)
            .await
            .unwrap();
    let _ = read_participant_state(&mut socket_1, "active").await;
    let _ = read_participant_state(&mut socket_2, "active").await;

    for _ in 0..20 {
        if factory.created_count() == 2 {
            server.abort();
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    server.abort();
    panic!("expected one fresh agent for each room");
}

#[tokio::test]
async fn agent_runtime_observes_messages_with_speaker_and_modality() {
    let observations = Arc::new(Mutex::new(Vec::new()));
    let router = build_router(
        live_game(),
        human_vs_agent_config(),
        ServeOptions {
            agent_factory: Some(Arc::new(RecordingObservationsAgentFactory {
                observations: observations.clone(),
            })),
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
    let _ = wait_for_export_event(router.clone(), "agent_started").await;

    send_ws_json(
        &mut socket,
        json!({"type": "message", "text": "typed hello"}),
    )
    .await;
    let _ = read_ws_type(&mut socket, "message").await;
    let (status, _) = participant_json_request(
        router,
        &human,
        http::Method::POST,
        &format!("/api/sessions/{public_session_id}/transcripts"),
        json!({

            "player": "A",
            "start_time_ms": 0,
            "end_time_ms": 10,
            "text": "spoken hello",
            "metadata": {}
        }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    for _ in 0..20 {
        let captured = observations.lock().unwrap().clone();
        if captured.contains(&"message:A:typed hello".to_string()) {
            server.abort();
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    server.abort();
    panic!("expected agent to observe typed messages");
}

#[tokio::test]
async fn agent_runtime_observes_actions_with_resulting_observation() {
    let observations = Arc::new(Mutex::new(Vec::new()));
    let router = build_router(
        live_game(),
        human_vs_agent_config(),
        ServeOptions {
            agent_factory: Some(Arc::new(RecordingObservationsAgentFactory {
                observations: observations.clone(),
            })),
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
    let _ = wait_for_export_event(router.clone(), "agent_started").await;

    send_ws_json(
        &mut socket,
        json!({"type": "action", "action": {"finish": false}}),
    )
    .await;
    let _ = read_ws_type(&mut socket, "transition").await;

    for _ in 0..20 {
        let captured = observations.lock().unwrap().clone();
        if captured.contains(&"action:A:false:false".to_string()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(observations
        .lock()
        .unwrap()
        .contains(&"action:A:false:false".to_string()));

    send_ws_json(
        &mut socket,
        json!({"type": "action", "action": {"finish": true}}),
    )
    .await;
    let _ = read_participant_state(&mut socket, "ended").await;
    for _ in 0..20 {
        let captured = observations.lock().unwrap().clone();
        if captured.contains(&"finish:success:10".to_string()) {
            assert!(captured.contains(&"action:A:true:true".to_string()));
            server.abort();
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    server.abort();
    panic!("expected agent to observe the terminal action and completion");
}

#[tokio::test]
async fn agent_runtime_persists_messages_and_validated_actions() {
    let factory = Arc::new(ScriptedAgentFactory::new(vec![vec![
        scripted_response(
            Some("agent says hello"),
            Some(TinyAction {
                finish: false,
                invalid: false,
            }),
        ),
        scripted_response(
            None,
            Some(TinyAction {
                finish: true,
                invalid: false,
            }),
        ),
    ]]));
    let router = build_router(
        live_game(),
        human_vs_agent_config(),
        ServeOptions {
            agent_factory: Some(factory),
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
    let first_update = read_ws_type(&mut socket, "transition").await;
    assert_eq!(first_update["type"], "transition");
    let message = read_ws_type(&mut socket, "message").await;
    assert_eq!(message["message"]["sender"], "B");
    assert_eq!(message["message"]["input"], "text");
    assert_eq!(message["message"]["text"], "agent says hello");
    let completed = read_participant_state(&mut socket, "ended").await;
    assert_eq!(completed["result"]["completion"]["done"], true);

    let export = wait_for_export_event(router, "session_completed").await;
    let events = export["session_events"].as_array().unwrap();
    assert!(events
        .iter()
        .any(|event| event["event_type"] == "agent_started"));
    assert!(events
        .iter()
        .any(|event| event["event_type"] == "agent_action"));
    assert!(events.iter().any(|event| {
        event["event_type"] == "conversation_message" && event["payload"]["origin"] == "agent"
    }));
    assert!(events
        .iter()
        .any(|event| event["event_type"] == "game_action_accepted"));
    server.abort();
}

#[tokio::test]
async fn agent_runtime_observes_accepted_action_before_next_decision() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let router = build_router(
        live_game(),
        human_vs_agent_config(),
        ServeOptions {
            agent_factory: Some(Arc::new(SequencedDecisionAgentFactory { log: log.clone() })),
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

    for _ in 0..20 {
        let snapshot = log.lock().unwrap().clone();
        if snapshot.iter().any(|entry| entry == "respond:2") {
            assert_eq!(
                snapshot,
                vec!["respond:1", "observe_transition:B", "respond:2"]
            );
            server.abort();
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    server.abort();
    panic!("timed out waiting for the second agent decision");
}

#[tokio::test]
async fn agent_runtime_stops_invalid_agents_cleanly() {
    let factory = Arc::new(ScriptedAgentFactory::new(vec![vec![
        scripted_response(
            None,
            Some(TinyAction {
                finish: false,
                invalid: true,
            }),
        ),
        scripted_response(
            None,
            Some(TinyAction {
                finish: false,
                invalid: true,
            }),
        ),
        scripted_response(
            None,
            Some(TinyAction {
                finish: false,
                invalid: false,
            }),
        ),
    ]]));
    let router = build_router(
        live_game(),
        human_vs_agent_config(),
        ServeOptions {
            agent_factory: Some(factory),
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

    let export = wait_for_export_event(router, "agent_error").await;
    let events = export["session_events"].as_array().unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| event["event_type"] == "agent_action")
            .count(),
        2
    );
    assert!(!events
        .iter()
        .any(|event| event["event_type"] == "game_action_accepted"));
    assert!(events.iter().any(|event| {
        event["event_type"] == "agent_error"
            && event["payload"]["last_error"]
                .as_str()
                .unwrap()
                .contains("invalid_tiny_action")
    }));
    server.abort();
}
