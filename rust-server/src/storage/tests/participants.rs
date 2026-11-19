use super::*;

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
                "factory": "space_game.back_and_forth",
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
            lifecycle: "running".to_string(),
            purpose: "research".to_string(),
            waiting_timeout_seconds: 600,
            maximum_lifetime_seconds: 14_400,
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
            lifecycle: "running".to_string(),
            purpose: "research".to_string(),
            waiting_timeout_seconds: 600,
            maximum_lifetime_seconds: 14_400,
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

    let experiment = experiment_rows(&store, "exp_returning").await;
    assert_eq!(experiment["participants"].as_array().unwrap().len(), 1);
    assert_eq!(
        experiment["session_participants"].as_array().unwrap().len(),
        2
    );
    assert!(experiment["participants"][0].get("role").is_none());
    assert_eq!(experiment["session_participants"][0]["role"], "A");
    assert_eq!(experiment["session_participants"][1]["role"], "B");

    assert_eq!(
        store
            .session_participants("exp_returning", second_session)
            .await
            .unwrap()[0]
            .participant_session_id,
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

    let mut participant_ids = Vec::new();
    for suffix in ["A", "B"] {
        let admission = store
            .admit_prolific_submission(
                ProlificSubmissionRecord {
                    experiment_id: "exp_prolific_purge".to_string(),
                    prolific_participant_id: format!("PROLIFIC-{suffix}"),
                    prolific_study_id: "STUDY-PRIVATE".to_string(),
                    prolific_session_id: format!("SUBMISSION-{suffix}"),
                    verification_method: "submission_api".to_string(),
                },
                true,
            )
            .await
            .unwrap()
            .unwrap();
        participant_ids.push(admission.participant_id);
    }
    let participant_a = participant_ids[0];
    let participant_b = participant_ids[1];

    let session_id = store
        .create_session(SessionRecord {
            experiment_id: "exp_prolific_purge".to_string(),
            config_revision: 1,
            game_version: "0.4.0".to_string(),
            public_session_id: "ROOM_PURGE".to_string(),
            mode: "human_vs_human".to_string(),
            lifecycle: "ended".to_string(),
            purpose: "research".to_string(),
            waiting_timeout_seconds: 600,
            maximum_lifetime_seconds: 14_400,
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
