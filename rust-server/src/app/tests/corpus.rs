use super::*;

/// Confirms the corpus is one experiment document with dashboard IDs and relative timing.
#[test]
fn corpus_export_is_nested_informative_and_structurally_minimized() {
    let full = json!({
        "consent_declarations": [],
        "participants": [
            {"participant_id": 7, "research_id": "calm-blue-otter", "participant_kind": "human", "external_id": "recruitment-id"},
            {"participant_id": 8, "research_id": "agent:tiny@1", "participant_kind": "agent", "metadata": {"agent_name": "tiny", "agent_version": "1"}},
            {"participant_id": 9, "research_id": "waiting-only", "participant_kind": "human"}
        ],
        "experiment": {"experiment_id": "study", "game_version": "0.4.0", "config_revision": 2, "config": {"game": {"condition": "example"}, "direct": {"participant_information_url": "secret"}}},
        "sessions": [
            {"experiment_id": "study", "purpose":"research", "public_session_id": "room", "session_id": 3, "dialogue_id": "softly-amber-harbor", "config_revision": 2, "game_version": "0.4.0", "mode": "direct", "lifecycle": "ended", "session_end": {"cause":{"type":"game_completed"}, "participant_results": {}, "completion":{"outcome":"success"}}, "created_at": "2026-01-01T00:00:00Z", "started_at": "2026-01-01T00:00:01Z", "ended_at": "2026-01-01T00:00:05Z", "completion": {"outcome": "success"}},
            {"experiment_id": "study", "purpose":"research", "public_session_id": "room2", "session_id": 4, "dialogue_id": "quiet-waiting-session", "config_revision": 2, "game_version": "0.4.0", "mode": "direct", "lifecycle": "ended", "created_at": "2026-01-01T00:01:00Z", "started_at": null, "ended_at": "2026-01-01T00:02:00Z", "completion": null}
        ],
        "session_participants": [
            {"session_id": 3, "participant_id": 7, "participant_session_id": "ps_secret", "role": "A"},
            {"session_id": 3, "participant_id": 8, "participant_session_id": "ps_agent", "role": "B"},
            {"session_id": 4, "participant_id": 9, "participant_session_id": "ps_waiting", "role": "A"}
        ],
        "session_events": [
            {
                "session_id": 3,
                "event_index": 4, "event_id": 4, "experiment_id": "study",
                "event_type": "conversation_message",
                "actor_participant_id": 7,
                "actor_role": "A",
                "payload": {"text": "hello", "origin": "typed", "sender_participant_session_id": "ps_secret"},
                "game_time_ms": 3000
            },
            {
                "session_id": 3,
                "event_index": 5, "event_id": 5, "experiment_id": "study",
                "event_type": "conversation_message",
                "actor_participant_id": 7,
                "actor_role": "A",
                "payload": {"text": "spoken hello", "origin": "voice_transcript", "metadata": {"start_game_time_ms": 3200, "end_game_time_ms": 3900, "tokens":[{"kind":"word","text":"spoken","start_ms":3200,"end_ms":3500,"confidence":0.9,"private_provider_field":"excluded"}]}},
                "game_time_ms": 4000
            },
            {
                "session_id": 3,
                "event_index": 6, "event_id": 6, "experiment_id": "study",
                "event_type": "log",
                "actor_participant_id": null,
                "actor_role": null,
                "payload": {"source": "game", "text": "game diagnostic"},
                "game_time_ms": 4100
            },
            {
                "session_id": 3,
                "event_index": 7, "event_id": 7, "experiment_id": "study",
                "event_type": "log",
                "actor_participant_id": 8,
                "actor_role": "B",
                "payload": {"source": "agent", "text": "agent diagnostic"},
                "game_time_ms": 4200
            }
        ]
    });
    let corpus =
        corpus_experiment_export(serde_json::from_value(full.clone()).unwrap(), "2").unwrap();
    let encoded = serde_json::to_string(&corpus).unwrap();
    assert_eq!(corpus["release_status"], "corpus_candidate");
    assert_eq!(corpus["data_inventory"]["participants"], 2);
    assert_eq!(corpus["data_inventory"]["sessions"], 1);
    assert!(encoded.contains("calm-blue-otter"));
    assert!(encoded.contains("softly-amber-harbor"));
    assert!(!encoded.contains("2026-01-01"));
    assert!(!encoded.contains("recruitment-id"));
    assert!(!encoded.contains("ps_secret"));
    assert!(!encoded.contains("2026-01-01"));
    assert!(!encoded.contains("participant_information_url"));
    assert_eq!(
        corpus["experiment"]["configuration"]["condition"],
        "example"
    );
    assert_eq!(
        corpus["experiment"]["sessions"][0]["events"][0]["text"],
        "hello"
    );
    assert_eq!(
        corpus["experiment"]["sessions"][0]["events"][0]["game_time_ms"],
        3_000
    );
    assert_eq!(
        corpus["experiment"]["sessions"][0]["events"][1]["utterance_timing"],
        json!({"origin": "game_clock", "start_ms": 3_200, "end_ms": 3_900})
    );
    assert_eq!(
        corpus["experiment"]["sessions"][0]["events"][1]["tokens"],
        json!([{"kind":"word","text":"spoken","start_ms":3200,"end_ms":3500,"confidence":0.9}])
    );
    assert!(corpus["experiment"]["sessions"][0]["events"][0]
        .get("tokens")
        .is_none());
    assert_eq!(corpus["data_inventory"]["logs"], 2);
    assert_eq!(
        corpus["experiment"]["sessions"][0]["events"][2],
        json!({
            "index": 6,
            "game_time_ms": 4_100,
            "kind": "log",
            "source": "game",
            "participant_id": null,
            "role": null,
            "text": "game diagnostic",
        })
    );
    assert_eq!(
        corpus["experiment"]["sessions"][0]["events"][3],
        json!({
            "index": 7,
            "game_time_ms": 4_200,
            "kind": "log",
            "source": "agent",
            "participant_id": "agent:tiny@1",
            "role": "B",
            "text": "agent diagnostic",
        })
    );
    assert_eq!(
        corpus["experiment"]["sessions"][0]["metadata"]["time_to_start_ms"],
        1_000
    );
    assert_eq!(
        corpus["experiment"]["sessions"][0]["metadata"]["duration_ms"],
        4_000
    );

    let csv = export_csv(&corpus);
    assert_eq!(
        csv.lines()
            .filter(|line| line.starts_with("manifest,"))
            .count(),
        1
    );
    assert_eq!(
        csv.lines()
            .filter(|line| line.starts_with("experiment,"))
            .count(),
        1
    );
    assert_eq!(
        csv.lines()
            .filter(|line| line.starts_with("session,"))
            .count(),
        1
    );

    // A component checkpoint is infrastructure data, even when the guest puts
    // private observations in its snapshot alongside the authoritative state.
    let mut checkpoint_export = full.clone();
    checkpoint_export["session_events"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "session_id": 3, "event_id": 8, "event_index": 8, "experiment_id": "study",
            "event_type": "game_action_accepted", "actor_participant_id": 7,
            "actor_role": "A", "game_time_ms": 4300,
            "payload": { "action": "move", "recovery_checkpoint": true },
            "game_state": { "private": "excluded_checkpoint_contents" }
        }));
    let exported =
        corpus_experiment_export(serde_json::from_value(checkpoint_export).unwrap(), "2").unwrap();
    assert!(!exported
        .to_string()
        .contains("excluded_checkpoint_contents"));
    assert!(exported["experiment"]["sessions"][0]["events"][4]["state"].is_null());

    let mut invalid_assignments = full.clone();
    invalid_assignments["session_participants"]
        .as_array_mut()
        .unwrap()
        .retain(|row| row["participant_id"] != 8);
    assert!(
        corpus_experiment_export(serde_json::from_value(invalid_assignments).unwrap(), "2")
            .unwrap_err()
            .to_string()
            .contains("expected exactly two")
    );

    let mut invalid = full;
    invalid["session_events"][0]["game_time_ms"] = json!(-1);
    assert!(
        corpus_experiment_export(serde_json::from_value(invalid).unwrap(), "2")
            .unwrap_err()
            .to_string()
            .contains("without valid game time")
    );

    let schema: Value = serde_json::from_str(CORPUS_EXPORT_SCHEMA_V1).unwrap();
    assert_eq!(
        schema["$id"],
        "https://parlando.dev/schemas/parlando.corpus.v1.json"
    );
}

