//! Projection of durable session events into dashboard rows.
use super::corpus::{attach_speech_timing, collect_speech_timings};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
mod display;
mod grouping;
use display::*;
pub(super) use grouping::admin_event_bundles;
/// Builds a compact timeline from durable events that matter during monitoring.
pub(super) fn important_admin_events(
    events: Vec<crate::storage::StoredSessionEvent>,
) -> Vec<AdminEventSummary> {
    let timings = collect_speech_timings(
        events
            .iter()
            .filter(|event| event.event_type == "conversation_speech_timing")
            .map(|event| &event.payload),
    );
    let published_messages = events
        .iter()
        .filter(|event| event.event_type == "tts_diagnostic")
        .filter(|event| {
            event.payload["event"] == "tts_publish_completed"
                && event.payload["metadata"]["bytes"]
                    .as_u64()
                    .is_some_and(|bytes| bytes > 0)
        })
        .filter_map(|event| event.payload["metadata"]["message_id"].as_str())
        .map(str::to_owned)
        .collect::<HashSet<_>>();
    events
        .into_iter()
        .filter(is_important_admin_event)
        .map(|mut event| {
            if event.event_type == "conversation_message" {
                attach_speech_timing(&mut event.payload, &timings);
                if event.payload["id"]
                    .as_str()
                    .is_some_and(|id| published_messages.contains(id))
                {
                    if !event.payload["metadata"].is_object() {
                        event.payload["metadata"] = json!({});
                    }
                    event.payload["metadata"]["speech_synthesized"] = json!(true);
                }
            }
            admin_event_summary(event)
        })
        .collect()
}

/// Returns whether a stored event should appear in the admin session monitor.
fn is_important_admin_event(event: &crate::storage::StoredSessionEvent) -> bool {
    matches!(
        event.event_type.as_str(),
        "agent_action"
            | "conversation_message"
            | "log"
            | "game_action_accepted"
            | "game_action_rejected"
            | "game_action_submitted"
            | "participant_connected"
            | "participant_disconnected"
            | "participant_joined"
            | "ready"
            | "session_abandoned"
            | "session_completed"
            | "session_created"
            | "session_expired"
            | "transcript_segment"
            | "voice_diagnostic"
    )
}

/// Typed dashboard projection of one durable event.
#[derive(Clone, Debug, Serialize)]
pub(super) struct AdminEventSummary {
    event_id: i64,
    event_index: i64,
    event_type: String,
    actor_participant_id: Option<i64>,
    actor_role: Option<String>,
    game_time_ms: i64,
    title: String,
    text: Option<String>,
    detail: Value,
    raw: Value,
}

/// Builds the typed dashboard projection for one durable event.
fn admin_event_summary(event: crate::storage::StoredSessionEvent) -> AdminEventSummary {
    let payload = event.payload.clone();
    let title = admin_event_title(&event.event_type, &payload);
    let text = admin_event_text(&event.event_type, &payload);
    let detail = admin_event_detail(&event.event_type, &payload);
    let raw = json!({
        "event_id": event.event_id,
        "experiment_id": event.experiment_id,
        "session_id": event.session_id,
        "event_index": event.event_index,
        "event_type": event.event_type,
        "actor_participant_id": event.actor_participant_id,
        "actor_role": event.actor_role,
        "payload": payload,
        "game_state": event.game_state,
        "game_time_ms": event.game_time_ms,
    });
    AdminEventSummary {
        event_id: event.event_id,
        event_index: event.event_index,
        event_type: event.event_type,
        actor_participant_id: event.actor_participant_id,
        actor_role: event.actor_role,
        game_time_ms: event.game_time_ms,
        title: title.to_string(),
        text,
        detail,
        raw,
    }
}

/// Produces a human-readable title for one admin timeline event.
fn admin_event_title(event_type: &str, payload: &Value) -> &'static str {
    match event_type {
        "agent_action" => "Agent action",
        "conversation_message" => match payload.get("origin").and_then(Value::as_str) {
            Some("voice_transcript") => "Transcription result",
            Some("agent") => "Agent message",
            _ => "Conversation message",
        },
        "log" => match payload.get("source").and_then(Value::as_str) {
            Some("agent") => "Agent log",
            _ => "Game log",
        },
        "game_action_accepted" => "Action accepted",
        "game_action_rejected" => "Action rejected",
        "game_action_submitted" => "Action submitted",
        "participant_connected" => "Participant connected",
        "participant_disconnected" => "Participant disconnected",
        "participant_joined" => "Participant joined",
        "ready" => "Participant ready",
        "session_abandoned" => "Session abandoned",
        "session_completed" => "Session completed",
        "session_created" => "Session created",
        "session_expired" => "Session expired",
        "transcript_segment" => "Transcript segment",
        "voice_diagnostic" => "Voice diagnostic",
        _ => "Event",
    }
}

/// Extracts the primary readable text from one admin timeline event.
fn admin_event_text(event_type: &str, payload: &Value) -> Option<String> {
    match event_type {
        "conversation_message" | "log" | "transcript_segment" => payload
            .get("text")
            .and_then(Value::as_str)
            .map(str::to_string),
        "game_action_accepted" | "game_action_submitted" | "agent_action" => payload
            .get("action")
            .map(compact_json)
            .or_else(|| Some(compact_json(payload))),
        "game_action_rejected" => payload
            .get("error")
            .and_then(Value::as_str)
            .map(str::to_string),
        "session_abandoned" | "session_expired" => payload
            .get("reason")
            .and_then(Value::as_str)
            .map(str::to_string),
        _ => None,
    }
}

/// Keeps compact structured details for one admin timeline event.
fn admin_event_detail(event_type: &str, payload: &Value) -> Value {
    match event_type {
        "conversation_message" => json!({
            "origin": payload.get("origin"),
            "sender_role": payload.get("sender_role"),
            "metadata": payload.get("metadata"),
        }),
        "log" => json!({
            "source": payload.get("source"),
        }),
        "transcript_segment" => json!({
            "player": payload.get("player"),
            "start_game_time_ms": payload.get("start_game_time_ms"),
            "end_game_time_ms": payload.get("end_game_time_ms"),
            "metadata": payload.get("metadata"),
        }),
        "game_action_accepted" => json!({
            "action": payload.get("action"),
            "events": payload.get("events"),
        }),
        "game_action_submitted" | "agent_action" => json!({
            "action": payload.get("action"),
        }),
        _ => payload.clone(),
    }
}

/// Serializes JSON into a stable one-line string for timeline labels.
fn compact_json(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "null".to_string())
}

#[derive(Clone, Debug)]
struct AdminEventBundle<'a> {
    kind: String,
    role: Option<&'a str>,
    key: String,
    events: Vec<&'a AdminEventSummary>,
    after_terminal_event: bool,
}

#[cfg(test)]
mod tests;
