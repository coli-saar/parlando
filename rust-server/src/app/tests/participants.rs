use super::*;

pub(super) fn step_five_config() -> ExperimentConfig {
    ExperimentConfig {
        experiment: ExperimentIdentityConfig {
            id: Some("step5".to_string()),
        },
        database: DatabaseConfig {
            url: "sqlite:///:memory:".to_string(),
        },
        direct: DirectConfig {
            enabled: true,
            participant_information_version: "test-v1".to_string(),
            participant_information_url: "https://example.test/privacy".to_string(),
            consents: vec![crate::config::ConsentItemConfig {
                id: "study".to_string(),
                title: "Study".to_string(),
                body: "Agree?".to_string(),
                required: true,
            }],
        },
        ..ExperimentConfig::default()
    }
}

pub(super) fn sqlite_config() -> (ExperimentConfig, tempfile::TempDir) {
    let temp = tempfile::tempdir().unwrap();
    let mut config = step_five_config();
    config.database.url = format!(
        "sqlite:///{}",
        temp.path().join("server-core.sqlite").display()
    );
    (config, temp)
}

pub(super) fn voice_enabled_config() -> ExperimentConfig {
    let mut config = step_five_config();
    config.voice.enabled = true;
    config
}

pub(super) fn human_vs_agent_config() -> ExperimentConfig {
    let mut config = step_five_config();
    config.agents = AgentsConfig {
        mode: AgentsMode::HumanVsAgent,
        human_vs_agent: Some(crate::config::HumanVsAgentConfig {
            factory: Some("test.agent".to_string()),
            act_timeout_seconds: 1.0,
            invalid_action_limit: 2,
            ..Default::default()
        }),
    };
    config
}

pub(super) async fn create_direct_participant(router: TestRouter, _name: &str) -> String {
    let (status, response) = json_request(
        router.clone(),
        http::Method::POST,
        "/api/participants",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let participant_session_id = response["participant_id"].as_str().unwrap().to_string();
    let credential = response["participant_credential"]
        .as_str()
        .unwrap()
        .to_string();
    router.remember(participant_session_id.clone(), credential);
    participant_session_id
}

/// Configures the synthetic Prolific study and all participant handoffs.
pub(super) fn synthetic_prolific_config() -> ExperimentConfig {
    let mut config = step_five_config();
    config.session.reconnect_grace_seconds = 1;
    config.recruitment.prolific.enabled = true;
    config.recruitment.prolific.study_id = "prolific-study".to_string();
    let paths = &mut config.recruitment.prolific.completion_paths;
    paths.completed = "COMPLETE".to_string();
    paths.partner_left = "PARTNERLEFT".to_string();
    paths.game_did_not_start = "NOSTART".to_string();
    paths.participation_ended_early = "EARLY".to_string();
    paths.technical_failure = "TECHNICAL".to_string();
    paths.no_consent = "NOCONSENT".to_string();
    config
}

/// Builds a local-testing router that exercises Prolific admission without an external provider.
pub(super) async fn build_prolific_testing_router() -> TestRouter {
    let config = synthetic_prolific_config();
    let router = crate::app::build_router(TinyAdapter, config, ServeOptions::default())
        .await
        .map(TestRouter::new)
        .unwrap();
    authenticate_test_admin(router.clone()).await.unwrap();
    let (status, _) = admin_json_request(
        router.clone(),
        http::Method::POST,
        "/api/admin/experiment/status",
        json!({"status": "testing", "participant_url_kind": "local"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    router
}

/// Admits one synthetic Prolific submission and retains its issued test credential.
pub(super) async fn create_prolific_testing_participant(
    router: TestRouter,
    prolific_participant_id: &str,
    prolific_session_id: &str,
) -> String {
    let (status, response) = json_request(
        router.clone(),
        http::Method::POST,
        "/api/participants",
        json!({"prolific": {
            "participant_id": prolific_participant_id,
            "study_id": "prolific-study",
            "session_id": prolific_session_id
        }}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{response:#}");
    let participant_id = response["participant_id"].as_str().unwrap().to_string();
    let credential = response["participant_credential"]
        .as_str()
        .unwrap()
        .to_string();
    router.remember(participant_id.clone(), credential);
    participant_id
}

/// Waits until a completed session has left runtime memory while remaining durable.
pub(super) async fn wait_for_completed_runtime_cleanup(router: TestRouter) {
    for _ in 0..40 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let (_, load) = admin_json_request(
            router.clone(),
            http::Method::GET,
            "/api/admin/load",
            Value::Null,
        )
        .await;
        if load["current"]["capacity"]["completed_retained_sessions"] == 0 {
            return;
        }
    }
    panic!("completed session remained in runtime memory");
}
