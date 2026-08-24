use serde_json::json;
use tempfile::tempdir;

use super::*;

/// Confirms agent identifiers expose the configured type, implementation name, and version.
#[test]
fn agent_identifier_uses_durable_type_and_version_metadata() {
    let participant = ParticipantRecord {
        experiment_id: "experiment-a".to_string(),
        participant_kind: "agent".to_string(),
        identity_provider: "remote_grpc".to_string(),
        external_id: Some("Python Agent@v1.2 beta".to_string()),
        metadata: json!({
            "agent_type": "remote_grpc",
            "agent_name": "Python Agent",
            "agent_version": "v1.2 beta",
        }),
    };

    assert_eq!(
        participant_identifier_candidate(&participant, 1),
        "agent:remote_grpc:Python-Agent@v1.2-beta"
    );
    assert_eq!(
        participant_identifier_candidate(&participant, 2),
        "agent:remote_grpc:Python-Agent@v1.2-beta~2"
    );

    let unversioned = ParticipantRecord {
        external_id: Some("Python Agent".to_string()),
        metadata: json!({
            "agent_type": "remote_grpc",
            "agent_name": "Python Agent",
        }),
        ..participant
    };
    assert_eq!(
        participant_identifier_candidate(&unversioned, 1),
        "agent:remote_grpc:Python-Agent@unversioned"
    );
}

/// Confirms only human durable participants receive random three-word names.
#[tokio::test]
async fn sqlite_assigns_random_names_only_to_humans() {
    let store = SqliteExperimentStore::connect("sqlite:///:memory:")
        .await
        .unwrap();
    let human = store
        .upsert_participant(ParticipantRecord {
            experiment_id: "experiment-a".to_string(),
            participant_kind: "human".to_string(),
            identity_provider: "direct".to_string(),
            external_id: None,
            metadata: Value::Null,
        })
        .await
        .unwrap();
    let agent = store
        .upsert_participant(ParticipantRecord {
            experiment_id: "experiment-a".to_string(),
            participant_kind: "agent".to_string(),
            identity_provider: "space_game".to_string(),
            external_id: Some("space_game.back_and_forth@0.2.0".to_string()),
            metadata: json!({
                "agent_type": "space_game.back_and_forth",
                "agent_name": "BackAndForthAgent",
                "agent_version": "0.2.0",
            }),
        })
        .await
        .unwrap();

    let human_identifier = store.participant_research_id(human).await.unwrap().unwrap();
    let agent_identifier = store.participant_research_id(agent).await.unwrap().unwrap();
    assert_eq!(human_identifier.split('-').count(), 3);
    assert_eq!(
        agent_identifier,
        "agent:space_game.back_and_forth:BackAndForthAgent@0.2.0"
    );
}

/// Confirms the schema contains both evaluation data and the isolated credential table.
#[tokio::test]
async fn sqlite_schema_has_evaluation_and_administrator_tables() {
    let store = SqliteExperimentStore::connect("sqlite:///:memory:")
        .await
        .unwrap();
    let tables = store.table_names().await.unwrap();

    assert_eq!(
        tables,
        vec![
            "administrator_credential",
            "administrator_sessions",
            "consent_declarations",
            "experiment_config_revisions",
            "experiment_secrets",
            "experiments",
            "game_secrets",
            "game_settings",
            "participants",
            "prolific_submissions",
            "schema_migrations",
            "session_events",
            "session_participants",
            "sessions",
        ]
    );
    let experiment_columns = sqlx::query_scalar::<_, String>(
        "select name from pragma_table_info('experiments') order by cid",
    )
    .fetch_all(&store.pool)
    .await
    .unwrap();
    assert!(!experiment_columns.iter().any(|column| column == "obsolete"));
    let session_columns = sqlx::query_scalar::<_, String>(
        "select name from pragma_table_info('sessions') order by cid",
    )
    .fetch_all(&store.pool)
    .await
    .unwrap();
    assert!(session_columns.iter().any(|column| column == "purpose"));
}

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
            status: "waiting".to_string(),
            purpose: "testing".to_string(),
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
            status: "waiting".to_string(),
            purpose: "research".to_string(),
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
            .export_session("purpose", testing_session)
            .await
            .unwrap()["sessions"][0]["purpose"],
        "testing"
    );
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

