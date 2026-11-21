//! Deadline precedence and retention policy for live and terminal sessions.
//!
//! Input handlers and the maintenance sweep consult session timing with the disconnected
//! seats' reconnect deadlines. The sweep also uses the cleanup decision to close farewell
//! audio or release an ended room once its retained human connections and grace periods
//! permit eviction.

use super::LiveSession;

/// Work due for one room during a lifecycle sweep.
pub(super) enum RoomCleanup {
    /// End a forming or running room with its elapsed deadline.
    Expire(&'static str),
    /// Close farewell audio while retaining any remaining game transports.
    CloseFarewell,
    /// Release a terminal room after every human's retention period.
    Evict,
}

/// Determines whether an active room has reached an authoritative deadline.
pub(super) fn due_live_deadline(
    session: &LiveSession,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<&'static str> {
    let reconnect_deadlines = session
        .participants
        .values()
        .filter(|participant| !participant.connected)
        .filter_map(|participant| participant.reconnect_deadline_at.as_deref());
    session
        .timing()
        .due_deadline(session.lifecycle(), reconnect_deadlines, now)
}

/// Keeps terminal rooms for farewell audio, connected browsers, and each human's grace.
/// A human who never opened a socket starts their retention period at the terminal commit.
pub(super) fn room_cleanup(
    session: &LiveSession,
    now: chrono::DateTime<chrono::Utc>,
    grace: chrono::Duration,
) -> Option<RoomCleanup> {
    if !session.lifecycle().is_ended() {
        return due_live_deadline(session, now).map(RoomCleanup::Expire);
    }
    if let Some(end) = session.timing().voice_ends_at.as_deref() {
        return (chrono::DateTime::parse_from_rfc3339(end).ok()? <= now)
            .then_some(RoomCleanup::CloseFarewell);
    }
    let ended_at = chrono::DateTime::parse_from_rfc3339(&session.timing().updated_at).ok()?;
    let released = session
        .participants
        .values()
        .filter(|p| p.source != "agent")
        .all(|p| {
            let until = p
                .reconnect_deadline_at
                .as_deref()
                .and_then(|stamp| chrono::DateTime::parse_from_rfc3339(stamp).ok())
                .unwrap_or(ended_at + grace);
            !p.connected && now >= until
        });
    released.then_some(RoomCleanup::Evict)
}
