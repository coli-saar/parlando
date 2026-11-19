use super::*;

/// Confirms YAML does not expose the internal ordinary-versus-RL distinction.
#[test]
fn yaml_rejects_obsolete_agent_kind_discriminator() {
    let yaml = experiment_yaml(Path::new("results")).replace(
        "    factory: tiny.agent",
        "    kind: factory\n    factory: tiny.agent",
    );
    let error = serde_yaml::from_str::<ExperimentFile>(&yaml).unwrap_err();
    assert!(error.to_string().contains("unknown field `kind`"));
}

/// Exercises the built-in policy's distinct message, rejection, action, and yield handoffs.
#[test]
fn alternate_after_action_uses_the_original_agent_response() {
    let mut schedule = AlternateAfterAction::new(PlayerRole::A);
    assert_eq!(
        <AlternateAfterAction as AgentSchedule<TinyGame>>::next(
            &mut schedule,
            PlayerRole::A,
            Some(&AgentResponse::message("hello")),
            None,
        ),
        Some(PlayerRole::A)
    );
    let action = AgentResponse::action(TinyAction { amount: 1 });
    assert_eq!(
        <AlternateAfterAction as AgentSchedule<TinyGame>>::next(
            &mut schedule,
            PlayerRole::A,
            Some(&action),
            Some(&ActionRejection::new("blocked")),
        ),
        Some(PlayerRole::A)
    );
    assert_eq!(
        <AlternateAfterAction as AgentSchedule<TinyGame>>::next(
            &mut schedule,
            PlayerRole::A,
            Some(&action),
            None,
        ),
        Some(PlayerRole::B)
    );
    assert_eq!(
        <AlternateAfterAction as AgentSchedule<TinyGame>>::next(
            &mut schedule,
            PlayerRole::A,
            None,
            None,
        ),
        Some(PlayerRole::B)
    );
    assert_eq!(
        <AlternateAfterAction as AgentSchedule<TinyGame>>::next(
            &mut schedule,
            PlayerRole::B,
            None,
            None,
        ),
        None
    );
}

/// Canonical hashing ignores object insertion order while retaining array order.
#[test]
fn canonical_value_hashes_object_order_stably() {
    let first = json!({"outer": {"b": 2, "a": 1}, "items": [1, 2]});
    let second = json!({"items": [1, 2], "outer": {"a": 1, "b": 2}});
    let reordered_array = json!({"items": [2, 1], "outer": {"a": 1, "b": 2}});
    assert_eq!(hash_value(&first).unwrap(), hash_value(&second).unwrap());
    assert_ne!(
        hash_value(&first).unwrap(),
        hash_value(&reordered_array).unwrap()
    );
}

/// The every-response schedule alternates after all responses and ends after both yield.
#[test]
fn alternate_every_response_has_deterministic_handoffs() {
    let mut schedule = AlternateEveryResponse::new(PlayerRole::B);
    assert_eq!(
        <AlternateEveryResponse as AgentSchedule<TinyGame>>::first(&mut schedule),
        PlayerRole::B
    );
    let message = AgentResponse::message("hello");
    assert_eq!(
        <AlternateEveryResponse as AgentSchedule<TinyGame>>::next(
            &mut schedule,
            PlayerRole::B,
            Some(&message),
            None
        ),
        Some(PlayerRole::A)
    );
    assert_eq!(
        <AlternateEveryResponse as AgentSchedule<TinyGame>>::next(
            &mut schedule,
            PlayerRole::A,
            None,
            None
        ),
        Some(PlayerRole::B)
    );
    assert_eq!(
        <AlternateEveryResponse as AgentSchedule<TinyGame>>::next(
            &mut schedule,
            PlayerRole::B,
            None,
            None
        ),
        None
    );
}

/// Rejects invalid experiment topology and limits before creating output or agents.
#[test]
fn experiment_validation_rejects_invalid_topology_and_limits() {
    let base = experiment_yaml(Path::new("unused-output"));
    let cases = [
        (
            base.replacen(EXPERIMENT_SCHEMA, "parlando-agent-experiment/v99", 1),
            "unsupported agent experiment schema",
        ),
        (
            base.replacen("game:\n  id: tiny", "game:\n  id: another", 1),
            "compiled for",
        ),
        (
            base.replacen("concurrency: 2", "concurrency: 0", 1),
            "execution.concurrency must be positive",
        ),
        (
            base.replacen("decisions: 4", "decisions: 0", 1),
            "all session limits must be positive",
        ),
        (
            base.replacen(
                "callback_timeout_seconds: 1",
                "callback_timeout_seconds: 6",
                1,
            ),
            "callback timeout must not exceed",
        ),
        (
            base.replacen("player_b: policy", "player_b: missing", 1),
            "unknown agent",
        ),
        (
            base.replacen("kind: alternate_every_response", "kind: missing", 1),
            "unknown agent schedule",
        ),
        (
            base.replacen("seeds: [7]", "seeds: []", 1),
            "names, seeds, and repetitions must be non-empty",
        ),
    ];

    for (yaml, expected) in cases {
        let spec: ExperimentFile = serde_yaml::from_str(&yaml).unwrap();
        let runner = test_runner(Arc::new(Mutex::new(Vec::new())));
        let error = runner.validate_spec(&spec).unwrap_err().to_string();
        assert!(
            error.contains(expected),
            "{error:?} did not contain {expected:?}"
        );
    }
}