/// Confirms first setup wins atomically and survives reopening the SQLite database.
#[tokio::test]
async fn sqlite_admin_setup_is_atomic_and_persistent() {
    let temp = tempdir().expect("tempdir");
    let database_url = format!("sqlite:///{}", temp.path().join("admin.sqlite").display());
    let store = SqliteExperimentStore::connect(&database_url).await.unwrap();
    let credential = StoredAdminCredential {
        username: "researcher".to_string(),
        password_hash: "$argon2id$test-hash".to_string(),
        role: "administrator".to_string(),
    };

    assert!(store
        .create_admin_credential(credential.clone())
        .await
        .unwrap());
    assert!(!store
        .create_admin_credential(StoredAdminCredential {
            username: "second".to_string(),
            ..credential.clone()
        })
        .await
        .unwrap());
    drop(store);

    let reopened = SqliteExperimentStore::connect(&database_url).await.unwrap();
    assert_eq!(reopened.admin_credential().await.unwrap(), Some(credential));
}

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
            status: "waiting".to_string(),
            purpose: "research".to_string(),
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
            status: "waiting".to_string(),
            purpose: "research".to_string(),
        })
        .await
        .unwrap();
    assert_eq!((session_one, session_two), (1, 2));

    store
        .add_session_participant(SessionParticipantRecord {
            experiment_id: "exp_eval".to_string(),
            session_id: session_one,
            participant_id,
            participant_session_id: "ps_1".to_string(),
            role: "A".to_string(),
            connection_status: "joined".to_string(),
        })
        .await
        .unwrap();
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

    let exported = store.export_session("exp_eval", session_one).await.unwrap();
    assert_eq!(exported["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(exported["session_events"].as_array().unwrap().len(), 2);
    let participant_pseudonym = exported["participants"][0]["research_id"]
        .as_str()
        .unwrap()
        .to_string();
    let dialogue_pseudonym = exported["sessions"][0]["dialogue_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(participant_pseudonym.split('-').count(), 3);
    assert_eq!(dialogue_pseudonym.split('-').count(), 3);
    let repeated_export = store.export_session("exp_eval", session_one).await.unwrap();
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
    assert!(store.start_session("exp_eval", session_one).await.unwrap());
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
            Some(json!({"outcome": "test"})),
        )
        .await
        .unwrap();
    store
        .delete_participant_data("exp_eval", participant_id)
        .await
        .unwrap();
    let deleted = store.export_session("exp_eval", session_one).await.unwrap();
    assert!(deleted["sessions"].as_array().unwrap().is_empty());
    assert!(deleted["participants"].as_array().unwrap().is_empty());
    assert!(deleted["session_events"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn sqlite_allows_returning_participant_in_multiple_sessions_with_different_roles() {
    let store = SqliteExperimentStore::connect("sqlite:///:memory:")
        .await
        .unwrap();
    store
        .ensure_experiment(ExperimentRecord {
            experiment_id: "exp_returning".to_string(),
            game_version: "0.4.0".to_string(),
            config: json!({"condition": "repeat-play"}),
            server_version: None,
            version_manifest: None,
            status: "active".to_string(),
            notes: Some("same Prolific participant appears twice".to_string()),
        })
        .await
        .unwrap();
    let participant_id = store
        .upsert_participant(ParticipantRecord {
            experiment_id: "exp_returning".to_string(),
            participant_kind: "human".to_string(),
            identity_provider: "prolific".to_string(),
            external_id: Some("PROLIFIC-REPEAT".to_string()),
            metadata: json!({"first_seen_batch": 7}),
        })
        .await
        .unwrap();
    store
        .ensure_experiment(ExperimentRecord {
            experiment_id: "exp_other".to_string(),
            game_version: "0.4.0".to_string(),
            config: Value::Null,
            server_version: None,
            version_manifest: None,
            status: "active".to_string(),
            notes: None,
        })
        .await
        .unwrap();
    let other_experiment_participant = store
        .upsert_participant(ParticipantRecord {
            experiment_id: "exp_other".to_string(),
            participant_kind: "human".to_string(),
            identity_provider: "prolific".to_string(),
            external_id: Some("PROLIFIC-REPEAT".to_string()),
            metadata: Value::Null,
        })
        .await
        .unwrap();
    assert_ne!(participant_id, other_experiment_participant);
    assert_ne!(
        store.participant_research_id(participant_id).await.unwrap(),
        store
            .participant_research_id(other_experiment_participant)
            .await
            .unwrap()
    );
    let first_session = store
        .create_session(SessionRecord {
            experiment_id: "exp_returning".to_string(),
            config_revision: 1,
            game_version: "0.4.0".to_string(),
            public_session_id: "ROOM_A".to_string(),
            mode: "human_vs_human".to_string(),
            status: "running".to_string(),
            purpose: "research".to_string(),
        })
        .await
        .unwrap();
    let second_session = store
        .create_session(SessionRecord {
            experiment_id: "exp_returning".to_string(),
            config_revision: 1,
            game_version: "0.4.0".to_string(),
            public_session_id: "ROOM_B".to_string(),
            mode: "role_swap_replay".to_string(),
            status: "running".to_string(),
            purpose: "research".to_string(),
        })
        .await
        .unwrap();

    store
        .add_session_participant(SessionParticipantRecord {
            experiment_id: "exp_returning".to_string(),
            session_id: first_session,
            participant_id,
            participant_session_id: "ps_repeat_a".to_string(),
            role: "A".to_string(),
            connection_status: "connected".to_string(),
        })
        .await
        .unwrap();
    store
        .add_session_participant(SessionParticipantRecord {
            experiment_id: "exp_returning".to_string(),
            session_id: second_session,
            participant_id,
            participant_session_id: "ps_repeat_b".to_string(),
            role: "B".to_string(),
            connection_status: "connected".to_string(),
        })
        .await
        .unwrap();

    let experiment = store.export_experiment("exp_returning").await.unwrap();
    assert_eq!(experiment["participants"].as_array().unwrap().len(), 1);
    assert_eq!(
        experiment["session_participants"].as_array().unwrap().len(),
        2
    );
    assert!(experiment["participants"][0].get("role").is_none());
    assert_eq!(experiment["session_participants"][0]["role"], "A");
    assert_eq!(experiment["session_participants"][1]["role"], "B");

    let second = store
        .export_session("exp_returning", second_session)
        .await
        .unwrap();
    assert_eq!(second["participants"].as_array().unwrap().len(), 1);
    assert_eq!(
        second["session_participants"][0]["participant_session_id"],
        "ps_repeat_b"
    );
}

/// Confirms purging private Prolific correlations leaves dashboard session assignments intact.
#[tokio::test]
async fn sqlite_session_participants_survive_prolific_correlation_purge() {
    let store = SqliteExperimentStore::connect("sqlite:///:memory:")
        .await
        .unwrap();
    store
        .ensure_experiment(ExperimentRecord {
            experiment_id: "exp_prolific_purge".to_string(),
            game_version: "0.4.0".to_string(),
            config: Value::Null,
            server_version: None,
            version_manifest: None,
            status: "active".to_string(),
            notes: None,
        })
        .await
        .unwrap();

    let participant_a = store
        .upsert_participant(ParticipantRecord {
            experiment_id: "exp_prolific_purge".to_string(),
            participant_kind: "human".to_string(),
            identity_provider: "prolific".to_string(),
            external_id: None,
            metadata: Value::Null,
        })
        .await
        .unwrap();
    let participant_b = store
        .upsert_participant(ParticipantRecord {
            experiment_id: "exp_prolific_purge".to_string(),
            participant_kind: "human".to_string(),
            identity_provider: "prolific".to_string(),
            external_id: None,
            metadata: Value::Null,
        })
        .await
        .unwrap();
    for (participant_id, suffix) in [(participant_a, "A"), (participant_b, "B")] {
        store
            .record_prolific_submission(ProlificSubmissionRecord {
                experiment_id: "exp_prolific_purge".to_string(),
                participant_id,
                prolific_participant_id: format!("PROLIFIC-{suffix}"),
                prolific_study_id: "STUDY-PRIVATE".to_string(),
                prolific_session_id: format!("SUBMISSION-{suffix}"),
            })
            .await
            .unwrap();
    }

    let session_id = store
        .create_session(SessionRecord {
            experiment_id: "exp_prolific_purge".to_string(),
            config_revision: 1,
            game_version: "0.4.0".to_string(),
            public_session_id: "ROOM_PURGE".to_string(),
            mode: "human_vs_human".to_string(),
            status: "completed".to_string(),
            purpose: "research".to_string(),
        })
        .await
        .unwrap();
    for (participant_id, role) in [(participant_a, "A"), (participant_b, "B")] {
        store
            .add_session_participant(SessionParticipantRecord {
                experiment_id: "exp_prolific_purge".to_string(),
                session_id,
                participant_id,
                participant_session_id: format!("ps_{role}"),
                role: role.to_string(),
                connection_status: "left".to_string(),
            })
            .await
            .unwrap();
    }

    let before_purge = store
        .session_participants("exp_prolific_purge", session_id)
        .await
        .unwrap();
    assert_eq!(before_purge.len(), 2);
    assert!(before_purge
        .iter()
        .all(|participant| participant.prolific_participant_id.is_some()));
    assert!(before_purge
        .iter()
        .all(|participant| participant.identity_provider.as_deref() == Some("prolific")));

    sqlx::query("delete from prolific_submissions where experiment_id = ?")
        .bind("exp_prolific_purge")
        .execute(&store.pool)
        .await
        .unwrap();

    let after_purge = store
        .session_participants("exp_prolific_purge", session_id)
        .await
        .unwrap();
    assert_eq!(
        after_purge
            .iter()
            .map(|participant| participant.role.as_str())
            .collect::<Vec<_>>(),
        vec!["A", "B"]
    );
    assert!(after_purge
        .iter()
        .all(|participant| participant.identity_provider.as_deref() == Some("prolific")));
    assert!(after_purge.iter().all(|participant| {
        participant.research_id.is_some()
            && participant.prolific_participant_id.is_none()
            && participant.prolific_study_id.is_none()
            && participant.prolific_session_id.is_none()
    }));
}

#[tokio::test]
async fn sqlite_holds_mixed_participant_and_event_shapes_for_weird_experiments() {
    let store = SqliteExperimentStore::connect("sqlite:///:memory:")
        .await
        .unwrap();
    store
        .ensure_experiment(ExperimentRecord {
            experiment_id: "exp_weird".to_string(),
            game_version: "0.4.0".to_string(),
            config: json!({
                "conditions": ["voice", "agent", "worker"],
                "nested": {"levels": [{"id": 1}, {"id": 2}]}
            }),
            server_version: Some("test".to_string()),
            version_manifest: None,
            status: "active".to_string(),
            notes: None,
        })
        .await
        .unwrap();

    let direct_one = store
        .upsert_participant(ParticipantRecord {
            experiment_id: "exp_weird".to_string(),
            participant_kind: "human".to_string(),
            identity_provider: "direct".to_string(),
            external_id: None,
            metadata: json!({"signup": 1}),
        })
        .await
        .unwrap();
    let direct_two = store
        .upsert_participant(ParticipantRecord {
            experiment_id: "exp_weird".to_string(),
            participant_kind: "human".to_string(),
            identity_provider: "direct".to_string(),
            external_id: None,
            metadata: json!({"signup": 2}),
        })
        .await
        .unwrap();
    assert_ne!(direct_one, direct_two);

    let prolific = store
        .upsert_participant(ParticipantRecord {
            experiment_id: "exp_weird".to_string(),
            participant_kind: "human".to_string(),
            identity_provider: "prolific".to_string(),
            external_id: Some("PID-WEIRD".to_string()),
            metadata: json!({"study_id": "STUDY42", "session_id": "SESSION99"}),
        })
        .await
        .unwrap();
    let agent = store
        .upsert_participant(ParticipantRecord {
            experiment_id: "exp_weird".to_string(),
            participant_kind: "agent".to_string(),
            identity_provider: "agent".to_string(),
            external_id: Some("back-and-forth@v2".to_string()),
            metadata: json!({"temperature": 0, "seed": 1234}),
        })
        .await
        .unwrap();
    let worker = store
        .upsert_participant(ParticipantRecord {
            experiment_id: "exp_weird".to_string(),
            participant_kind: "worker".to_string(),
            identity_provider: "worker".to_string(),
            external_id: Some("transcriber-1".to_string()),
            metadata: json!({"provider": "speechmatics"}),
        })
        .await
        .unwrap();
    let session_id = store
        .create_session(SessionRecord {
            experiment_id: "exp_weird".to_string(),
            config_revision: 1,
            game_version: "0.4.0".to_string(),
            public_session_id: "ROOM_WEIRD".to_string(),
            mode: "human_agent_with_worker".to_string(),
            status: "running".to_string(),
            purpose: "research".to_string(),
        })
        .await
        .unwrap();

    for (participant_id, handle, role) in [
        (prolific, "ps_prolific", "A"),
        (agent, "ps_agent", "B"),
        (worker, "worker-transcriber-1", "worker"),
    ] {
        store
            .add_session_participant(SessionParticipantRecord {
                experiment_id: "exp_weird".to_string(),
                session_id,
                participant_id,
                participant_session_id: handle.to_string(),
                role: role.to_string(),
                connection_status: "connected".to_string(),
            })
            .await
            .unwrap();
    }

    store
        .record_consent_declaration(ConsentDeclarationRecord {
            experiment_id: "exp_weird".to_string(),
            session_id: None,
            participant_id: prolific,
            purpose: "research".to_string(),
            consent_item_id: "screening".to_string(),
            accepted: true,
            consent_text_hash: Some("hash-screening".to_string()),
            metadata: json!({"before_room": true}),
        })
        .await
        .unwrap();
    store
        .record_consent_declaration(ConsentDeclarationRecord {
            experiment_id: "exp_weird".to_string(),
            session_id: Some(session_id),
            participant_id: prolific,
            purpose: "research".to_string(),
            consent_item_id: "record_audio".to_string(),
            accepted: false,
            consent_text_hash: Some("hash-audio".to_string()),
            metadata: json!({"reason": "declined_optional"}),
        })
        .await
        .unwrap();

    let event_payloads = [
        (
            "transcript_segment",
            Some(prolific),
            Some("A"),
            json!({"text": "hello there", "alternatives": [], "confidence": 0.91}),
            None,
        ),
        (
            "game_action_accepted",
            Some(agent),
            Some("B"),
            json!({"action": {"type": "toggle", "target": "aux"}, "events": [{"kind": "system_on"}]}),
            Some(json!({"systems": {"aux": true}, "history": [{"actor": "B"}]})),
        ),
        (
            "voice_diagnostic",
            Some(worker),
            Some("worker"),
            json!({"event": "temporary_key_minted", "latency_ms": 42}),
            None,
        ),
    ];
    for (event_type, actor_participant_id, actor_role, payload, game_state) in event_payloads {
        store
            .append_session_event(SessionEventRecord {
                experiment_id: "exp_weird".to_string(),
                session_id,
                event_type: event_type.to_string(),
                actor_participant_id,
                actor_role: actor_role.map(str::to_string),
                payload,
                game_state,
            })
            .await
            .unwrap();
    }

    let exported = store.export_session("exp_weird", session_id).await.unwrap();
    assert_eq!(exported["participants"].as_array().unwrap().len(), 3);
    assert_eq!(
        exported["session_participants"].as_array().unwrap().len(),
        3
    );
    assert_eq!(
        exported["consent_declarations"].as_array().unwrap().len(),
        1,
        "session export should include only session-scoped consent"
    );
    assert_eq!(exported["session_events"].as_array().unwrap().len(), 3);
    assert_eq!(exported["session_events"][0]["event_index"], 1);
    assert_eq!(exported["session_events"][1]["event_index"], 2);
    assert_eq!(
        exported["session_events"][1]["game_state"]["systems"]["aux"],
        true
    );

    let experiment = store.export_experiment("exp_weird").await.unwrap();
    assert_eq!(
        experiment["consent_declarations"].as_array().unwrap().len(),
        2
    );
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
        .update_game_settings(
            settings.revision,
            "Saarland University".to_string(),
            vec!["192.0.2.0/24".to_string()],
            "wss://eu.rt.speechmatics.com/v2".to_string(),
            "wss://api.elevenlabs.io".to_string(),
            provider_updates,
            vec![],
        )
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
        .update_game_settings(
            settings.revision,
            "Stale".to_string(),
            vec![],
            "wss://eu.rt.speechmatics.com/v2".to_string(),
            "wss://api.elevenlabs.io".to_string(),
            HashMap::new(),
            vec![],
        )
        .await
        .is_err());
}

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
            status: "running".to_string(),
            purpose: "research".to_string(),
        })
        .await
        .unwrap();

    store
        .expire_session("expiry", session_id, "idle_timeout")
        .await
        .unwrap();
    let exported = store.export_session("expiry", session_id).await.unwrap();
    assert_eq!(exported["sessions"][0]["status"], "expired");
    assert_eq!(exported["session_events"].as_array().unwrap().len(), 1);
    assert_eq!(
        exported["session_events"][0]["payload"]["reason"],
        "idle_timeout"
    );
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
            status: "running".to_string(),
            purpose: "research".to_string(),
        })
        .await
        .unwrap();

    store
        .abandon_session(SessionEventRecord {
            experiment_id: "departure".to_string(),
            session_id,
            event_type: "session_abandoned".to_string(),
            actor_participant_id: None,
            actor_role: Some("A".to_string()),
            payload: json!({"reason": "participant_left"}),
            game_state: None,
        })
        .await
        .unwrap();
    store
        .expire_session("departure", session_id, "reconnect_timeout")
        .await
        .unwrap();

    let exported = store.export_session("departure", session_id).await.unwrap();
    assert_eq!(exported["sessions"][0]["status"], "abandoned");
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

