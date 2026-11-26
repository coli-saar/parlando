use super::*;

/// Provider-secret status rows retain their stable order and calculate availability once.
#[test]
fn provider_secret_statuses_report_configured_and_missing_values() {
    let stored = HashMap::from([
        ("speechmatics.api_key".to_string(), "speech-key".to_string()),
        (
            "prolific.api_token".to_string(),
            "prolific-token".to_string(),
        ),
    ]);

    assert_eq!(
        configured_provider_secret_statuses(&stored),
        vec![
            json!({"key": "speechmatics.api_key", "configured": true, "source": "game"}),
            json!({"key": "tts.api_key", "configured": false, "source": "missing"}),
            json!({"key": "prolific.api_token", "configured": true, "source": "game"}),
        ]
    );
}

/// Confirms reviewed consent prose enters evidence hashes without runtime rewriting.
#[test]
fn consent_hash_uses_exact_stored_participant_text() {
    let mut config = ExperimentConfig::default();
    config.direct.participant_information_version = "local-v3".to_string();
    config.direct.participant_information_url = "https://example.test/information".to_string();
    config.direct.consents = vec![crate::config::ConsentItemConfig {
        id: "template".to_string(),
        title: "Template".to_string(),
        required: true,
        body: "Reviewed exact consent prose.".to_string(),
        editor_data: None,
    }];
    let hash = consent_configuration_hash(&config).unwrap();
    assert!(hash.starts_with("sha256:"));
    config.direct.consents[0].editor_data = Some(crate::config::ConsentEditorData {
        values: std::collections::HashMap::from([("name".to_string(), "Alex".to_string())]),
        bindings: vec![],
    });
    let saved = serde_json::to_value(&config).unwrap();
    let reopened: ExperimentConfig = serde_json::from_value(saved).unwrap();
    assert_eq!(
        reopened.direct.consents[0]
            .editor_data
            .as_ref()
            .unwrap()
            .values["name"],
        "Alex"
    );
    assert_eq!(consent_configuration_hash(&reopened).unwrap(), hash);
    config.direct.consents[0].body.push_str(" Updated.");
    assert_ne!(consent_configuration_hash(&config).unwrap(), hash);
}

/// Confirms current validation errors do not prevent a stored revision from being inspected.
#[test]
fn dashboard_configuration_parsing_does_not_apply_runtime_validation() {
    let config = step_five_config();
    let mut stored = persistable_config_json(&config).unwrap();
    stored["session"]["reconnect_grace_seconds"] = json!(61);

    assert!(experiment_config_from_json(stored.clone(), &config, "step5").is_err());
    let visible = experiment_config_from_json_unvalidated(stored, &config, "step5").unwrap();
    assert_eq!(visible.session.reconnect_grace_seconds, 61);
}

/// Confirms game YAML rejects embedded credentials in favor of the secret store.
#[test]
fn game_configuration_rejects_credential_shaped_fields() {
    validate_game_config_contains_no_secrets(&json!({"difficulty": 2})).unwrap();
    let error = validate_game_config_contains_no_secrets(&json!({
        "provider": {"access_token": "sentinel"}
    }))
    .unwrap_err();
    assert!(error.to_string().contains("provider.access_token"));
    assert!(error.to_string().contains("Game secrets"));
}

/// Preserves semantic references while erasing actual values under credential-shaped keys.
#[test]
fn persisted_configuration_distinguishes_secret_references_from_values() {
    let mut reference = json!({"api_key": "game.agent_secret"});
    redact_secret_fields(&mut reference);
    assert_eq!(reference["api_key"], "game.agent_secret");

    let mut credential = json!({"api_key": "sentinel-secret-value"});
    redact_secret_fields(&mut credential);
    assert_eq!(credential["api_key"], "");
}

