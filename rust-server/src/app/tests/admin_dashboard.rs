use super::*;

/// Confirms the protected dashboard shell and its separately embedded assets are servable.
#[tokio::test]
async fn admin_dashboard_serves_html_css_and_javascript_assets() {
    let router = build_router(live_game(), step_five_config(), ServeOptions::default())
        .await
        .unwrap();
    authenticate_test_admin(router.clone()).await.unwrap();

    let page = admin_raw_request(router.clone(), http::Method::GET, "/admin/experiments").await;
    assert_eq!(page.status(), StatusCode::OK);
    assert_eq!(
        page.headers().get(http::header::CONTENT_TYPE).unwrap(),
        "text/html; charset=utf-8"
    );
    assert!(page.headers().contains_key("content-security-policy"));
    let page_body = to_bytes(page.into_body(), usize::MAX).await.unwrap();
    let page_html = String::from_utf8(page_body.to_vec()).unwrap();
    assert!(page_html.contains("href=\"/admin/assets/admin-dashboard.css\""));
    assert!(page_html.contains("src=\"/admin/assets/admin-dashboard.js\""));
    assert!(page_html.contains("data-tab=\"progress\""));
    assert!(page_html.contains("id=\"progressPanel\""));
    assert!(page_html.contains("id=\"sessionEndChart\""));
    assert!(page_html.contains("id=\"icon-prolific\""));

    for (path, expected_content_type) in [
        ("/admin/assets/parlando-mark.svg", "image/svg+xml"),
        ("/admin/assets/parlando-wordmark.svg", "image/svg+xml"),
        ("/admin/assets/palette.css", "text/css; charset=utf-8"),
        (
            "/admin/assets/admin-dashboard.css",
            "text/css; charset=utf-8",
        ),
        (
            "/admin/assets/admin-dashboard.js",
            "text/javascript; charset=utf-8",
        ),
        (
            "/admin/assets/admin-dashboard-state.js",
            "text/javascript; charset=utf-8",
        ),
        (
            "/admin/assets/admin-dashboard-format.js",
            "text/javascript; charset=utf-8",
        ),
        (
            "/admin/assets/admin-dashboard-api.js",
            "text/javascript; charset=utf-8",
        ),
        (
            "/admin/assets/communication-config.js",
            "text/javascript; charset=utf-8",
        ),
    ] {
        let asset = admin_raw_request(router.clone(), http::Method::GET, path).await;
        assert_eq!(asset.status(), StatusCode::OK);
        assert_eq!(
            asset.headers().get(http::header::CONTENT_TYPE).unwrap(),
            expected_content_type
        );
        assert_eq!(
            asset.headers().get(http::header::CACHE_CONTROL).unwrap(),
            "no-store"
        );
        assert!(!to_bytes(asset.into_body(), usize::MAX)
            .await
            .unwrap()
            .is_empty());
    }
}

/// Confirms the protected load endpoint exposes bounded operational telemetry.
#[tokio::test]
async fn admin_load_exposes_capacity_counters_and_liveness_policy() {
    let (config, _tmp) = sqlite_config();
    let router = crate::app::build_router(live_game(), config, ServeOptions::default())
        .await
        .map(TestRouter::new)
        .unwrap();
    authenticate_test_admin(router.clone()).await.unwrap();
    let response = admin_raw_request(router, http::Method::GET, "/api/admin/load").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let load: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(load["sampling"]["interval_seconds"], 5);
    assert_eq!(load["sampling"]["retention_seconds"], 3_600);
    assert_eq!(load["liveness_thresholds_ms"]["live"], 5_000);
    assert_eq!(load["liveness_thresholds_ms"]["socket_timeout"], 90_000);
    assert_eq!(load["current"]["capacity"]["active_reserved_sessions"], 0);
    assert!(load["current"]["counters"]["requests_total"].is_number());
    assert!(load["history"].is_array());
    assert!(load["sessions"].is_array());
}

