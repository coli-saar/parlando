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

/// Confirms reopening an existing database replaces legacy random agent names.
#[tokio::test]
async fn sqlite_migrates_legacy_agent_random_names() {
    let temp = tempdir().expect("tempdir");
    let database_url = format!("sqlite:///{}", temp.path().join("agents.sqlite").display());
    let store = SqliteExperimentStore::connect(&database_url).await.unwrap();
    let agent = store
        .upsert_participant(ParticipantRecord {
            experiment_id: "experiment-a".to_string(),
            participant_kind: "agent".to_string(),
            identity_provider: "remote_grpc".to_string(),
            external_id: Some("planner@test-7".to_string()),
            metadata: json!({
                "agent_type": "remote_grpc",
                "agent_name": "planner",
                "agent_version": "test-7",
            }),
        })
        .await
        .unwrap();
    sqlx::query("update participants set research_id = 'calm-blue-otter' where participant_id = ?")
        .bind(agent)
        .execute(&store.pool)
        .await
        .unwrap();
    sqlx::query("delete from schema_migrations")
        .execute(&store.pool)
        .await
        .unwrap();
    store.pool.close().await;

    let reopened = SqliteExperimentStore::connect(&database_url).await.unwrap();
    assert_eq!(
        reopened.participant_research_id(agent).await.unwrap(),
        Some("agent:remote_grpc:planner@test-7".to_string())
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
            "consent_declarations",
            "experiment_config_revisions",
            "experiments",
            "game_settings",
            "participants",
            "schema_migrations",
            "session_events",
            "session_participants",
            "sessions",
        ]
    );
}

/// Confirms the catalogue migration recovers an exact game version from old manifests.
#[tokio::test]
async fn sqlite_migration_recovers_historical_game_version() {
    let temp = tempdir().expect("tempdir");
    let database_url = format!(
        "sqlite:///{}",
        temp.path().join("versions.sqlite").display()
    );
    let store = SqliteExperimentStore::connect(&database_url).await.unwrap();
    store
        .create_experiment(ExperimentRecord {
            experiment_id: "historical".to_string(),
            game_version: "0.4.0".to_string(),
            config: json!({}),
            server_version: Some("0.2.0".to_string()),
            version_manifest: Some(json!({"game": {"version": "0.4.0"}})),
            status: "inactive".to_string(),
            notes: None,
        })
        .await
        .unwrap();
    sqlx::query("update experiments set game_version = 'legacy'")
        .execute(&store.pool)
        .await
        .unwrap();
    sqlx::query("delete from schema_migrations where version = 2")
        .execute(&store.pool)
        .await
        .unwrap();
    store.pool.close().await;

    let reopened = SqliteExperimentStore::connect(&database_url).await.unwrap();
    let definition = reopened
        .experiment_definition("historical")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(definition.game_version, "0.4.0");
}

