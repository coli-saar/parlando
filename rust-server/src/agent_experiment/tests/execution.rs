use super::*;

/// A pending agent callback becomes a finalized failure instead of wedging execution or resume.
#[tokio::test]
async fn callback_timeout_is_bounded_finalized_and_skipped_on_resume() {
    let temporary = TempDir::new().unwrap();
    let output = temporary.path().join("timeout-output");
    let yaml_path = temporary.path().join("timeout.yaml");
    let yaml = experiment_yaml(&output).replace("factory: tiny.agent", "factory: tiny.hanging");
    tokio::fs::write(&yaml_path, yaml).await.unwrap();
    let runner = || {
        ExperimentRunner::new(
            GameMetadata {
                id: "tiny".to_string(),
                name: "Tiny".to_string(),
                version: semver::Version::new(1, 0, 0),
                build_manifest: json!({"test": true}),
            },
            TinyGameFactory::default(),
        )
        .unwrap()
        .agent(HangingStartFactory)
        .unwrap()
    };

    let first = tokio::time::timeout(Duration::from_secs(3), runner().run_yaml(&yaml_path))
        .await
        .expect("runner exceeded its outer test deadline")
        .unwrap();
    assert_eq!(first.failed, 1);
    let result_path = std::fs::read_dir(output.join("results"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let result: Value =
        serde_json::from_slice(&tokio::fs::read(result_path).await.unwrap()).unwrap();
    assert_eq!(result["status"], "failed");
    assert_eq!(result["failure_phase"], "start");
    assert_eq!(result["failure_role"], "A");
    assert!(result["diagnostic"].as_str().unwrap().contains("timed out"));

    let resumed = runner().run_yaml(&yaml_path).await.unwrap();
    assert_eq!(resumed.skipped, 1);
    assert_eq!(resumed.failed, 0);
}

/// Result and trace artifacts retain role-safe provenance without raw config or runner secrets.
#[tokio::test]
async fn result_artifacts_exclude_authoritative_config_and_secrets() {
    let temporary = TempDir::new().unwrap();
    let output = temporary.path().join("private-output");
    let yaml_path = temporary.path().join("private.yaml");
    tokio::fs::write(
        &yaml_path,
        experiment_yaml(&output).replace("trace: decisions", "trace: full"),
    )
    .await
    .unwrap();
    test_runner(Arc::new(Mutex::new(Vec::new())))
        .secret("unused", "sentinel-secret-value")
        .unwrap()
        .run_yaml(&yaml_path)
        .await
        .unwrap();

    let mut artifact_text = tokio::fs::read_to_string(output.join("run.json"))
        .await
        .unwrap();
    let mut results = Vec::new();
    for entry in std::fs::read_dir(output.join("results")).unwrap() {
        let text = tokio::fs::read_to_string(entry.unwrap().path())
            .await
            .unwrap();
        results.push(serde_json::from_str::<Value>(&text).unwrap());
        artifact_text.push_str(&text);
    }
    assert!(!artifact_text.contains("sentinel-secret-value"));
    assert!(!artifact_text.contains("\"target\":1"));
    assert!(artifact_text.contains("settings_fingerprint"));
    assert_eq!(results[0]["agents"][0]["role"], "A");
    assert_eq!(results[0]["agents"][1]["role"], "B");
}

/// Session planning asks the reusable game factory for one fresh value per expanded plan.
#[tokio::test]
async fn game_factory_creates_exactly_one_instance_per_planned_session() {
    let temporary = TempDir::new().unwrap();
    let output = temporary.path().join("factory-output");
    let yaml_path = temporary.path().join("factory.yaml");
    tokio::fs::write(
        &yaml_path,
        experiment_yaml(&output).replace("seeds: [7]", "seeds: [7, 8]"),
    )
    .await
    .unwrap();
    let creations = Arc::new(AtomicUsize::new(0));
    let factory = TinyGameFactory {
        creations: creations.clone(),
    };
    let runner = ExperimentRunner::new(
        GameMetadata {
            id: "tiny".to_string(),
            name: "Tiny".to_string(),
            version: semver::Version::new(1, 0, 0),
            build_manifest: json!({"test": true}),
        },
        factory,
    )
    .unwrap()
    .agent(TinyAgentFactory {
        events: Arc::new(Mutex::new(Vec::new())),
        fail_role: None,
    })
    .unwrap();

    let summary = runner.run_yaml(&yaml_path).await.unwrap();
    assert_eq!(summary.planned, 2);
    assert_eq!(creations.load(Ordering::Acquire), 2);
}
