use super::*;

/// Confirms lifecycle transitions preserve the semantic boundary between results and deletion.
#[test]
fn experiment_lifecycle_rejects_ambiguous_shortcuts() {
    assert!(ExperimentLifecycle::Inactive.can_transition_to(ExperimentLifecycle::Testing));
    assert!(!ExperimentLifecycle::Testing.can_transition_to(ExperimentLifecycle::Active));
    assert!(ExperimentLifecycle::Active.can_transition_to(ExperimentLifecycle::Completed));
    assert!(ExperimentLifecycle::Completed.can_transition_to(ExperimentLifecycle::Archived));
    assert!(ExperimentLifecycle::Archived.can_transition_to(ExperimentLifecycle::Inactive));
    assert!(!ExperimentLifecycle::Testing.can_transition_to(ExperimentLifecycle::Completed));
    assert!(!ExperimentLifecycle::Active.can_transition_to(ExperimentLifecycle::Archived));
    assert!(!ExperimentLifecycle::Archived.can_transition_to(ExperimentLifecycle::Active));
}

/// Confirms lifecycle changes gate intake and persist across restarts.
#[tokio::test]
async fn experiment_starts_inactive_and_admin_controls_intake() {
    let (mut config, _tmp) = sqlite_config();
    config.experiment.id = Some("bootstrap".to_string());
    let router = crate::app::build_router(TinyAdapter, config.clone(), ServeOptions::default())
        .await
        .map(TestRouter::new)
        .unwrap();

    let (status, public) = json_request(
        router.clone(),
        http::Method::GET,
        "/api/config",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(public["experiment_status"], "inactive");
    let (status, _) = json_request(
        router.clone(),
        http::Method::POST,
        "/api/participants",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);

    authenticate_test_admin(router.clone()).await.unwrap();

    let (settings_status, _) = admin_json_request(
        router.clone(),
        http::Method::POST,
        "/api/admin/game/settings",
        json!({"expected_revision": 1, "institution": "Test University", "admin_allowed_ip_ranges": []}),
    )
    .await;
    assert_eq!(settings_status, StatusCode::OK);
    let (status, testing) = admin_json_request(
        router.clone(),
        http::Method::POST,
        "/api/admin/experiment/status",
        json!({"status": "testing"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(testing["status"], "testing");
    let (status, _) = json_request(
        router.clone(),
        http::Method::POST,
        "/api/participants",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = admin_json_request(
        router.clone(),
        http::Method::POST,
        "/api/admin/experiment/status",
        json!({"status": "inactive"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, activation) = admin_json_request(
        router.clone(),
        http::Method::POST,
        "/api/admin/experiment/status",
        json!({"status": "active"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(activation["status"], "active");
    let (status, participant) = json_request(
        router.clone(),
        http::Method::POST,
        "/api/participants",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(participant["participant_id"].is_string());
    assert!(participant.get("participant_session_id").is_none());

    let (status, _) = admin_json_request(
        router.clone(),
        http::Method::POST,
        "/api/admin/experiment/status",
        json!({"status": "inactive"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = json_request(
        router.clone(),
        http::Method::POST,
        "/api/participants",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    let (status, archived) = admin_json_request(
        router.clone(),
        http::Method::POST,
        "/api/admin/experiment/status",
        json!({"status": "archived"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(archived["status"], "archived");

    let restarted = crate::app::build_router(TinyAdapter, config, ServeOptions::default())
        .await
        .map(TestRouter::new)
        .unwrap();
    authenticate_test_admin(restarted.clone()).await.unwrap();
    let (status, experiment) = admin_json_request(
        restarted,
        http::Method::GET,
        "/api/admin/experiment",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(experiment["experiment"]["status"], "archived");
}

/// Confirms incomplete hosted voice services remain editable but block testing and active intake.
#[tokio::test]
async fn hosted_voice_services_without_credentials_block_experiment_start() {
    let (mut config, _tmp) = sqlite_config();
    config.transcription.enabled = true;
    config.speechmatics.api_key.clear();
    let router = crate::app::build_router(TinyAdapter, config, ServeOptions::default())
        .await
        .map(TestRouter::new)
        .expect("an incomplete inactive draft still builds");
    authenticate_test_admin(router.clone()).await.unwrap();

    let (config_status, dashboard_config) = admin_json_request(
        router.clone(),
        http::Method::GET,
        "/api/admin/experiments/step5/config",
        Value::Null,
    )
    .await;
    assert_eq!(config_status, StatusCode::OK);
    assert!(dashboard_config["activation_issues"]
        .as_array()
        .unwrap()
        .iter()
        .any(|issue| issue.as_str().unwrap().contains("Speechmatics API key")));
    assert_eq!(
        dashboard_config["experiment"]["config"]["session"]["waiting_session_timeout_seconds"],
        600
    );
    assert_eq!(
        dashboard_config["experiment"]["config"]["session"]["session_max_lifetime_seconds"],
        14_400
    );
    assert_eq!(
        dashboard_config["experiment"]["config"]["capacity"]["max_active_sessions"],
        30
    );
    assert_eq!(
        dashboard_config["experiment"]["config"]["capacity"]["storage_reserve_megabytes"],
        256
    );

    let (status, problem) = admin_json_request(
        router,
        http::Method::POST,
        "/api/admin/experiment/status",
        json!({"status": "testing"}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(problem["raw"]
        .as_str()
        .unwrap()
        .contains("Speechmatics API key"));
}

/// Confirms catalogue badges and lifecycle controls receive identical agent readiness.
#[tokio::test]
async fn catalogue_readiness_includes_missing_agent_secret_references() {
    let (mut config, _tmp) = sqlite_config();
    config.agents = AgentsConfig {
        mode: AgentsMode::HumanVsAgent,
        human_vs_agent: Some(crate::config::HumanVsAgentConfig {
            factory: Some("secret.agent".to_string()),
            config: json!({"api_key": "game.agent_secret"}),
            ..Default::default()
        }),
    };
    let router = crate::app::build_router(
        TinyAdapter,
        config,
        ServeOptions {
            agent_factory: Some(Arc::new(SecretAgentFactory)),
            ..ServeOptions::default()
        },
    )
    .await
    .map(TestRouter::new)
    .expect("a missing agent secret remains an editable inactive draft");
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
    assert_eq!(experiment["local_testing_runnable"], false);
    assert_eq!(experiment["runnable"], false);
    assert!(
        experiment["runnable_issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|issue| issue.as_str().unwrap().contains("game.agent_secret")),
        "unexpected catalogue readiness: {experiment:#}"
    );

    let (_, dashboard_config) = admin_json_request(
        router.clone(),
        http::Method::GET,
        "/api/admin/experiments/step5/config",
        Value::Null,
    )
    .await;
    assert_eq!(
        experiment["runnable_issues"], dashboard_config["activation_issues"],
        "catalogue badges and lifecycle controls must share readiness diagnostics"
    );
}
