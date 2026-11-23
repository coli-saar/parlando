use super::*;

/// Confirms lifecycle expiry is terminal, durable, and analytically classified.
#[tokio::test]
async fn sqlite_session_expiry_records_reason_and_status_atomically() {
    let store = SqliteExperimentStore::connect("sqlite:///:memory:")
        .await
        .unwrap();
    store
        .create_experiment(ExperimentRecord {
            experiment_id: "expiry".to_string(),
            game_version: "0.4.0".to_string(),
            config: json!({}),
            server_version: None,
            version_manifest: None,
            status: "inactive".to_string(),
            notes: None,
        })
        .await
        .unwrap();
    let session_id = store
        .create_session(SessionRecord {
            experiment_id: "expiry".to_string(),
            config_revision: 1,
            game_version: "0.4.0".to_string(),
            public_session_id: "EXPIRING".to_string(),
            mode: "direct".to_string(),
            lifecycle: "running".to_string(),
            purpose: "research".to_string(),
            waiting_timeout_seconds: 600,
            maximum_lifetime_seconds: 14_400,
        })
        .await
        .unwrap();

    store
        .end_session(
            SessionEventRecord {
                experiment_id: "expiry".to_string(),
                session_id,
                event_type: "session_ended".to_string(),
                actor_participant_id: None,
                actor_role: None,
                payload: json!({"reason": "idle_timeout"}),
                game_state: None,
            },
            test_session_end(SessionEndCause::IdleTimedOut, None),
        )
        .await
        .unwrap();
    let exported = experiment_rows(&store, "expiry").await;
    assert_eq!(exported["sessions"][0]["lifecycle"], "ended");
    assert_eq!(exported["session_events"].as_array().unwrap().len(), 1);
    assert_eq!(
        exported["session_events"][0]["payload"]["reason"],
        "idle_timeout"
    );
}

/// Proves terminal participant state remains recoverable after the live room has been removed.
#[tokio::test]
async fn sqlite_retains_recipient_terminal_state_for_reconciliation() {
    let store = SqliteExperimentStore::connect("sqlite:///:memory:")
        .await
        .unwrap();
    store
        .create_experiment(ExperimentRecord {
            experiment_id: "reconcile".to_string(),
            game_version: "0.4.0".to_string(),
            config: json!({}),
            server_version: None,
            version_manifest: None,
            status: "inactive".to_string(),
            notes: None,
        })
        .await
        .unwrap();
    let participant_id = store
        .upsert_participant(ParticipantRecord {
            experiment_id: "reconcile".to_string(),
            participant_kind: "human".to_string(),
            identity_provider: "direct".to_string(),
            external_id: None,
            metadata: Value::Null,
        })
        .await
        .unwrap();
    let session_id = store
        .create_session(SessionRecord {
            experiment_id: "reconcile".to_string(),
            config_revision: 1,
            game_version: "0.4.0".to_string(),
            public_session_id: "RETAINED".to_string(),
            mode: "direct".to_string(),
            lifecycle: "running".to_string(),
            purpose: "research".to_string(),
            waiting_timeout_seconds: 600,
            maximum_lifetime_seconds: 14_400,
        })
        .await
        .unwrap();
    store
        .add_session_participant(SessionParticipantRecord {
            experiment_id: "reconcile".to_string(),
            session_id,
            participant_id,
            participant_session_id: "participant-handle".to_string(),
            role: "A".to_string(),
            connection_status: "connected".to_string(),
        })
        .await
        .unwrap();
    let result = ParticipantResult {
        outcome: crate::protocol::ParticipantOutcomeKind::IdleLimitReached,
        reason: "idle_timeout".to_string(),
        completion: None,
        final_observation: Some(json!({"turn": 4})),
        handoff: None,
    };
    let mut results = HashMap::new();
    results.insert("A".to_string(), result.clone());
    store
        .end_session(
            SessionEventRecord {
                experiment_id: "reconcile".to_string(),
                session_id,
                event_type: "session_ended".to_string(),
                actor_participant_id: None,
                actor_role: None,
                payload: json!({"reason": "idle_timeout"}),
                game_state: None,
            },
            SessionEnd {
                cause: SessionEndCause::IdleTimedOut,
                completion: None,
                participant_results: results,
            },
        )
        .await
        .unwrap();

    let recovered = store
        .terminal_participant_state("reconcile", "participant-handle")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(recovered.public_session_id, "RETAINED");
    assert_eq!(recovered.role, "A");
    assert_eq!(recovered.result, result);
}