/// Confirms test sessions and their dependent rows are absent from every export variant input.
#[test]
fn export_filter_removes_testing_session_graph() {
    let exported = json!({
        "participants": [
            {"participant_id": 1, "research_id": "test-only"},
            {"participant_id": 2, "research_id": "research"}
        ],
        "sessions": [
            {"experiment_id":"e","mode":"direct","lifecycle":"forming","config_revision":1,"game_version":"1.0.0","created_at":"2026-01-01T00:00:00Z","public_session_id":"r10","dialogue_id":"d10","session_id":10,"purpose":"testing"},
            {"experiment_id":"e","mode":"direct","lifecycle":"forming","config_revision":1,"game_version":"1.0.0","created_at":"2026-01-01T00:00:00Z","public_session_id":"r20","dialogue_id":"d20","session_id":20,"purpose":"research"}
        ],
        "session_participants": [
            {"session_id": 10, "participant_id": 1},
            {"session_id": 20, "participant_id": 2}
        ],
        "consent_declarations": [
            {"session_id": 10, "participant_id": 1, "purpose": "testing"},
            {"session_id": null, "participant_id": 1, "purpose": "testing"},
            {"session_id": null, "participant_id": 2, "purpose": "research"}
        ],
        "session_events": [
            {"session_id": 10, "experiment_id": "e", "event_id": 1,"event_type":"log","event_index":1,"payload":{},"game_time_ms":0},
            {"session_id": 20, "experiment_id": "e", "event_id": 1,"event_type":"log","event_index":1,"payload":{},"game_time_ms":0}
        ]
    });

    let mut snapshot = serde_json::from_value(exported).unwrap();
    retain_export_sessions(&mut snapshot, false);
    let exported = serde_json::to_value(snapshot).unwrap();

    assert_eq!(exported["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(
        exported["session_participants"].as_array().unwrap().len(),
        1
    );
    assert_eq!(exported["session_events"].as_array().unwrap().len(), 1);
    assert_eq!(exported["participants"].as_array().unwrap().len(), 1);
    assert_eq!(exported["participants"][0]["participant_id"], 2);
    assert_eq!(
        exported["consent_declarations"].as_array().unwrap().len(),
        1
    );
    assert_eq!(exported["consent_declarations"][0]["participant_id"], 2);
}

/// Corrupt timing cannot silently enter a corpus with a false timestamp.
#[test]
fn corpus_tokens_reject_invalid_intervals() {
    assert!(export_transcript_tokens(
        &json!([{"kind":"word","text":"a","start_ms":20,"end_ms":10}])
    )
    .is_err());
    assert!(export_transcript_tokens(
        &json!([{"kind":"word","text":"a","start_ms":0,"end_ms":10,"confidence":2}])
    )
    .is_err());
}
