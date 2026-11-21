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
            tiny_game(),
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

/// Independent Wasm sessions each initialize a fresh counter for every expanded plan.
#[tokio::test]
async fn each_planned_session_starts_from_a_fresh_counter() {
    let temporary = TempDir::new().unwrap();
    let output = temporary.path().join("factory-output");
    let yaml_path = temporary.path().join("factory.yaml");
    tokio::fs::write(
        &yaml_path,
        experiment_yaml(&output).replace("seeds: [7]", "seeds: [7, 8]"),
    )
    .await
    .unwrap();
    let runner = ExperimentRunner::new(
        GameMetadata {
            id: "tiny".to_string(),
            name: "Tiny".to_string(),
            version: semver::Version::new(1, 0, 0),
            build_manifest: json!({"test": true}),
        },
        tiny_game(),
    )
    .unwrap()
    .agent(TinyAgentFactory {
        events: Arc::new(Mutex::new(Vec::new())),
        fail_role: None,
        increments: vec![1],
    })
    .unwrap();

    let summary = runner.run_yaml(&yaml_path).await.unwrap();
    assert_eq!(summary.planned, 2);
    assert_eq!(summary.completed, 2);
}

/// Fail-fast finalizes the first failure before returning and starts no later plan.
#[tokio::test]
async fn fail_fast_finalizes_only_the_first_failed_plan() {
    let temporary = TempDir::new().unwrap();
    let output = temporary.path().join("results-root");
    let yaml_path = temporary.path().join("fail-fast.yaml");
    let yaml = experiment_yaml(&output)
        .replace("seeds: [7]", "seeds: [7, 8, 9]")
        .replace("concurrency: 2", "concurrency: 3\n  fail_fast: true");
    tokio::fs::write(&yaml_path, yaml).await.unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let error = failing_test_runner(events.clone())
        .run_yaml(&yaml_path)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("stopped after a failed session"));
    assert_eq!(
        events.lock().unwrap().as_slice(),
        ["create:A", "create:B", "shutdown:A"]
    );
    let results: Vec<_> = std::fs::read_dir(output.join("results")).unwrap().collect();
    assert_eq!(results.len(), 1);
    let bytes = std::fs::read(results[0].as_ref().unwrap().path()).unwrap();
    let result: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(result["status"], "failed");
    assert_eq!(result["game_seed"], 7);
}

/// A final move wins at the action limit. Otherwise an action cap, decision cap
/// or two consecutive yields each produces its own durable status and counts.
#[tokio::test]
async fn decision_loop_persists_the_cause_and_exact_counters() {
    for (target, action_limit, decision_limit, status, decisions) in [
        (1, 1, 1, "completed", 1),
        (2, 1, 4, "action_limit", 1),
        (2, 2, 2, "decision_limit", 2),
        (2, 2, 4, "quiescent", 3),
    ] {
        let temporary = TempDir::new().unwrap();
        let output = temporary.path().join("run");
        let yaml_path = temporary.path().join("experiment.yaml");
        let yaml = experiment_yaml(&output)
            .replace("target: 1", &format!("target: {target}"))
            .replace(
                "accepted_actions: 2",
                &format!("accepted_actions: {action_limit}"),
            )
            .replace("decisions: 4", &format!("decisions: {decision_limit}"));
        tokio::fs::write(&yaml_path, yaml).await.unwrap();
        let summary = test_runner(Arc::new(Mutex::new(Vec::new())))
            .run_yaml(yaml_path)
            .await
            .unwrap();
        let success = matches!(status, "completed" | "quiescent");
        assert_eq!(
            (summary.completed, summary.failed),
            (usize::from(success), usize::from(!success))
        );
        let path = std::fs::read_dir(output.join("results"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let result: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(result["status"], status);
        assert_eq!(result["decisions"], decisions);
        assert_eq!(result["accepted_actions"], 1);
        assert_eq!(result["completion"].is_object(), status == "completed");
    }
}

/// The runner uses role views from accepted steps and distinguishes rejections from faults.
#[tokio::test]
async fn game_step_rejections_and_faults_have_distinct_session_effects() {
    let temporary = TempDir::new().unwrap();
    let output = temporary.path().join("run");
    let yaml_path = temporary.path().join("experiment.yaml");
    let yaml = experiment_yaml(&output)
        .replace("target: 1", "target: 2")
        .replace("decisions: 4", "decisions: 5")
        .replace("trace: decisions", "trace: full");
    tokio::fs::write(&yaml_path, yaml).await.unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let summary = test_runner_with_increments(events.clone(), vec![0, 1, u8::MAX])
        .run_yaml(yaml_path)
        .await
        .unwrap();
    assert_eq!(summary.failed, 1);
    let path = std::fs::read_dir(output.join("results"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let result: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(result["status"], "failed");
    assert_eq!(result["failure_phase"], "step");
    assert_eq!(result["failure_role"], "A");
    assert!(result["diagnostic"].as_str().unwrap().contains("wasm trap"));
    assert_eq!(result["decisions"], 5);
    assert_eq!(result["accepted_actions"], 1);
    assert_eq!(result["rejected_actions"], 1);
    let trace = result["trace"].as_array().unwrap();
    assert_eq!(trace.len(), 4);
    assert_eq!(trace[0]["rejection"]["code"], "zero_increment");
    assert_eq!(
        trace[1]["input"]["observation"],
        json!({"count": 0, "role": "B"})
    );
    assert_eq!(
        trace[2]["input"]["observation"],
        json!({"count": 0, "role": "A"})
    );
    assert_eq!(
        trace[3]["input"]["observation"],
        json!({"count": 1, "role": "B"})
    );
    assert_eq!(trace[1]["input"]["available_actions"], Value::Null);
    assert_eq!(trace[3]["input"]["available_actions"], Value::Null);
    assert_eq!(
        events.lock().unwrap().as_slice(),
        [
            "create:A",
            "create:B",
            "start:A",
            "start:B",
            "observe:A",
            "observe:B",
            "shutdown:A",
            "shutdown:B",
        ]
    );
}