/// Confirms intentional departure has its own terminal status and durable actor event.
#[tokio::test]
async fn sqlite_session_abandonment_records_actor_and_status_atomically() {
    let store = SqliteExperimentStore::connect("sqlite:///:memory:")
        .await
        .unwrap();
    store
        .create_experiment(ExperimentRecord {
            experiment_id: "departure".to_string(),
            game_version: "0.4.0".to_string(),
            config: json!({}),
            server_version: None,
            version_manifest: None,
            status: "inactive".to_string(),
            notes: None,
        })
        .await
        .unwrap();
    let session_id = store
        .create_session(SessionRecord {
            experiment_id: "departure".to_string(),
            config_revision: 1,
            game_version: "0.4.0".to_string(),
            public_session_id: "ABANDONED".to_string(),
            mode: "direct".to_string(),
            lifecycle: "running".to_string(),
            purpose: "research".to_string(),
            waiting_timeout_seconds: 600,
            maximum_lifetime_seconds: 14_400,
        })
        .await
        .unwrap();

    store
        .end_session(
            SessionEventRecord {
                experiment_id: "departure".to_string(),
                session_id,
                event_type: "session_abandoned".to_string(),
                actor_participant_id: None,
                actor_role: Some("A".to_string()),
                payload: json!({"reason": "participant_left"}),
                game_state: None,
            },
            test_session_end(
                SessionEndCause::ParticipantLeft {
                    actor: "A".to_string(),
                },
                None,
            ),
        )
        .await
        .unwrap();

    let exported = experiment_rows(&store, "departure").await;
    assert_eq!(exported["sessions"][0]["lifecycle"], "ended");
    assert_eq!(
        exported["session_events"][0]["event_type"],
        "session_abandoned"
    );
    assert_eq!(exported["session_events"][0]["actor_role"], "A");
    assert_eq!(
        exported["session_events"][0]["payload"]["reason"],
        "participant_left"
    );
}

/// Confirms concurrent event writers receive a unique gap-free session-local order.
#[tokio::test]
async fn concurrent_event_appends_are_gap_free() {
    let store = Arc::new(
        SqliteExperimentStore::connect("sqlite:///:memory:")
            .await
            .unwrap(),
    );
    store
        .create_experiment(ExperimentRecord {
            experiment_id: "concurrent-events".to_string(),
            game_version: "1.0.0".to_string(),
            config: json!({}),
            server_version: None,
            version_manifest: None,
            status: "inactive".to_string(),
            notes: None,
        })
        .await
        .unwrap();
    let session_id = store
        .create_session(SessionRecord {
            experiment_id: "concurrent-events".to_string(),
            config_revision: 1,
            game_version: "1.0.0".to_string(),
            public_session_id: "CONCURRENT_EVENTS".to_string(),
            mode: "direct".to_string(),
            lifecycle: "running".to_string(),
            purpose: "research".to_string(),
            waiting_timeout_seconds: 600,
            maximum_lifetime_seconds: 14_400,
        })
        .await
        .unwrap();
    let mut writers = Vec::new();
    for writer in 0..64 {
        let store = Arc::clone(&store);
        writers.push(tokio::spawn(async move {
            store
                .append_session_event(SessionEventRecord {
                    experiment_id: "concurrent-events".to_string(),
                    session_id,
                    event_type: "parallel".to_string(),
                    actor_participant_id: None,
                    actor_role: None,
                    payload: json!({"writer": writer}),
                    game_state: None,
                })
                .await
                .unwrap()
        }));
    }
    let mut returned = Vec::new();
    for writer in writers {
        returned.push(writer.await.unwrap());
    }
    returned.sort_unstable();

    assert_eq!(returned, (1..=64).collect::<Vec<_>>());
    let stored = store
        .session_events("concurrent-events", session_id, None)
        .await
        .unwrap();
    assert_eq!(
        stored
            .iter()
            .map(|event| event.event_index)
            .collect::<Vec<_>>(),
        returned
    );
}

