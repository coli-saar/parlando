use super::*;

#[tokio::test]
async fn transcript_endpoints_are_private_and_diagnostics_are_not_persisted() {
    let (config, _temp) = sqlite_config();
    let router = build_router(live_game(), config, ServeOptions::default())
        .await
        .unwrap();
    let (a, _b, public_session_id) = create_joined_room(router.clone()).await;

    let (conversation_get_status, _) = json_request(
        router.clone(),
        http::Method::GET,
        &format!("/api/sessions/{public_session_id}/conversation"),
        Value::Null,
    )
    .await;
    assert_eq!(conversation_get_status, StatusCode::NOT_FOUND);
    let (conversation_post_status, _) = json_request(
        router.clone(),
        http::Method::POST,
        &format!("/api/sessions/{public_session_id}/conversation"),
        json!({"text": "typed hello"}),
    )
    .await;
    assert_eq!(conversation_post_status, StatusCode::NOT_FOUND);
    let (transcript_get_status, _) = json_request(
        router.clone(),
        http::Method::GET,
        &format!("/api/sessions/{public_session_id}/transcripts"),
        Value::Null,
    )
    .await;
    assert_eq!(transcript_get_status, StatusCode::NOT_FOUND);
    let (transcript_stream_status, _) = json_request(
        router.clone(),
        http::Method::GET,
        &format!("/api/sessions/{public_session_id}/transcripts/stream"),
        Value::Null,
    )
    .await;
    assert_eq!(transcript_stream_status, StatusCode::NOT_FOUND);
    let (transcription_context_status, _) = json_request(
        router.clone(),
        http::Method::GET,
        &format!("/api/sessions/{public_session_id}/transcription-context"),
        Value::Null,
    )
    .await;
    assert_eq!(transcription_context_status, StatusCode::NOT_FOUND);

    let (transcript_status, _transcript) = participant_json_request(
        router.clone(),
        &a,
        http::Method::POST,
        &format!("/api/sessions/{public_session_id}/transcripts"),
        json!({

            "player": "B",
            "start_time_ms": 10,
            "end_time_ms": 40,
            "text": "spoken hello",
            "metadata": {"confidence": 0.9}
        }),
    )
    .await;
    assert_eq!(transcript_status, StatusCode::NOT_FOUND);

    let (diagnostic_status, diagnostic) = participant_json_request(
        router.clone(),
        &a,
        http::Method::POST,
        &format!("/api/sessions/{public_session_id}/voice-diagnostics"),
        json!({

            "event": "mic_started",
            "metadata": {"device": "test"}
        }),
    )
    .await;
    assert_eq!(diagnostic_status, StatusCode::OK);
    assert_eq!(diagnostic["stored"], false);

    let (export_status, export) = admin_json_request(
        router,
        http::Method::GET,
        "/api/admin/export?variant=full",
        Value::Null,
    )
    .await;
    assert_eq!(export_status, StatusCode::OK);
    let events = export["session_events"].as_array().unwrap();
    assert!(events.iter().all(|event| !matches!(
        event["event_type"].as_str(),
        Some("transcript_segment" | "voice_diagnostic")
    )));
}
