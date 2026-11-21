//! Projection of a durable experiment snapshot into the versioned research corpus.
//!
//! The admin export handler supplies a consistent SQLite snapshot. The projector selects
//! eligible research sessions, preserves game-owned payloads and combines message records
//! with their speech timings. The versioned schema describes the output consumed by
//! research tools and cited by privacy reporting.
use crate::protocol::SessionEndCause;
use crate::storage::{ExperimentSnapshot, ExportSession, StoredSessionEvent};
use anyhow::Error;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

pub(super) const CORPUS_EXPORT_SCHEMA_V1: &str = include_str!("corpus-export-schema-v1.json");

/// Restricts every linked table to the selected research sessions; corpus v1 also requires gameplay.
pub(super) fn retain_export_sessions(snapshot: &mut ExperimentSnapshot, started_only: bool) {
    snapshot.sessions.retain(|session| {
        session.purpose != "testing" && (!started_only || session.started_at.is_some())
    });
    let session_ids: HashSet<_> = snapshot
        .sessions
        .iter()
        .map(|session| session.session_id)
        .collect();
    snapshot
        .session_events
        .retain(|event| session_ids.contains(&event.session_id));
    snapshot.session_participants.retain(|row| {
        row["session_id"]
            .as_i64()
            .is_some_and(|id| session_ids.contains(&id))
    });
    let participant_ids: HashSet<_> = snapshot
        .session_participants
        .iter()
        .filter_map(|row| row["participant_id"].as_i64())
        .collect();
    snapshot.participants.retain(|row| {
        row["participant_id"]
            .as_i64()
            .is_some_and(|id| participant_ids.contains(&id))
    });
    snapshot.consent_declarations.retain(|row| {
        row["purpose"] != "testing"
            && match row["session_id"].as_i64() {
                Some(id) => session_ids.contains(&id),
                None => row["participant_id"]
                    .as_i64()
                    .is_some_and(|id| participant_ids.contains(&id)),
            }
    });
}

/// Validates and projects only the documented token fields into a corpus candidate.
pub(super) fn export_transcript_tokens(value: &Value) -> Result<Value, Error> {
    let items = value
        .as_array()
        .ok_or_else(|| export_integrity_error("transcript tokens are not an array"))?;
    let mut tokens = Vec::with_capacity(items.len());
    for item in items {
        let kind = item["kind"]
            .as_str()
            .filter(|kind| matches!(*kind, "word" | "punctuation"))
            .ok_or_else(|| export_integrity_error("invalid transcript token kind"))?;
        let text = item["text"]
            .as_str()
            .ok_or_else(|| export_integrity_error("missing transcript token text"))?;
        let start = item["start_ms"]
            .as_i64()
            .filter(|value| *value >= 0)
            .ok_or_else(|| export_integrity_error("invalid token onset"))?;
        let end = item["end_ms"]
            .as_i64()
            .filter(|value| *value >= start)
            .ok_or_else(|| export_integrity_error("invalid token endpoint"))?;
        let mut token = json!({"kind":kind,"text":text,"start_ms":start,"end_ms":end});
        if let Some(confidence) = item.get("confidence") {
            let confidence = confidence
                .as_f64()
                .filter(|value| (0.0..=1.0).contains(value))
                .ok_or_else(|| export_integrity_error("invalid token confidence"))?;
            token["confidence"] = json!(confidence);
        }
        tokens.push(token);
    }
    Ok(json!(tokens))
}

/// Indexes published speech timing records by their existing conversation message.
pub(super) fn collect_speech_timings<'a>(
    payloads: impl Iterator<Item = &'a Value>,
) -> HashMap<String, Value> {
    payloads
        .filter_map(|payload| {
            Some((
                payload.get("message_id")?.as_str()?.to_string(),
                payload.get("metadata")?.clone(),
            ))
        })
        .collect()
}

/// Enriches a read projection without altering the originally committed message event.
pub(super) fn attach_speech_timing(payload: &mut Value, timings: &HashMap<String, Value>) {
    let Some(timing) = payload
        .get("id")
        .and_then(Value::as_str)
        .and_then(|id| timings.get(id))
    else {
        return;
    };
    let Some(fields) = timing.as_object() else {
        return;
    };
    if !payload["metadata"].is_object() {
        payload["metadata"] = json!({});
    }
    payload["metadata"]
        .as_object_mut()
        .expect("metadata is an object")
        .extend(fields.clone());
}

