use super::*;

/// Checks the completion-code prerequisites for a Prolific-configured study.
#[test]
fn prolific_local_preview_requires_all_completion_paths() {
    let mut config = ExperimentConfig::default();
    config.recruitment.prolific.enabled = true;
    config.recruitment.prolific.completion_paths.completed = "COMPLETE".to_string();

    assert_eq!(
        prolific_completion_path_issues(&config),
        vec!["Configure all six Prolific completion paths before starting this experiment."]
    );

    let paths = &mut config.recruitment.prolific.completion_paths;
    paths.partner_left = "PARTNER".to_string();
    paths.game_did_not_start = "NOSTART".to_string();
    paths.participation_ended_early = "EARLY".to_string();
    paths.technical_failure = "TECHNICAL".to_string();
    paths.no_consent = "NOCONSENT".to_string();
    assert!(prolific_completion_path_issues(&config).is_empty());
}

/// Confirms Prolific accepts only the exact setup template generated for one experiment.
#[test]
fn prolific_external_study_url_contract_is_exact() {
    let participant_base = experiment_public_base_url("https://games.example.test", "study");
    let expected = prolific_setup_url(&participant_base);

    assert_eq!(
        expected,
        "https://games.example.test/e/study/?PROLIFIC_PID={{%PROLIFIC_PID%}}&STUDY_ID={{%STUDY_ID%}}&SESSION_ID={{%SESSION_ID%}}"
    );
    for rejected in [
        "https://games.example.test/e/other/?PROLIFIC_PID={{%PROLIFIC_PID%}}&STUDY_ID={{%STUDY_ID%}}&SESSION_ID={{%SESSION_ID%}}",
        "https://games.example.test/e/study/child?PROLIFIC_PID={{%PROLIFIC_PID%}}&STUDY_ID={{%STUDY_ID%}}&SESSION_ID={{%SESSION_ID%}}",
        "https://games.example.test/e/study/#PROLIFIC_PID={{%PROLIFIC_PID%}}&STUDY_ID={{%STUDY_ID%}}&SESSION_ID={{%SESSION_ID%}}",
        "https://games.example.test/e/study/?note=PROLIFIC_PID-STUDY_ID-SESSION_ID",
        "https://games.example.test/e/study/?PROLIFIC_PID=constant&STUDY_ID={{%STUDY_ID%}}&SESSION_ID=constant",
        "https://games.example.test/e/study/?STUDY_ID={{%STUDY_ID%}}&PROLIFIC_PID={{%PROLIFIC_PID%}}&SESSION_ID={{%SESSION_ID%}}",
        "https://games.example.test/e/study/?PROLIFIC_PID={{%PROLIFIC_PID%}}&STUDY_ID={{%STUDY_ID%}}&SESSION_ID={{%SESSION_ID%}}&extra=value",
    ] {
        assert_ne!(rejected, expected);
    }
}

/// Confirms every stored experiment receives the same public route before readiness checks.
#[test]
fn stored_experiment_hydration_scopes_the_public_base() {
    let bootstrap = ExperimentConfig::default();
    let stored = serde_json::to_value(ExperimentConfig::default()).unwrap();

    let hydrated = experiment_config_from_json_unvalidated(stored, &bootstrap, "pilot").unwrap();

    assert_eq!(
        hydrated.server.public_base_url,
        "http://localhost:8000/e/pilot"
    );
    assert_eq!(
        prolific_setup_url(&hydrated.server.public_base_url),
        "http://localhost:8000/e/pilot/?PROLIFIC_PID={{%PROLIFIC_PID%}}&STUDY_ID={{%STUDY_ID%}}&SESSION_ID={{%SESSION_ID%}}"
    );
    assert_eq!(
        experiment_public_base_url(&hydrated.server.public_base_url, "pilot"),
        hydrated.server.public_base_url
    );
}