/// Confirms an existing installation loses the obsolete participant display-name column.
#[tokio::test]
async fn sqlite_migration_removes_participant_display_names() {
    let temp = tempdir().expect("tempdir");
    let database_path = temp.path().join("legacy.sqlite");
    let setup_pool = SqlitePoolOptions::new()
        .connect_with(
            SqliteConnectOptions::new()
                .filename(&database_path)
                .create_if_missing(true),
        )
        .await
        .unwrap();
    sqlx::query(
        r#"
            create table participants (
                participant_id integer primary key autoincrement,
                participant_kind text not null,
                identity_provider text not null,
                external_id text,
                display_name text,
                metadata_json text,
                created_at text not null
            )
            "#,
    )
    .execute(&setup_pool)
    .await
    .unwrap();
    sqlx::query(
            "create unique index idx_participants_provider_external on participants(identity_provider, external_id) where external_id is not null",
        )
        .execute(&setup_pool)
        .await
        .unwrap();
    sqlx::query(
            "insert into participants (participant_kind, identity_provider, external_id, display_name, metadata_json, created_at) values ('human', 'prolific', 'same-recruitment-id', 'Legacy Name', '{}', '2026-01-01T00:00:00Z')",
        )
        .execute(&setup_pool)
        .await
        .unwrap();
    sqlx::query(
            "create table session_participants (experiment_id text not null, participant_id integer not null)",
        )
        .execute(&setup_pool)
        .await
        .unwrap();
    sqlx::query("insert into session_participants values ('experiment-a', 1), ('experiment-b', 1)")
        .execute(&setup_pool)
        .await
        .unwrap();
    setup_pool.close().await;

    let database_url = format!("sqlite:///{}", database_path.display());
    let store = SqliteExperimentStore::connect(&database_url).await.unwrap();
    let columns = sqlx::query_as::<_, (i64, String, String, i64, Option<String>, i64)>(
        "pragma table_info(participants)",
    )
    .fetch_all(&store.pool)
    .await
    .unwrap();
    assert!(columns.iter().all(|column| column.1 != "display_name"));
    assert!(columns.iter().any(|column| column.1 == "research_id"));
    let participants = sqlx::query_as::<_, (String, String, String)>(
        "select experiment_id, research_id, external_id from participants order by experiment_id",
    )
    .fetch_all(&store.pool)
    .await
    .unwrap();
    assert_eq!(participants.len(), 2);
    assert_eq!(participants[0].0, "experiment-a");
    assert_eq!(participants[1].0, "experiment-b");
    assert_ne!(participants[0].1, participants[1].1);
    assert_eq!(participants[0].2, "same-recruitment-id");
    assert_eq!(participants[1].2, "same-recruitment-id");
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
            identity_provider: "prolific".to_string(),
            external_id: Some("PID123".to_string()),
            metadata: json!({"source": "fixture"}),
        })
        .await
        .unwrap();
    let same_participant_id = store
        .upsert_participant(ParticipantRecord {
            experiment_id: "exp_eval".to_string(),
            participant_kind: "human".to_string(),
            identity_provider: "prolific".to_string(),
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
            room_id: "ROOM1".to_string(),
            mode: "direct".to_string(),
            status: "waiting".to_string(),
        })
        .await
        .unwrap();
    let session_two = store
        .create_session(SessionRecord {
            experiment_id: "exp_eval".to_string(),
            config_revision: 1,
            game_version: "0.4.0".to_string(),
            room_id: "ROOM2".to_string(),
            mode: "direct".to_string(),
            status: "waiting".to_string(),
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
    store
        .delete_participant_data("exp_eval", participant_id)
        .await
        .unwrap();
    let redacted = store.export_session("exp_eval", session_one).await.unwrap();
    assert!(redacted["consent_declarations"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(redacted["participants"][0]["research_id"], Value::Null);
    assert_eq!(redacted["participants"][0]["external_id"], Value::Null);
    assert_eq!(
        redacted["session_events"][0]["actor_participant_id"],
        Value::Null
    );
    assert_eq!(
        redacted["session_events"][0]["actor_role"],
        "deleted_participant"
    );
    assert_ne!(
        redacted["session_participants"][0]["participant_session_id"],
        "ps_1"
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
            room_id: "ROOM_A".to_string(),
            mode: "human_vs_human".to_string(),
            status: "playing".to_string(),
        })
        .await
        .unwrap();
    let second_session = store
        .create_session(SessionRecord {
            experiment_id: "exp_returning".to_string(),
            config_revision: 1,
            game_version: "0.4.0".to_string(),
            room_id: "ROOM_B".to_string(),
            mode: "role_swap_replay".to_string(),
            status: "playing".to_string(),
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
            room_id: "ROOM_WEIRD".to_string(),
            mode: "human_agent_with_worker".to_string(),
            status: "playing".to_string(),
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
        .save_experiment_revision(
            "pilot",
            1,
            json!({"study": {"name": "Revised pilot"}}),
            Some("Clarified title".to_string()),
        )
        .await
        .unwrap();
    assert_eq!(revision, 2);
    assert!(store
        .save_experiment_revision("pilot", 1, Value::Null, None)
        .await
        .is_err());

    store
        .update_experiment_catalogue("pilot", true, false, Some("Important".to_string()))
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
    let revision = store
        .update_game_settings(settings.revision, "Saarland University".to_string())
        .await
        .unwrap();
    assert_eq!(revision, settings.revision + 1);
    assert_eq!(
        store.game_settings().await.unwrap().institution,
        "Saarland University"
    );
    assert!(store
        .update_game_settings(settings.revision, "Stale".to_string())
        .await
        .is_err());
}

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

#[tokio::test]
async fn empty_database_url_is_rejected() {
    let error = match experiment_store_from_url("").await {
        Ok(_) => panic!("empty database url should fail"),
        Err(error) => error,
    };

    assert!(error.to_string().contains("database.url is required"));
}
