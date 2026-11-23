use super::*;

/// Verifies execution, immutable result identity, lifecycle cleanup, and resume.
#[tokio::test]
async fn yaml_run_executes_once_and_resume_skips_finalized_result() {
    let temporary = TempDir::new().unwrap();
    let output = temporary.path().join("output");
    let yaml_path = temporary.path().join("experiment.yaml");
    tokio::fs::write(&yaml_path, experiment_yaml(&output))
        .await
        .unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));

    let first = test_runner(events.clone())
        .run_yaml(&yaml_path)
        .await
        .unwrap();
    assert_eq!(first.planned, 1);
    assert_eq!(first.completed, 1);
    assert_eq!(first.failed, 0);
    assert_eq!(first.skipped, 0);

    let result_path = std::fs::read_dir(output.join("results"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let result: Value =
        serde_json::from_slice(&tokio::fs::read(result_path).await.unwrap()).unwrap();
    assert_eq!(result["run_id"], first.run_id);
    assert!(result["plan_id"].as_str().unwrap().starts_with("sha256-"));
    assert!(result.get("session_id").is_none());
    assert_eq!(result["status"], "completed");

    let second = test_runner(events.clone())
        .run_yaml(&yaml_path)
        .await
        .unwrap();
    assert_eq!(second.run_id, first.run_id);
    assert_eq!(second.skipped, 1);
    assert_eq!(second.completed, 0);
    assert_eq!(
        events.lock().unwrap().as_slice(),
        [
            "create:A",
            "create:B",
            "start:A",
            "start:B",
            "observe:A",
            "observe:B",
            "finish:A",
            "finish:B",
            "shutdown:A",
            "shutdown:B",
        ]
    );
}

/// Confirms a new run ID does not change an equivalent plan ID.
#[tokio::test]
async fn plan_identity_is_stable_across_distinct_runs() {
    let temporary = TempDir::new().unwrap();
    let first_output = temporary.path().join("first");
    let second_output = temporary.path().join("second");
    let first_yaml = temporary.path().join("first.yaml");
    let second_yaml = temporary.path().join("second.yaml");
    tokio::fs::write(&first_yaml, experiment_yaml(&first_output))
        .await
        .unwrap();
    tokio::fs::write(&second_yaml, experiment_yaml(&second_output))
        .await
        .unwrap();

    let first = test_runner(Arc::new(Mutex::new(Vec::new())))
        .run_yaml(first_yaml)
        .await
        .unwrap();
    let second = test_runner(Arc::new(Mutex::new(Vec::new())))
        .run_yaml(second_yaml)
        .await
        .unwrap();
    assert_ne!(first.run_id, second.run_id);

    let first_name = std::fs::read_dir(first_output.join("results"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .file_name();
    let second_name = std::fs::read_dir(second_output.join("results"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .file_name();
    assert_eq!(first_name, second_name);
}

/// Verifies constructor failure is finalized once and cleans up the first agent.
#[tokio::test]
async fn failed_session_is_finalized_and_not_retried_on_resume() {
    let temporary = TempDir::new().unwrap();
    let output = temporary.path().join("output");
    let yaml_path = temporary.path().join("experiment.yaml");
    tokio::fs::write(&yaml_path, experiment_yaml(&output))
        .await
        .unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));

    let first = failing_test_runner(events.clone())
        .run_yaml(&yaml_path)
        .await
        .unwrap();
    assert_eq!(first.failed, 1);
    assert_eq!(first.skipped, 0);
    assert_eq!(
        events.lock().unwrap().as_slice(),
        ["create:A", "create:B", "shutdown:A"]
    );

    let second = failing_test_runner(events.clone())
        .run_yaml(&yaml_path)
        .await
        .unwrap();
    assert_eq!(second.run_id, first.run_id);
    assert_eq!(second.failed, 0);
    assert_eq!(second.skipped, 1);
    assert_eq!(
        events.lock().unwrap().as_slice(),
        ["create:A", "create:B", "shutdown:A"]
    );
}

/// Existing output belongs to one immutable normalized experiment specification.
#[tokio::test]
async fn resume_rejects_corrupt_or_mismatched_run_manifests() {
    let temporary = TempDir::new().unwrap();
    let output = temporary.path().join("output");
    let yaml_path = temporary.path().join("experiment.yaml");
    tokio::fs::write(&yaml_path, experiment_yaml(&output))
        .await
        .unwrap();
    test_runner(Arc::new(Mutex::new(Vec::new())))
        .run_yaml(&yaml_path)
        .await
        .unwrap();

    tokio::fs::write(output.join("run.json"), b"{truncated")
        .await
        .unwrap();
    let corrupt = test_runner(Arc::new(Mutex::new(Vec::new())))
        .run_yaml(&yaml_path)
        .await
        .unwrap_err()
        .to_string();
    assert!(corrupt.contains("run.json") || corrupt.contains("expected"));

    tokio::fs::write(
        &yaml_path,
        experiment_yaml(&output).replace("trace: decisions", "trace: full"),
    )
    .await
    .unwrap();
    let stale_manifest = json!({
        "schema": EXPERIMENT_SCHEMA, "run_id": Uuid::new_v4().to_string(), "game_id": "tiny",
        "game_version": "1.0.0", "spec_hash": "sha256-stale",
    });
    tokio::fs::write(
        output.join("run.json"),
        serde_json::to_vec(&stale_manifest).unwrap(),
    )
    .await
    .unwrap();
    let mismatch = test_runner(Arc::new(Mutex::new(Vec::new())))
        .run_yaml(&yaml_path)
        .await
        .unwrap_err()
        .to_string();
    assert!(mismatch.contains("different experiment specification"));
}
