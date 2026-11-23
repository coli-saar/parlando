use super::*;

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
            metadata: json!({
                "factory": "back-and-forth",
                "agent_name": "Back and forth",
                "agent_version": "v2",
                "temperature": 0,
                "seed": 1234
            }),
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
            lifecycle: "running".to_string(),
            purpose: "research".to_string(),
            waiting_timeout_seconds: 600,
            maximum_lifetime_seconds: 14_400,
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

    let exported = experiment_rows(&store, "exp_weird").await;
    assert_eq!(exported["participants"].as_array().unwrap().len(), 3);
    assert_eq!(
        exported["session_participants"].as_array().unwrap().len(),
        3
    );
    assert_eq!(
        exported["consent_declarations"].as_array().unwrap().len(),
        2
    );
    assert_eq!(exported["session_events"].as_array().unwrap().len(), 3);
    assert_eq!(exported["session_events"][0]["event_index"], 1);
    assert_eq!(exported["session_events"][1]["event_index"], 2);
    assert_eq!(
        exported["session_events"][1]["game_state"]["systems"]["aux"],
        true
    );
}

/// Surfaces malformed persisted JSON instead of replacing it with JSON null in exports.
#[tokio::test]
async fn sqlite_export_rejects_malformed_persisted_json() {
    let store = SqliteExperimentStore::connect("sqlite:///:memory:")
        .await
        .unwrap();
    store
        .create_experiment(ExperimentRecord {
            experiment_id: "malformed-export".to_string(),
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
            experiment_id: "malformed-export".to_string(),
            config_revision: 1,
            game_version: "0.4.0".to_string(),
            public_session_id: "MALFORMED".to_string(),
            mode: "direct".to_string(),
            lifecycle: "running".to_string(),
            purpose: "research".to_string(),
            waiting_timeout_seconds: 600,
            maximum_lifetime_seconds: 14_400,
        })
        .await
        .unwrap();
    sqlx::query("update sessions set completion_json = ? where session_id = ?")
        .bind("{")
        .bind(session_id)
        .execute(&store.pool)
        .await
        .unwrap();

    let error = store.export_snapshot("malformed-export").await.unwrap_err();
    assert!(error.to_string().contains("completion_json"));
}