/// Confirms live session logs drain in order with runtime-owned game and agent attribution.
#[tokio::test]
async fn live_session_logger_drains_with_exact_attribution() {
    use crate::{game::PlayerRole, session_log::SessionLogger};

    let store = Arc::new(
        SqliteExperimentStore::connect("sqlite:///:memory:")
            .await
            .unwrap(),
    );
    let session_id = running_test_session(&store, "live-logs").await;
    let participant_id = store
        .upsert_participant(ParticipantRecord {
            experiment_id: "live-logs".to_string(),
            participant_kind: "agent".to_string(),
            identity_provider: "test".to_string(),
            external_id: Some("logger@1".to_string()),
            metadata: json!({"factory": "test", "agent_name": "logger", "agent_version": "1"}),
        })
        .await
        .unwrap();
    let shared: SharedExperimentStore = store.clone();
    let (game, writer) = SessionLogger::live(shared, "live-logs".to_string(), session_id);
    let agent = game.for_agent(participant_id, PlayerRole::B);

    game.log("game line").unwrap();
    agent.log("agent line").unwrap();
    game.log("last line").unwrap();
    writer.shutdown().await;

    let events = store
        .session_events("live-logs", session_id, Some("log"))
        .await
        .unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(
        events[0].payload,
        json!({"source": "game", "text": "game line"})
    );
    assert_eq!(events[0].actor_participant_id, None);
    assert_eq!(
        events[1].payload,
        json!({"source": "agent", "text": "agent line"})
    );
    assert_eq!(events[1].actor_participant_id, Some(participant_id));
    assert_eq!(events[1].actor_role.as_deref(), Some("B"));
    assert_eq!(
        events[2].payload,
        json!({"source": "game", "text": "last line"})
    );
}

