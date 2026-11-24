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

    let sessions = store
        .recent_sessions("purpose", None, None, 10)
        .await
        .unwrap();
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

/// Finds older Prolific sessions using an experiment-scoped literal suffix and lifecycle.
#[tokio::test]
async fn sqlite_session_search_matches_prolific_suffix_before_limit() {
    let store = SqliteExperimentStore::connect("sqlite:///:memory:")
        .await
        .unwrap();
    let mut matching_sessions = Vec::new();
    for experiment_id in ["search-a", "search-b"] {
        store
            .create_experiment(ExperimentRecord {
                experiment_id: experiment_id.into(),
                game_version: "0.4.0".into(),
                config: json!({}),
                server_version: None,
                version_manifest: None,
                status: "active".into(),
                notes: None,
            })
            .await
            .unwrap();
        for (session_name, lifecycle, participant_suffix) in
            [("older", "running", "a21"), ("newer", "forming", "f03")]
        {
            let session_id = store
                .create_session(SessionRecord {
                    experiment_id: experiment_id.into(),
                    config_revision: 1,
                    game_version: "0.4.0".into(),
                    public_session_id: format!("{experiment_id}-{session_name}"),
                    mode: "direct".into(),
                    lifecycle: lifecycle.into(),
                    purpose: "research".into(),
                    waiting_timeout_seconds: 600,
                    maximum_lifetime_seconds: 14_400,
                })
                .await
                .unwrap();
            if session_name == "older" {
                matching_sessions.push(session_id);
            }
            let admission = store
                .admit_prolific_submission(
                    ProlificSubmissionRecord {
                        experiment_id: experiment_id.into(),
                        prolific_participant_id: format!("123prefix{participant_suffix}"),
                        prolific_study_id: format!("study-{experiment_id}"),
                        prolific_session_id: format!("submission-{session_name}"),
                        verification_method: "submission_api".into(),
                    },
                    true,
                )
                .await
                .unwrap()
                .unwrap();
            store
                .add_session_participant(SessionParticipantRecord {
                    experiment_id: experiment_id.into(),
                    session_id,
                    participant_id: admission.participant_id,
                    participant_session_id: admission.participant_session_id,
                    role: "A".into(),
                    connection_status: "connected".into(),
                })
                .await
                .unwrap();
        }
    }

    let latest = store
        .recent_sessions("search-a", None, None, 1)
        .await
        .unwrap();
    assert_eq!(latest[0].lifecycle, "forming");
    for (experiment_id, expected) in ["search-a", "search-b"].into_iter().zip(matching_sessions) {
        let matches = store
            .recent_sessions(experiment_id, None, Some("a21"), 1)
            .await
            .unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].session_id, expected);
        assert_eq!(matches[0].participant_count, 1);
    }
    let filtered = store
        .recent_sessions("search-a", Some("running"), Some("a21"), 1)
        .await
        .unwrap();
    assert_eq!(filtered.len(), 1);
    assert!(store
        .recent_sessions("search-a", Some("forming"), Some("a21"), 1)
        .await
        .unwrap()
        .is_empty());
    for non_suffix in ["prefix", "%_", "study-search-a", "submission-older"] {
        assert!(store
            .recent_sessions("search-a", None, Some(non_suffix), 1)
            .await
            .unwrap()
            .is_empty());
    }
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

/// Dashboard cursors cover both directions and retain alignment recorded outside a message's page.
#[tokio::test]
async fn timeline_pages_bound_rows_and_join_late_speech_timing() {
    let store = SqliteExperimentStore::connect("sqlite:///:memory:")
        .await
        .unwrap();
    let id = running_test_session(&store, "timeline").await;
    sqlx::query("with recursive numbers(i) as (select 1 union all select i+1 from numbers where i<211)
        insert into session_events (experiment_id,session_id,event_index,event_type,payload_json,game_state_json,game_time_ms)
        select 'timeline', ?, i, case i when 5 then 'conversation_message' when 25 then 'participant_disconnected'
          when 211 then 'conversation_speech_timing' else 'log' end,
          case i when 5 then '{\"id\":\"m\"}' when 211 then '{\"message_id\":\"m\",\"metadata\":{\"start_game_time_ms\":9}}'
          else '{}' end, '{\"checkpoint\":true}', i from numbers")
        .bind(id).execute(&store.pool).await.unwrap();
    assert_eq!(store.latest_event_index("timeline", id).await.unwrap(), 211);
    let last = store
        .timeline_page("timeline", id, None, None)
        .await
        .unwrap();
    assert_eq!((last.older_before, last.last_index), (Some(112), 211));
    assert!(last.after_terminal);
    assert!(last.events.iter().all(|event| event.game_state.is_none()));
    let middle = store
        .timeline_page("timeline", id, last.older_before, None)
        .await
        .unwrap();
    assert_eq!((middle.older_before, middle.last_index), (Some(12), 111));
    let first = store
        .timeline_page("timeline", id, middle.older_before, None)
        .await
        .unwrap();
    assert_eq!((first.older_before, first.last_index), (None, 11));
    assert!(first
        .events
        .iter()
        .any(|event| event.event_index == 211 && event.payload["message_id"] == "m"));
    let forwards = store
        .timeline_page("timeline", id, None, Some(80))
        .await
        .unwrap();
    assert_eq!(forwards.last_index, 180);
    assert_eq!(forwards.events[0].event_index, 81);
    assert!(store
        .timeline_page("other", id, None, None)
        .await
        .unwrap()
        .events
        .is_empty());
}
