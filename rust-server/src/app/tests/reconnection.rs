use super::*;

// Verifies that explicit departure terminates both participant views and all later input.
#[tokio::test]
async fn explicit_leave_abandons_session_and_notifies_partner() {
    let router = build_router(live_game(), step_five_config(), ServeOptions::default())
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

    let (export_status, running_export) = admin_json_request(
        router.clone(),
        http::Method::GET,
        "/api/admin/export?variant=full",
        Value::Null,
    )
    .await;
    assert_eq!(export_status, StatusCode::OK);
    assert_eq!(running_export["sessions"][0]["lifecycle"], "running");
    assert!(running_export["sessions"][0]["started_at"].is_string());

    let (leave_status, leave_response) = participant_json_request(
        router.clone(),
        &a,
        http::Method::POST,
        &format!("/api/sessions/{public_session_id}/leave"),
        json!({}),
    )
    .await;
    assert_eq!(leave_status, StatusCode::OK);
    let withdrew = leave_response["participant_state"].clone();
    assert_eq!(withdrew["public_session_id"], public_session_id);
    assert_eq!(withdrew["result"]["outcome"], "left_game");
    assert!(withdrew["voice_ends_at"].is_null());
    let abandoned = read_participant_state(&mut socket_b, "ended").await;
    assert_eq!(abandoned["public_session_id"], public_session_id);
    assert_eq!(abandoned["result"]["outcome"], "partner_left");
    assert!(abandoned["voice_ends_at"].is_null());

    send_ws_json(
        &mut socket_a,
        json!({"type": "action", "action": {"finish": false}}),
    )
    .await;
    let rejected_action = read_ws_type(&mut socket_a, "action_rejected").await;
    assert_eq!(rejected_action["code"], "session_complete");
    send_ws_json(
        &mut socket_a,
        json!({"type": "message", "text": "This must not be recorded"}),
    )
    .await;
    let rejected_message = read_ws_type(&mut socket_a, "error").await;
    assert_eq!(rejected_message["code"], "message_rejected");

    let export = wait_for_export_event(router.clone(), "session_abandoned").await;
    assert_eq!(export["sessions"][0]["lifecycle"], "ended");
    let event = export["session_events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|event| event["event_type"] == "session_abandoned")
        .unwrap();
    assert_eq!(event["actor_role"], "A");
    assert_eq!(event["payload"]["reason"], "participant_left");
    assert!(!export["session_events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|event| {
            event["event_type"] == "conversation_message"
                && event["payload"]["text"] == "This must not be recorded"
        }));
    server.abort();
}

