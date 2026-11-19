use super::*;

/// Groups durable admin events into display rows for the experiment dashboard.
pub(in crate::app) fn admin_event_bundles(events: &[AdminEventSummary]) -> Vec<Value> {
    let mut bundles = Vec::<AdminEventBundle>::new();
    let mut open_bundles = HashMap::<String, usize>::new();
    let mut terminal_event_seen = false;
    for event in events
        .iter()
        .filter(|event| show_admin_event_in_timeline(event))
    {
        let kind = admin_bundle_kind(event);
        let role = admin_event_role(event);
        let key = admin_bundle_key(event, &kind, role);
        let existing_index = open_bundles.get(&key).copied();
        if let Some(index) = existing_index {
            if can_append_admin_bundle(&bundles[index], event, &kind, role) {
                bundles[index].events.push(event);
                bundles[index].after_terminal_event =
                    bundles[index].after_terminal_event || terminal_event_seen;
                if admin_bundle_is_closed(&bundles[index]) {
                    open_bundles.remove(&key);
                }
                if admin_event_is_terminal_boundary(event) {
                    terminal_event_seen = true;
                }
                continue;
            }
        }

        let bundle = AdminEventBundle {
            kind: kind.clone(),
            role,
            key: key.clone(),
            events: vec![event],
            after_terminal_event: terminal_event_seen,
        };
        bundles.push(bundle);
        let index = bundles.len() - 1;
        if !admin_bundle_is_closed(&bundles[index]) {
            open_bundles.insert(key, index);
        }
        if admin_event_is_terminal_boundary(event) {
            terminal_event_seen = true;
        }
    }
    bundles.into_iter().map(admin_bundle_json).collect()
}

/// Identifies events after which teardown messages belong to a completed interaction.
fn admin_event_is_terminal_boundary(event: &AdminEventSummary) -> bool {
    matches!(
        event.event_type.as_str(),
        "session_completed" | "session_abandoned" | "session_expired" | "participant_disconnected"
    )
}

/// Identifies the request side of an action bundle.
fn admin_event_is_action_request(event: &AdminEventSummary) -> bool {
    matches!(
        event.event_type.as_str(),
        "agent_action" | "game_action_submitted"
    )
}

/// Filters routine diagnostics before creating dashboard rows.
fn show_admin_event_in_timeline(event: &AdminEventSummary) -> bool {
    if event.event_type != "voice_diagnostic" {
        return true;
    }
    let text = admin_event_text_value(event);
    text.contains("voice_connect_requested")
        || text.contains("stt_initialized")
        || text.contains("transcription_stream_connecting")
        || text.contains("transcription_stream_started")
        || text.to_ascii_lowercase().contains("fail")
        || text.to_ascii_lowercase().contains("error")
        || text.to_ascii_lowercase().contains("disconnect")
}

/// Selects the row category used to group this event.
fn admin_bundle_kind(event: &AdminEventSummary) -> String {
    match event.event_type.as_str() {
        "agent_action"
        | "game_action_accepted"
        | "game_action_rejected"
        | "game_action_submitted" => "action".to_string(),
        "participant_joined" | "participant_connected" | "participant_disconnected" | "ready" => {
            "participant".to_string()
        }
        "voice_diagnostic" => "voice".to_string(),
        "transcript_segment" => "transcript".to_string(),
        "conversation_message"
            if event.detail.get("origin").and_then(Value::as_str) == Some("voice_transcript") =>
        {
            "transcript".to_string()
        }
        "conversation_message" => "conversation".to_string(),
        other => other.to_string(),
    }
}

/// Builds the identity used to find a compatible open row.
fn admin_bundle_key(event: &AdminEventSummary, kind: &str, role: Option<&str>) -> String {
    let role = role.unwrap_or("system");
    match kind {
        "action" => format!(
            "action:{role}:{}",
            event
                .detail
                .get("action")
                .map(compact_json)
                .unwrap_or_else(|| "null".to_string())
        ),
        "participant" | "voice" => format!("{kind}:{role}"),
        "transcript" => format!("{kind}:{role}:{}", normalized_transcript_text(event)),
        _ => format!("{kind}:{role}:{}", event.event_index),
    }
}

/// Checks whether this event may continue the candidate row.
fn can_append_admin_bundle(
    bundle: &AdminEventBundle<'_>,
    event: &AdminEventSummary,
    kind: &str,
    role: Option<&str>,
) -> bool {
    if bundle.kind != kind {
        return false;
    }
    if (bundle.role.is_some() || role.is_some()) && bundle.role != role {
        return false;
    }
    match kind {
        "action" => {
            !admin_bundle_is_closed(bundle)
                && bundle
                    .events
                    .first()
                    .and_then(|event| admin_event_action(event))
                    == admin_event_action(event)
                && !(bundle
                    .events
                    .iter()
                    .any(|event| admin_event_is_action_request(event))
                    && admin_event_is_action_request(event))
        }
        "transcript" => {
            !(event.event_type == "conversation_message"
                && bundle
                    .events
                    .iter()
                    .any(|item| item.event_type == "conversation_message"))
                && bundle
                    .events
                    .first()
                    .map(|event| normalized_transcript_text(event))
                    .unwrap_or_default()
                    == normalized_transcript_text(event)
        }
        "participant" | "voice" => true,
        _ => false,
    }
}

/// Recognizes the final step for each multi-event row type.
fn admin_bundle_is_closed(bundle: &AdminEventBundle<'_>) -> bool {
    match bundle.kind.as_str() {
        "action" => bundle.events.iter().any(|event| {
            matches!(
                event.event_type.as_str(),
                "game_action_accepted" | "game_action_rejected"
            )
        }),
        "participant" => bundle.events.iter().any(|event| {
            matches!(
                event.event_type.as_str(),
                "ready" | "participant_disconnected"
            )
        }),
        "voice" => bundle.events.iter().any(|event| {
            let text = admin_event_text_value(event).to_ascii_lowercase();
            text.contains("transcription_stream_started")
                || text.contains("ready")
                || text.contains("fail")
                || text.contains("error")
                || text.contains("disconnect")
        }),
        _ => false,
    }
}
/// Resolves the role from the durable actor or event-specific payload.
pub(super) fn admin_event_role(event: &AdminEventSummary) -> Option<&str> {
    event
        .actor_role
        .as_deref()
        .or_else(|| event.detail.get("player").and_then(Value::as_str))
        .or_else(|| event.detail.get("sender_role").and_then(Value::as_str))
        .or_else(|| event.detail.get("role").and_then(Value::as_str))
}

/// Selects the text available for grouping and display.
pub(super) fn admin_event_text_value(event: &AdminEventSummary) -> &str {
    event
        .text
        .as_deref()
        .or_else(|| event.detail.get("event").and_then(Value::as_str))
        .unwrap_or(event.event_type.as_str())
}

/// Normalizes punctuation and case when matching transcript events.
pub(super) fn normalized_transcript_text(event: &AdminEventSummary) -> String {
    event
        .text
        .as_deref()
        .or_else(|| {
            event
                .raw
                .get("payload")
                .and_then(|payload| payload.get("text"))
                .and_then(Value::as_str)
        })
        .unwrap_or_default()
        .trim()
        .trim_end_matches(['.', '!', '?'])
        .to_ascii_lowercase()
}

/// Reads the action from the summarized detail or original payload.
pub(super) fn admin_event_action(event: &AdminEventSummary) -> Option<&Value> {
    event.detail.get("action").or_else(|| {
        event
            .raw
            .get("payload")
            .and_then(|payload| payload.get("action"))
    })
}
