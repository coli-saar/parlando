use super::*;

#[test]
fn admin_event_bundles_close_ready_before_later_disconnect() {
    let events = vec![
        admin_test_event(
            1,
            "session_created",
            None,
            json!({"public_session_id": "571EBA"}),
        ),
        admin_test_event(2, "participant_joined", Some("A"), json!({"role": "A"})),
        admin_test_event(3, "participant_joined", Some("B"), json!({"role": "B"})),
        admin_test_event(4, "participant_connected", Some("A"), Value::Null),
        admin_test_event(5, "ready", Some("A"), Value::Null),
        admin_test_event(
            8,
            "participant_connected",
            Some("B"),
            json!({"source": "agent"}),
        ),
        admin_test_event(133, "participant_disconnected", Some("A"), Value::Null),
    ];

    let bundles = admin_event_bundles(&events);

    let a_ready = bundles
        .iter()
        .find(|bundle| {
            bundle["kind"] == "participant" && bundle["role"] == "A" && bundle["first_index"] == 2
        })
        .unwrap();
    assert_eq!(a_ready["first_index"], 2);
    assert_eq!(a_ready["last_index"], 5);
    assert_eq!(a_ready["problem"], false);
    assert_eq!(a_ready["housekeeping"], true);

    let a_disconnect = bundles
        .iter()
        .find(|bundle| {
            bundle["kind"] == "participant" && bundle["role"] == "A" && bundle["first_index"] == 133
        })
        .unwrap();
    assert_eq!(a_disconnect["first_index"], 133);
    assert_eq!(a_disconnect["last_index"], 133);
    assert_eq!(a_disconnect["problem"], true);
    assert_eq!(a_disconnect["title"], "Participant");
}

#[test]
fn admin_event_bundles_merge_interleaved_action_events_by_action() {
    let action = json!({"type": "moveStep", "player": "B", "direction": "down"});
    let events = vec![
        admin_test_event(70, "agent_action", Some("B"), json!({"action": action})),
        admin_test_event(
            71,
            "game_action_accepted",
            Some("B"),
            json!({"action": {"type": "moveStep", "player": "B", "direction": "down"}}),
        ),
        admin_test_event(
            72,
            "conversation_message",
            Some("A"),
            json!({"origin": "voice_transcript", "sender_role": "A", "text": "Guten Abend."}),
        ),
    ];

    let bundles = admin_event_bundles(&events);

    let accepted = bundles
        .iter()
        .find(|bundle| bundle["title"] == "Action" && bundle["role"] == "B")
        .unwrap();
    assert_eq!(accepted["first_index"], 70);
    assert_eq!(accepted["last_index"], 71);
    assert_eq!(accepted["problem"], false);
    assert_eq!(accepted["housekeeping"], false);
    assert_eq!(accepted["action"]["type"], "moveStep");
    assert_eq!(accepted["action"]["direction"], "down");
}

#[test]
fn admin_timeline_includes_logs_with_readable_attribution() {
    let stored = crate::storage::StoredSessionEvent {
        event_id: 73,
        experiment_id: "experiment".to_string(),
        session_id: 1,
        event_index: 73,
        event_type: "log".to_string(),
        actor_participant_id: None,
        actor_role: None,
        payload: json!({
            "source": "game",
            "text": "accepted Great Tree action from role A: SetSun"
        }),
        game_state: None,
        game_time_ms: 73_000,
    };

    let events = important_admin_events(vec![stored]);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].title, "Game log");
    assert_eq!(
        events[0].text.as_deref(),
        Some("accepted Great Tree action from role A: SetSun")
    );

    let bundles = admin_event_bundles(&events);
    assert_eq!(bundles.len(), 1);
    assert_eq!(bundles[0]["title"], "Game log");
    assert_eq!(
        bundles[0]["text"],
        "accepted Great Tree action from role A: SetSun"
    );
    assert_eq!(bundles[0]["housekeeping"], false);
}

#[test]
fn admin_event_bundles_hide_routine_voice_rows_but_keep_setup_status() {
    let events = vec![
        admin_test_event(
            6,
            "voice_diagnostic",
            Some("A"),
            json!({"event": "voice_connect_requested"}),
        ),
        admin_test_event(
            7,
            "voice_diagnostic",
            Some("A"),
            json!({"event": "stt_initialized"}),
        ),
        admin_test_event(
            10,
            "voice_diagnostic",
            Some("A"),
            json!({"event": "voice_token_received"}),
        ),
        admin_test_event(
            11,
            "voice_diagnostic",
            Some("A"),
            json!({"event": "transcription_stream_connecting"}),
        ),
        admin_test_event(
            15,
            "voice_diagnostic",
            Some("A"),
            json!({"event": "transcription_stream_started"}),
        ),
        admin_test_event(
            18,
            "voice_diagnostic",
            Some("A"),
            json!({"event": "local_track_published"}),
        ),
    ];

    let bundles = admin_event_bundles(&events);

    assert!(bundles.iter().all(|bundle| !bundle["steps"]
        .as_str()
        .unwrap_or("")
        .contains("voice_token_received")));
    assert!(bundles.iter().all(|bundle| !bundle["steps"]
        .as_str()
        .unwrap_or("")
        .contains("local_track_published")));
    let voice_bundles = bundles
        .iter()
        .filter(|bundle| bundle["kind"] == "voice" && bundle["role"] == "A")
        .collect::<Vec<_>>();
    assert_eq!(voice_bundles.len(), 1);
    let stream_bundle = voice_bundles[0];
    assert_eq!(stream_bundle["first_index"], 6);
    assert_eq!(stream_bundle["last_index"], 15);
    assert_eq!(stream_bundle["title"], "Voice");
    assert_eq!(stream_bundle["problem"], false);
    assert_eq!(stream_bundle["problem_reason"], Value::Null);
    assert_eq!(stream_bundle["housekeeping"], true);
}