/// Confirms provider credentials come only from the explicit game secret store.
#[tokio::test]
async fn administrator_can_store_and_explicitly_reveal_experiment_secrets() {
    let (mut config, _tmp) = sqlite_config();
    config.experiment.id = Some("secret-config".to_string());
    config.speechmatics.api_key = "in-memory-speech-secret".to_string();
    let descriptor = GameMetadata {
        id: "tiny-game".to_string(),
        name: "Tiny Game".to_string(),
        version: semver::Version::parse("0.4.0").unwrap(),
        build_manifest: json!({}),
    };
    let router = super::build_game_router(live_game(), config, descriptor, |_| {
        Ok(ServeOptions::default())
    })
    .await
    .map(TestRouter::new)
    .unwrap();
    authenticate_test_admin(router.clone()).await.unwrap();

    let (status, _) = admin_json_request(
        router.clone(),
        http::Method::POST,
        "/api/admin/experiments",
        json!({"experiment_id": "secret-config"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, current) = admin_json_request(
        router.clone(),
        http::Method::GET,
        "/api/admin/experiments/secret-config/config",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!serde_json::to_string(&current)
        .unwrap()
        .contains("in-memory-speech-secret"));
    assert_eq!(current["configured_secrets"], json!([]));

    let (status, catalogue) = admin_json_request(
        router.clone(),
        http::Method::GET,
        "/api/admin/experiments",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(catalogue["game_provider_secrets"][0]["source"], "missing");

    let (status, _) = admin_json_request(
        router.clone(),
        http::Method::POST,
        "/api/admin/game/secrets/reveal",
        json!({"key": "speechmatics.api_key"}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, updated) = admin_json_request(
        router.clone(),
        http::Method::POST,
        "/api/admin/game/settings",
        json!({
            "expected_revision": 1,
            "institution": "Test University",
            "admin_allowed_ip_ranges": [],
            "speechmatics_realtime_url": "wss://eu.rt.speechmatics.com/v2",
            "tts_base_url": "wss://api.elevenlabs.io",
            "secret_updates": {
                "speechmatics.api_key": "stored-speechmatics-secret",
                "tts.api_key": "stored-elevenlabs-secret",
                "prolific.api_token": "stored-prolific-token"
            },
            "secret_deletions": []
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["revision"], 2);
    let (status, catalogue) = admin_json_request(
        router.clone(),
        http::Method::GET,
        "/api/admin/experiments",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(catalogue["game_provider_secrets"][0]["source"], "game");
    assert_eq!(catalogue["game_provider_secrets"][1]["source"], "game");
    assert_eq!(catalogue["game_provider_secrets"][2]["source"], "game");
    let (status, revealed) = admin_json_request(
        router.clone(),
        http::Method::POST,
        "/api/admin/game/secrets/reveal",
        json!({"key": "tts.api_key"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(revealed["value"], "stored-elevenlabs-secret");

    let (status, saved) = admin_json_request(
        router.clone(),
        http::Method::POST,
        "/api/admin/experiments/secret-config/config",
        json!({
            "expected_revision": 1,
            "config": current["experiment"]["config"].clone(),
            "game_yaml": "difficulty: 2\n",
            "secret_updates": {"game.copy_key": "copy-me"},
            "secret_deletions": [],
            "change_summary": "Configure game"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    let (status, revealed) = admin_json_request(
        router,
        http::Method::POST,
        "/api/admin/experiments/secret-config/secrets/reveal",
        json!({"key": "game.copy_key"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(revealed["value"], "copy-me");
}

/// Confirms endpoint defaults are copied once and explicit experiment endpoints always win.
#[tokio::test]
async fn experiment_provider_endpoints_are_revisioned_and_override_game_defaults() {
    let (config, _tmp) = sqlite_config();
    let descriptor = GameMetadata {
        id: "tiny-game".to_string(),
        name: "Tiny Game".to_string(),
        version: semver::Version::parse("0.4.0").unwrap(),
        build_manifest: json!({}),
    };
    let router = super::build_game_router(live_game(), config, descriptor, |_| {
        Ok(ServeOptions::default())
    })
    .await
    .map(TestRouter::new)
    .unwrap();
    authenticate_test_admin(router.clone()).await.unwrap();

    let (status, first_defaults) = admin_json_request(
        router.clone(),
        http::Method::POST,
        "/api/admin/game/settings",
        json!({
            "expected_revision": 1,
            "institution": "",
            "admin_allowed_ip_ranges": [],
            "speechmatics_realtime_url": "wss://speech-default-one.test/v2",
            "tts_base_url": "wss://tts-default-one.test",
            "secret_updates": {},
            "secret_deletions": []
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{first_defaults}");

    let mut sparse = serde_json::to_value(ExperimentConfig::default()).unwrap();
    sparse["speechmatics"]
        .as_object_mut()
        .unwrap()
        .remove("realtime_url");
    sparse["tts"].as_object_mut().unwrap().remove("base_url");
    let (status, created) = admin_json_request(
        router.clone(),
        http::Method::POST,
        "/api/admin/experiments",
        json!({"experiment_id": "inherited-endpoints", "config": sparse}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{created}");

    let (status, inherited) = admin_json_request(
        router.clone(),
        http::Method::GET,
        "/api/admin/experiments/inherited-endpoints/config",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        inherited["experiment"]["config"]["speechmatics"]["realtime_url"],
        "wss://speech-default-one.test/v2"
    );
    assert_eq!(
        inherited["experiment"]["config"]["tts"]["base_url"],
        "wss://tts-default-one.test"
    );

    let (status, second_defaults) = admin_json_request(
        router.clone(),
        http::Method::POST,
        "/api/admin/game/settings",
        json!({
            "expected_revision": 2,
            "institution": "",
            "admin_allowed_ip_ranges": [],
            "speechmatics_realtime_url": "wss://speech-default-two.test/v2",
            "tts_base_url": "wss://tts-default-two.test",
            "secret_updates": {},
            "secret_deletions": []
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{second_defaults}");

    let (status, unchanged) = admin_json_request(
        router.clone(),
        http::Method::GET,
        "/api/admin/experiments/inherited-endpoints/config",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        unchanged["experiment"]["config"]["speechmatics"]["realtime_url"],
        "wss://speech-default-one.test/v2"
    );
    assert_eq!(
        unchanged["experiment"]["config"]["tts"]["base_url"],
        "wss://tts-default-one.test"
    );

    let mut explicit = serde_json::to_value(ExperimentConfig::default()).unwrap();
    explicit["speechmatics"]["realtime_url"] = json!("wss://speech-experiment.test/v2");
    explicit["tts"]["base_url"] = json!("wss://tts-experiment.test");
    let (status, created) = admin_json_request(
        router.clone(),
        http::Method::POST,
        "/api/admin/experiments",
        json!({"experiment_id": "explicit-endpoints", "config": explicit}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let (status, selected) = admin_json_request(
        router,
        http::Method::GET,
        "/api/admin/experiments/explicit-endpoints/config",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        selected["experiment"]["config"]["speechmatics"]["realtime_url"],
        "wss://speech-experiment.test/v2"
    );
    assert_eq!(
        selected["experiment"]["config"]["tts"]["base_url"],
        "wss://tts-experiment.test"
    );
}

/// Confirms the first experiment in a new game starts with editable provider URLs populated.
#[tokio::test]
async fn new_experiment_populates_default_provider_endpoints() {
    let (config, _tmp) = sqlite_config();
    let descriptor = GameMetadata {
        id: "tiny-game".to_string(),
        name: "Tiny Game".to_string(),
        version: semver::Version::parse("0.4.0").unwrap(),
        build_manifest: json!({}),
    };
    let router = super::build_game_router(live_game(), config, descriptor, |_| {
        Ok(ServeOptions::default())
    })
    .await
    .map(TestRouter::new)
    .unwrap();
    authenticate_test_admin(router.clone()).await.unwrap();

    let (status, created) = admin_json_request(
        router.clone(),
        http::Method::POST,
        "/api/admin/experiments",
        json!({"experiment_id": "default-provider-endpoints"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{created}");

    let (status, response) = admin_json_request(
        router,
        http::Method::GET,
        "/api/admin/experiments/default-provider-endpoints/config",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        response["experiment"]["config"]["speechmatics"]["realtime_url"],
        "wss://eu.rt.speechmatics.com/v2"
    );
    assert_eq!(
        response["experiment"]["config"]["tts"]["base_url"],
        "wss://api.elevenlabs.io"
    );
}

/// Proves sentinel provider credentials cannot cross the durable configuration boundary.
#[test]
fn persisted_configuration_redacts_provider_credentials() {
    let mut config = step_five_config();
    config.speechmatics.realtime_url = "wss://speech-experiment.test/v2".to_string();
    config.speechmatics.api_key = "speechmatics-sentinel-secret".to_string();
    config.tts.base_url = "wss://tts-experiment.test".to_string();
    config.tts.api_key = "elevenlabs-sentinel-secret".to_string();
    config.agents.human_vs_agent = Some(crate::config::HumanVsAgentConfig {
        config: json!({
            "auth_token": "nested-auth-sentinel",
            "client_secret": "nested-client-sentinel",
            "private_key": "nested-key-sentinel",
        }),
        ..Default::default()
    });

    let serialized = serde_json::to_string(&persistable_config_json(&config).unwrap()).unwrap();

    assert!(!serialized.contains("speechmatics-sentinel-secret"));
    assert!(!serialized.contains("elevenlabs-sentinel-secret"));
    assert!(!serialized.contains("nested-auth-sentinel"));
    assert!(!serialized.contains("nested-client-sentinel"));
    assert!(!serialized.contains("nested-key-sentinel"));
    assert!(serialized.contains("wss://speech-experiment.test/v2"));
    assert!(serialized.contains("wss://tts-experiment.test"));
    assert!(!serialized.contains("\"experiment\""));
    assert!(!serialized.contains("\"server\""));
    assert!(!serialized.contains("\"database\""));
}

/// Imprint text and selected country survive stored revisions with country and size validation.
#[test]
fn imprint_configuration_round_trips_and_validates() {
    let mut config = step_five_config();
    config.direct.imprint.country = "FR".to_owned();
    config.direct.imprint.text = "Reviewed public notice".to_owned();
    let stored = persistable_config_json(&config).unwrap();
    let restored = experiment_config_from_json(stored, &config, "step5").unwrap();
    assert_eq!(restored.direct.imprint.text, "Reviewed public notice");
    config.direct.imprint.country = "ZZ".to_owned();
    assert!(config.validate().is_err());
    config.direct.imprint.country = "DE".to_owned();
    config.direct.imprint.text = "a".repeat(20_001);
    assert!(config.validate().is_err());
}