/// Confirms paid Prolific intake cannot use remote cleartext participant URLs.
#[test]
fn prolific_readiness_requires_https_outside_loopback() {
    let mut config = ExperimentConfig::default();
    config.recruitment.prolific.enabled = true;
    config.recruitment.prolific.study_id = "study".to_string();
    config.recruitment.prolific.api_token = "token".to_string();
    config.server.public_base_url = "http://games.example.test/e/study".to_string();

    assert!(prolific_local_activation_issues(&config)
        .iter()
        .any(|issue| issue.contains("must use https")));
}

/// Confirms completion routing uses session-start context for otherwise identical connection loss.
#[test]
fn prolific_handoff_distinguishes_prestart_and_running_connection_loss() {
    let mut config = ExperimentConfig::default();
    let paths = &mut config.recruitment.prolific.completion_paths;
    paths.completed = "COMPLETE".to_string();
    paths.partner_left = "PARTNER".to_string();
    paths.game_did_not_start = "NOSTART".to_string();
    paths.participation_ended_early = "EARLY".to_string();
    paths.technical_failure = "TECHNICAL".to_string();
    paths.no_consent = "NOCONSENT".to_string();

    let before_start = prolific_handoff(&config, &ParticipantOutcomeKind::ConnectionLost, false)
        .expect("pre-start connection loss has a configured handoff");
    let after_start = prolific_handoff(&config, &ParticipantOutcomeKind::ConnectionLost, true)
        .expect("running connection loss has a configured handoff");

    assert_eq!(before_start.code, "NOSTART");
    assert_eq!(after_start.code, "EARLY");
}

/// Confirms every durable participant result resolves to its designated configured code.
#[test]
fn prolific_handoff_uses_the_designated_six_path_codes() {
    let mut config = ExperimentConfig::default();
    let paths = &mut config.recruitment.prolific.completion_paths;
    paths.completed = "COMPLETE".to_string();
    paths.partner_left = "PARTNER".to_string();
    paths.game_did_not_start = "NOSTART".to_string();
    paths.participation_ended_early = "EARLY".to_string();
    paths.technical_failure = "TECHNICAL".to_string();
    paths.no_consent = "NOCONSENT".to_string();

    for (outcome, game_started, expected) in [
        (ParticipantOutcomeKind::Completed, true, "COMPLETE"),
        (ParticipantOutcomeKind::PartnerLeft, false, "PARTNER"),
        (ParticipantOutcomeKind::LeftWaitingRoom, false, "NOSTART"),
        (ParticipantOutcomeKind::PartnerUnavailable, false, "NOSTART"),
        (ParticipantOutcomeKind::ConnectionLost, false, "NOSTART"),
        (ParticipantOutcomeKind::LeftGame, true, "EARLY"),
        (ParticipantOutcomeKind::ConnectionLost, true, "EARLY"),
        (ParticipantOutcomeKind::IdleLimitReached, true, "EARLY"),
        (ParticipantOutcomeKind::TechnicalFailure, false, "TECHNICAL"),
        (
            ParticipantOutcomeKind::LifetimeLimitReached,
            true,
            "TECHNICAL",
        ),
    ] {
        let handoff = prolific_handoff(&config, &outcome, game_started)
            .expect("configured outcome has a handoff");
        assert_eq!(handoff.code, expected, "unexpected code for {outcome:?}");
        assert_eq!(
            handoff.url,
            format!("https://app.prolific.com/submissions/complete?cc={expected}")
        );
    }
}

/// Confirms participant readiness, rather than draft parsing, requires the six completion codes.
#[test]
fn prolific_readiness_rejects_an_incomplete_six_path_draft() {
    let mut config = ExperimentConfig::default();
    config.database.url = "sqlite:///:memory:".to_string();
    config.recruitment.prolific.enabled = true;
    config.recruitment.prolific.study_id = "study".to_string();
    config.recruitment.prolific.api_token = "token".to_string();

    assert!(config.validate().is_ok());
    assert!(prolific_completion_path_issues(&config)
        .iter()
        .any(|issue| issue.contains("all six")));
}