/// Derives a publication-oriented dialogue corpus with consistent readable identifiers.
pub(super) fn corpus_experiment_export(
    mut snapshot: ExperimentSnapshot,
    privacy_contract_version: &str,
) -> Result<Value, Error> {
    retain_export_sessions(&mut snapshot, true);
    let experiment = snapshot
        .experiment
        .as_ref()
        .ok_or_else(|| export_integrity_error("experiment metadata is missing"))?;
    let participant_rows = &snapshot.participants;
    let mut participant_ids = HashMap::<i64, String>::new();
    let participants = participant_rows
        .iter()
        .map(|row| {
            let internal_id = row
                .get("participant_id")
                .and_then(Value::as_i64)
                .ok_or_else(|| export_integrity_error("a participant lacks its database key"))?;
            let public_id = row
                .get("research_id")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| export_integrity_error("a participant lacks its dashboard ID"))?
                .to_string();
            participant_ids.insert(internal_id, public_id.clone());
            let kind = row.get("participant_kind").cloned().unwrap_or(Value::Null);
            let agent_identity = (kind.as_str() == Some("agent")).then(|| {
                let metadata = row.get("metadata").unwrap_or(&Value::Null);
                json!({
                    "factory_id": metadata.get("factory"),
                    "agent_name": metadata.get("agent_name"),
                    "agent_version": metadata.get("agent_version"),
                    "configuration_fingerprint": metadata.get("configuration_fingerprint"),
                })
            });
            Ok(json!({
                "participant_id": public_id,
                "kind": kind,
                "agent_identity": agent_identity,
            }))
        })
        .collect::<Result<Vec<_>, Error>>()?;

    let mut memberships = HashMap::<i64, Vec<&Value>>::new();
    for row in &snapshot.session_participants {
        let id = row["session_id"]
            .as_i64()
            .ok_or_else(|| export_integrity_error("a session assignment lacks its session key"))?;
        memberships.entry(id).or_default().push(row);
    }
    let mut events = HashMap::<i64, Vec<_>>::new();
    for event in &snapshot.session_events {
        events.entry(event.session_id).or_default().push(event);
    }
    let mut counts = CorpusEventCounts::default();
    let mut sessions = Vec::with_capacity(snapshot.sessions.len());
    for session in &snapshot.sessions {
        if session.dialogue_id.is_empty() {
            return Err(export_integrity_error("a session lacks its dashboard ID"));
        }
        let assignments = corpus_assignments(
            &session.dialogue_id,
            memberships.remove(&session.session_id).unwrap_or_default(),
            &participant_ids,
        )?;
        let session_events = corpus_events(
            &session.dialogue_id,
            events.remove(&session.session_id).unwrap_or_default(),
            &participant_ids,
            &mut counts,
        )?;
        sessions.push(json!({
            "session_id": session.dialogue_id,
            "metadata": corpus_session_metadata(session)?,
            "participants": assignments,
            "completion": session.completion,
            "events": session_events,
        }));
    }
    let config = experiment.get("config").unwrap_or(&Value::Null);
    Ok(json!({
        "export_schema_version": "parlando.corpus.v1",
        "export_variant": "corpus",
        "release_status": "corpus_candidate",
        "content_review_required": true,
        "privacy_contract_version": privacy_contract_version,
        "data_inventory": {
            "participants": participants.len(),
            "sessions": sessions.len(),
            "events": counts.actions + counts.messages + counts.logs,
            "actions": counts.actions,
            "messages": counts.messages,
            "logs": counts.logs,
        },
        "experiment": {
            "experiment_id": experiment.get("experiment_id"),
            "game": {
                "version": experiment.get("game_version"),
            },
            "configuration": config.get("game").cloned().unwrap_or_else(|| json!({})),
            "participants": participants,
            "sessions": sessions,
        },
    }))
}

/// Counts only records included in the public corpus.
#[derive(Default)]
struct CorpusEventCounts {
    actions: usize,
    messages: usize,
    logs: usize,
}

/// Resolves the two assignments to pseudonymous participant identifiers.
fn corpus_assignments(
    session_id: &str,
    rows: Vec<&Value>,
    participant_ids: &HashMap<i64, String>,
) -> Result<Vec<Value>, Error> {
    let assignments = rows
        .into_iter()
        .map(|row| {
            let participant = row
                .get("participant_id")
                .and_then(Value::as_i64)
                .and_then(|id| participant_ids.get(&id))
                .ok_or_else(|| {
                    export_integrity_error("a session assignment has no exported participant")
                })?;
            Ok(json!({
                "participant_id": participant,
                "role": row.get("role"),
            }))
        })
        .collect::<Result<Vec<_>, Error>>()?;
    if assignments.len() != 2 {
        return Err(export_integrity_error(format!(
            "session {session_id} has {} participant assignments; expected exactly two",
            assignments.len()
        )));
    }
    Ok(assignments)
}