#[tokio::test]
async fn admin_sessions_api_reads_actions_from_database() {
    let (config, _temp) = sqlite_config();
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
    let _assigned_a = read_participant_state(&mut socket_a, "active").await;
    let _assigned_b = read_participant_state(&mut socket_b, "active").await;

    send_ws_json(
        &mut socket_a,
        json!({"type": "action", "action": {"finish": false}}),
    )
    .await;
    let _state_a = read_ws_type(&mut socket_a, "transition").await;
    let _state_b = read_ws_type(&mut socket_b, "transition").await;
    let (transcript_status, _transcript) = participant_json_request(
        router.clone(),
        &a,
        http::Method::POST,
        &format!("/api/sessions/{public_session_id}/transcripts"),
        json!({

            "player": "A",
            "start_time_ms": 10,
            "end_time_ms": 40,
            "text": "admin-visible transcript",
            "metadata": {"confidence": 0.95}
        }),
    )
    .await;
    assert_eq!(transcript_status, StatusCode::NOT_FOUND);

    let (sessions_status, sessions) = admin_json_request(
        router.clone(),
        http::Method::GET,
        "/api/admin/sessions",
        Value::Null,
    )
    .await;
    assert_eq!(sessions_status, StatusCode::OK);
    let session_id = sessions["sessions"][0]["session_id"].as_i64().unwrap();
    assert_eq!(sessions["sessions"][0]["lifecycle"], "running");
    assert!(sessions["sessions"][0].get("status").is_none());
    assert!(sessions["sessions"][0]["event_count"].as_i64().unwrap() >= 6);
    let (filter_status, filtered) = admin_json_request(
        router.clone(),
        http::Method::GET,
        "/api/admin/sessions?lifecycle=running",
        Value::Null,
    )
    .await;
    assert_eq!(filter_status, StatusCode::OK);
    assert_eq!(filtered["sessions"].as_array().unwrap().len(), 1);
    let (invalid_filter_status, _) = admin_json_request(
        router.clone(),
        http::Method::GET,
        "/api/admin/sessions?lifecycle=completed",
        Value::Null,
    )
    .await;
    assert_eq!(invalid_filter_status, StatusCode::BAD_REQUEST);

    let (search_status, search) = admin_json_request(
        router.clone(),
        http::Method::POST,
        "/api/admin/sessions/search?lifecycle=running",
        json!({"prolific_pid_suffix": "a21"}),
    )
    .await;
    assert_eq!(search_status, StatusCode::OK);
    assert_eq!(search["sessions"], json!([]));

    let (detail_status, detail) = admin_json_request(
        router.clone(),
        http::Method::GET,
        &format!("/api/admin/sessions/{session_id}"),
        Value::Null,
    )
    .await;
    assert_eq!(detail_status, StatusCode::OK);
    assert_eq!(detail["participants"].as_array().unwrap().len(), 2);
    assert!(detail["participants"]
        .as_array()
        .unwrap()
        .iter()
        .all(|participant| participant["participant_state"]["state"] == "active"));
    let events = detail["events"].as_array().unwrap();
    assert!(events.iter().any(|event| {
        event["event_type"] == "game_action_accepted"
            && event["text"].as_str().unwrap().contains("\"finish\":false")
    }));

    let after_action = events
        .iter()
        .find(|event| event["event_type"] == "game_action_accepted")
        .and_then(|event| event["event_index"].as_i64())
        .unwrap();
    let (progress_status, progress) = admin_json_request(
        router.clone(),
        http::Method::GET,
        "/api/admin/progress",
        Value::Null,
    )
    .await;
    assert_eq!(progress_status, StatusCode::OK);
    assert_eq!(progress["progress"]["total_sessions"], 1);
    assert_eq!(progress["progress"]["ended_sessions"], 0);
    assert_eq!(
        progress["progress"]["sessions"].as_array().unwrap().len(),
        1
    );
    assert_eq!(
        progress["progress"]["sessions"][0]["session_id"],
        session_id
    );
    assert!(progress["progress"].get("planned_sessions").is_none());
    let (poll_status, poll) = admin_json_request(
        router,
        http::Method::GET,
        &format!("/api/admin/sessions/{session_id}/events?after={after_action}"),
        Value::Null,
    )
    .await;
    assert_eq!(poll_status, StatusCode::OK);
    assert!(poll["events"].as_array().unwrap().is_empty());
    server.abort();
}
