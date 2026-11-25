use super::*;

/// Confirms the hosted server rejects route state in its configured public origin.
#[tokio::test]
async fn game_host_requires_an_origin_only_public_url() {
    let (mut config, _tmp) = sqlite_config();
    config.server.public_base_url = "https://games.example.test/deployment".to_string();
    let descriptor = GameMetadata {
        id: "tiny-game".to_string(),
        name: "Tiny Game".to_string(),
        version: semver::Version::parse("0.4.0").unwrap(),
        build_manifest: json!({}),
    };

    let result = super::build_game_router(live_game(), config, descriptor, |_| {
        Ok(ServeOptions::default())
    })
    .await;

    let error = match result {
        Ok(_) => panic!("a public URL path must be rejected"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("origin without a path or query"));
}

/// Confirms one compiled-game router can activate and isolate two experiment runtimes.
#[tokio::test]
async fn compiled_game_router_hosts_multiple_experiments() {
    let mut config = step_five_config();
    config.experiment.id = Some("primary".to_string());
    let client_dist = tempfile::tempdir().unwrap();
    fs::create_dir(client_dist.path().join("assets")).unwrap();
    fs::write(
        client_dist.path().join("index.html"),
        "<html><body>participant client</body></html>",
    )
    .unwrap();
    fs::write(
        client_dist.path().join("assets/app.js"),
        "console.log('participant asset');",
    )
    .unwrap();
    config.server.client_dist_path = Some(client_dist.path().display().to_string());
    let descriptor = GameMetadata {
        id: "tiny-game".to_string(),
        name: "Tiny Game".to_string(),
        version: semver::Version::parse("0.4.0").unwrap(),
        build_manifest: json!({"name": "tiny-game", "version": "0.4.0"}),
    };
    let router = build_game_router(live_game(), config.clone(), descriptor.clone(), |_config| {
        Ok(ServeOptions::default())
    })
    .await
    .map(TestRouter::new)
    .unwrap();

    let root = router
        .clone()
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert!(root.status().is_redirection());
    assert_eq!(
        root.headers()
            .get(http::header::LOCATION)
            .and_then(|value| value.to_str().ok()),
        Some("/admin/experiments")
    );
    let participant_root = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/e/primary")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(participant_root.status(), StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(
        participant_root
            .headers()
            .get(http::header::LOCATION)
            .and_then(|value| value.to_str().ok()),
        Some("/e/primary/")
    );
    let unauthenticated_dashboard = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/admin/experiments")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(unauthenticated_dashboard.status().is_redirection());
    assert_eq!(
        unauthenticated_dashboard
            .headers()
            .get(http::header::LOCATION)
            .and_then(|value| value.to_str().ok()),
        Some("/admin/login")
    );
    let unauthenticated_api = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/admin/experiments")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauthenticated_api.status(), StatusCode::UNAUTHORIZED);
    authenticate_test_admin(router.clone()).await.unwrap();

    let (_, empty_catalogue) = admin_json_request(
        router.clone(),
        http::Method::GET,
        "/api/admin/experiments",
        Value::Null,
    )
    .await;
    assert!(empty_catalogue["experiments"]
        .as_array()
        .unwrap()
        .is_empty());

    let (create_status, created) = admin_json_request(
        router.clone(),
        http::Method::POST,
        "/api/admin/experiments",
        json!({"experiment_id": "primary", "notes": "# Primary\n\nPrivate catalogue note"}),
    )
    .await;
    assert_eq!(create_status, StatusCode::OK);
    assert_eq!(created["experiment_id"], "primary");
    for audience in ["participants", "data-protection", "conductors", "imprint"] {
        let path = format!("/e/primary/information/{audience}/");
        assert!(created["information_sites"][audience]
            .as_str()
            .unwrap()
            .ends_with(&path));
        let (status, body, content_type) =
            raw_request(router.clone(), http::Method::GET, &path, Body::empty()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(content_type.as_deref(), Some("text/html; charset=utf-8"));
        if matches!(audience, "participants" | "conductors" | "data-protection") {
            assert!(body.contains("Information for "));
            if audience == "data-protection" {
                assert!(!body.contains("Agree?"));
                assert!(body.contains("View the full informed consent form"));
                assert!(body.contains("/e/primary/information/participants/"));
            } else {
                assert!(body.contains("Agree?"));
            }
        } else {
            assert!(body.contains("<body></body>"));
        }
        let download_path =
            format!("/api/admin/experiments/primary/information/{audience}/download");
        let (unauthorized, _, _) = raw_request(
            router.clone(),
            http::Method::GET,
            &download_path,
            Body::empty(),
        )
        .await;
        assert_eq!(unauthorized, StatusCode::UNAUTHORIZED);
        let cookie = router
            .admin
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .cookie
            .clone();
        let download = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(&download_path)
                    .header(http::header::COOKIE, cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(download.status(), StatusCode::OK);
        assert_eq!(
            download.headers()[http::header::CONTENT_TYPE],
            "text/markdown; charset=utf-8"
        );
        assert_eq!(
            download.headers()[http::header::CONTENT_DISPOSITION],
            format!("attachment; filename=\"primary-{audience}.md\"")
        );
        let downloaded = to_bytes(download.into_body(), usize::MAX).await.unwrap();
        let markdown = String::from_utf8(downloaded.to_vec()).unwrap();
        assert!(!markdown.contains("<html"));
        if audience == "imprint" {
            assert!(markdown.starts_with("# Imprint"));
        } else {
            assert!(markdown.starts_with("# Information for "));
            if audience == "data-protection" {
                assert!(markdown.contains("[View the full informed consent form](<"));
                assert!(!markdown.contains("Agree?"));
            } else {
                assert!(markdown.contains("Agree?"));
            }
        }

        let (status, body, _) =
            raw_request(router.clone(), http::Method::HEAD, &path, Body::empty()).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.is_empty());
        let (status, _, _) =
            raw_request(router.clone(), http::Method::POST, &path, Body::empty()).await;
        assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    }
    for path in [
        "/e/missing/information/participants/",
        "/e/primary/information/unknown/",
    ] {
        let (status, _, _) =
            raw_request(router.clone(), http::Method::GET, path, Body::empty()).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    let (shell_status, shell_body, _) = raw_request(
        router.clone(),
        http::Method::GET,
        "/e/primary/",
        Body::empty(),
    )
    .await;
    assert_eq!(shell_status, StatusCode::OK);
    assert!(shell_body.contains("participant client"));
    let (asset_status, asset_body, _) = raw_request(
        router.clone(),
        http::Method::GET,
        "/e/primary/assets/app.js",
        Body::empty(),
    )
    .await;
    assert_eq!(asset_status, StatusCode::OK);
    assert_eq!(asset_body, "console.log('participant asset');");
    let unknown_participant_path = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/e/primary/not-a-supported-route")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unknown_participant_path.status(), StatusCode::NOT_FOUND);

    let (catalogue_status, catalogue) = admin_json_request(
        router.clone(),
        http::Method::GET,
        "/api/admin/experiments",
        Value::Null,
    )
    .await;
    assert_eq!(catalogue_status, StatusCode::OK);
    assert_eq!(
        catalogue["experiments"][0]["notes"],
        "# Primary\n\nPrivate catalogue note"
    );
    assert_eq!(catalogue["game"]["name"], "Tiny Game");
    assert_eq!(catalogue["experiments"][0]["configuration_valid"], true);
    assert_eq!(catalogue["experiments"][0]["runnable"], true);

    let (clone_status, _) = admin_json_request(
        router.clone(),
        http::Method::POST,
        "/api/admin/experiments/primary/clone",
        json!({"experiment_id": "primary-clone"}),
    )
    .await;
    assert_eq!(clone_status, StatusCode::OK);
    let (clone_definition_status, clone_definition) = admin_json_request(
        router.clone(),
        http::Method::GET,
        "/api/admin/experiments/primary-clone/config",
        Value::Null,
    )
    .await;
    assert_eq!(clone_definition_status, StatusCode::OK);
    assert!(clone_definition["experiment"]["notes"].is_null());

    let (settings_status, _) = admin_json_request(
        router.clone(),
        http::Method::POST,
        "/api/admin/game/settings",
        json!({"expected_revision": 1, "institution": "Test University", "admin_allowed_ip_ranges": []}),
    )
    .await;
    assert_eq!(settings_status, StatusCode::OK);

    let (create_status, created) = admin_json_request(
        router.clone(),
        http::Method::POST,
        "/api/admin/experiments",
        json!({"experiment_id": "secondary"}),
    )
    .await;
    assert_eq!(create_status, StatusCode::OK);
    assert_eq!(created["game_version"], "0.4.0");
    let (config_status, mut stored_config) = admin_json_request(
        router.clone(),
        http::Method::GET,
        "/api/admin/experiments/secondary/config",
        Value::Null,
    )
    .await;
    assert_eq!(config_status, StatusCode::OK);
    stored_config["experiment"]["config"]["session"]["waiting_session_timeout_seconds"] =
        Value::Number(300.into());
    stored_config["experiment"]["config"]["direct"]["imprint"] = json!({
        "country": "DE", "operator": "Institute", "text": "Institute & team\n<script>literal</script>"
    });
    let (save_status, saved) = admin_json_request(
        router.clone(),
        http::Method::POST,
        "/api/admin/experiments/secondary/config",
        json!({
            "expected_revision": stored_config["experiment"]["config_revision"],
            "config": stored_config["experiment"]["config"],
            "change_summary": "Edited in the dashboard"
        }),
    )
    .await;
    assert_eq!(save_status, StatusCode::OK, "config save failed: {saved}");
    assert_eq!(saved["revision"], 2);

    // A saved imprint is public before activation and isolated from the other study.
    let (status, public_imprint, _) = raw_request(
        router.clone(),
        http::Method::GET,
        "/e/secondary/information/imprint/",
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(public_imprint.contains("Institute &amp; team\n&lt;script&gt;literal&lt;/script&gt;"));
    let (_, primary_imprint, _) = raw_request(
        router.clone(),
        http::Method::GET,
        "/e/primary/information/imprint/",
        Body::empty(),
    )
    .await;
    assert!(!primary_imprint.contains("Institute &amp; team"));
    let cookie = router
        .admin
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .cookie
        .clone();
    let downloaded = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/admin/experiments/secondary/information/imprint/download")
                .header(http::header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(downloaded.status(), StatusCode::OK);
    let bytes = to_bytes(downloaded.into_body(), usize::MAX).await.unwrap();
    assert_eq!(String::from_utf8(bytes.to_vec()).unwrap(), "# Imprint\n\nInstitute &amp; team\n&lt;script&gt;literal&lt;/script&gt;\n");
    let (status, body, _) = raw_request(
        router.clone(),
        http::Method::HEAD,
        "/e/secondary/information/imprint/",
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.is_empty());

    for (experiment_id, lifecycle) in [("primary", "active"), ("secondary", "testing")] {
        let (status, body) = admin_json_request(
            router.clone(),
            http::Method::POST,
            &format!("/api/admin/runtime/{experiment_id}/experiment/status"),
            json!({"status": lifecycle}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "activation failed: {body}");
    }
    let (active_config_status, active_config) = admin_json_request(
        router.clone(),
        http::Method::GET,
        "/api/admin/experiments/primary/config",
        Value::Null,
    )
    .await;
    assert_eq!(active_config_status, StatusCode::OK);
    let (active_save_status, active_save) = admin_json_request(
        router.clone(),
        http::Method::POST,
        "/api/admin/experiments/primary/config",
        json!({
            "expected_revision": active_config["experiment"]["config_revision"],
            "config": active_config["experiment"]["config"],
            "change_summary": "Must be rejected while active"
        }),
    )
    .await;
    assert_eq!(active_save_status, StatusCode::CONFLICT);
    assert!(active_save["raw"]
        .as_str()
        .unwrap()
        .contains("only be edited while the experiment is inactive"));

    let (_, primary_participant) = json_request(
        router.clone(),
        http::Method::POST,
        "/e/primary/api/participants",
        json!({}),
    )
    .await;
    let (_, secondary_participant) = json_request(
        router.clone(),
        http::Method::POST,
        "/e/secondary/api/participants",
        json!({}),
    )
    .await;
    assert_ne!(
        primary_participant["participant_id"],
        secondary_participant["participant_id"]
    );
    for participant in [&primary_participant, &secondary_participant] {
        router.remember(
            participant["participant_id"].as_str().unwrap().to_string(),
            participant["participant_credential"]
                .as_str()
                .unwrap()
                .to_string(),
        );
    }
    let primary_id = primary_participant["participant_id"].as_str().unwrap();
    let (consent_status, _) = participant_json_request(
        router.clone(),
        primary_id,
        http::Method::POST,
        "/e/primary/api/consent",
        json!({ "decisions": {"study": true}}),
    )
    .await;
    assert_eq!(consent_status, StatusCode::OK);
    let (room_status, room) = participant_json_request(
        router.clone(),
        primary_id,
        http::Method::POST,
        "/e/primary/api/sessions",
        json!({}),
    )
    .await;
    assert_eq!(room_status, StatusCode::OK);
    let public_session_id = room["participant_state"]["public_session_id"]
        .as_str()
        .unwrap();
    let (session_status, session) = participant_json_request(
        router.clone(),
        primary_id,
        http::Method::POST,
        &format!("/e/primary/api/sessions/{public_session_id}/game-session"),
        json!({}),
    )
    .await;
    assert_eq!(session_status, StatusCode::OK);
    assert_eq!(
        session["websocket_url"],
        format!("/e/primary/ws/game/{public_session_id}")
    );
    let game_ticket = session["token"].as_str().unwrap();
    let (base_url, server) = spawn_test_server(router.clone()).await;
    let host = base_url.trim_start_matches("http://");
    let (mut game_socket, _) = connect_async(format!(
        "ws://{host}/e/primary/ws/game/{public_session_id}?token={game_ticket}"
    ))
    .await
    .expect("experiment dispatcher must preserve the WebSocket upgrade handle");
    let presence = read_ws_type(&mut game_socket, "presence").await;
    assert_eq!(presence["presence"]["A"]["connected"], true);
    game_socket.close(None).await.unwrap();
    server.abort();
    let (_, process_load) = admin_json_request(
        router.clone(),
        http::Method::GET,
        "/api/admin/load",
        Value::Null,
    )
    .await;
    assert_eq!(process_load["current"]["capacity"]["waiting_sessions"], 1);
    assert!(process_load["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["experiment_id"] == "primary"));
    let (_, secondary_config) = json_request(
        router.clone(),
        http::Method::GET,
        "/e/secondary/api/config",
        Value::Null,
    )
    .await;
    assert_eq!(secondary_config["institution"], "Test University");
    assert_eq!(secondary_config["experiment_status"], "testing");
    assert!(secondary_config.get("participant_title").is_none());

    let (_, catalogue) = admin_json_request(
        router.clone(),
        http::Method::GET,
        "/api/admin/experiments",
        Value::Null,
    )
    .await;
    assert_eq!(catalogue["game"]["name"], "Tiny Game");
    assert_eq!(catalogue["experiments"].as_array().unwrap().len(), 3);
    let (_, privacy) = admin_json_request(
        router.clone(),
        http::Method::GET,
        "/api/admin/privacy",
        Value::Null,
    )
    .await;
    assert!(privacy.get("experiment_count").is_none());

    let restarted = build_game_router(live_game(), config, descriptor, |_config| {
        Ok(ServeOptions::default())
    })
    .await
    .map(TestRouter::new)
    .unwrap();
    authenticate_test_admin(restarted.clone()).await.unwrap();
    let (_, restarted_catalogue) = admin_json_request(
        restarted,
        http::Method::GET,
        "/api/admin/experiments",
        Value::Null,
    )
    .await;
    assert!(restarted_catalogue["experiments"]
        .as_array()
        .unwrap()
        .iter()
        .all(|experiment| experiment["status"] == "inactive"));
}

/// Checks that an existing experiment can start and edit its settings after a game update.
#[tokio::test]
async fn compiled_game_router_reuses_experiments_after_a_game_update() {
    let (mut config, _tmp) = sqlite_config();
    config.experiment.id = Some("current".to_string());
    let store = experiment_store_from_url(&config.database.url)
        .await
        .unwrap();
    let mut old_config = config.clone();
    old_config.experiment.id = Some("historical".to_string());
    store
        .create_experiment(ExperimentRecord {
            experiment_id: "historical".to_string(),
            game_version: "0.3.0".to_string(),
            config: persistable_config_json(&old_config).unwrap(),
            server_version: Some("0.2.0".to_string()),
            version_manifest: Some(json!({"game": "tiny-game", "version": "0.3.0"})),
            status: "inactive".to_string(),
            notes: None,
        })
        .await
        .unwrap();
    let descriptor = GameMetadata {
        id: "tiny-game".to_string(),
        name: "Tiny Game".to_string(),
        version: semver::Version::parse("0.4.0").unwrap(),
        build_manifest: json!({"name": "tiny-game", "version": "0.4.0"}),
    };
    let router = build_game_router(live_game(), config.clone(), descriptor.clone(), |_config| {
        Ok(ServeOptions::default())
    })
    .await
    .map(TestRouter::new)
    .unwrap();
    authenticate_test_admin(router.clone()).await.unwrap();

    let (_, catalogue) = admin_json_request(
        router.clone(),
        http::Method::GET,
        "/api/admin/experiments",
        Value::Null,
    )
    .await;
    let historical = catalogue["experiments"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["experiment_id"] == "historical")
        .unwrap();
    assert_eq!(historical["local_testing_runnable"], true);
    for lifecycle in ["testing", "inactive"] {
        let (status, body) = admin_json_request(
            router.clone(),
            http::Method::POST,
            "/api/admin/runtime/historical/experiment/status",
            json!({"status": lifecycle}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    let (status, mut loaded) = admin_json_request(
        router.clone(),
        http::Method::GET,
        "/api/admin/experiments/historical/config",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    loaded["experiment"]["config"]["session"]["waiting_session_timeout_seconds"] = json!(300);
    let (status, saved) = admin_json_request(
        router.clone(),
        http::Method::POST,
        "/api/admin/experiments/historical/config",
        json!({"expected_revision": 1, "config": loaded["experiment"]["config"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["revision"], 2);

    let (clone_status, clone) = admin_json_request(
        router.clone(),
        http::Method::POST,
        "/api/admin/experiments/historical/clone",
        json!({"experiment_id": "historical-on-0.4"}),
    )
    .await;
    assert_eq!(clone_status, StatusCode::OK, "clone failed: {clone}");
    assert_eq!(clone["game_version"], "0.4.0");
    let (activation_status, _) = admin_json_request(
        router,
        http::Method::POST,
        "/api/admin/runtime/historical-on-0.4/experiment/status",
        json!({"status": "active"}),
    )
    .await;
    assert_eq!(activation_status, StatusCode::OK);

    let restarted = build_game_router(live_game(), config, descriptor, |_config| {
        Ok(ServeOptions::default())
    })
    .await
    .map(TestRouter::new)
    .unwrap();
    authenticate_test_admin(restarted.clone()).await.unwrap();
    let (catalogue_status, catalogue) = admin_json_request(
        restarted,
        http::Method::GET,
        "/api/admin/experiments",
        Value::Null,
    )
    .await;
    assert_eq!(catalogue_status, StatusCode::OK);
    assert!(catalogue.get("running_experiment_id").is_none());
    let experiments = catalogue["experiments"].as_array().unwrap();
    assert_eq!(
        experiments
            .iter()
            .find(|experiment| experiment["experiment_id"] == "historical-on-0.4")
            .unwrap()["status"],
        "inactive"
    );
    assert_eq!(
        experiments
            .iter()
            .find(|experiment| experiment["experiment_id"] == "historical")
            .unwrap()["status"],
        "inactive"
    );
}

#[tokio::test]
async fn reusable_router_builds_with_typed_adapter() {
    let mut config = ExperimentConfig::default();
    config.database.url = "sqlite:///:memory:".to_string();
    let _router = build_router(
        live_game(),
        config,
        ServeOptions {
            agent_factory: None,
            ..ServeOptions::default()
        },
    )
    .await
    .expect("router builds with a typed adapter");
}

#[tokio::test]
async fn health_and_public_config_expose_client_bootstrap_shape() {
    let mut config = step_five_config();
    config.communication.mode = CommunicationMode::Speech;
    config.transcription.enabled = true;
    config.speechmatics.api_key = "test-key".to_string();
    config.tts.enabled = true;
    config.tts.voice_id = "voice-1".to_string();
    config.tts.api_key = "tts-secret".to_string();
    config.tts.voice_name = "Agent Voice".to_string();
    config.recruitment.prolific.enabled = false;
    config.recruitment.prolific.study_id = "TEST-STUDY".to_string();
    config.recruitment.prolific.completion_paths.completed = "CLEARWREN".to_string();
    config.recruitment.prolific.completion_paths.partner_left = "AMBERBADGER".to_string();
    config
        .recruitment
        .prolific
        .completion_paths
        .game_did_not_start = "MINTFALCON".to_string();
    config
        .recruitment
        .prolific
        .completion_paths
        .participation_ended_early = "CEDAROTTER".to_string();
    config
        .recruitment
        .prolific
        .completion_paths
        .technical_failure = "SILVERHERON".to_string();
    config.recruitment.prolific.completion_paths.no_consent = "NOCONSENT".to_string();
    let router = build_router(live_game(), config, ServeOptions::default())
        .await
        .unwrap();

    let (health_status, health) =
        json_request(router.clone(), http::Method::GET, "/health", Value::Null).await;
    assert_eq!(health_status, StatusCode::OK);
    assert_eq!(health["status"], "ok");
    assert_eq!(health["storage"], "read_write");

    let (config_status, public_config) =
        json_request(router, http::Method::GET, "/api/config", Value::Null).await;
    assert_eq!(config_status, StatusCode::OK);
    assert_eq!(public_config["game_name"], "Embedded game");
    assert!(public_config.get("participant_title").is_none());
    assert!(public_config["institution"].is_null());
    assert_eq!(public_config["consents"][0]["id"], "study");
    assert_eq!(public_config["communication"]["mode"], "speech");
    assert!(public_config.get("voice").is_none());
    assert_eq!(public_config["recruitment"]["provider"], "direct");
    assert!(public_config.get("transcription").is_none());
    assert!(public_config.get("tts").is_none());
    assert!(public_config.get("agents").is_none());
    assert!(public_config.get("privacy").is_none());
}

#[tokio::test]
async fn static_serving_exposes_only_the_shell_assets_and_named_api_routes() {
    let temp = tempfile::tempdir().unwrap();
    let dist = temp.path().join("dist");
    fs::create_dir_all(dist.join("assets")).unwrap();
    fs::write(dist.join("index.html"), "<main>Parlando client</main>").unwrap();
    fs::write(dist.join("assets/app.js"), "console.log('asset');").unwrap();
    fs::write(temp.path().join("secret.txt"), "do not serve me").unwrap();

    let mut config = step_five_config();
    config.server.client_dist_path = Some(dist.display().to_string());
    let router = build_router(live_game(), config, ServeOptions::default())
        .await
        .unwrap();

    let (root_status, root_body, root_type) =
        raw_request(router.clone(), http::Method::GET, "/", Body::empty()).await;
    assert_eq!(root_status, StatusCode::OK);
    assert!(root_body.contains("Parlando client"));
    assert!(root_type
        .as_deref()
        .is_some_and(|value| value.contains("text/html")));

    let (asset_status, asset_body, asset_type) = raw_request(
        router.clone(),
        http::Method::GET,
        "/assets/app.js",
        Body::empty(),
    )
    .await;
    assert_eq!(asset_status, StatusCode::OK);
    assert_eq!(asset_body, "console.log('asset');");
    assert!(asset_type
        .as_deref()
        .is_some_and(|value| value.contains("javascript")));

    let (unknown_status, unknown_body, _) = raw_request(
        router.clone(),
        http::Method::GET,
        "/room/abc",
        Body::empty(),
    )
    .await;
    assert_eq!(unknown_status, StatusCode::NOT_FOUND);
    assert!(!unknown_body.contains("Parlando client"));

    let (api_status, api_body, _) = raw_request(
        router.clone(),
        http::Method::GET,
        "/api/config",
        Body::empty(),
    )
    .await;
    assert_eq!(api_status, StatusCode::OK);
    assert!(api_body.contains("experiment_status"));
    assert!(!api_body.contains("participant_title"));
    assert!(!api_body.contains("Parlando client"));

    let (_traversal_status, traversal_body, _) = raw_request(
        router,
        http::Method::GET,
        "/assets/../secret.txt",
        Body::empty(),
    )
    .await;
    assert!(!traversal_body.contains("do not serve me"));
}
