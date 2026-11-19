//! Produces per-session operational liveness snapshots.

use super::{
    game_connection_health, session_lifecycle_is_ended, AppState, Arc, Game, ParticipantLiveness,
    PlayerRole, SessionLiveness,
};

/// Adds one candidate timeout and keeps the earliest deadline affecting a session.
fn retain_absolute_deadline(
    current: &mut Option<(chrono::DateTime<chrono::Utc>, String)>,
    timestamp: &str,
    reason: &str,
) {
    let Some(deadline) = chrono::DateTime::parse_from_rfc3339(timestamp)
        .ok()
        .map(|value| value.with_timezone(&chrono::Utc))
    else {
        return;
    };
    let candidate = (deadline, reason.to_string());
    if current
        .as_ref()
        .is_none_or(|(deadline, _)| candidate.0 < *deadline)
    {
        *current = Some(candidate);
    }
}

/// Minimal participant projection copied while taking a liveness snapshot.
struct ParticipantOperationalSnapshot {
    role: PlayerRole,
    source: String,
    connected: bool,
    audio_ready: bool,
    reconnect_deadline_at: Option<String>,
}

/// Minimal session projection that intentionally excludes potentially large game state.
struct SessionOperationalSnapshot {
    session_id: i64,
    public_session_id: String,
    lifecycle: String,
    paused: bool,
    updated_at: String,
    waiting_deadline_at: String,
    lifetime_deadline_at: String,
    idle_deadline_at: Option<String>,
    participants: Vec<ParticipantOperationalSnapshot>,
}

/// Builds participant and lifecycle liveness without writing heartbeat data to storage.
pub(super) async fn build_session_liveness<A: Game>(
    state: &Arc<AppState<A>>,
) -> Vec<SessionLiveness> {
    let sessions = {
        let memory = state.memory.read().await;
        memory
            .sessions
            .values()
            .map(|session| SessionOperationalSnapshot {
                session_id: session.session_id,
                public_session_id: session.id.clone(),
                lifecycle: session.lifecycle().as_str().to_string(),
                paused: session.pause().is_some(),
                updated_at: session.timing().updated_at.clone(),
                waiting_deadline_at: session.timing().waiting_deadline_at.clone(),
                lifetime_deadline_at: session.timing().lifetime_deadline_at.clone(),
                idle_deadline_at: session.timing().idle_deadline_at.clone(),
                participants: session
                    .participants
                    .values()
                    .map(|participant| ParticipantOperationalSnapshot {
                        role: participant.role,
                        source: participant.source.clone(),
                        connected: participant.connected,
                        audio_ready: participant.audio_ready(),
                        reconnect_deadline_at: participant.reconnect_deadline_at.clone(),
                    })
                    .collect(),
            })
            .collect::<Vec<_>>()
    };
    let game = state.game_connections.snapshots().await;
    let audio = state.audio_connections.snapshots().await;
    let now_ms = chrono::Utc::now().timestamp_millis();
    let mut sessions = sessions
        .into_iter()
        .map(|session| {
            let mut deadline = None;
            if session.lifecycle == "forming" {
                retain_absolute_deadline(
                    &mut deadline,
                    &session.waiting_deadline_at,
                    "waiting timeout",
                );
            } else if session.lifecycle == "running" {
                if let Some(value) = session.idle_deadline_at.as_deref() {
                    retain_absolute_deadline(&mut deadline, value, "idle timeout");
                }
            }
            if !session_lifecycle_is_ended(&session.lifecycle) {
                retain_absolute_deadline(
                    &mut deadline,
                    &session.lifetime_deadline_at,
                    "maximum lifetime",
                );
            }
            if !session_lifecycle_is_ended(&session.lifecycle)
                && session.participants.iter().any(|row| !row.connected)
            {
                if let Some(latest) = session
                    .participants
                    .iter()
                    .filter(|row| !row.connected)
                    .min_by(|left, right| {
                        left.reconnect_deadline_at.cmp(&right.reconnect_deadline_at)
                    })
                {
                    if let Some(value) = latest.reconnect_deadline_at.as_deref() {
                        retain_absolute_deadline(&mut deadline, value, "reconnect timeout");
                    }
                }
            }
            let participants = session
                .participants
                .iter()
                .map(|participant| {
                    let key = format!(
                        "{}:{}",
                        session.public_session_id,
                        participant.role.as_str()
                    );
                    let game_transport = game.get(&key).cloned();
                    let game_health = if participant.source == "agent" {
                        "server"
                    } else {
                        game_transport
                            .as_ref()
                            .map(|snapshot| game_connection_health(snapshot, now_ms))
                            .unwrap_or(if session.lifecycle == "forming" {
                                "waiting"
                            } else {
                                "disconnected"
                            })
                    };
                    ParticipantLiveness {
                        role: participant.role.as_str().to_string(),
                        source: participant.source.clone(),
                        participant_state: if session.lifecycle == "forming" {
                            "waiting"
                        } else if session.lifecycle == "ended" {
                            "ended"
                        } else if session.paused {
                            "paused"
                        } else {
                            "active"
                        }
                        .to_string(),
                        game_health: game_health.to_string(),
                        game: game_transport,
                        audio: audio.get(&key).cloned(),
                        audio_ready: participant.audio_ready,
                    }
                })
                .collect::<Vec<_>>();
            let browser_participants = participants
                .iter()
                .filter(|participant| participant.source != "agent")
                .collect::<Vec<_>>();
            let health = if session_lifecycle_is_ended(&session.lifecycle) {
                "ended"
            } else if browser_participants
                .iter()
                .any(|participant| participant.game_health == "stale")
            {
                "stale"
            } else if browser_participants
                .iter()
                .any(|participant| participant.game_health == "delayed")
            {
                "delayed"
            } else if session.lifecycle == "running"
                && browser_participants
                    .iter()
                    .any(|participant| participant.game_health == "disconnected")
            {
                "disconnected"
            } else if browser_participants
                .iter()
                .any(|participant| participant.game_health == "live")
            {
                "live"
            } else if session.lifecycle == "forming" {
                "waiting"
            } else {
                "disconnected"
            };
            SessionLiveness {
                experiment_id: state.experiment_id.clone(),
                session_id: session.session_id,
                public_session_id: session.public_session_id,
                lifecycle: session.lifecycle,
                health: health.to_string(),
                meaningful_activity_at: session.updated_at,
                lifecycle_deadline_at: deadline.as_ref().map(|(value, _)| value.to_rfc3339()),
                deadline_reason: deadline.map(|(_, reason)| reason),
                participants,
            }
        })
        .collect::<Vec<_>>();
    sessions.sort_by_key(|session| session.session_id);
    sessions
}