/// Confirms only a Prolific launch receives a configured no-consent return URL.
#[test]
fn prolific_no_consent_url_requires_a_prolific_launch() {
    let mut config = ExperimentConfig::default();
    config.direct.consents.push(ConsentItemConfig {
        id: "study".to_string(),
        title: "Study consent".to_string(),
        body: "I agree.".to_string(),
        required: true,
        editor_data: None,
    });
    config.recruitment.prolific.completion_paths.no_consent = "NOCONSENT".to_string();
    let local = RunningParticipantUrl {
        kind: "local".to_string(),
        participant_id: Some("TESTPARTICIPANT".to_string()),
        session_id: Some("TESTSUBMISSION".to_string()),
    };
    let prolific = RunningParticipantUrl {
        kind: "prolific".to_string(),
        participant_id: None,
        session_id: None,
    };

    assert_eq!(prolific_no_consent_url(&config, Some(&local)), None);
    assert_eq!(
        prolific_no_consent_url(&config, Some(&prolific)).as_deref(),
        Some("https://app.prolific.com/submissions/complete?cc=NOCONSENT")
    );
    config
        .recruitment
        .prolific
        .completion_paths
        .no_consent
        .clear();
    assert_eq!(prolific_no_consent_url(&config, Some(&local)), None);
    assert_eq!(prolific_no_consent_url(&config, Some(&prolific)), None);
}

/// Confirms Prolific experiments are not shown as ready without the global API credential.
#[tokio::test]
async fn catalogue_readiness_includes_global_prolific_prerequisites() {
    let (mut config, _tmp) = sqlite_config();
    config.recruitment.prolific.enabled = true;
    config.recruitment.prolific.study_id = "prolific-study".to_string();
    config.recruitment.prolific.completion_paths.completed = "COMPLETE".to_string();
    config.recruitment.prolific.completion_paths.partner_left = "PARTNERLEFT".to_string();
    config
        .recruitment
        .prolific
        .completion_paths
        .game_did_not_start = "NOPARTNER".to_string();
    config
        .recruitment
        .prolific
        .completion_paths
        .participation_ended_early = "TIMEDOUT".to_string();
    config
        .recruitment
        .prolific
        .completion_paths
        .technical_failure = "TECHFAIL".to_string();
    config.recruitment.prolific.completion_paths.no_consent = "NOCONSENT".to_string();
    let router = crate::app::build_router(live_game(), config, ServeOptions::default())
        .await
        .map(TestRouter::new)
        .expect("an incomplete inactive Prolific draft still builds");
    authenticate_test_admin(router.clone()).await.unwrap();

    let (_, catalogue) = admin_json_request(
        router.clone(),
        http::Method::GET,
        "/api/admin/experiments",
        Value::Null,
    )
    .await;
    let experiment = &catalogue["experiments"][0];
    assert_eq!(experiment["configuration_valid"], true);
    assert_eq!(experiment["local_testing_runnable"], true);
    assert_eq!(experiment["runnable"], false);
    let issues = experiment["runnable_issues"].as_array().unwrap();
    assert!(issues
        .iter()
        .any(|issue| issue.as_str().unwrap().contains("Prolific API token")));

    let (_, dashboard_config) = admin_json_request(
        router.clone(),
        http::Method::GET,
        "/api/admin/experiments/step5/config",
        Value::Null,
    )
    .await;
    assert_eq!(dashboard_config["activation_issues"], json!([]));

    let (testing_status, testing) = admin_json_request(
        router.clone(),
        http::Method::POST,
        "/api/admin/experiment/status",
        json!({"status": "testing", "participant_url_kind": "local"}),
    )
    .await;
    assert_eq!(testing_status, StatusCode::OK);
    let running_url = testing["participant_url"].clone();
    assert_eq!(running_url["kind"], "local");
    assert!(running_url["participant_id"]
        .as_str()
        .unwrap()
        .starts_with("TEST"));
    assert!(running_url["session_id"]
        .as_str()
        .unwrap()
        .starts_with("TEST"));
    let (_, refreshed_catalogue) = admin_json_request(
        router.clone(),
        http::Method::GET,
        "/api/admin/experiments",
        Value::Null,
    )
    .await;
    assert_eq!(
        refreshed_catalogue["experiments"][0]["participant_url"],
        running_url
    );
    let (synthetic_status, synthetic) = json_request(
        router.clone(),
        http::Method::POST,
        "/api/participants",
        json!({"prolific": {
            "participant_id": running_url["participant_id"],
            "study_id": "prolific-study",
            "session_id": running_url["session_id"]
        }}),
    )
    .await;
    assert_eq!(synthetic_status, StatusCode::OK, "{synthetic:#}");
    let (real_status, _) = json_request(
        router.clone(),
        http::Method::POST,
        "/api/participants",
        json!({"prolific": {
            "participant_id": "REALPARTICIPANT",
            "study_id": "prolific-study",
            "session_id": "REALSESSION"
        }}),
    )
    .await;
    assert_eq!(real_status, StatusCode::SERVICE_UNAVAILABLE);
    let (stopped_status, stopped) = admin_json_request(
        router.clone(),
        http::Method::POST,
        "/api/admin/experiment/status",
        json!({"status": "inactive"}),
    )
    .await;
    assert_eq!(stopped_status, StatusCode::OK);
    assert_eq!(stopped["participant_url"], Value::Null);
    let (_, stopped_catalogue) = admin_json_request(
        router,
        http::Method::GET,
        "/api/admin/experiments",
        Value::Null,
    )
    .await;
    assert_eq!(
        stopped_catalogue["experiments"][0]["participant_url"],
        Value::Null
    );
}