/// Rejects a database stamped below the deliberately supported schema baseline.
#[tokio::test]
async fn sqlite_rejects_prebaseline_schema_versions() {
    let temp = tempdir().expect("tempdir");
    let database_url = format!(
        "sqlite:///{}",
        temp.path().join("prebaseline.sqlite").display()
    );
    let store = SqliteExperimentStore::connect(&database_url).await.unwrap();
    sqlx::query("delete from schema_migrations")
        .execute(&store.pool)
        .await
        .unwrap();
    sqlx::query("insert into schema_migrations (version, applied_at) values (10, ?)")
        .bind(now_iso())
        .execute(&store.pool)
        .await
        .unwrap();
    store.pool.close().await;

    let error = match SqliteExperimentStore::connect(&database_url).await {
        Ok(_) => panic!("pre-baseline schema should fail"),
        Err(error) => error,
    };
    assert!(error
        .to_string()
        .contains("schema version 10 is unsupported"));
    assert!(error.to_string().contains("export"));
}

/// Rejects storage URLs for backends that this installation does not implement.
#[tokio::test]
async fn unsupported_database_scheme_fails_clearly() {
    let error = match experiment_store_from_url("postgres://localhost/parlando").await {
        Ok(_) => panic!("unsupported scheme should fail"),
        Err(error) => error,
    };

    assert!(error
        .to_string()
        .contains("unsupported database url scheme"));
}

