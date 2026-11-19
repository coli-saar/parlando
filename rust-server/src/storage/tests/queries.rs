use super::*;

/// Confirms session purpose is fixed from lifecycle at creation and never inferred later.
#[tokio::test]
async fn sqlite_stamps_testing_and_research_session_purpose() {
    let store = SqliteExperimentStore::connect("sqlite:///:memory:")
        .await
        .unwrap();
    store
        .create_experiment(ExperimentRecord {
            experiment_id: "purpose".to_string(),
            game_version: "0.4.0".to_string(),
            config: json!({}),
            server_version: None,
            version_manifest: None,
            status: "testing".to_string(),
            notes: None,
        })
        .await
        .unwrap();
    store
        .update_experiment_status("purpose", "testing")
        .await
        .unwrap();
    let testing_session = store
        .create_session(SessionRecord {
            experiment_id: "purpose".to_string(),
            config_revision: 1,
            game_version: "0.4.0".to_string(),
            public_session_id: "TESTING".to_string(),
            mode: "direct".to_string(),
            lifecycle: "forming".to_string(),
            purpose: "testing".to_string(),
            waiting_timeout_seconds: 600,
            maximum_lifetime_seconds: 14_400,
        })
        .await
        .unwrap();
    store
        .update_experiment_status("purpose", "active")
        .await
        .unwrap();
    let research_session = store
        .create_session(SessionRecord {
            experiment_id: "purpose".to_string(),
            config_revision: 1,
            game_version: "0.4.0".to_string(),
            public_session_id: "RESEARCH".to_string(),
            mode: "direct".to_string(),
            lifecycle: "forming".to_string(),
            purpose: "research".to_string(),
            waiting_timeout_seconds: 600,
            maximum_lifetime_seconds: 14_400,
        })
        .await
        .unwrap();

    let sessions = store.recent_sessions("purpose", 10).await.unwrap();
    assert_eq!(sessions[0].session_id, research_session);
    assert_eq!(sessions[0].purpose, "research");
    assert_eq!(sessions[1].session_id, testing_session);
    assert_eq!(sessions[1].purpose, "testing");
    assert_eq!(
        store
            .session_detail("purpose", testing_session)
            .await
            .unwrap()
            .unwrap()
            .purpose,
        "testing"
    );
}

/// Confirms progress counts include every session rather than the recent-session display limit.
#[tokio::test]
async fn sqlite_session_progress_counts_the_complete_experiment() {
    let store = SqliteExperimentStore::connect("sqlite:///:memory:")
        .await
        .unwrap();
    store
        .create_experiment(ExperimentRecord {
            experiment_id: "progress".to_string(),
            game_version: "0.4.0".to_string(),
            config: json!({}),
            server_version: None,
            version_manifest: None,
            status: "inactive".to_string(),
            notes: None,
        })
        .await
        .unwrap();

    for index in 0..82 {
        let session_id = store
            .create_session(SessionRecord {
                experiment_id: "progress".to_string(),
                config_revision: 1,
                game_version: "0.4.0".to_string(),
                public_session_id: format!("PROGRESS-{index}"),
                mode: "direct".to_string(),
                lifecycle: "forming".to_string(),
                purpose: "research".to_string(),
                waiting_timeout_seconds: 600,
                maximum_lifetime_seconds: 14_400,
            })
            .await
            .unwrap();
        if index < 81 {
            let cause = if index % 2 == 0 {
                SessionEndCause::GameCompleted
            } else {
                SessionEndCause::TechnicalFailure
            };
            sqlx::query(
                "update sessions set lifecycle = 'ended', ended_at = ?, session_end_json = ? where experiment_id = ? and session_id = ?",
            )
            .bind(now_iso())
            .bind(serde_json::to_string(&test_session_end(cause, None)).unwrap())
            .bind("progress")
            .bind(session_id)
            .execute(&store.pool)
            .await
            .unwrap();
        }
    }
    let testing_session_id = store
        .create_session(SessionRecord {
            experiment_id: "progress".to_string(),
            config_revision: 1,
            game_version: "test".to_string(),
            public_session_id: "testing-progress-session".to_string(),
            mode: "direct".to_string(),
            lifecycle: "forming".to_string(),
            purpose: "testing".to_string(),
            waiting_timeout_seconds: 600,
            maximum_lifetime_seconds: 14_400,
        })
        .await
        .unwrap();

    let progress = store.session_progress("progress").await.unwrap();
    assert_eq!(progress.total_sessions, 82);
    assert_eq!(progress.ended_sessions, 81);
    assert_eq!(progress.end_causes["game_completed"], 41);
    assert_eq!(progress.end_causes["technical_failure"], 40);
    assert_eq!(progress.sessions.len(), 83);
    assert_eq!(progress.sessions[0].session_id, testing_session_id);
    assert_eq!(progress.sessions[0].purpose, "testing");
    assert!(progress
        .sessions
        .windows(2)
        .all(|pair| pair[0].session_id > pair[1].session_id));
}

