//! Records and coalesces rejected input without retaining its contents.
//!
//! The input dispatcher supplies the rejection reason and optional submitted bytes.
//! Durable diagnostic records contain bounded evidence such as size, a digest, and
//! occurrence counts. The game socket flushes outstanding aggregates when its transport
//! ends.

use super::{json, persist_session_event, AppState, Arc, Digest, RejectionWindow, Sha256};

/// Persists one analyzable, size-bounded rejected-action event.
pub(super) async fn persist_rejected_action(
    state: &Arc<AppState>,
    public_session_id: &str,
    participant_session_id: &str,
    reason_code: &str,
    error: &str,
    raw_action: Option<&[u8]>,
) {
    persist_rejected_input(
        state,
        public_session_id,
        participant_session_id,
        "game_action_rejected",
        reason_code,
        error,
        raw_action,
    )
    .await;
}

/// Persists the first and then one periodic aggregate for repeated rejected inputs.
pub(super) async fn persist_rejected_input(
    state: &Arc<AppState>,
    public_session_id: &str,
    participant_session_id: &str,
    event_type: &'static str,
    reason_code: &str,
    error: &str,
    raw_input: Option<&[u8]>,
) {
    match event_type {
        "game_action_rejected" => state
            .context
            .installation
            .telemetry
            .record_action_rejected(),
        "chat_message_rejected" => state.context.installation.telemetry.record_chat_rejected(),
        _ => {}
    }
    let now = chrono::Utc::now().timestamp();
    let key = format!("{public_session_id}\0{participant_session_id}\0{event_type}\0{reason_code}");
    let aggregate = {
        let mut windows = state.rejection_windows.write().await;
        let window = windows.entry(key).or_insert(RejectionWindow {
            first_seen: now,
            last_seen: now,
            last_persisted: now - 61,
            occurrences: 0,
        });
        window.occurrences += 1;
        window.last_seen = now;
        if window.last_persisted > now - 60 {
            return;
        }
        let aggregate = (window.first_seen, window.last_seen, window.occurrences);
        window.first_seen = now;
        window.last_persisted = now;
        window.occurrences = 0;
        aggregate
    };
    let mut payload = json!({
        "reason_code": reason_code,
        "error": error.chars().take(512).collect::<String>(),
        "first_seen_at": aggregate.0,
        "last_seen_at": aggregate.1,
        "occurrences": aggregate.2,
    });
    if let Some(raw_input) = raw_input {
        if let Some(object) = payload.as_object_mut() {
            object.insert("submitted_bytes".to_string(), json!(raw_input.len()));
            object.insert(
                if event_type == "game_action_rejected" {
                    "action_sha256"
                } else {
                    "input_sha256"
                }
                .to_string(),
                json!(format!("{:x}", Sha256::digest(raw_input))),
            );
        }
    }
    persist_session_event(
        state,
        public_session_id,
        Some(participant_session_id),
        event_type,
        payload,
        None,
    )
    .await;
}

/// Flushes suppressed rejection counts when the participant's current game transport ends.
pub(super) async fn flush_rejected_input_aggregates(
    state: &Arc<AppState>,
    public_session_id: &str,
    participant_session_id: &str,
) {
    let prefix = format!("{public_session_id}\0{participant_session_id}\0");
    let aggregates = {
        let mut windows = state.rejection_windows.write().await;
        let keys = windows
            .keys()
            .filter(|key| key.starts_with(&prefix))
            .cloned()
            .collect::<Vec<_>>();
        keys.into_iter()
            .filter_map(|key| {
                let window = windows.remove(&key)?;
                let mut parts = key.split('\0');
                let _room = parts.next()?;
                let _participant = parts.next()?;
                let event_type = match parts.next()? {
                    "game_action_rejected" => "game_action_rejected",
                    "chat_message_rejected" => "chat_message_rejected",
                    "client_message_rejected" => "client_message_rejected",
                    _ => return None,
                };
                let reason_code = parts.next()?.to_string();
                (window.occurrences > 0).then_some((event_type, reason_code, window))
            })
            .collect::<Vec<_>>()
    };
    for (event_type, reason_code, window) in aggregates {
        persist_session_event(
            state,
            public_session_id,
            Some(participant_session_id),
            event_type,
            json!({
                "reason_code": reason_code,
                "first_seen_at": window.first_seen,
                "last_seen_at": window.last_seen,
                "occurrences": window.occurrences,
                "coalesced": true,
            }),
            None,
        )
        .await;
    }
}
