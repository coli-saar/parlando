use super::*;

#[tokio::test]
async fn websocket_rejects_actions_until_both_players_are_connected() {
    let router = build_router(TinyAdapter, step_five_config(), ServeOptions::default())
        .await
        .unwrap();
    let (a, _b, public_session_id) = create_joined_room(router.clone()).await;
    let (base_url, server) = spawn_test_server(router.clone()).await;
    let (mut socket_a, _) =
        connect_async(game_socket_url(&router, &base_url, &public_session_id, &a).await)
            .await
            .unwrap();

    send_ws_json(
        &mut socket_a,
        json!({"type": "action", "action": {"finish": false}}),
    )
    .await;
    let error = read_ws_type(&mut socket_a, "action_rejected").await;
    assert_eq!(error["code"], "participant_not_active");

    let (export_status, export) = admin_json_request(
        router,
        http::Method::GET,
        "/api/admin/export?variant=full",
        Value::Null,
    )
    .await;
    assert_eq!(export_status, StatusCode::OK);
    assert!(export["session_events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|event| {
            event["event_type"] == "game_action_rejected"
                && event["payload"]["reason_code"] == "participant_not_active"
        }));
    server.abort();
}

/// Rejected oversized actions retain bounded analytical metadata, not attacker input.
#[tokio::test]
async fn oversized_action_rejection_is_bounded_and_analyzable() {
    let router = build_router(TinyAdapter, step_five_config(), ServeOptions::default())
        .await
        .unwrap();
    let (a, b, public_session_id) = create_joined_room(router.clone()).await;
    let (base_url, server) = spawn_test_server(router.clone()).await;
    let (mut socket_a, _) =
        connect_async(game_socket_url(&router, &base_url, &public_session_id, &a).await)
            .await
            .unwrap();
    let (mut socket_b, _) =
        connect_async(game_socket_url(&router, &base_url, &public_session_id, &b).await)
            .await
            .unwrap();
    let _assigned_a = read_participant_state(&mut socket_a, "active").await;
    let _assigned_b = read_participant_state(&mut socket_b, "active").await;

    send_ws_json(
        &mut socket_a,
        json!({"type": "action", "action": {"padding": "x".repeat(5_000)}}),
    )
    .await;
    let error = read_ws_type(&mut socket_a, "error").await;
    assert_eq!(error["code"], "action_too_large");
    assert_no_ws_type(&mut socket_b, "error").await;

    let (export_status, export) = admin_json_request(
        router,
        http::Method::GET,
        "/api/admin/export?variant=full",
        Value::Null,
    )
    .await;
    assert_eq!(export_status, StatusCode::OK);
    let rejected = export["session_events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|event| event["event_type"] == "game_action_rejected")
        .unwrap();
    assert_eq!(rejected["payload"]["reason_code"], "action_too_large");
    assert!(rejected["payload"]["submitted_bytes"].as_u64().unwrap() > 4_096);
    assert_eq!(
        rejected["payload"]["action_sha256"].as_str().unwrap().len(),
        64
    );
    assert!(!rejected["payload"].to_string().contains(&"x".repeat(100)));
    server.abort();
}

/// Replays store one initial state and reconstruct every later frame from accepted actions.
#[tokio::test]
async fn recorded_session_reconstructs_without_per_action_states() {
    let router = build_router(TinyAdapter, step_five_config(), ServeOptions::default())
        .await
        .unwrap();
    let (a, b, public_session_id) = create_joined_room(router.clone()).await;
    let (base_url, server) = spawn_test_server(router.clone()).await;
    let (mut socket_a, _) =
        connect_async(game_socket_url(&router, &base_url, &public_session_id, &a).await)
            .await
            .unwrap();
    let (mut socket_b, _) =
        connect_async(game_socket_url(&router, &base_url, &public_session_id, &b).await)
            .await
            .unwrap();
    let _assigned_a = read_participant_state(&mut socket_a, "active").await;
    let _assigned_b = read_participant_state(&mut socket_b, "active").await;
    send_ws_json(
        &mut socket_a,
        json!({"type": "message", "text": "replay hello"}),
    )
    .await;
    let _message_a = read_ws_type(&mut socket_a, "message").await;
    let _message_b = read_ws_type(&mut socket_b, "message").await;
    send_ws_json(
        &mut socket_a,
        json!({"type": "action", "action": {"finish": true}}),
    )
    .await;
    let _completed = read_participant_state(&mut socket_a, "ended").await;

    let (sessions_status, sessions) = admin_json_request(
        router.clone(),
        http::Method::GET,
        "/api/admin/sessions",
        Value::Null,
    )
    .await;
    assert_eq!(sessions_status, StatusCode::OK);
    let session_id = sessions["sessions"][0]["session_id"].as_i64().unwrap();
    let (export_status, export) = admin_json_request(
        router.clone(),
        http::Method::GET,
        "/api/admin/export?variant=full",
        Value::Null,
    )
    .await;
    assert_eq!(export_status, StatusCode::OK);
    let events = export["session_events"].as_array().unwrap();
    let created = events
        .iter()
        .find(|event| event["event_type"] == "session_created")
        .unwrap();
    assert_eq!(created["payload"]["replay_compatibility"]["state"], "1");
    assert!(!created["game_state"].is_null());
    let accepted = events
        .iter()
        .find(|event| event["event_type"] == "game_action_accepted")
        .unwrap();
    assert!(accepted["game_state"].is_null());
    let (replay_status, replay) = admin_json_request(
        router,
        http::Method::GET,
        &format!("/api/admin/sessions/{session_id}/replay"),
        Value::Null,
    )
    .await;
    assert_eq!(replay_status, StatusCode::OK);
    assert_eq!(replay["roles"]["A"].as_array().unwrap().len(), 3);
    assert_eq!(replay["roles"]["B"].as_array().unwrap().len(), 3);
    assert_eq!(replay["roles"]["A"][0]["observation"]["done"], false);
    assert_eq!(
        replay["roles"]["A"][1]["conversation"][0]["text"],
        "replay hello"
    );
    assert_eq!(replay["roles"]["A"][2]["observation"]["done"], true);
    assert_eq!(replay["roles"]["A"][2]["transition"]["actor"], "A");
    server.abort();
}

#[tokio::test]
async fn human_human_game_accepts_actions_after_second_human_connects() {
    let router = build_router(TinyAdapter, step_five_config(), ServeOptions::default())
        .await
        .unwrap();
    let (a, b, public_session_id) = create_joined_room(router.clone()).await;
    let (base_url, server) = spawn_test_server(router.clone()).await;
    let (mut socket_a, _) =
        connect_async(game_socket_url(&router, &base_url, &public_session_id, &a).await)
            .await
            .unwrap();
    let _ = read_participant_state(&mut socket_a, "waiting").await;

    send_ws_json(
        &mut socket_a,
        json!({"type": "action", "action": {"finish": false}}),
    )
    .await;
    let waiting_error = read_ws_type(&mut socket_a, "action_rejected").await;
    assert_eq!(waiting_error["code"], "participant_not_active");

    let (mut socket_b, _) =
        connect_async(game_socket_url(&router, &base_url, &public_session_id, &b).await)
            .await
            .unwrap();
    let _assigned_a = read_participant_state(&mut socket_a, "active").await;
    let _assigned_b = read_participant_state(&mut socket_b, "active").await;
    send_ws_json(
        &mut socket_a,
        json!({"type": "action", "action": {"finish": true}}),
    )
    .await;
    let completed = read_participant_state(&mut socket_a, "ended").await;
    assert_eq!(completed["result"]["completion"]["done"], true);
    server.abort();
}

#[tokio::test]
async fn websocket_accepts_actions_chat_completion_and_persists_state_changes() {
    let router = build_router(TinyAdapter, step_five_config(), ServeOptions::default())
        .await
        .unwrap();
    let (a, b, public_session_id) = create_joined_room(router.clone()).await;
    let (base_url, server) = spawn_test_server(router.clone()).await;
    let (mut socket_a, _) =
        connect_async(game_socket_url(&router, &base_url, &public_session_id, &a).await)
            .await
            .unwrap();
    let (mut socket_b, _) =
        connect_async(game_socket_url(&router, &base_url, &public_session_id, &b).await)
            .await
            .unwrap();
    let _assigned_a = read_participant_state(&mut socket_a, "active").await;
    let _assigned_b = read_participant_state(&mut socket_b, "active").await;

    send_ws_json(
        &mut socket_a,
        json!({"type": "message", "text": "hello from A"}),
    )
    .await;
    let chat_a = read_ws_type(&mut socket_a, "message").await;
    let chat_b = read_ws_type(&mut socket_b, "message").await;
    assert_eq!(chat_a["message"]["text"], "hello from A");
    assert_eq!(chat_b["message"]["input"], "text");

    send_ws_json(
        &mut socket_a,
        json!({"type": "action", "action": {"finish": false}}),
    )
    .await;
    let state_a = read_ws_type(&mut socket_a, "transition").await;
    let state_b = read_ws_type(&mut socket_b, "transition").await;
    assert_eq!(state_a["actor"], "A");
    assert_eq!(state_b["actor"], "A");
    assert_eq!(state_a["action"]["finish"], false);
    assert_eq!(state_b["action"], state_a["action"]);
    assert_eq!(state_a["observation"]["done"], false);

    send_ws_json(
        &mut socket_a,
        json!({"type": "action", "action": {"finish": true}}),
    )
    .await;
    let completed_a = read_participant_state(&mut socket_a, "ended").await;
    let completed_b = read_participant_state(&mut socket_b, "ended").await;
    assert_eq!(completed_a["result"]["completion"]["done"], true);
    assert_eq!(completed_a["result"]["completion"]["outcome"], "success");
    assert_eq!(completed_a["result"]["completion"]["dyad_score"], 10);
    assert_eq!(completed_a["result"]["completion"]["player_scores"]["A"], 6);
    assert_eq!(completed_a["result"]["completion"]["player_scores"]["B"], 4);
    assert_eq!(
        completed_b["result"]["completion"],
        completed_a["result"]["completion"]
    );

    let (export_status, export) = admin_json_request(
        router,
        http::Method::GET,
        "/api/admin/export?variant=full",
        Value::Null,
    )
    .await;
    assert_eq!(export_status, StatusCode::OK);
    let event_types = export["session_events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| event["event_type"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(event_types.contains(&"conversation_message"));
    assert!(event_types.contains(&"game_action_accepted"));
    assert!(!event_types.contains(&"state_changed"));
    assert!(event_types.contains(&"session_completed"));
    server.abort();
}

#[tokio::test]
async fn loss_completion_is_broadcast_and_exported() {
    let router = build_router(
        LossSummaryAdapter,
        step_five_config(),
        ServeOptions::default(),
    )
    .await
    .unwrap();
    let (a, b, public_session_id) = create_joined_room(router.clone()).await;
    let (base_url, server) = spawn_test_server(router.clone()).await;
    let (mut socket_a, _) =
        connect_async(game_socket_url(&router, &base_url, &public_session_id, &a).await)
            .await
            .unwrap();
    let (mut socket_b, _) =
        connect_async(game_socket_url(&router, &base_url, &public_session_id, &b).await)
            .await
            .unwrap();
    let _assigned_a = read_participant_state(&mut socket_a, "active").await;
    let _assigned_b = read_participant_state(&mut socket_b, "active").await;

    send_ws_json(
        &mut socket_a,
        json!({"type": "action", "action": {"finish": true}}),
    )
    .await;
    let completed = read_participant_state(&mut socket_a, "ended").await;
    assert_eq!(completed["result"]["completion"]["done"], true);
    assert_eq!(completed["result"]["completion"]["outcome"], "loss");
    assert_eq!(completed["result"]["completion"]["dyad_score"], 0);
    assert_eq!(completed["result"]["completion"]["player_scores"]["A"], 0);
    assert_eq!(completed["result"]["completion"]["player_scores"]["B"], 0);

    let corpus = assert_corpus_session(router.clone(), "completed").await;
    assert_eq!(
        corpus["experiment"]["sessions"][0]["completion"]["outcome"],
        "loss"
    );

    let (export_status, export) = admin_json_request(
        router,
        http::Method::GET,
        "/api/admin/export?variant=full",
        Value::Null,
    )
    .await;
    assert_eq!(export_status, StatusCode::OK);
    let completed_events = export["session_events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| event["event_type"] == "session_completed")
        .collect::<Vec<_>>();
    assert_eq!(completed_events.len(), 1);
    assert_eq!(export["sessions"][0]["completion"]["outcome"], "loss");
    server.abort();
}

#[tokio::test]
async fn completed_rooms_reject_late_game_channel_input() {
    let router = build_router(TinyAdapter, step_five_config(), ServeOptions::default())
        .await
        .unwrap();
    let (a, b, public_session_id) = create_joined_room(router.clone()).await;
    let (base_url, server) = spawn_test_server(router.clone()).await;
    let (mut socket_a, _) =
        connect_async(game_socket_url(&router, &base_url, &public_session_id, &a).await)
            .await
            .unwrap();
    let (mut socket_b, _) =
        connect_async(game_socket_url(&router, &base_url, &public_session_id, &b).await)
            .await
            .unwrap();
    let _assigned_a = read_participant_state(&mut socket_a, "active").await;
    let _assigned_b = read_participant_state(&mut socket_b, "active").await;

    send_ws_json(
        &mut socket_a,
        json!({"type": "action", "action": {"finish": true}}),
    )
    .await;
    let _completed_a = read_participant_state(&mut socket_a, "ended").await;

    send_ws_json(
        &mut socket_a,
        json!({"type": "action", "action": {"finish": false}}),
    )
    .await;
    let action_error = read_ws_type(&mut socket_a, "action_rejected").await;
    assert_eq!(action_error["code"], "session_complete");

    send_ws_json(
        &mut socket_a,
        json!({"type": "message", "text": "late hello"}),
    )
    .await;
    let chat_error = read_ws_type(&mut socket_a, "error").await;
    assert_eq!(chat_error["code"], "message_rejected");
    assert_no_ws_type(&mut socket_b, "message").await;

    let (transcript_status, transcript_response) = participant_json_request(
        router.clone(),
        &a,
        http::Method::POST,
        &format!("/api/sessions/{public_session_id}/transcripts"),
        json!({

            "player": "A",
            "text": "late transcript",
            "metadata": {}
        }),
    )
    .await;
    assert_eq!(transcript_status, StatusCode::NOT_FOUND);
    assert!(transcript_response["raw"].as_str().is_none());

    let (export_status, export) = admin_json_request(
        router,
        http::Method::GET,
        "/api/admin/export?variant=full",
        Value::Null,
    )
    .await;
    assert_eq!(export_status, StatusCode::OK);
    let events = export["session_events"].as_array().unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| event["event_type"] == "session_completed")
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event["event_type"] == "conversation_message")
            .count(),
        0
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event["event_type"] == "transcript_segment")
            .count(),
        0
    );
    server.abort();
}
