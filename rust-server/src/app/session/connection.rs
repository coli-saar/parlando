//! Connection, reconnect-grace and pause transitions for a live session.
//!
//! The game socket applies these membership changes under the session transition lock.
//! Reconnect clears the returning seat's disconnect timing. Disconnect records the grace
//! deadline, which the deadline policy can use to terminate the room. Participant
//! projections expose the resulting pause.
use super::{LiveSession, SessionLifecycle};
use crate::{protocol::ParticipantPauseReason, storage::now_iso};

impl LiveSession {
    /// Clears the seat's reconnect deadline and resumes when all seats are connected.
    pub(in crate::app) fn reconnect(&mut self, participant_session_id: &str) {
        if let Some(participant) = self.participants.get_mut(participant_session_id) {
            participant.connected = true;
            participant.updated_at = now_iso();
            participant.disconnected_at = None;
            participant.reconnect_deadline_at = None;
        }
        if self
            .participants
            .values()
            .all(|participant| participant.connected)
        {
            self.pause = None;
        }
    }

    /// Records the disconnection before the transport publishes offline presence.
    pub(in crate::app) fn disconnect(
        &mut self,
        participant_session_id: &str,
        disconnected_at: &str,
        reconnect_deadline_at: &str,
    ) {
        if let Some(participant) = self.participants.get_mut(participant_session_id) {
            participant.connected = false;
            participant.updated_at = disconnected_at.to_owned();
            participant.disconnected_at = Some(disconnected_at.to_owned());
            participant.reconnect_deadline_at = Some(reconnect_deadline_at.to_owned());
        }
    }

    /// Suspends an active game after offline presence has been published.
    pub(in crate::app) fn pause_for_reconnect(&mut self, reconnect_deadline_at: &str) {
        if self.lifecycle == SessionLifecycle::Running {
            self.pause = Some(ParticipantPauseReason::PartnerReconnecting {
                deadline_at: reconnect_deadline_at.to_owned(),
            });
        }
    }
}
