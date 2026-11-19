use super::*;

#[tokio::test]
async fn direct_participant_creation_respects_direct_enabled_flag() {
    let mut config = step_five_config();
    config.direct.enabled = false;
    let router = build_router(TinyAdapter, config, ServeOptions::default())
        .await
        .unwrap();

    let (status, response) = json_request(
        router.clone(),
        http::Method::POST,
        "/api/participants",
        json!({}),
    )
    .await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(response["raw"], "Direct mode is disabled.");
}

#[tokio::test]
async fn public_participant_creation_rejects_external_identity_sources() {
    let router = build_router(TinyAdapter, step_five_config(), ServeOptions::default())
        .await
        .unwrap();
    let (status, response) = json_request(
        router.clone(),
        http::Method::POST,
        "/api/participants",
        json!({
            "source": "prolific",
            "external_id": "PROLIFIC-1",
            "metadata": {"cohort": "pilot"}
        }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(response["raw"].as_str().unwrap().contains("unknown field"));
}

#[tokio::test]
async fn direct_room_creation_requires_consent_then_assigns_role_a() {
    let router = build_router(TinyAdapter, step_five_config(), ServeOptions::default())
        .await
        .unwrap();
    let participant_session_id = create_direct_participant(router.clone(), "Direct Player").await;

    let (blocked_status, _blocked) = participant_json_request(
        router.clone(),
        &participant_session_id,
        http::Method::POST,
        "/api/sessions",
        json!({}),
    )
    .await;
    assert_eq!(blocked_status, StatusCode::FORBIDDEN);

    consent_participant(router.clone(), &participant_session_id).await;
    let (status, room) = participant_json_request(
        router,
        &participant_session_id,
        http::Method::POST,
        "/api/sessions",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(room.get("participant_session_id").is_none());
    assert_eq!(room["participant_state"]["role"], "A");
    assert!(room["participant_state"]["public_session_id"]
        .as_str()
        .is_some_and(|id| !id.is_empty()));
}

#[tokio::test]
async fn room_response_uses_null_when_game_does_not_enumerate_actions() {
    let router = build_router(
        NoAvailableActionsAdapter,
        step_five_config(),
        ServeOptions::default(),
    )
    .await
    .unwrap();
    let participant_session_id = create_direct_participant(router.clone(), "No Actions").await;
    consent_participant(router.clone(), &participant_session_id).await;

    let (status, room) = participant_json_request(
        router,
        &participant_session_id,
        http::Method::POST,
        "/api/sessions",
        json!({}),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(room["available_actions"].is_null());
}

#[tokio::test]
async fn required_consent_must_be_accepted_not_only_declared() {
    let router = build_router(TinyAdapter, step_five_config(), ServeOptions::default())
        .await
        .unwrap();
    let participant_session_id = create_direct_participant(router.clone(), "Nope").await;
    let (consent_status, _consent_response) = participant_json_request(
        router.clone(),
        &participant_session_id,
        http::Method::POST,
        "/api/consent",
        json!({

            "decisions": {"study": false}
        }),
    )
    .await;
    assert_eq!(consent_status, StatusCode::OK);

    let (room_status, response) = participant_json_request(
        router,
        &participant_session_id,
        http::Method::POST,
        "/api/sessions",
        json!({}),
    )
    .await;

    assert_eq!(room_status, StatusCode::FORBIDDEN);
    assert!(response["raw"]
        .as_str()
        .unwrap()
        .contains("Consent is required"));
}

#[tokio::test]
async fn two_independent_waiting_room_entries_pair_into_one_room() {
    let router = build_router(TinyAdapter, step_five_config(), ServeOptions::default())
        .await
        .unwrap();
    let first = create_direct_participant(router.clone(), "First tab").await;
    let second = create_direct_participant(router.clone(), "Second tab").await;
    consent_participant(router.clone(), &first).await;
    consent_participant(router.clone(), &second).await;

    let (first_status, first_room) = participant_json_request(
        router.clone(),
        &first,
        http::Method::POST,
        "/api/sessions",
        json!({}),
    )
    .await;
    let (second_status, second_room) = participant_json_request(
        router.clone(),
        &second,
        http::Method::POST,
        "/api/sessions",
        json!({}),
    )
    .await;

    assert_eq!(first_status, StatusCode::OK);
    assert_eq!(second_status, StatusCode::OK);
    assert_eq!(first_room["participant_state"]["role"], "A");
    assert_eq!(second_room["participant_state"]["role"], "B");
    assert_eq!(
        first_room["participant_state"]["public_session_id"],
        second_room["participant_state"]["public_session_id"]
    );
    assert!(first_room["participant_state"]["presence"]["A"]
        .get("participantSessionId")
        .is_none());
    assert!(first_room["participant_state"]["presence"]
        .get("B")
        .is_none());
    assert!(second_room["participant_state"]["presence"]["A"]
        .get("participantSessionId")
        .is_none());
    assert!(second_room["participant_state"]["presence"]["B"]
        .get("participantSessionId")
        .is_none());

    let (export_status, export) = admin_json_request(
        router,
        http::Method::GET,
        "/api/admin/export?variant=full",
        Value::Null,
    )
    .await;
    assert_eq!(export_status, StatusCode::OK);
    assert_eq!(export["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(export["session_participants"].as_array().unwrap().len(), 2);
    let roles = export["session_participants"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["role"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(roles.contains(&"A"));
    assert!(roles.contains(&"B"));
}

#[tokio::test]
async fn adversarial_room_routes_reject_caller_controlled_role_fields() {
    let router = build_router(TinyAdapter, step_five_config(), ServeOptions::default())
        .await
        .unwrap();
    let a = create_direct_participant(router.clone(), "A").await;
    consent_participant(router.clone(), &a).await;
    let (create_status, create_response) = participant_json_request(
        router.clone(),
        &a,
        http::Method::POST,
        "/api/sessions",
        json!({ "role": "B"}),
    )
    .await;
    assert_eq!(create_status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(create_response["raw"]
        .as_str()
        .unwrap()
        .contains("unknown field `role`"));
}

/// Confirms authenticated APIs reject the removed caller-controlled participant field.
#[tokio::test]
async fn authenticated_routes_reject_participant_identity_in_request_bodies() {
    let router = build_router(TinyAdapter, step_five_config(), ServeOptions::default())
        .await
        .unwrap();
    let participant_session_id = create_direct_participant(router.clone(), "A").await;
    let (status, _) = participant_json_request(
        router,
        &participant_session_id,
        http::Method::POST,
        "/api/consent",
        json!({"participant_session_id": participant_session_id, "decisions": {"study": true}}),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn room_routes_persist_evaluation_session_and_join_events() {
    let router = build_router(TinyAdapter, step_five_config(), ServeOptions::default())
        .await
        .unwrap();
    let a = create_direct_participant(router.clone(), "A").await;
    let b = create_direct_participant(router.clone(), "B").await;
    consent_participant(router.clone(), &a).await;
    consent_participant(router.clone(), &b).await;
    let (_, created) = participant_json_request(
        router.clone(),
        &a,
        http::Method::POST,
        "/api/sessions",
        json!({}),
    )
    .await;
    assert_eq!(created["participant_state"]["role"], "A");
    let public_session_id = created["participant_state"]["public_session_id"]
        .as_str()
        .unwrap()
        .to_string();
    let (_, joined) = participant_json_request(
        router.clone(),
        &b,
        http::Method::POST,
        "/api/sessions",
        json!({}),
    )
    .await;
    assert_eq!(joined["participant_state"]["role"], "B");
    assert_eq!(
        joined["participant_state"]["public_session_id"],
        public_session_id
    );

    let (export_status, export) = admin_json_request(
        router,
        http::Method::GET,
        "/api/admin/export?variant=full",
        Value::Null,
    )
    .await;
    assert_eq!(export_status, StatusCode::OK);
    assert_eq!(export["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(
        export["sessions"][0]["public_session_id"],
        public_session_id
    );
    assert_eq!(export["session_participants"].as_array().unwrap().len(), 2);
    let roles = export["session_participants"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["role"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(roles.contains(&"A"));
    assert!(roles.contains(&"B"));
    let event_types = export["session_events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| event["event_type"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        event_types,
        vec![
            "session_created",
            "participant_joined",
            "participant_joined"
        ]
    );
}

#[tokio::test]
async fn websocket_role_assignment_is_targeted_to_one_connection() {
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

    let (mut socket_b, _) =
        connect_async(game_socket_url(&router, &base_url, &public_session_id, &b).await)
            .await
            .unwrap();
    let assigned_a = read_participant_state(&mut socket_a, "active").await;
    assert_eq!(assigned_a["role"], "A");
    let assigned_b = read_participant_state(&mut socket_b, "active").await;
    assert_eq!(assigned_b["role"], "B");
    assert_no_ws_type(&mut socket_a, "participant_state").await;
    server.abort();
}

/// Confirms a durable terminal admission cannot enter a second session after runtime cleanup.
#[tokio::test]
async fn completed_admission_rejoins_recorded_terminal_session() {
    let mut config = step_five_config();
    config.session.reconnect_grace_seconds = 1;
    let router = build_router(TinyAdapter, config, ServeOptions::default())
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

    send_ws_json(
        &mut socket_a,
        json!({"type": "action", "action": {"finish": true}}),
    )
    .await;
    let _ = read_participant_state(&mut socket_a, "ended").await;
    let _ = read_participant_state(&mut socket_b, "ended").await;
    socket_a.close(None).await.unwrap();
    socket_b.close(None).await.unwrap();

    wait_for_completed_runtime_cleanup(router.clone()).await;

    let (status, replay) = participant_json_request(
        router.clone(),
        &a,
        http::Method::POST,
        "/api/sessions",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replay["participant_state"]["state"], "ended");
    assert_eq!(
        replay["participant_state"]["public_session_id"],
        public_session_id
    );

    let (_, export) = admin_json_request(
        router,
        http::Method::GET,
        "/api/admin/export?variant=full",
        Value::Null,
    )
    .await;
    assert_eq!(export["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(export["session_participants"].as_array().unwrap().len(), 2);
    server.abort();
}
