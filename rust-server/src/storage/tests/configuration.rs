use super::*;

/// Confirms secret changes are atomic with revisions and never enter revision JSON.
#[tokio::test]
async fn sqlite_stores_experiment_secrets_outside_configuration_revisions() {
    let store = SqliteExperimentStore::connect("sqlite:///:memory:")
        .await
        .unwrap();
    store
        .create_experiment(ExperimentRecord {
            experiment_id: "secrets".to_string(),
            game_version: "0.4.0".to_string(),
            config: json!({"game": {"difficulty": 2}}),
            server_version: None,
            version_manifest: None,
            status: "inactive".to_string(),
            notes: None,
        })
        .await
        .unwrap();
    store
        .save_experiment_configuration(
            "secrets",
            1,
            json!({"game": {"difficulty": 3}}),
            Some("Configure provider".to_string()),
            HashMap::from([("game.service_token".to_string(), "game-secret".to_string())]),
            vec![],
        )
        .await
        .unwrap();

    let secrets = store.experiment_secrets("secrets").await.unwrap();
    assert_eq!(secrets["game.service_token"], "game-secret");
    let revisions = store.experiment_revisions("secrets").await.unwrap();
    assert!(!serde_json::to_string(&revisions)
        .unwrap()
        .contains("game-secret"));

    store
        .save_experiment_configuration(
            "secrets",
            2,
            json!({"game": {"difficulty": 3}}),
            None,
            HashMap::new(),
            vec!["game.service_token".to_string()],
        )
        .await
        .unwrap();
    assert!(!store
        .experiment_secrets("secrets")
        .await
        .unwrap()
        .contains_key("game.service_token"));
}

/// Confirms active experiment configuration and secrets cannot change at the storage boundary.
#[tokio::test]
async fn sqlite_rejects_configuration_changes_for_active_experiments() {
    let store = SqliteExperimentStore::connect("sqlite:///:memory:")
        .await
        .unwrap();
    store
        .create_experiment(ExperimentRecord {
            experiment_id: "active-config".to_string(),
            game_version: "0.4.0".to_string(),
            config: json!({"game": {"difficulty": 2}}),
            server_version: None,
            version_manifest: None,
            status: "inactive".to_string(),
            notes: None,
        })
        .await
        .unwrap();
    store
        .update_experiment_status("active-config", "active")
        .await
        .unwrap();

    let error = store
        .save_experiment_configuration(
            "active-config",
            1,
            json!({"game": {"difficulty": 3}}),
            Some("Must not persist".to_string()),
            HashMap::from([(
                "game.service_token".to_string(),
                "must-not-persist".to_string(),
            )]),
            vec![],
        )
        .await
        .unwrap_err();

    assert!(error.to_string().contains("not inactive"));
    let definition = store
        .experiment_definition("active-config")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(definition.config_revision, 1);
    assert_eq!(definition.config["game"]["difficulty"], 2);
    assert!(store
        .experiment_secrets("active-config")
        .await
        .unwrap()
        .is_empty());
}

/// Confirms a newly initialized game exposes ready-to-use provider endpoint defaults.
#[tokio::test]
async fn sqlite_initializes_provider_endpoint_defaults() {
    let store = SqliteExperimentStore::connect("sqlite:///:memory:")
        .await
        .unwrap();

    let settings = store.game_settings().await.unwrap();

    assert_eq!(
        settings.speechmatics_realtime_url,
        "wss://eu.rt.speechmatics.com/v2"
    );
    assert_eq!(settings.tts_base_url, "wss://api.elevenlabs.io");
}

/// Confirms process startup closes only lifecycle states that permit intake.
#[tokio::test]
async fn sqlite_deactivates_every_open_experiment_without_changing_terminal_states() {
    let store = SqliteExperimentStore::connect("sqlite:///:memory:")
        .await
        .unwrap();
    for (experiment_id, status) in [
        ("active-one", "active"),
        ("testing-one", "testing"),
        ("completed-one", "completed"),
        ("archived-one", "archived"),
    ] {
        store
            .create_experiment(ExperimentRecord {
                experiment_id: experiment_id.to_string(),
                game_version: "0.4.0".to_string(),
                config: json!({}),
                server_version: None,
                version_manifest: None,
                status: status.to_string(),
                notes: None,
            })
            .await
            .unwrap();
        store
            .update_experiment_status(experiment_id, status)
            .await
            .unwrap();
        if status != "inactive" {
            store
                .update_experiment_status(experiment_id, status)
                .await
                .unwrap();
        }
    }

    assert_eq!(store.deactivate_open_experiments().await.unwrap(), 2);
    let experiments = store.list_experiments(10).await.unwrap();
    let statuses = experiments
        .into_iter()
        .map(|experiment| (experiment.experiment_id, experiment.status))
        .collect::<HashMap<_, _>>();
    assert_eq!(statuses["active-one"], "inactive");
    assert_eq!(statuses["testing-one"], "inactive");
    assert_eq!(statuses["completed-one"], "completed");
    assert_eq!(statuses["archived-one"], "archived");
}

