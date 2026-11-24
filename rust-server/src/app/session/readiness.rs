//! Seat-level declarations and the policy for activating both player roles.
//!
//! The activation coordinator and action preparation share the same role checks. Both
//! roles must be connected. Software seats also declare startup complete, and
//! transcription-gated browser seats complete audio setup. The worker seat has no
//! microphone requirement.

use super::*;

impl SessionParticipant {
    /// Records the first ready declaration for the seat.
    pub(in crate::app) fn declare_ready(&mut self) -> bool {
        if self.ready {
            return false;
        }
        self.ready = true;
        self.updated_at = now_iso();
        true
    }

    /// Allows a declaration to be retried after a failed durable write.
    pub(in crate::app) fn rollback_ready_declaration(&mut self) {
        self.ready = false;
    }

    /// Records completed audio initialization, returning whether this is the first notification.
    pub(in crate::app) fn mark_audio_ready(&mut self) -> bool {
        let changed = !self.audio_ready;
        self.audio_ready = true;
        self.updated_at = now_iso();
        changed
    }

    /// Clears the acknowledgement when a forming seat loses or replaces its audio stream.
    pub(in crate::app) fn clear_audio_ready(&mut self) {
        self.audio_ready = false;
    }

    /// Returns whether the seat has finished audio setup.
    pub(in crate::app) fn audio_ready(&self) -> bool {
        self.audio_ready
    }

    /// A browser participant uses an audio stream during required transcription setup.
    pub(in crate::app) fn uses_browser_audio(&self) -> bool {
        self.source != "agent" && self.source != "worker"
    }

    /// Every seat must be connected. Agents also require their ready declaration.
    /// Browser participants require audio readiness when transcription gates startup.
    /// A worker seat has no browser microphone to initialize.
    pub(in crate::app) fn ready_for_game(&self, transcription_required: bool) -> bool {
        self.connected
            && (self.source != "agent" || self.ready)
            && (!self.uses_browser_audio() || !transcription_required || self.audio_ready)
    }
}

/// Returns whether session start must wait for Speechmatics setup to finish.
pub(in crate::app) fn speechmatics_readiness_required(config: &ExperimentConfig) -> bool {
    config.transcription_active() && config.transcription.provider == "speechmatics"
}

/// Returns whether both player roles are connected and past required audio setup.
pub(in crate::app) fn session_ready_for_game(
    config: &ExperimentConfig,
    session: &LiveSession,
) -> bool {
    let ready_roles = session
        .participants
        .values()
        .filter(|participant| participant.ready_for_game(speechmatics_readiness_required(config)))
        .map(|participant| participant.role)
        .collect::<HashSet<_>>();
    ready_roles == HashSet::from([PlayerRole::A, PlayerRole::B])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A software seat may retry a failed declaration, and disconnect always disables it.
    #[test]
    fn agent_declaration_is_idempotent_and_retryable() {
        let mut seat = SessionParticipant::agent("agent".into(), 1);
        assert!(!seat.ready_for_game(true));
        assert!(seat.declare_ready());
        assert!(!seat.declare_ready());
        assert!(seat.ready_for_game(true));
        seat.rollback_ready_declaration();
        assert!(!seat.ready_for_game(false));
        assert!(seat.declare_ready());
        seat.connected = false;
        assert!(!seat.ready_for_game(false));
    }

    /// A connected human requires audio setup only for a transcription-gated game.
    #[test]
    fn human_activation_depends_on_audio_requirement() {
        let mut seat = SessionParticipant {
            participant_session_id: "participant".into(),
            participant_id: 1,
            source: "direct".into(),
            role: PlayerRole::A,
            connected: true,
            ready: false,
            audio_ready: false,
            consent_decisions: HashMap::new(),
            joined_at: now_iso(),
            updated_at: now_iso(),
            disconnected_at: None,
            reconnect_deadline_at: None,
        };
        assert!(seat.ready_for_game(false));
        assert!(!seat.ready_for_game(true));
        assert!(seat.mark_audio_ready());
        assert!(!seat.mark_audio_ready());
        assert!(seat.ready_for_game(true));
        seat.connected = false;
        assert!(!seat.ready_for_game(false));
        seat.source = "worker".into();
        seat.audio_ready = false;
        assert!(!seat.ready_for_game(true));
        seat.connected = true;
        assert!(seat.ready_for_game(true));
    }
}
