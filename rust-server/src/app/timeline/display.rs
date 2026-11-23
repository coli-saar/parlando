//! Formats a grouped event bundle for the dashboard. Its title, text, steps and problem
//! status are derived from the bundle’s events and terminal context. Group membership has
//! already been decided by the timeline grouper.

use super::{
    grouping::{admin_event_action, admin_event_text_value},
    *,
};

/// Projects one grouped row into the dashboard response.
pub(super) fn admin_bundle_json(bundle: AdminEventBundle<'_>) -> Value {
    let first = bundle
        .events
        .first()
        .expect("admin event bundles are constructed with one event");
    let last = bundle
        .events
        .last()
        .expect("admin event bundles are constructed with one event");
    let steps = if bundle.events.len() > 1 {
        bundle
            .events
            .iter()
            .map(|event| readable_admin_event_step(event))
            .collect::<Vec<_>>()
            .join(" -> ")
    } else {
        String::new()
    };
    let problem_reason = admin_bundle_problem_reason(&bundle);
    let problem = problem_reason.is_some();
    let action = if bundle.kind == "action" {
        bundle
            .events
            .iter()
            .find_map(|event| admin_event_action(event))
    } else {
        None
    };
    json!({
        "kind": bundle.kind,
        "key": bundle.key,
        "role": bundle.role,
        "first_index": first.event_index,
        "last_index": last.event_index,
        "game_time_ms": last.game_time_ms,
        "title": admin_bundle_title(&bundle),
        "problem": problem,
        "problem_reason": problem_reason,
        "housekeeping": admin_bundle_is_housekeeping(&bundle),
        "steps": steps,
        "text": admin_bundle_text(&bundle),
        "origin": last.detail.get("origin"),
        "spoken": bundle.kind == "transcript" || (last.detail["origin"] == "agent"
            && (last.detail["metadata"]["timing_source"] == "tts_alignment"
                || last.detail["metadata"]["speech_synthesized"] == true)),
        "action": action,
        "utterance_timing": last.detail.get("metadata").map(|metadata| json!({"start_ms":metadata.get("start_game_time_ms"),"end_ms":metadata.get("end_game_time_ms")})),
        "tokens": last.detail.get("metadata").and_then(|metadata| metadata.get("tokens")),
        "events": bundle.events,
    })
}

/// Classifies routine rows for the dashboard filter.
fn admin_bundle_is_housekeeping(bundle: &AdminEventBundle<'_>) -> bool {
    matches!(
        bundle.kind.as_str(),
        "participant" | "voice" | "session_created" | "session_completed"
    )
}

/// Explains failures while accounting for normal post-terminal teardown.
fn admin_bundle_problem_reason(bundle: &AdminEventBundle<'_>) -> Option<String> {
    if bundle
        .events
        .iter()
        .any(|event| event.event_type.contains("rejected"))
    {
        return Some(admin_rejection_reason(bundle));
    }
    if let Some(text) = bundle.events.iter().find_map(|event| {
        let text = admin_event_text_value(event).to_ascii_lowercase();
        (event.event_type == "transcription_start_failed"
            || text.contains("fail")
            || text.contains("error")
            || text.contains("reject")
            || (!bundle.after_terminal_event && text.contains("disconnect")))
        .then(|| admin_event_text_value(event))
    }) {
        return Some(text.to_string());
    }
    match bundle.kind.as_str() {
        "action" => (!bundle.after_terminal_event
            && !bundle.events.iter().any(|event| {
                matches!(
                    event.event_type.as_str(),
                    "game_action_accepted" | "game_action_rejected"
                )
            }))
        .then(|| "Action was sent but no accepted/rejected result was logged.".to_string()),
        "voice" => (!bundle.after_terminal_event
            && !bundle.events.iter().any(|event| {
                let text = admin_event_text_value(event).to_ascii_lowercase();
                text.contains("stt_initialized")
                    || text.contains("transcription_stream_started")
                    || text.contains("transcription_ready")
                    || text.contains("ready")
            }))
        .then(|| "Voice setup did not log a ready/started event.".to_string()),
        "participant" => (bundle
            .events
            .iter()
            .any(|event| event.event_type == "participant_disconnected")
            && !bundle.after_terminal_event)
            .then(|| "Participant disconnected.".to_string()),
        _ => None,
    }
}

/// Selects the available explanation from rejected action records.
fn admin_rejection_reason(bundle: &AdminEventBundle<'_>) -> String {
    bundle
        .events
        .iter()
        .find_map(|event| {
            event
                .detail
                .get("error")
                .and_then(Value::as_str)
                .or(event.text.as_deref())
                .map(str::to_string)
        })
        .unwrap_or_else(|| "Action was rejected.".to_string())
}

/// Labels the row according to its category and outcome.
fn admin_bundle_title(bundle: &AdminEventBundle<'_>) -> String {
    match bundle.kind.as_str() {
        "action" => "Action".to_string(),
        "participant" => "Participant".to_string(),
        "voice" => "Voice".to_string(),
        "transcript" => "Voice Message".to_string(),
        "conversation" => "Message".to_string(),
        "session_created" | "session_completed" | "session_abandoned" | "session_expired" => {
            "Session".to_string()
        }
        _ => bundle
            .events
            .first()
            .map(|event| event.title.clone())
            .unwrap_or_else(|| "Event".to_string()),
    }
}

/// Combines the distinct readable messages in the row.
fn admin_bundle_text(bundle: &AdminEventBundle<'_>) -> String {
    if matches!(
        bundle.kind.as_str(),
        "action" | "participant" | "session_created"
    ) {
        return String::new();
    }
    bundle
        .events
        .iter()
        .filter_map(|event| {
            let text = admin_event_text_value(event);
            (!text.is_empty()).then_some(text)
        })
        .fold(Vec::<String>::new(), |mut texts, text| {
            let normalized = normalize_display_text(text);
            if !texts
                .iter()
                .any(|existing| normalize_display_text(existing) == normalized)
            {
                texts.push(text.to_string());
            }
            texts
        })
        .into_iter()
        .rev()
        .take(2)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join(" · ")
}

/// Normalizes text for deduplication in one dashboard row.
fn normalize_display_text(text: &str) -> String {
    text.trim()
        .trim_end_matches(['.', '!', '?'])
        .to_ascii_lowercase()
}

/// Labels an individual step in the expanded dashboard row.
fn readable_admin_event_step(event: &AdminEventSummary) -> String {
    match event.event_type.as_str() {
        "participant_joined" => "Joined".to_string(),
        "participant_connected" => "Connected".to_string(),
        "participant_disconnected" => "Disconnected".to_string(),
        "ready" => "Ready".to_string(),
        "transcript_segment" => "Transcript saved".to_string(),
        "conversation_message"
            if event.detail.get("origin").and_then(Value::as_str) == Some("voice_transcript") =>
        {
            "Transcript displayed".to_string()
        }
        "voice_diagnostic" => admin_event_text_value(event).to_string(),
        _ => event.title.clone(),
    }
}