/// Confirms storage-only archival permits only closed catalogue lifecycle states.
#[tokio::test]
async fn sqlite_archives_only_inactive_or_completed_experiments() {
    let store = SqliteExperimentStore::connect("sqlite:///:memory:")
        .await
        .unwrap();
    for (experiment_id, status) in [
        ("inactive", "inactive"),
        ("completed", "completed"),
        ("active", "active"),
        ("testing", "testing"),
        ("archived", "archived"),
    ] {
        store
            .create_experiment(ExperimentRecord {
                experiment_id: experiment_id.to_string(),
                game_version: "0.4.0".to_string(),
                config: json!({}),
                server_version: None,
                version_manifest: None,
                status: status.to_string(),
                notes: None,
            })
            .await
            .unwrap();
        if status != "inactive" {
            store
                .update_experiment_status(experiment_id, status)
                .await
                .unwrap();
        }
    }
    store.archive_experiment("inactive").await.unwrap();
    store.archive_experiment("completed").await.unwrap();
    assert!(store
        .archive_experiment("active")
        .await
        .unwrap_err()
        .to_string()
        .contains("active"));
    assert!(store
        .archive_experiment("testing")
        .await
        .unwrap_err()
        .to_string()
        .contains("testing"));
    assert!(store
        .archive_experiment("archived")
        .await
        .unwrap_err()
        .to_string()
        .contains("already archived"));
    assert!(store
        .archive_experiment("missing")
        .await
        .unwrap_err()
        .to_string()
        .contains("not found"));
}

/// Confirms catalogue metadata and immutable revisions support dashboard-owned configuration.
#[tokio::test]
async fn sqlite_stores_multi_experiment_catalogue_and_revisions() {
    let store = SqliteExperimentStore::connect("sqlite:///:memory:")
        .await
        .unwrap();
    store
        .create_experiment(ExperimentRecord {
            experiment_id: "pilot".to_string(),
            game_version: "0.4.0".to_string(),
            config: json!({"study": {"name": "Pilot"}}),
            server_version: Some("0.2.0".to_string()),
            version_manifest: None,
            status: "inactive".to_string(),
            notes: None,
        })
        .await
        .unwrap();

    let revision = store
        .save_experiment_configuration(
            "pilot",
            1,
            json!({"study": {"name": "Revised pilot"}}),
            Some("Clarified title".to_string()),
            HashMap::new(),
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(revision, 2);
    assert!(store
        .save_experiment_configuration("pilot", 1, Value::Null, None, HashMap::new(), vec![],)
        .await
        .is_err());

    store
        .update_experiment_catalogue("pilot", true, Some("Important".to_string()))
        .await
        .unwrap();
    let experiments = store.list_experiments(100).await.unwrap();
    assert_eq!(experiments.len(), 1);
    assert_eq!(experiments[0].game_version, "0.4.0");
    assert_eq!(experiments[0].config_revision, 2);
    assert!(experiments[0].pinned);
    assert_eq!(experiments[0].notes.as_deref(), Some("Important"));

    let revisions = store.experiment_revisions("pilot").await.unwrap();
    assert_eq!(revisions.len(), 2);
    assert_eq!(revisions[0].revision, 2);
    assert_eq!(revisions[1].revision, 1);
}

/// Confirms shared institution settings use optimistic concurrency.
#[tokio::test]
async fn sqlite_game_settings_reject_stale_updates() {
    let store = SqliteExperimentStore::connect("sqlite:///:memory:")
        .await
        .unwrap();
    let settings = store.game_settings().await.unwrap();
    let mut provider_updates = HashMap::new();
    provider_updates.insert(
        "speechmatics.api_key".to_string(),
        "shared-speechmatics-key".to_string(),
    );
    let revision = store
        .update_game_settings(GameSettingsUpdate {
            expected_revision: settings.revision,
            institution: "Saarland University".to_string(),
            admin_allowed_ip_ranges: vec!["192.0.2.0/24".to_string()],
            speechmatics_realtime_url: "wss://eu.rt.speechmatics.com/v2".to_string(),
            tts_base_url: "wss://api.elevenlabs.io".to_string(),
            prolific_api_base_url: "https://api.prolific.com".to_string(),
            secret_updates: provider_updates,
            secret_deletions: vec![],
        })
        .await
        .unwrap();
    assert_eq!(revision, settings.revision + 1);
    assert_eq!(
        store.game_settings().await.unwrap().institution,
        "Saarland University"
    );
    assert_eq!(
        store.game_settings().await.unwrap().admin_allowed_ip_ranges,
        vec!["192.0.2.0/24"]
    );
    assert_eq!(
        store
            .game_secrets()
            .await
            .unwrap()
            .get("speechmatics.api_key")
            .map(String::as_str),
        Some("shared-speechmatics-key")
    );
    assert!(store
        .update_game_settings(GameSettingsUpdate {
            expected_revision: settings.revision,
            institution: "Stale".to_string(),
            admin_allowed_ip_ranges: vec![],
            speechmatics_realtime_url: "wss://eu.rt.speechmatics.com/v2".to_string(),
            tts_base_url: "wss://api.elevenlabs.io".to_string(),
            prolific_api_base_url: "https://api.prolific.com".to_string(),
            secret_updates: HashMap::new(),
            secret_deletions: vec![],
        })
        .await
        .is_err());
}