/// Confirms a green provider-backed badge and Start share one verified study contract.
#[tokio::test]
async fn prolific_run_readiness_verifies_the_linked_study_before_start() {
    let study_requests = Arc::new(AtomicUsize::new(0));
    let project_requests = Arc::new(AtomicUsize::new(0));
    let (mut config, _tmp) = sqlite_config();
    let expected_url = prolific_setup_url(&experiment_public_base_url(
        &config.server.public_base_url,
        "step5",
    ));
    let catalogue_expected_url = expected_url.clone();
    config.recruitment.prolific.enabled = true;
    config.recruitment.prolific.study_id = "linkedstudy".to_string();
    config.recruitment.prolific.completion_paths.completed = "COMPLETE".to_string();
    config.recruitment.prolific.completion_paths.partner_left = "PARTNERLEFT".to_string();
    config
        .recruitment
        .prolific
        .completion_paths
        .game_did_not_start = "NOPARTNER".to_string();
    config
        .recruitment
        .prolific
        .completion_paths
        .participation_ended_early = "TIMEDOUT".to_string();
    config
        .recruitment
        .prolific
        .completion_paths
        .technical_failure = "TECHFAIL".to_string();
    config.recruitment.prolific.completion_paths.no_consent = "NOCONSENT".to_string();
    let study_counter = study_requests.clone();
    let project_counter = project_requests.clone();
    let provider = Router::new()
        .route(
            "/api/v1/studies/linkedstudy/",
            axum::routing::get(move || {
                let expected_url = expected_url.clone();
                let study_counter = study_counter.clone();
                async move {
                    study_counter.fetch_add(1, Ordering::SeqCst);
                    Json(json!({
                        "id": "linkedstudy",
                        "external_study_url": expected_url,
                        "prolific_id_option": "url_parameters",
                        "completion_codes": [
                            {"code": "COMPLETE", "code_type": "CUSTOM", "actions": [{"action": "AUTOMATICALLY_APPROVE"}]},
                            {"code": "PARTNERLEFT", "code_type": "CUSTOM", "actions": [{"action": "AUTOMATICALLY_APPROVE"}]},
                            {"code": "NOPARTNER", "code_type": "CUSTOM", "actions": [{"action": "REQUEST_RETURN"}]},
                            {"code": "TIMEDOUT", "code_type": "CUSTOM", "actions": [{"action": "REQUEST_RETURN"}]},
                            {"code": "TECHFAIL", "code_type": "CUSTOM", "actions": [{"action": "AUTOMATICALLY_APPROVE"}]},
                            {"code": "NOCONSENT", "code_type": "NO_CONSENT", "actions": [{"action": "REQUEST_RETURN"}]}
                        ],
                        "estimated_completion_time": 120,
                        "total_available_places": 20,
                        "maximum_allowed_time": null,
                        "project": "project1"
                    }))
                }
            }),
        )
        .route(
            "/api/v1/projects/project1/",
            axum::routing::get(move || {
                let project_counter = project_counter.clone();
                async move {
                    project_counter.fetch_add(1, Ordering::SeqCst);
                    Json(json!({"id": "project1", "workspace": "workspace1"}))
                }
            }),
        );
    let (provider_url, provider_task) = spawn_test_server(TestRouter::new(provider)).await;
    let router = crate::app::build_router(live_game(), config, ServeOptions::default())
        .await
        .map(TestRouter::new)
        .unwrap();
    authenticate_test_admin(router.clone()).await.unwrap();
    let (settings_status, settings) = admin_json_request(
        router.clone(),
        http::Method::POST,
        "/api/admin/game/settings",
        json!({
            "expected_revision": 1,
            "institution": "",
            "admin_allowed_ip_ranges": [],
            "prolific_api_base_url": provider_url,
            "secret_updates": {"prolific.api_token": "provider-token"}
        }),
    )
    .await;
    assert_eq!(settings_status, StatusCode::OK, "{settings}");
    let (_, current) = admin_json_request(
        router.clone(),
        http::Method::GET,
        "/api/admin/experiments/step5/config",
        Value::Null,
    )
    .await;
    let mut missing_study = current["experiment"]["config"].clone();
    missing_study["recruitment"]["prolific"]["study_id"] = json!("missingstudy");
    missing_study["recruitment"]["prolific"]["completion_paths"]["no_consent"] =
        json!("NEWNOCONSENT");
    let (unready_status, unready) = admin_json_request(
        router.clone(),
        http::Method::POST,
        "/api/admin/experiments/step5/config",
        json!({
            "expected_revision": 1,
            "config": missing_study,
            "game_yaml": current["game_yaml"],
            "secret_updates": {},
            "secret_deletions": []
        }),
    )
    .await;
    assert_eq!(unready_status, StatusCode::OK, "{unready}");
    assert_eq!(unready["revision"], 2);
    assert!(unready["prolific_issues"]
        .to_string()
        .contains("Prolific API resource was not found (404 Not Found)"));
    let (_, persisted_unready) = admin_json_request(
        router.clone(),
        http::Method::GET,
        "/api/admin/experiments/step5/config",
        Value::Null,
    )
    .await;
    assert_eq!(
        persisted_unready["experiment"]["config"]["recruitment"]["prolific"]["completion_paths"]
            ["no_consent"],
        "NEWNOCONSENT"
    );
    let (save_status, saved) = admin_json_request(
        router.clone(),
        http::Method::POST,
        "/api/admin/experiments/step5/config",
        json!({
            "expected_revision": 2,
            "config": current["experiment"]["config"],
            "game_yaml": current["game_yaml"],
            "secret_updates": {},
            "secret_deletions": []
        }),
    )
    .await;
    assert_eq!(save_status, StatusCode::OK, "{saved}");
    let (_, catalogue) = admin_json_request(
        router.clone(),
        http::Method::GET,
        "/api/admin/experiments",
        Value::Null,
    )
    .await;
    assert_eq!(catalogue["experiments"][0]["runnable"], true);
    assert_eq!(
        catalogue["experiments"][0]["prolific_setup_url"],
        catalogue_expected_url
    );
    assert_eq!(study_requests.load(Ordering::SeqCst), 1);
    assert_eq!(project_requests.load(Ordering::SeqCst), 1);
    let (progress_status, progress) = admin_json_request(
        router.clone(),
        http::Method::GET,
        "/api/admin/progress",
        Value::Null,
    )
    .await;
    assert_eq!(progress_status, StatusCode::OK, "{progress}");
    assert!(progress["progress"].get("planned_sessions").is_none());
    let (refresh_status, refreshed) = admin_json_request(
        router.clone(),
        http::Method::POST,
        "/api/admin/progress",
        Value::Null,
    )
    .await;
    assert_eq!(refresh_status, StatusCode::OK, "{refreshed}");
    assert_eq!(refreshed["progress"]["planned_sessions"], 10);
    assert_eq!(study_requests.load(Ordering::SeqCst), 2);
    assert_eq!(project_requests.load(Ordering::SeqCst), 2);
    let (start_status, started) = admin_json_request(
        router,
        http::Method::POST,
        "/api/admin/experiment/status",
        json!({
            "status": "testing",
            "verify_prolific": true,
            "participant_url_kind": "prolific"
        }),
    )
    .await;
    assert_eq!(start_status, StatusCode::OK, "{started}");
    assert_eq!(study_requests.load(Ordering::SeqCst), 2);
    assert_eq!(project_requests.load(Ordering::SeqCst), 2);
    provider_task.abort();
}