/// Races all terminal writers and proves the first durable terminal state cannot be overwritten.
#[tokio::test]
async fn concurrent_terminal_transitions_have_exactly_one_winner() {
    let store = Arc::new(
        SqliteExperimentStore::connect("sqlite:///:memory:")
            .await
            .unwrap(),
    );
    let session_id = running_test_session(&store, "terminal-race").await;
    let barrier = Arc::new(tokio::sync::Barrier::new(4));

    let completion = {
        let store = store.clone();
        let barrier = barrier.clone();
        tokio::spawn(async move {
            barrier.wait().await;
            store
                .commit_session_transition(
                    vec![SessionEventRecord {
                        experiment_id: "terminal-race".to_string(),
                        session_id,
                        event_type: "session_completed".to_string(),
                        actor_participant_id: None,
                        actor_role: Some("A".to_string()),
                        payload: json!({"winner": "completion"}),
                        game_state: Some(json!({"done": true})),
                    }],
                    Some(test_session_end(
                        SessionEndCause::GameCompleted,
                        Some(json!({"winner": "completion"})),
                    )),
                )
                .await
                .unwrap();
        })
    };
    let expiry = {
        let store = store.clone();
        let barrier = barrier.clone();
        tokio::spawn(async move {
            barrier.wait().await;
            store
                .end_session(
                    SessionEventRecord {
                        experiment_id: "terminal-race".to_string(),
                        session_id,
                        event_type: "session_ended".to_string(),
                        actor_participant_id: None,
                        actor_role: None,
                        payload: json!({"winner": "expiry"}),
                        game_state: None,
                    },
                    test_session_end(SessionEndCause::IdleTimedOut, None),
                )
                .await
                .unwrap();
        })
    };
    let abandonment = {
        let store = store.clone();
        let barrier = barrier.clone();
        tokio::spawn(async move {
            barrier.wait().await;
            store
                .end_session(
                    SessionEventRecord {
                        experiment_id: "terminal-race".to_string(),
                        session_id,
                        event_type: "session_abandoned".to_string(),
                        actor_participant_id: None,
                        actor_role: Some("B".to_string()),
                        payload: json!({"winner": "abandonment"}),
                        game_state: None,
                    },
                    test_session_end(
                        SessionEndCause::ParticipantLeft {
                            actor: "B".to_string(),
                        },
                        None,
                    ),
                )
                .await
                .unwrap();
        })
    };
    barrier.wait().await;
    completion.await.unwrap();
    expiry.await.unwrap();
    abandonment.await.unwrap();

    let export = experiment_rows(&store, "terminal-race").await;
    assert_eq!(export["sessions"][0]["lifecycle"], "ended");
    let terminal_events = export["session_events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| {
            matches!(
                event["event_type"].as_str(),
                Some("session_completed" | "session_ended" | "session_abandoned")
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(terminal_events.len(), 1);
    assert!(matches!(
        terminal_events[0]["event_type"].as_str(),
        Some("session_completed" | "session_ended" | "session_abandoned")
    ));
}

/// A failure after the first insert must restore the session and its event sequence.
#[tokio::test]
async fn failed_event_batch_rolls_back_every_event_and_terminal_state() {
    let store = SqliteExperimentStore::connect("sqlite:///:memory:")
        .await
        .unwrap();
    let experiment_id = "batch-rollback";
    let session_id = running_test_session(&store, experiment_id).await;
    sqlx::query(
        "CREATE TRIGGER refuse_test_event BEFORE INSERT ON session_events
         WHEN NEW.event_type = 'refused' BEGIN SELECT RAISE(ABORT, 'refused test event'); END",
    )
    .execute(&store.pool)
    .await
    .unwrap();

    let event = SessionEventRecord {
        experiment_id: experiment_id.to_owned(),
        session_id,
        event_type: "accepted".to_owned(),
        actor_participant_id: None,
        actor_role: None,
        payload: Value::Null,
        game_state: None,
    };
    let rejected = SessionEventRecord {
        event_type: "refused".to_owned(),
        ..event.clone()
    };
    let error = store
        .commit_session_transition(
            vec![event.clone(), rejected],
            Some(test_session_end(
                SessionEndCause::GameCompleted,
                Some(json!({})),
            )),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("refused test event"));

    let snapshot = experiment_rows(&store, experiment_id).await;
    assert_eq!(snapshot["sessions"][0]["lifecycle"], "running");
    assert_eq!(snapshot["session_events"], json!([]));
    store.append_session_event(event).await.unwrap();
    let retried = experiment_rows(&store, experiment_id).await;
    assert_eq!(retried["session_events"][0]["event_index"], 1);
}

/// Resolves a failed COMMIT and retries the same terminal batch without duplicating it.
#[tokio::test]
async fn prepared_turn_identity_disambiguates_terminal_commit_and_retry() {
    let store = SqliteExperimentStore::connect("sqlite:///:memory:")
        .await
        .unwrap();
    let experiment_id = "turn-identity";
    let session_id = running_test_session(&store, experiment_id).await;
    for sql in [
        "create table test_parent (id integer primary key)",
        "create table test_child (id integer references test_parent(id)
         deferrable initially deferred)",
        "create trigger fail_commit after insert on session_events
         when new.event_type = 'game_action_accepted'
         begin insert into test_child values (123); end",
    ] {
        sqlx::query(sql).execute(&store.pool).await.unwrap();
    }
    let turn_id = "test-turn";
    let accepted = SessionEventRecord {
        experiment_id: experiment_id.to_owned(),
        session_id,
        event_type: "game_action_accepted".to_owned(),
        actor_participant_id: None,
        actor_role: None,
        payload: json!({"turn_id": turn_id, "action": 1}),
        game_state: None,
    };
    let completed = SessionEventRecord {
        event_type: "session_completed".to_owned(),
        payload: json!({}),
        ..accepted.clone()
    };
    let events = vec![accepted, completed];
    let end = Some(test_session_end(
        SessionEndCause::GameCompleted,
        Some(json!({})),
    ));
    let error = store
        .commit_session_transition(events.clone(), end.clone())
        .await
        .unwrap_err();
    assert!(error.is::<crate::storage::TransitionCommitUnknown>());
    assert!(!store
        .session_transition_committed(experiment_id, session_id, turn_id)
        .await
        .unwrap());
    sqlx::query("drop trigger fail_commit")
        .execute(&store.pool)
        .await
        .unwrap();

    for _ in 0..2 {
        assert_eq!(
            store
                .commit_session_transition(events.clone(), end.clone())
                .await
                .unwrap(),
            TransitionCommit::Committed
        );
        assert!(store
            .session_transition_committed(experiment_id, session_id, turn_id)
            .await
            .unwrap());
    }
    let snapshot = experiment_rows(&store, experiment_id).await;
    assert_eq!(snapshot["sessions"][0]["lifecycle"], "ended");
    assert_eq!(snapshot["session_events"].as_array().unwrap().len(), 2);
}
