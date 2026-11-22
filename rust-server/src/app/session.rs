//! In-memory rooms, participant membership, and ordered software-seat delivery.
//!
//! Stores the authoritative game state, membership, lifecycle, and clocks for one live
//! room. Admission constructs the room and assigns seats. Transition coordinators install
//! committed changes through session operations. Participant projections and monitoring
//! read the resulting state.
use super::*;
use crate::session_log::SessionLogWriter;

mod connection;
pub(super) mod construction;
mod readiness;
mod timing;
pub(super) use readiness::{session_ready_for_game, speechmatics_readiness_required};
use timing::LiveSessionTiming;

/// Runtime participant session record used by active server tasks.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct ParticipantSession {
    pub id: String,
    pub participant_id: i64,
    pub research_id: String,
    pub source: String,
    /// Immutable data-use purpose selected when participant intake opened.
    pub purpose: String,
    pub status: String,
    pub consent_decisions: HashMap<String, bool>,
    pub created_at: String,
    pub updated_at: String,
}

/// Runtime record for one participant's membership in a session.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct SessionParticipant {
    pub participant_session_id: String,
    pub participant_id: i64,
    /// Runtime participant source, used to distinguish humans from agents.
    pub source: String,
    pub role: PlayerRole,
    pub connected: bool,
    /// Whether the browser declared its game channel ready at least once.
    ready: bool,
    /// Whether this participant has completed required audio/STT setup for this session.
    audio_ready: bool,
    pub consent_decisions: HashMap<String, bool>,
    pub joined_at: String,
    pub updated_at: String,
    /// Most recent disconnection instant for restart-safe grace handling.
    pub disconnected_at: Option<String>,
    /// Fixed reconnect deadline derived when the connection was lost.
    pub reconnect_deadline_at: Option<String>,
}

impl SessionParticipant {
    /// Allocates the connected software seat before its task declares readiness.
    pub(super) fn agent(agent_session_id: String, participant_id: i64) -> Self {
        Self {
            participant_session_id: agent_session_id,
            participant_id,
            source: "agent".to_string(),
            role: PlayerRole::B,
            connected: true,
            ready: false,
            audio_ready: true,
            consent_decisions: HashMap::new(),
            joined_at: now_iso(),
            updated_at: now_iso(),
            disconnected_at: None,
            reconnect_deadline_at: None,
        }
    }

    /// Prepares a human seat with the current audio setup requirements.
    pub(super) fn human(
        participant: &ParticipantSession,
        role: PlayerRole,
        config: &ExperimentConfig,
    ) -> Self {
        let joined_at = now_iso();
        Self {
            participant_session_id: participant.id.clone(),
            participant_id: participant.participant_id,
            source: participant.source.clone(),
            role,
            connected: false,
            ready: false,
            audio_ready: !speechmatics_readiness_required(config),
            consent_decisions: participant.consent_decisions.clone(),
            joined_at: joined_at.clone(),
            updated_at: joined_at,
            disconnected_at: None,
            reconnect_deadline_at: None,
        }
    }

    /// Produces the durable membership record before the room publishes this seat.
    pub(super) fn record(&self, experiment_id: &str, session_id: i64) -> SessionParticipantRecord {
        SessionParticipantRecord {
            experiment_id: experiment_id.to_string(),
            session_id,
            participant_id: self.participant_id,
            participant_session_id: self.participant_session_id.clone(),
            role: self.role.as_str().to_string(),
            connection_status: if self.connected {
                "connected"
            } else {
                "joined"
            }
            .to_string(),
        }
    }
}

/// Runtime session record parameterized by a concrete game state type.
pub(super) struct LiveSession {
    pub id: String,
    pub experiment_id: String,
    pub session_id: i64,
    pub mode: String,
    /// Immutable data-use purpose shared by every participant in this session.
    pub purpose: String,
    /// Session-local behavior object retaining session-scoped capabilities.
    pub game: GameSession,
    /// Authoritative serializable mechanics state.
    state: GameFrame,
    /// Base game-scoped handle for this session's log.
    pub logger: SessionLogger,
    /// Owned writer task drained when the session log is closed.
    pub log_writer: Option<SessionLogWriter>,
    /// Authoritative shared lifecycle for this forming, running, or ended session.
    lifecycle: SessionLifecycle,
    /// Temporary interaction pause while a required role may reconnect.
    pause: Option<crate::protocol::ParticipantPauseReason>,
    /// Bounded, ordered delivery to the one reserved software seat, when started.
    pub agent_inbox: Option<mpsc::Sender<AgentObservation>>,
    /// Serializes membership, lifecycle, and publication of committed game input.
    pub transition_lock: Arc<Mutex<()>>,
    /// Releases submitting callers when storage cannot yet prove the current turn's outcome.
    pub action_commit_pending: tokio::sync::watch::Sender<bool>,
    pub participants: HashMap<String, SessionParticipant>,
    timing: LiveSessionTiming,
}