/// Derives the existing v1 status and intervals from the authoritative lifecycle record.
fn corpus_session_metadata(session: &ExportSession) -> Result<Value, Error> {
    let session_id = &session.dialogue_id;
    let started_at = required_timestamp_ms(session.started_at.as_deref(), "session start")?;
    let created_at = required_timestamp_ms(Some(&session.created_at), "session creation")?;
    let ended_at = session
        .ended_at
        .as_deref()
        .map(|value| required_timestamp_ms(Some(value), "session end"))
        .transpose()?;
    let status = match (session.lifecycle.as_str(), &session.session_end, ended_at) {
        ("running", None, None) => "playing",
        ("ended", Some(end), Some(_)) => match end.cause {
            SessionEndCause::GameCompleted => "completed",
            SessionEndCause::ParticipantLeft { .. } => "abandoned",
            SessionEndCause::ReconnectTimedOut { .. }
            | SessionEndCause::IdleTimedOut
            | SessionEndCause::LifetimeTimedOut
            | SessionEndCause::PartnerUnavailable => "expired",
            SessionEndCause::TechnicalFailure => "failed",
        },
        _ => {
            return Err(export_integrity_error(format!(
                "session {session_id} has inconsistent lifecycle or terminal data"
            )))
        }
    };
    let time_to_start_ms = checked_interval(started_at, created_at, session_id, "time to start")?;
    let duration_ms = ended_at
        .map(|end| checked_interval(end, started_at, session_id, "duration"))
        .transpose()?;
    Ok(json!({
        "game_version": session.game_version,
        "config_revision": session.config_revision,
        "mode": session.mode,
        "status": status,
        "time_to_start_ms": time_to_start_ms,
        "duration_ms": duration_ms,
    }))
}

/// Projects the session's chronological semantic events and merges speech alignment by message id.
fn corpus_events(
    session_id: &str,
    event_rows: Vec<&StoredSessionEvent>,
    participant_ids: &HashMap<i64, String>,
    counts: &mut CorpusEventCounts,
) -> Result<Vec<Value>, Error> {
    let speech_timings = collect_speech_timings(
        event_rows
            .iter()
            .filter(|row| row.event_type == "conversation_speech_timing")
            .map(|row| &row.payload),
    );
    event_rows
        .into_iter()
        .filter(|row| {
            matches!(
                row.event_type.as_str(),
                "game_action_accepted" | "conversation_message" | "log"
            )
        })
        // The v1 corpus clock starts at gameplay. Setup logs remain available in the dashboard.
        .filter(|row| row.event_type != "log" || row.game_time_ms >= 0)
        .map(|row| {
            let game_time_ms = row.game_time_ms;
            if game_time_ms < 0 {
                return Err(export_integrity_error(format!(
                    "session {session_id} contains a semantic event without valid game time"
                )));
            }
            let index = row.event_index;
            let participant = row
                .actor_participant_id
                .and_then(|id| participant_ids.get(&id));
            let mut payload = row.payload.clone();
            if row.event_type == "conversation_message" {
                attach_speech_timing(&mut payload, &speech_timings);
            }

            match row.event_type.as_str() {
                "game_action_accepted" => {
                    let participant = participant.ok_or_else(|| {
                        export_integrity_error("an action event lacks an exported actor")
                    })?;
                    counts.actions += 1;
                    Ok(json!({
                        "index": index,
                        "game_time_ms": game_time_ms,
                        "kind": "action",
                        "participant_id": participant,
                        "role": row.actor_role,
                        "action": payload.get("action"),
                        "transition_metadata": payload.get("metadata"),
                        // Guest recovery frames can contain private data for both roles.
                        "state": if payload["recovery_checkpoint"] == true { None } else { row.game_state.as_ref() },
                    }))
                }
                "conversation_message" => {
                    let participant = participant.ok_or_else(|| {
                        export_integrity_error("a message event lacks an exported actor")
                    })?;
                    counts.messages += 1;
                    let mut event = json!({
                        "index": index,
                        "game_time_ms": game_time_ms,
                        "kind": "message",
                        "participant_id": participant,
                        "role": row.actor_role,
                        "origin": payload.get("origin"),
                        "text": payload.get("text"),
                    });
                    let start_time = payload
                        .get("metadata")
                        .and_then(|value| value.get("start_game_time_ms"));
                    let end_time = payload
                        .get("metadata")
                        .and_then(|value| value.get("end_game_time_ms"));
                    if start_time.is_some() || end_time.is_some() {
                        event["utterance_timing"] = json!({
                            "origin": "game_clock",
                            "start_ms": start_time,
                            "end_ms": end_time,
                        });
                    }
                    if let Some(tokens) = payload
                        .get("metadata")
                        .and_then(|metadata| metadata.get("tokens"))
                    {
                        event["tokens"] = export_transcript_tokens(tokens)?;
                    }
                    Ok(event)
                }
                "log" => {
                    counts.logs += 1;
                    Ok(json!({
                        "index": index,
                        "game_time_ms": game_time_ms,
                        "kind": "log",
                        "source": payload.get("source"),
                        "participant_id": participant,
                        "role": row.actor_role,
                        "text": payload.get("text"),
                    }))
                }
                _ => unreachable!(),
            }
        })
        .collect::<Result<Vec<_>, Error>>()
}