// Verifies that abandonment closes a human-versus-agent runtime through normal shutdown.
#[tokio::test]
async fn explicit_leave_shuts_down_the_session_agent() {
    let shutdowns = Arc::new(AtomicUsize::new(0));
    let router = build_router(
        live_game(),
        human_vs_agent_config(),
        ServeOptions {
            agent_factory: Some(Arc::new(ShutdownRecordingAgentFactory {
                shutdowns: shutdowns.clone(),
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

    let (leave_status, response) = participant_json_request(
        router.clone(),
        &human,
        http::Method::POST,
        &format!("/api/sessions/{public_session_id}/leave"),
        json!({}),
    )
    .await;
    assert_eq!(leave_status, StatusCode::OK);
    let ended = &response["participant_state"];
    assert_eq!(ended["result"]["outcome"], "left_game");
    let export = wait_for_export_event(router, "session_abandoned").await;
    assert_eq!(export["sessions"][0]["lifecycle"], "ended");

    for _ in 0..20 {
        if shutdowns.load(Ordering::SeqCst) == 1 {
            server.abort();
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    server.abort();
    panic!("expected the abandoned session agent to shut down");
}

#[tokio::test]
async fn brief_disconnect_pauses_partner_and_reconnect_preserves_session() {
    let mut config = step_five_config();
    config.session.reconnect_grace_seconds = 2;
    let router = build_router(live_game(), config, ServeOptions::default())
        .await
        .unwrap();
    let (a, b, public_session_id) = create_joined_room(router.clone()).await;
    let (_, forming_sessions) = admin_json_request(
        router.clone(),
        http::Method::GET,
        "/api/admin/sessions",
        Value::Null,
    )
    .await;
    let forming_session_id = forming_sessions["sessions"][0]["session_id"]
        .as_i64()
        .unwrap();
    assert_eq!(forming_sessions["sessions"][0]["lifecycle"], "forming");
    let (_, forming_detail) = admin_json_request(
        router.clone(),
        http::Method::GET,
        &format!("/api/admin/sessions/{forming_session_id}"),
        Value::Null,
    )
    .await;
    assert!(forming_detail["participants"]
        .as_array()
        .unwrap()
        .iter()
        .all(|participant| participant["participant_state"]["state"] == "waiting"));
    let (base_url, server) = spawn_test_server(router.clone()).await;
    let (mut socket_a, _) =
        connect_async(game_socket_url(&router, &base_url, &public_session_id, &a).await)
            .await
            .unwrap();
    let (mut socket_b, _) =
        connect_async(game_socket_url(&router, &base_url, &public_session_id, &b).await)
            .await
            .unwrap();
    let _ = read_participant_state(&mut socket_a, "active").await;
    let _ = read_participant_state(&mut socket_b, "active").await;

    socket_a.close(None).await.unwrap();
    let reconnecting = read_participant_state(&mut socket_b, "paused").await;
    assert!(reconnecting["reason"]["deadline_at"].as_str().is_some());
    let (_, sessions) = admin_json_request(
        router.clone(),
        http::Method::GET,
        "/api/admin/sessions",
        Value::Null,
    )
    .await;
    let session_id = sessions["sessions"][0]["session_id"].as_i64().unwrap();
    assert_eq!(session_id, forming_session_id);
    let (_, paused_detail) = admin_json_request(
        router.clone(),
        http::Method::GET,
        &format!("/api/admin/sessions/{session_id}"),
        Value::Null,
    )
    .await;
    assert!(paused_detail["participants"]
        .as_array()
        .unwrap()
        .iter()
        .all(|participant| participant["participant_state"]["state"] == "paused"));
    let (_, paused_load) = admin_json_request(
        router.clone(),
        http::Method::GET,
        "/api/admin/load",
        Value::Null,
    )
    .await;
    assert!(paused_load["sessions"][0]["participants"]
        .as_array()
        .unwrap()
        .iter()
        .all(|participant| participant["participant_state"] == "paused"));

    let (mut replacement_a, _) =
        connect_async(game_socket_url(&router, &base_url, &public_session_id, &a).await)
            .await
            .unwrap();
    let reconnected = read_participant_state(&mut socket_b, "active").await;
    assert_eq!(reconnected["public_session_id"], public_session_id);
    let (_, active_detail) = admin_json_request(
        router.clone(),
        http::Method::GET,
        &format!("/api/admin/sessions/{session_id}"),
        Value::Null,
    )
    .await;
    assert!(active_detail["participants"]
        .as_array()
        .unwrap()
        .iter()
        .all(|participant| participant["participant_state"]["state"] == "active"));
    let (_, active_load) = admin_json_request(
        router.clone(),
        http::Method::GET,
        "/api/admin/load",
        Value::Null,
    )
    .await;
    assert!(active_load["sessions"][0]["participants"]
        .as_array()
        .unwrap()
        .iter()
        .all(|participant| participant["participant_state"] == "active"));
    send_ws_json(&mut replacement_a, json!({"type": "heartbeat"})).await;
    server.abort();
}

#[tokio::test]
async fn disconnect_past_grace_gives_partner_a_terminal_outcome() {
    let mut config = step_five_config();
    config.session.reconnect_grace_seconds = 1;
    let router = build_router(live_game(), config, ServeOptions::default())
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
    let _ = read_participant_state(&mut socket_a, "active").await;
    let _ = read_participant_state(&mut socket_b, "active").await;

    socket_a.close(None).await.unwrap();
    let _ = read_participant_state(&mut socket_b, "paused").await;
    let ended = read_participant_state(&mut socket_b, "ended").await;
    assert_eq!(ended["result"]["outcome"], "partner_left");
    assert_eq!(ended["result"]["reason"], "reconnect_timeout");
    assert_corpus_session(router.clone(), "expired").await;
    let export = wait_for_export_event(router.clone(), "session_ended").await;
    assert_eq!(export["sessions"][0]["lifecycle"], "ended");
    let session_id = export["sessions"][0]["session_id"].as_i64().unwrap();
    let (_, ended_detail) = admin_json_request(
        router.clone(),
        http::Method::GET,
        &format!("/api/admin/sessions/{session_id}"),
        Value::Null,
    )
    .await;
    assert_eq!(ended_detail["session"]["lifecycle"], "ended");
    assert_eq!(
        ended_detail["session"]["session_end"]["cause"]["type"],
        "reconnect_timed_out"
    );
    assert_eq!(
        ended_detail["participants"]
            .as_array()
            .unwrap()
            .iter()
            .find(|participant| participant["role"] == "A")
            .unwrap()["terminal_result"]["outcome"],
        "connection_lost"
    );
    assert!(ended_detail["participants"]
        .as_array()
        .unwrap()
        .iter()
        .all(|participant| participant["participant_state"]["state"] == "ended"));
    server.abort();
}

/// Confirms room-wide reconnect expiry does not invent a connected good-faith partner.
#[tokio::test]
async fn two_disconnects_past_grace_give_both_connection_lost() {
    let mut config = step_five_config();
    config.session.reconnect_grace_seconds = 1;
    let router = build_router(live_game(), config, ServeOptions::default())
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
    let _ = read_participant_state(&mut socket_a, "active").await;
    let _ = read_participant_state(&mut socket_b, "active").await;

    socket_a.close(None).await.unwrap();
    socket_b.close(None).await.unwrap();
    let export = wait_for_export_event(router.clone(), "session_ended").await;
    let session_id = export["sessions"][0]["session_id"].as_i64().unwrap();
    let (_, ended_detail) = admin_json_request(
        router,
        http::Method::GET,
        &format!("/api/admin/sessions/{session_id}"),
        Value::Null,
    )
    .await;
    let outcomes = ended_detail["participants"]
        .as_array()
        .unwrap()
        .iter()
        .map(|participant| participant["terminal_result"]["outcome"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(outcomes.len(), 2);
    assert!(outcomes.iter().all(|outcome| *outcome == "connection_lost"));
    server.abort();
}