impl LiveSession {
    /// Borrows the current session timestamps for participant views and monitoring.
    pub(super) fn timing(&self) -> &LiveSessionTiming {
        &self.timing
    }

    /// Updates the session activity timestamp and its corresponding idle deadline together.
    pub(super) fn record_activity(
        &mut self,
        at: chrono::DateTime<chrono::Utc>,
        idle: chrono::DateTime<chrono::Utc>,
    ) {
        self.timing.record_activity(at, idle);
    }

    /// Closes the optional post-completion audio period after its deadline is reached.
    pub(super) fn finish_farewell(&mut self) {
        self.timing.voice_ends_at = None;
    }

    /// Returns the shared lifecycle visible to every participant.
    pub(super) fn lifecycle(&self) -> &SessionLifecycle {
        &self.lifecycle
    }

    /// Returns the reason interaction is temporarily suspended, if any.
    pub(super) fn pause(&self) -> Option<&ParticipantPauseReason> {
        self.pause.as_ref()
    }

    /// Reports whether the running session accepts interaction at this moment.
    pub(super) fn accepts_interaction(&self) -> bool {
        self.lifecycle == SessionLifecycle::Running && self.pause.is_none()
    }

    /// Borrows the latest committed mechanics state for observations and transition preparation.
    pub(super) fn state(&self) -> &GameFrame {
        &self.state
    }

    /// Projects a durable abandonment or expiry and closes the agent inbox.
    pub(super) fn terminate(&mut self, end: SessionEnd) {
        self.lifecycle = SessionLifecycle::Ended(end);
        self.agent_inbox.take();
        self.pause = None;
        self.timing.updated_at = now_iso();
    }

    /// Installs an already committed action and closes agent delivery on game completion.
    pub(super) fn install_action(
        &mut self,
        after: GameFrame,
        end: Option<SessionEnd>,
        farewell: Option<chrono::Duration>,
    ) {
        self.state = after;
        if let Some(end) = end {
            let completed_at = chrono::Utc::now();
            self.timing.updated_at = completed_at.to_rfc3339();
            self.timing.voice_ends_at = farewell.map(|grace| (completed_at + grace).to_rfc3339());
            self.lifecycle = SessionLifecycle::Ended(end);
            self.agent_inbox.take();
        }
    }

    /// Applies a committed activation with its first meaningful activity and idle deadline.
    pub(super) fn activate(
        &mut self,
        started: chrono::DateTime<chrono::Utc>,
        idle: chrono::DateTime<chrono::Utc>,
    ) {
        self.lifecycle = SessionLifecycle::Running;
        self.pause = None;
        self.record_activity(started, idle);
    }
}

/// Typed shared session lifecycle used by every runtime transition.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "state", content = "end", rename_all = "snake_case")]
pub(super) enum SessionLifecycle {
    /// Participants or required services are still assembling.
    Forming,
    /// The game has started and may be active or temporarily paused.
    Running,
    /// One immutable shared terminal value has been committed.
    Ended(SessionEnd),
}

impl SessionLifecycle {
    /// Returns the stable database lifecycle value.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Forming => "forming",
            Self::Running => "running",
            Self::Ended(_) => "ended",
        }
    }

    /// Reports whether the session has reached its absorbing state.
    pub fn is_ended(&self) -> bool {
        matches!(self, Self::Ended(_))
    }
}

/// In-memory active-session cache used for live WebSocket and game execution state.
pub(super) struct MemoryState {
    pub participants: HashMap<String, ParticipantSession>,
    pub sessions: HashMap<String, LiveSession>,
}

impl Default for MemoryState {
    fn default() -> Self {
        Self {
            participants: HashMap::new(),
            sessions: HashMap::new(),
        }
    }
}

impl MemoryState {
    /// Creates and stores an active participant session after durable identity creation.
    pub fn create_participant(
        &mut self,
        participant_id: i64,
        research_id: String,
        source: String,
        purpose: String,
    ) -> ParticipantSession {
        self.create_participant_with_id(new_id("ps"), participant_id, research_id, source, purpose)
    }

    /// Restores or creates a participant session with a durable admission subject.
    pub fn create_participant_with_id(
        &mut self,
        participant_session_id: String,
        participant_id: i64,
        research_id: String,
        source: String,
        purpose: String,
    ) -> ParticipantSession {
        if let Some(existing) = self.participants.get(&participant_session_id) {
            return existing.clone();
        }
        let now = now_iso();
        let participant = ParticipantSession {
            id: participant_session_id,
            participant_id,
            research_id,
            source,
            purpose,
            status: "created".to_string(),
            consent_decisions: HashMap::new(),
            created_at: now.clone(),
            updated_at: now,
        };
        self.participants
            .insert(participant.id.clone(), participant.clone());
        participant
    }
}

/// Event delivered to one started agent instance.
pub(super) enum AgentObservation {
    /// Starts decision processing after the session's activation has been committed.
    Activated,
    /// One committed role-specific delivery with any terminal result.
    Delivery {
        observation: crate::agent::Observation,
        completion: Option<ComponentValue>,
    },
}