/// Explains why a storage snapshot violates the publication contract.
fn export_integrity_error(message: impl Into<String>) -> Error {
    anyhow::anyhow!("Corpus export integrity error: {}", message.into())
}

/// Parses a required RFC3339 timestamp used only to derive a relative interval.
fn required_timestamp_ms(value: Option<&str>, label: &str) -> Result<i64, Error> {
    value
        .and_then(rfc3339_millis)
        .ok_or_else(|| export_integrity_error(format!("{label} timestamp is missing or invalid")))
}

/// Computes a checked non-negative session interval.
fn checked_interval(end: i64, start: i64, session_id: &str, label: &str) -> Result<i64, Error> {
    end.checked_sub(start)
        .filter(|value| *value >= 0)
        .ok_or_else(|| {
            export_integrity_error(format!("session {session_id} has a negative {label}"))
        })
}

/// Parses an RFC3339 timestamp into milliseconds for export-relative timing.
fn rfc3339_millis(value: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|timestamp| timestamp.timestamp_millis())
}

/// Serializes all tables in an export variant into a two-column CSV stream.
pub(super) fn export_csv(exported: &Value) -> String {
    let mut output = String::from("table,row_json\n");
    if exported.get("export_schema_version").is_some() {
        push_csv_row(
            &mut output,
            "manifest",
            &json!({
                "export_schema_version": exported.get("export_schema_version"),
                "export_variant": exported.get("export_variant"),
                "release_status": exported.get("release_status"),
                "content_review_required": exported.get("content_review_required"),
                "privacy_contract_version": exported.get("privacy_contract_version"),
                "data_inventory": exported.get("data_inventory"),
            }),
        );
        if let Some(experiment) = exported.get("experiment") {
            let mut metadata = experiment.clone();
            let sessions = metadata
                .as_object_mut()
                .and_then(|object| object.remove("sessions"))
                .unwrap_or_else(|| json!([]));
            push_csv_row(&mut output, "experiment", &metadata);
            if let Some(rows) = sessions.as_array() {
                for row in rows {
                    push_csv_row(&mut output, "session", row);
                }
            }
        }
        return output;
    }
    for table in [
        "experiment",
        "participants",
        "sessions",
        "session_participants",
        "consent_declarations",
        "session_events",
        "events",
        "messages",
    ] {
        let Some(value) = exported.get(table) else {
            continue;
        };
        if let Some(rows) = value.as_array() {
            for row in rows {
                push_csv_row(&mut output, table, row);
            }
        } else if !value.is_null() {
            push_csv_row(&mut output, table, value);
        }
    }
    output
}

/// Appends one CSV row with the JSON value encoded as a single column.
fn push_csv_row(output: &mut String, table: &str, row: &Value) {
    output.push_str(&csv_escape(table));
    output.push(',');
    output.push_str(&csv_escape(
        &serde_json::to_string(row).unwrap_or_else(|_| "null".to_string()),
    ));
    output.push('\n');
}

/// Quotes CSV values containing delimiters, newlines, or quote characters.
fn csv_escape(value: &str) -> String {
    if value
        .chars()
        .any(|ch| matches!(ch, ',' | '"' | '\n' | '\r'))
    {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}
