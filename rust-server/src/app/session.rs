//! In-memory rooms, participant membership, and ordered software-seat delivery.
use super::*;
use crate::session_log::SessionLogWriter;

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
    pub ready: bool,
    /// Whether this participant has completed required audio/STT setup for this session.
    pub audio_ready: bool,
    pub consent_decisions: HashMap<String, bool>,
    pub joined_at: String,
    pub updated_at: String,
    /// Most recent disconnection instant for restart-safe grace handling.
    pub disconnected_at: Option<String>,
    /// Fixed reconnect deadline derived when the connection was lost.
    pub reconnect_deadline_at: Option<String>,
}

impl SessionParticipant {
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
pub(super) struct LiveSession<G: Game> {
    pub id: String,
    pub experiment_id: String,
    pub session_id: i64,
    pub mode: String,
    /// Immutable data-use purpose shared by every participant in this session.
    pub purpose: String,
    /// Session-local behavior object retaining session-scoped capabilities.
    pub game: G,
    /// Authoritative serializable mechanics state.
    pub state: G::State,
    /// Base game-scoped handle for this session's log.
    pub logger: SessionLogger,
    /// Owned writer task drained when the session log is closed.
    pub log_writer: Option<SessionLogWriter>,
    /// Authoritative shared lifecycle for this forming, running, or ended session.
    pub lifecycle: SessionLifecycle,
    /// Temporary interaction pause while a required role may reconnect.
    pub pause: Option<crate::protocol::ParticipantPauseReason>,
    /// Bounded, ordered delivery to the one reserved software seat, when started.
    pub agent_inbox: Option<mpsc::Sender<AgentObservation<G>>>,
    /// Serializes membership, lifecycle, and publication of committed game input.
    pub transition_lock: Arc<Mutex<()>>,
    pub participants: HashMap<String, SessionParticipant>,
    /// Fixed beginning of the unmatched waiting phase.
    pub waiting_started_at: String,
    /// Fixed server-owned deadline for finding a partner.
    pub waiting_deadline_at: String,
    /// Fixed absolute infrastructure lifetime deadline.
    pub lifetime_deadline_at: String,
    /// Last accepted message or action; heartbeats never advance this value.
    pub last_meaningful_activity_at: Option<String>,
    /// Fixed idle deadline derived from the last meaningful activity.
    pub idle_deadline_at: Option<String>,
    /// Fixed end of the optional post-completion human voice relay period.
    pub voice_ends_at: Option<String>,
    pub updated_at: String,
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
pub(super) struct MemoryState<G: Game> {
    pub participants: HashMap<String, ParticipantSession>,
    pub sessions: HashMap<String, LiveSession<G>>,
}

impl<G: Game> Default for MemoryState<G> {
    fn default() -> Self {
        Self {
            participants: HashMap::new(),
            sessions: HashMap::new(),
        }
    }
}

impl<G: Game> MemoryState<G> {
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
pub(super) enum AgentObservation<A: Game> {
    /// Starts decision processing after the session's activation has been committed.
    Activated,
    /// Accepted action plus role-specific state snapshot after the action.
    Action {
        actor: PlayerRole,
        action: A::Action,
        resulting_observation: A::Observation,
        completion: Option<A::Completion>,
    },
    /// Conversation message with its sender role.
    Message { speaker: PlayerRole, text: String },
}