#[test]
fn admin_event_bundles_keep_late_teardown_after_completion_non_problematic() {
    let events = vec![
        admin_test_event(131, "session_completed", Some("B"), json!({"done": true})),
        admin_test_event(133, "participant_disconnected", Some("A"), Value::Null),
        admin_test_event(
            134,
            "voice_diagnostic",
            Some("A"),
            json!({"event": "transcription_stream_disconnected"}),
        ),
        admin_test_event(
            135,
            "voice_diagnostic",
            Some("A"),
            json!({"event": "audio_transport_disconnected"}),
        ),
    ];

    let bundles = admin_event_bundles(&events);

    let participant_teardown = bundles
        .iter()
        .find(|bundle| bundle["kind"] == "participant" && bundle["first_index"] == 133)
        .unwrap();
    assert_eq!(participant_teardown["problem"], false);
    assert_eq!(participant_teardown["problem_reason"], Value::Null);

    let voice_teardown = bundles
        .iter()
        .find(|bundle| bundle["kind"] == "voice" && bundle["first_index"] == 134)
        .unwrap();
    assert_eq!(voice_teardown["problem"], false);
    assert_eq!(voice_teardown["problem_reason"], Value::Null);
}

#[test]
fn admin_event_bundles_do_not_flag_pending_agent_actions_after_disconnect() {
    let events = vec![
        admin_test_event(133, "participant_disconnected", Some("A"), Value::Null),
        admin_test_event(
            136,
            "agent_action",
            Some("B"),
            json!({"action": {"type": "moveStep", "player": "B", "direction": "down"}}),
        ),
        admin_test_event(
            137,
            "agent_action",
            Some("B"),
            json!({"action": {"type": "moveStep", "player": "B", "direction": "up"}}),
        ),
        admin_test_event(
            138,
            "agent_action",
            Some("B"),
            json!({"action": {"type": "moveStep", "player": "B", "direction": "down"}}),
        ),
    ];

    let bundles = admin_event_bundles(&events);

    let disconnect = bundles
        .iter()
        .find(|bundle| bundle["first_index"] == 133)
        .unwrap();
    assert_eq!(disconnect["problem"], true);

    let late_actions = bundles
        .iter()
        .filter(|bundle| bundle["kind"] == "action")
        .collect::<Vec<_>>();
    assert_eq!(late_actions.len(), 3);
    assert!(late_actions
        .iter()
        .all(|bundle| bundle["problem"] == false && bundle["problem_reason"] == Value::Null));
}

#[test]
fn admin_event_bundles_merge_transcript_storage_and_display_rows() {
    let events = vec![
        admin_test_event(
            73,
            "transcript_segment",
            Some("A"),
            json!({"player": "A", "text": "Guten Abend."}),
        ),
        admin_test_event(
            74,
            "conversation_message",
            Some("A"),
            json!({"origin": "voice_transcript", "sender_role": "A", "text": "Guten Abend"}),
        ),
    ];

    let bundles = admin_event_bundles(&events);

    assert_eq!(bundles.len(), 1);
    assert_eq!(bundles[0]["title"], "Voice Message");
    assert_eq!(bundles[0]["first_index"], 73);
    assert_eq!(bundles[0]["last_index"], 74);
    assert_eq!(bundles[0]["text"], "Guten Abend.");
}

/// Equal words in consecutive turns must retain separate utterance parents.
#[test]
fn admin_bundles_keep_repeated_utterances_separate() {
    let events = vec![
        admin_test_event(
            1,
            "conversation_message",
            Some("A"),
            json!({"origin":"voice_transcript","text":"yes","metadata":{"start_game_time_ms":100,"end_game_time_ms":200,"tokens":[{"kind":"word","text":"yes","start_ms":100,"end_ms":200}]}}),
        ),
        admin_test_event(
            2,
            "conversation_message",
            Some("A"),
            json!({"origin":"voice_transcript","text":"yes","metadata":{"start_game_time_ms":400,"end_game_time_ms":500}}),
        ),
    ];
    let bundles = admin_event_bundles(&events);
    assert_eq!(bundles.len(), 2);
    assert_eq!(bundles[0]["tokens"][0]["start_ms"], 100);
    assert_eq!(bundles[1]["utterance_timing"]["start_ms"], 400);
}

/// Builds a durable event and applies the dashboard projection to it.
fn admin_test_event(
    index: i64,
    event_type: &str,
    role: Option<&str>,
    payload: Value,
) -> AdminEventSummary {
    let stored = crate::storage::StoredSessionEvent {
        event_id: index,
        experiment_id: "experiment".to_string(),
        session_id: 1,
        event_index: index,
        event_type: event_type.to_string(),
        actor_participant_id: None,
        actor_role: role.map(str::to_string),
        payload,
        game_state: None,
        game_time_ms: index * 1_000,
    };
    admin_event_summary(stored)
}