/// Verifies durable projections, shared-session deletion, rollback and retry.
#[tokio::test]
async fn sqlite_experiment_sessions_participants_and_events_are_queryable() {
    let temp = tempdir().expect("tempdir");
    let database_url = format!("sqlite:///{}", temp.path().join("eval.sqlite").display());
    let store = SqliteExperimentStore::connect(&database_url).await.unwrap();
    store
        .ensure_experiment(ExperimentRecord {
            experiment_id: "exp_eval".to_string(),
            game_version: "0.4.0".to_string(),
            config: json!({"study": "demo"}),
            server_version: Some("test".to_string()),
            version_manifest: None,
            status: "active".to_string(),
            notes: None,
        })
        .await
        .unwrap();
    let participant_id = store
        .upsert_participant(ParticipantRecord {
            experiment_id: "exp_eval".to_string(),
            participant_kind: "human".to_string(),
            identity_provider: "direct".to_string(),
            external_id: Some("PID123".to_string()),
            metadata: json!({"source": "fixture"}),
        })
        .await
        .unwrap();
    let same_participant_id = store
        .upsert_participant(ParticipantRecord {
            experiment_id: "exp_eval".to_string(),
            participant_kind: "human".to_string(),
            identity_provider: "direct".to_string(),
            external_id: Some("PID123".to_string()),
            metadata: Value::Null,
        })
        .await
        .unwrap();
    assert_eq!(participant_id, same_participant_id);

    let session_one = store
        .create_session(SessionRecord {
            experiment_id: "exp_eval".to_string(),
            config_revision: 1,
            game_version: "0.4.0".to_string(),
            public_session_id: "ROOM1".to_string(),
            mode: "direct".to_string(),
            lifecycle: "forming".to_string(),
            purpose: "research".to_string(),
            waiting_timeout_seconds: 600,
            maximum_lifetime_seconds: 14_400,
        })
        .await
        .unwrap();
    let session_two = store
        .create_session(SessionRecord {
            experiment_id: "exp_eval".to_string(),
            config_revision: 1,
            game_version: "0.4.0".to_string(),
            public_session_id: "ROOM2".to_string(),
            mode: "direct".to_string(),
            lifecycle: "forming".to_string(),
            purpose: "research".to_string(),
            waiting_timeout_seconds: 600,
            maximum_lifetime_seconds: 14_400,
        })
        .await
        .unwrap();
    assert_eq!((session_one, session_two), (1, 2));

    assert!(store
        .complete_session_initialization(SessionParticipantRecord {
            experiment_id: "exp_eval".to_string(),
            session_id: session_one,
            participant_id,
            participant_session_id: "ps_1".to_string(),
            role: "A".to_string(),
            connection_status: "joined".to_string(),
        })
        .await
        .unwrap());
    store
        .record_consent_declaration(ConsentDeclarationRecord {
            experiment_id: "exp_eval".to_string(),
            session_id: Some(session_one),
            participant_id,
            purpose: "research".to_string(),
            consent_item_id: "study".to_string(),
            accepted: true,
            consent_text_hash: None,
            metadata: Value::Null,
        })
        .await
        .unwrap();
    let first_event = store
        .append_session_event(SessionEventRecord {
            experiment_id: "exp_eval".to_string(),
            session_id: session_one,
            event_type: "game_action_accepted".to_string(),
            actor_participant_id: Some(participant_id),
            actor_role: Some("A".to_string()),
            payload: json!({"action": "noop"}),
            game_state: Some(json!({"step": 1})),
        })
        .await
        .unwrap();
    let second_event = store
        .append_session_event(SessionEventRecord {
            experiment_id: "exp_eval".to_string(),
            session_id: session_one,
            event_type: "state_changed".to_string(),
            actor_participant_id: None,
            actor_role: None,
            payload: json!({"reason": "test"}),
            game_state: Some(json!({"step": 2})),
        })
        .await
        .unwrap();
    assert_eq!((first_event, second_event), (1, 2));

    let exported = experiment_rows(&store, "exp_eval").await;
    assert_eq!(exported["sessions"].as_array().unwrap().len(), 2);
    assert_eq!(exported["session_events"].as_array().unwrap().len(), 2);
    let participant_pseudonym = exported["participants"][0]["research_id"]
        .as_str()
        .unwrap()
        .to_string();
    let dialogue_pseudonym = exported["sessions"][0]["dialogue_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        store
            .participant_id_for_research_id("exp_eval", &participant_pseudonym)
            .await
            .unwrap(),
        Some(participant_id)
    );
    assert_eq!(
        store
            .participant_id_for_research_id("other", &participant_pseudonym)
            .await
            .unwrap(),
        None
    );
    assert_eq!(participant_pseudonym.split('-').count(), 3);
    assert_eq!(dialogue_pseudonym.split('-').count(), 3);
    let repeated_export = experiment_rows(&store, "exp_eval").await;
    assert_eq!(
        repeated_export["participants"][0]["research_id"],
        participant_pseudonym
    );
    assert_eq!(
        repeated_export["sessions"][0]["dialogue_id"],
        dialogue_pseudonym
    );

    let preview = store
        .participant_data_preview("exp_eval", participant_id)
        .await
        .unwrap();
    assert_eq!(preview.session_count, 1);
    assert_eq!(preview.consent_count, 1);
    assert_eq!(preview.other_event_count, 1);
    assert!(preview.has_non_terminal_session);
    assert!(store
        .delete_participant_data("exp_eval", participant_id)
        .await
        .is_err());
    let partner_id = store
        .upsert_participant(ParticipantRecord {
            experiment_id: "exp_eval".into(),
            participant_kind: "human".into(),
            identity_provider: "direct".into(),
            external_id: Some("other".into()),
            metadata: Value::Null,
        })
        .await
        .unwrap();
    for (session_id, role, registration) in [
        (session_one, "B", "peer-one"),
        (session_two, "A", "peer-two"),
    ] {
        store
            .add_session_participant(SessionParticipantRecord {
                experiment_id: "exp_eval".into(),
                session_id,
                participant_id: partner_id,
                participant_session_id: registration.into(),
                role: role.into(),
                connection_status: "joined".into(),
            })
            .await
            .unwrap();
    }
    let preview = store
        .participant_data_preview("exp_eval", participant_id)
        .await
        .unwrap();
    assert_eq!(preview.session_ids, [dialogue_pseudonym]);
    assert_eq!(preview.other_participant_ids.len(), 1);
    assert_eq!(
        store
            .participant_id_for_research_id("exp_eval", &preview.other_participant_ids[0])
            .await
            .unwrap(),
        Some(partner_id)
    );
    let started_at = now_iso();
    let idle_deadline_at = (chrono::DateTime::parse_from_rfc3339(&started_at).unwrap()
        + chrono::Duration::seconds(300))
    .to_rfc3339();
    assert!(store
        .start_session("exp_eval", session_one, &started_at, &idle_deadline_at,)
        .await
        .unwrap());
    let rebased = store
        .session_events("exp_eval", session_one, None)
        .await
        .unwrap();
    assert!(rebased
        .iter()
        .all(|event| (-1_000..=0).contains(&event.game_time_ms)));
    assert!(
        store
            .session_game_time_ms("exp_eval", session_one, &now_iso())
            .await
            .unwrap()
            >= 0
    );
    store
        .commit_session_transition(
            vec![SessionEventRecord {
                experiment_id: "exp_eval".to_string(),
                session_id: session_one,
                event_type: "session_completed".to_string(),
                actor_participant_id: None,
                actor_role: None,
                payload: json!({"outcome": "test"}),
                game_state: None,
            }],
            Some(test_session_end(
                SessionEndCause::GameCompleted,
                Some(json!({"outcome": "test"})),
            )),
        )
        .await
        .unwrap();
    // A failure at the last delete must restore the whole session graph for a retry.
    let before_delete = experiment_rows(&store, "exp_eval").await;
    sqlx::query("create trigger refuse_deletion before delete on participants begin select raise(abort, 'fixture refusal'); end")
        .execute(&store.pool).await.unwrap();
    let failure = store
        .delete_participant_data("exp_eval", participant_id)
        .await
        .unwrap_err();
    assert!(failure.to_string().contains("fixture refusal"));
    assert_eq!(experiment_rows(&store, "exp_eval").await, before_delete);
    sqlx::query("drop trigger refuse_deletion")
        .execute(&store.pool)
        .await
        .unwrap();
    store
        .delete_participant_data("exp_eval", participant_id)
        .await
        .unwrap();
    let deleted = experiment_rows(&store, "exp_eval").await;
    assert!(store
        .session_detail("exp_eval", session_one)
        .await
        .unwrap()
        .is_none());
    assert_eq!(deleted["sessions"][0]["session_id"], session_two);
    assert_eq!(deleted["participants"].as_array().unwrap().len(), 1);
    assert_eq!(
        deleted["participants"][0]["research_id"],
        preview.other_participant_ids[0]
    );
    assert_eq!(deleted["session_participants"].as_array().unwrap().len(), 1);
    assert_eq!(
        deleted["session_participants"][0]["session_id"],
        session_two
    );
    assert!(deleted["consent_declarations"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(deleted["session_events"].as_array().unwrap().is_empty());
}
