//! Participant and administrator contracts across installation-managed runtime replacements.
use super::*;

/// Calls a JSON endpoint and includes the response when a mounted route fails.
async fn mounted_admin(router: &TestRouter, path: &str, body: Option<Value>) -> Value {
    let method = if body.is_some() {
        http::Method::POST
    } else {
        http::Method::GET
    };
    let (status, result) =
        admin_json_request(router.clone(), method, path, body.unwrap_or(Value::Null)).await;
    assert_eq!(status, StatusCode::OK, "{path}: {result}");
    result
}

/// Creates one scoped participant and accepts the current study consent.
async fn mounted_registration(router: &TestRouter) -> String {
    let (status, registered) = json_request(
        router.clone(),
        http::Method::POST,
        "/e/study/api/participants",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{registered}");
    let id = registered["participant_id"].as_str().unwrap().to_owned();
    router.remember(
        id.clone(),
        registered["participant_credential"]
            .as_str()
            .unwrap()
            .to_owned(),
    );
    let (status, body) = participant_json_request(
        router.clone(),
        &id,
        http::Method::POST,
        "/e/study/api/consent",
        json!({"decisions":{"study":true}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    id
}

/// Admits one new participant into the current room.
async fn mounted_seat(router: &TestRouter) -> (String, String) {
    let id = mounted_registration(router).await;
    let (status, room) = participant_json_request(
        router.clone(),
        &id,
        http::Method::POST,
        "/e/study/api/sessions",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{room}");
    (
        id,
        room["participant_state"]["public_session_id"]
            .as_str()
            .unwrap()
            .to_owned(),
    )
}

/// Issues a real one-use ticket for a participant in the mounted room.
async fn mounted_ticket(router: &TestRouter, base: &str, id: &str, room: &str) -> String {
    let (status, plan) = participant_json_request(
        router.clone(),
        id,
        http::Method::POST,
        &format!("/e/study/api/sessions/{room}/game-session"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{plan}");
    format!(
        "{}{}?token={}",
        base.replacen("http://", "ws://", 1),
        plan["websocket_url"].as_str().unwrap(),
        plan["token"].as_str().unwrap()
    )
}

/// Starts an authenticated installation backed by the supplied experiment template.
async fn mounted_router(config: ExperimentConfig) -> TestRouter {
    let descriptor = GameMetadata {
        id: "tiny-game".into(),
        name: "Tiny Game".into(),
        version: semver::Version::new(0, 4, 0),
        build_manifest: json!({}),
    };
    let router = TestRouter::new(
        build_game_router(live_game(), config, descriptor, |_| {
            Ok(ServeOptions::default())
        })
        .await
        .unwrap(),
    );
    authenticate_test_admin(router.clone()).await.unwrap();
    router
}

/// Exercises intake shutdown and config replacement while a real room disconnects and resumes.
#[tokio::test]
async fn mounted_room_reconnects_across_intake_and_config_changes() {
    let router = mounted_router(step_five_config()).await;
    mounted_admin(
        &router,
        "/api/admin/experiments",
        Some(json!({"experiment_id":"study"})),
    )
    .await;
    mounted_admin(
        &router,
        "/api/admin/experiments",
        Some(json!({"experiment_id":"other"})),
    )
    .await;
    let lifecycle = "/api/admin/runtime/study/experiment/status";
    mounted_admin(&router, lifecycle, Some(json!({"status":"active"}))).await;
    let unassigned = mounted_registration(&router).await;
    let (a, room) = mounted_seat(&router).await;
    let (b, same) = mounted_seat(&router).await;
    assert_eq!(same, room);
    let (base, server) = spawn_test_server(router.clone()).await;
    let (mut socket_a, _) = connect_async(mounted_ticket(&router, &base, &a, &room).await)
        .await
        .unwrap();
    let (mut socket_b, _) = connect_async(mounted_ticket(&router, &base, &b, &room).await)
        .await
        .unwrap();
    read_participant_state(&mut socket_b, "active").await;
    let reconnect = mounted_ticket(&router, &base, &a, &room).await;
    mounted_admin(&router, lifecycle, Some(json!({"status":"inactive"}))).await;
    socket_a.close(None).await.unwrap();
    read_participant_state(&mut socket_b, "paused").await;
    let definition = mounted_admin(&router, "/api/admin/experiments/study/config", None).await;
    let mut config = definition["experiment"]["config"].clone();
    config["session"]["waiting_session_timeout_seconds"] = json!(321);
    config["capacity"]["max_active_sessions"] = json!(1);
    let saved = mounted_admin(
        &router,
        "/api/admin/experiments/study/config",
        Some(json!({"expected_revision":1,"config":config})),
    )
    .await;
    assert_eq!(saved["revision"], 2);
    let (closed, _) = json_request(
        router.clone(),
        http::Method::POST,
        "/e/study/api/participants",
        json!({}),
    )
    .await;
    assert_ne!(closed, StatusCode::OK);
    let (status, snapshot) = participant_json_request(
        router.clone(),
        &a,
        http::Method::GET,
        "/e/study/api/participant-state",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(snapshot["participant_state"]["state"], "paused");
    let (wrong_scope, _) = participant_json_request(
        router.clone(),
        &a,
        http::Method::GET,
        "/e/other/api/participant-state",
        Value::Null,
    )
    .await;
    assert_eq!(wrong_scope, StatusCode::UNAUTHORIZED);
    mounted_admin(&router, lifecycle, Some(json!({"status":"active"}))).await;
    let (_, new_room) = mounted_seat(&router).await;
    assert_ne!(new_room, room);
    let (unconsented, _) = participant_json_request(
        router.clone(),
        &unassigned,
        http::Method::POST,
        "/e/study/api/sessions",
        json!({}),
    )
    .await;
    assert_eq!(unconsented, StatusCode::FORBIDDEN);
    let (status, _) = participant_json_request(
        router.clone(),
        &unassigned,
        http::Method::POST,
        "/e/study/api/consent",
        json!({"decisions":{"study":true}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (full, _) = participant_json_request(
        router.clone(),
        &unassigned,
        http::Method::POST,
        "/e/study/api/sessions",
        json!({}),
    )
    .await;
    assert_eq!(full, StatusCode::SERVICE_UNAVAILABLE);
    let load = mounted_admin(&router, "/api/admin/load", None).await;
    assert_eq!(load["current"]["capacity"]["active_reserved_sessions"], 1);
    assert_eq!(load["current"]["capacity"]["waiting_sessions"], 1);
    let rooms = load["sessions"].as_array().unwrap();
    for (public_id, revision) in [(&room, 1), (&new_room, 2)] {
        let row = rooms
            .iter()
            .find(|r| r["public_session_id"] == public_id.as_str())
            .unwrap();
        let detail = mounted_admin(
            &router,
            &format!("/api/admin/runtime/study/sessions/{}", row["session_id"]),
            None,
        )
        .await;
        assert_eq!(detail["session"]["config_revision"], revision, "{detail}");
        if revision == 1 {
            assert_eq!(
                detail["participants"][0]["participant_state"]["state"], "paused",
                "{detail}"
            );
        }
    }
    let (mut reconnected, _) = connect_async(&reconnect).await.unwrap();
    assert!(connect_async(&reconnect).await.is_err());
    read_participant_state(&mut reconnected, "active").await;
    send_ws_json(
        &mut reconnected,
        json!({"type":"action","action":{"finish":true}}),
    )
    .await;
    assert_eq!(
        read_participant_state(&mut socket_b, "ended").await["result"]["completion"]["done"],
        true
    );
    let (status, assigned) = participant_json_request(
        router.clone(),
        &unassigned,
        http::Method::POST,
        "/e/study/api/sessions",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{assigned}");
    assert_eq!(assigned["participant_state"]["public_session_id"], new_room);
    server.abort();
}

/// A repeated verified submission obtains a new credential for its original waiting room.
#[tokio::test]
async fn mounted_prolific_submission_keeps_its_room_after_reopening() {
    let router = mounted_router(synthetic_prolific_config()).await;
    mounted_admin(
        &router,
        "/api/admin/experiments",
        Some(json!({"experiment_id":"study"})),
    )
    .await;
    let lifecycle = "/api/admin/runtime/study/experiment/status";
    let open = json!({"status":"testing","participant_url_kind":"local"});
    let registration = json!({"prolific":{"participant_id":"TESTALPHA","session_id":"TESTSESSION","study_id":"prolific-study"}});
    mounted_admin(&router, lifecycle, Some(open.clone())).await;
    let (status, first) = json_request(
        router.clone(),
        http::Method::POST,
        "/e/study/api/participants",
        registration.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{first}");
    let id = first["participant_id"].as_str().unwrap();
    router.remember(
        id.to_owned(),
        first["participant_credential"].as_str().unwrap().to_owned(),
    );
    let (status, _) = participant_json_request(
        router.clone(),
        id,
        http::Method::POST,
        "/e/study/api/consent",
        json!({"decisions":{"study":true}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, room) = participant_json_request(
        router.clone(),
        id,
        http::Method::POST,
        "/e/study/api/sessions",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    mounted_admin(&router, lifecycle, Some(json!({"status":"inactive"}))).await;
    mounted_admin(&router, lifecycle, Some(open)).await;
    let (status, second) = json_request(
        router.clone(),
        http::Method::POST,
        "/e/study/api/participants",
        registration,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{second}");
    assert_eq!(first["participant_id"], second["participant_id"]);
    assert_ne!(
        first["participant_credential"],
        second["participant_credential"]
    );
    router.remember(
        id.to_owned(),
        second["participant_credential"]
            .as_str()
            .unwrap()
            .to_owned(),
    );
    let (status, same) = participant_json_request(
        router.clone(),
        id,
        http::Method::POST,
        "/e/study/api/sessions",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{same}");
    assert_eq!(
        same["participant_state"]["public_session_id"],
        room["participant_state"]["public_session_id"]
    );
    let load = mounted_admin(&router, "/api/admin/load", None).await;
    assert_eq!(load["current"]["capacity"]["waiting_sessions"], 1);
}

/// Unknown scopes share a bounded set of build slots and never instantiate a game runtime.
#[tokio::test]
async fn unknown_namespaces_share_fixed_mount_slots() {
    let router = mounted_router(step_five_config()).await;
    let mut slots = HashSet::new();
    for id in 0..(MOUNT_LOCK_COUNT * 2) {
        let name = format!("absent-{id}");
        slots.insert(mount_lock_slot(&name));
        let (status, _) = json_request(
            router.clone(),
            http::Method::GET,
            &format!("/e/{name}/api/config"),
            Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
    assert!(slots.len() <= MOUNT_LOCK_COUNT);
    let load = mounted_admin(&router, "/api/admin/load", None).await;
    assert_eq!(load["sessions"], json!([]));
}

/// A direct credential cannot substitute for a verified submission after a recruitment change.
#[tokio::test]
async fn reopened_prolific_intake_requires_new_registration_for_unassigned_direct_participant() {
    let router = mounted_router(step_five_config()).await;
    mounted_admin(
        &router,
        "/api/admin/experiments",
        Some(json!({"experiment_id":"study"})),
    )
    .await;
    let lifecycle = "/api/admin/runtime/study/experiment/status";
    let open = json!({"status":"testing","participant_url_kind":"local"});
    mounted_admin(&router, lifecycle, Some(open.clone())).await;
    let id = mounted_registration(&router).await;
    mounted_admin(&router, lifecycle, Some(json!({"status":"inactive"}))).await;
    let definition = mounted_admin(&router, "/api/admin/experiments/study/config", None).await;
    let mut config = definition["experiment"]["config"].clone();
    config["recruitment"]["prolific"] =
        serde_json::to_value(synthetic_prolific_config().recruitment.prolific).unwrap();
    mounted_admin(
        &router,
        "/api/admin/experiments/study/config",
        Some(json!({"expected_revision":1,"config":config})),
    )
    .await;
    mounted_admin(&router, lifecycle, Some(open)).await;
    let (status, _) = participant_json_request(
        router.clone(),
        &id,
        http::Method::POST,
        "/e/study/api/sessions",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = json_request(
        router.clone(),
        http::Method::POST,
        "/e/study/api/participants",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, verified) = json_request(router,http::Method::POST,"/e/study/api/participants",json!({"prolific":{"participant_id":"TESTBRAVO","session_id":"TESTSUBMISSION","study_id":"prolific-study"}})).await;
    assert_eq!(status, StatusCode::OK, "{verified}");
}
