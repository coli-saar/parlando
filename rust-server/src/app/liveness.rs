//! Produces per-session operational liveness snapshots.
//!
//! The load endpoint combines a snapshot of room and participant metadata with the
//! transport registries' liveness readings. It uses the session timing policy to show the
//! next scheduled deadline. Expiration and cleanup remain decisions for the transition
//! and maintenance units.

use super::{
    game_connection_health, session_lifecycle_is_ended, AppState, Arc, ParticipantLiveness,
    PlayerRole, SessionLiveness,
};

/// Minimal participant projection copied while taking a liveness snapshot.
struct ParticipantOperationalSnapshot {
    role: PlayerRole,
    source: String,
    audio_ready: bool,
}

/// Minimal session projection that intentionally excludes potentially large game state.
struct SessionOperationalSnapshot {
    session_id: i64,
    public_session_id: String,
    lifecycle: String,
    paused: bool,
    updated_at: String,
    deadline: Option<(chrono::DateTime<chrono::Utc>, &'static str)>,
    participants: Vec<ParticipantOperationalSnapshot>,
}

/// Builds participant and lifecycle liveness without writing heartbeat data to storage.
pub(super) async fn build_session_liveness(state: &Arc<AppState>) -> Vec<SessionLiveness> {
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
                deadline: session.timing().next_deadline(
                    session.lifecycle(),
                    session
                        .participants
                        .values()
                        .filter(|seat| !seat.connected)
                        .filter_map(|seat| seat.reconnect_deadline_at.as_deref()),
                ),
                participants: session
                    .participants
                    .values()
                    .map(|participant| ParticipantOperationalSnapshot {
                        role: participant.role,
                        source: participant.source.clone(),
                        audio_ready: participant.audio_ready(),
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
                lifecycle_deadline_at: session
                    .deadline
                    .as_ref()
                    .map(|(value, _)| value.to_rfc3339()),
                deadline_reason: session.deadline.map(|(_, reason)| reason.replace('_', " ")),
                participants,
            }
        })
        .collect::<Vec<_>>();
    sessions.sort_by_key(|session| session.session_id);
    sessions
}