/// Rejects an absent database URL instead of silently creating an implicit store.
#[tokio::test]
async fn empty_database_url_is_rejected() {
    let error = match experiment_store_from_url("").await {
        Ok(_) => panic!("empty database url should fail"),
        Err(error) => error,
    };

    assert!(error.to_string().contains("database.url is required"));
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
            status: "running".to_string(),
            purpose: "research".to_string(),
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

/// Creates one running session for storage-level lifecycle and logging tests.
async fn running_test_session(store: &SqliteExperimentStore, experiment_id: &str) -> i64 {
    store
        .create_experiment(ExperimentRecord {
            experiment_id: experiment_id.to_string(),
            game_version: "1.0.0".to_string(),
            config: json!({}),
            server_version: None,
            version_manifest: None,
            status: "inactive".to_string(),
            notes: None,
        })
        .await
        .unwrap();
    store
        .create_session(SessionRecord {
            experiment_id: experiment_id.to_string(),
            config_revision: 1,
            game_version: "1.0.0".to_string(),
            public_session_id: format!("{experiment_id}-public"),
            mode: "direct".to_string(),
            status: "running".to_string(),
            purpose: "research".to_string(),
        })
        .await
        .unwrap()
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
            metadata: json!({"agent_type": "test", "agent_name": "logger", "agent_version": "1"}),
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
                    Some(json!({"winner": "completion"})),
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
                .expire_session("terminal-race", session_id, "idle")
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
                .abandon_session(SessionEventRecord {
                    experiment_id: "terminal-race".to_string(),
                    session_id,
                    event_type: "session_abandoned".to_string(),
                    actor_participant_id: None,
                    actor_role: Some("B".to_string()),
                    payload: json!({"winner": "abandonment"}),
                    game_state: None,
                })
                .await
                .unwrap();
        })
    };
    barrier.wait().await;
    completion.await.unwrap();
    expiry.await.unwrap();
    abandonment.await.unwrap();

    let export = store
        .export_session("terminal-race", session_id)
        .await
        .unwrap();
    let status = export["sessions"][0]["status"].as_str().unwrap();
    assert!(matches!(status, "completed" | "expired" | "abandoned"));
    let terminal_events = export["session_events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| {
            matches!(
                event["event_type"].as_str(),
                Some("session_completed" | "session_expired" | "session_abandoned")
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(terminal_events.len(), 1);
    assert_eq!(
        terminal_events[0]["event_type"].as_str().unwrap(),
        format!("session_{status}")
    );
}