/// Confirms one Prolific user may enter again when Prolific issues a new submission id.
#[tokio::test]
async fn prolific_participant_with_new_session_id_enters_new_session() {
    let router = build_prolific_testing_router().await;
    let first =
        create_prolific_testing_participant(router.clone(), "TESTRETURNING", "TESTSUBMISSIONONE")
            .await;
    let partner = create_prolific_testing_participant(
        router.clone(),
        "TESTPARTNERONE",
        "TESTPARTNERSUBMISSIONONE",
    )
    .await;
    consent_participant(router.clone(), &first).await;
    consent_participant(router.clone(), &partner).await;
    let (_, first_entry) = participant_json_request(
        router.clone(),
        &first,
        http::Method::POST,
        "/api/sessions",
        json!({}),
    )
    .await;
    let first_session_id = first_entry["participant_state"]["public_session_id"]
        .as_str()
        .unwrap()
        .to_string();
    let (_, _) = participant_json_request(
        router.clone(),
        &partner,
        http::Method::POST,
        "/api/sessions",
        json!({}),
    )
    .await;
    let (base_url, server) = spawn_test_server(router.clone()).await;
    let (mut first_socket, _) =
        connect_async(game_socket_url(&router, &base_url, &first_session_id, &first).await)
            .await
            .unwrap();
    let (mut partner_socket, _) =
        connect_async(game_socket_url(&router, &base_url, &first_session_id, &partner).await)
            .await
            .unwrap();
    let active = read_participant_state(&mut first_socket, "active").await;
    assert_eq!(active["recruitment_source"], "local_preview");
    let _ = read_participant_state(&mut partner_socket, "active").await;
    send_ws_json(
        &mut first_socket,
        json!({"type": "action", "action": {"finish": true}}),
    )
    .await;
    let ended = read_participant_state(&mut first_socket, "ended").await;
    assert_eq!(ended["result"]["handoff"], Value::Null);
    let _ = read_participant_state(&mut partner_socket, "ended").await;

    let returning =
        create_prolific_testing_participant(router.clone(), "TESTRETURNING", "TESTSUBMISSIONTWO")
            .await;
    assert_ne!(returning, first);
    consent_participant(router.clone(), &returning).await;
    let (status, second_entry) = participant_json_request(
        router.clone(),
        &returning,
        http::Method::POST,
        "/api/sessions",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(second_entry["participant_state"]["state"], "waiting");
    assert_ne!(
        second_entry["participant_state"]["public_session_id"],
        first_session_id
    );

    let (_, sessions) = admin_json_request(
        router,
        http::Method::GET,
        "/api/admin/sessions?limit=80",
        Value::Null,
    )
    .await;
    assert_eq!(sessions["sessions"].as_array().unwrap().len(), 2);
    server.abort();
}

/// Confirms an ended Prolific submission always returns its original recorded outcome.
#[tokio::test]
async fn prolific_participant_with_ended_session_id_rejoins_recorded_outcome() {
    let router = build_prolific_testing_router().await;
    let participant =
        create_prolific_testing_participant(router.clone(), "TESTENDED", "TESTENDEDSUBMISSION")
            .await;
    let partner = create_prolific_testing_participant(
        router.clone(),
        "TESTENDEDPARTNER",
        "TESTENDEDPARTNERSUBMISSION",
    )
    .await;
    consent_participant(router.clone(), &participant).await;
    consent_participant(router.clone(), &partner).await;
    let (_, entry) = participant_json_request(
        router.clone(),
        &participant,
        http::Method::POST,
        "/api/sessions",
        json!({}),
    )
    .await;
    let public_session_id = entry["participant_state"]["public_session_id"]
        .as_str()
        .unwrap()
        .to_string();
    let (_, _) = participant_json_request(
        router.clone(),
        &partner,
        http::Method::POST,
        "/api/sessions",
        json!({}),
    )
    .await;
    let (base_url, server) = spawn_test_server(router.clone()).await;
    let (mut participant_socket, _) =
        connect_async(game_socket_url(&router, &base_url, &public_session_id, &participant).await)
            .await
            .unwrap();
    let (mut partner_socket, _) =
        connect_async(game_socket_url(&router, &base_url, &public_session_id, &partner).await)
            .await
            .unwrap();
    let _ = read_participant_state(&mut participant_socket, "active").await;
    let _ = read_participant_state(&mut partner_socket, "active").await;
    send_ws_json(
        &mut participant_socket,
        json!({"type": "action", "action": {"finish": true}}),
    )
    .await;
    let _ = read_participant_state(&mut participant_socket, "ended").await;
    let _ = read_participant_state(&mut partner_socket, "ended").await;
    participant_socket.close(None).await.unwrap();
    partner_socket.close(None).await.unwrap();
    wait_for_completed_runtime_cleanup(router.clone()).await;

    let returning =
        create_prolific_testing_participant(router.clone(), "TESTENDED", "TESTENDEDSUBMISSION")
            .await;
    assert_eq!(returning, participant);
    let (status, replay) = participant_json_request(
        router.clone(),
        &returning,
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

    let (_, sessions) = admin_json_request(
        router,
        http::Method::GET,
        "/api/admin/sessions?limit=80",
        Value::Null,
    )
    .await;
    assert_eq!(sessions["sessions"].as_array().unwrap().len(), 1);
    server.abort();
}

/// Confirms a running Prolific submission reconnects to its existing active session.
#[tokio::test]
async fn prolific_participant_with_running_session_id_rejoins_active_session() {
    let router = build_prolific_testing_router().await;
    let participant =
        create_prolific_testing_participant(router.clone(), "TESTRUNNING", "TESTRUNNINGSUBMISSION")
            .await;
    let partner = create_prolific_testing_participant(
        router.clone(),
        "TESTRUNNINGPARTNER",
        "TESTRUNNINGPARTNERSUBMISSION",
    )
    .await;
    consent_participant(router.clone(), &participant).await;
    consent_participant(router.clone(), &partner).await;
    let (_, entry) = participant_json_request(
        router.clone(),
        &participant,
        http::Method::POST,
        "/api/sessions",
        json!({}),
    )
    .await;
    let public_session_id = entry["participant_state"]["public_session_id"]
        .as_str()
        .unwrap()
        .to_string();
    let (_, _) = participant_json_request(
        router.clone(),
        &partner,
        http::Method::POST,
        "/api/sessions",
        json!({}),
    )
    .await;
    let (base_url, server) = spawn_test_server(router.clone()).await;
    let (mut participant_socket, _) =
        connect_async(game_socket_url(&router, &base_url, &public_session_id, &participant).await)
            .await
            .unwrap();
    let (mut partner_socket, _) =
        connect_async(game_socket_url(&router, &base_url, &public_session_id, &partner).await)
            .await
            .unwrap();
    let _ = read_participant_state(&mut participant_socket, "active").await;
    let _ = read_participant_state(&mut partner_socket, "active").await;

    let returning =
        create_prolific_testing_participant(router.clone(), "TESTRUNNING", "TESTRUNNINGSUBMISSION")
            .await;
    assert_eq!(returning, participant);
    let (status, replay) = participant_json_request(
        router.clone(),
        &returning,
        http::Method::POST,
        "/api/sessions",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replay["participant_state"]["state"], "active");
    assert_eq!(replay["participant_state"]["role"], "A");
    assert_eq!(
        replay["participant_state"]["public_session_id"],
        public_session_id
    );

    let (_, sessions) = admin_json_request(
        router,
        http::Method::GET,
        "/api/admin/sessions?limit=80",
        Value::Null,
    )
    .await;
    assert_eq!(sessions["sessions"].as_array().unwrap().len(), 1);
    participant_socket.close(None).await.unwrap();
    partner_socket.close(None).await.unwrap();
    server.abort();
}

/// Prolific participant places become whole planned sessions for each pairing mode.
#[test]
fn prolific_places_are_normalized_to_planned_sessions() {
    let mut config = ExperimentConfig::default();
    let mut study = crate::prolific::Study {
        id: "study".to_string(),
        external_study_url: "https://example.test".to_string(),
        prolific_id_option: "url_parameters".to_string(),
        completion_codes: Vec::new(),
        estimated_completion_time: 10,
        total_available_places: 20.0,
        maximum_allowed_time: Some(30.0),
        is_external_study_url_secure: true,
        device_compatibility: Vec::new(),
        peripheral_requirements: Vec::new(),
        project: Some("project".to_string()),
        status: Some("ACTIVE".to_string()),
    };
    assert_eq!(prolific_planned_sessions(&config, &study), Ok(10));
    config.agents.mode = AgentsMode::HumanVsAgent;
    assert_eq!(prolific_planned_sessions(&config, &study), Ok(20));
    config.agents.mode = AgentsMode::HumanVsHuman;
    study.total_available_places = 21.0;
    assert!(prolific_planned_sessions(&config, &study)
        .expect_err("an odd place count cannot form two-person sessions")
        .contains("cannot form complete"));
    for invalid in [0.0, -2.0, 20.5, f64::NAN, f64::INFINITY] {
        study.total_available_places = invalid;
        assert!(prolific_planned_sessions(&config, &study).is_err());
    }
}

/// A configured direct-recruitment target is available before any session exists.
#[tokio::test]
async fn configured_progress_target_is_returned_for_an_empty_experiment() {
    let (mut config, _temp) = sqlite_config();
    config.progress.planned_sessions = Some(6);
    let router = build_router(live_game(), config, ServeOptions::default())
        .await
        .unwrap();
    authenticate_test_admin(router.clone()).await.unwrap();
    let (status, response) = admin_json_request(
        router,
        http::Method::GET,
        "/api/admin/progress",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(response["progress"]["planned_sessions"], 6);
    assert_eq!(response["progress"]["plan_source"], "configured");
    assert_eq!(response["progress"]["total_sessions"], 0);
    assert!(response["progress"]["sessions"]
        .as_array()
        .unwrap()
        .is_empty());
}
