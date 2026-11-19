//! Reservation, capacity, and assignment of participants to sessions.

use super::construction::prepare_agent_construction;
use super::participant_state::existing_assignment;
use super::{
    anyhow, authenticated_participant_id, construct_and_join_agent, create_live_session_locked,
    experiment_occupancy, json, participant_state_response, persist_session_event,
    refresh_prolific_progress, require_consent, require_open_experiment, session_transition_lock,
    speechmatics_readiness_required, AgentsMode, AppError, AppState, Arc, CreateSessionRequest,
    CreateSessionResponse, Extension, Game, HashSet, Json, LiveSession, MemoryState,
    ParticipantPrincipal, PlayerRole, Result, SessionLifecycle, SessionParticipant, State,
    StatusCode,
};

/// Serializes participant allocation across runtime revisions and cleanup.
#[derive(Clone, Default)]
pub(super) struct AdmissionQueue(Arc<tokio::sync::Mutex<()>>);

/// Proof that a caller exclusively owns the experiment's admission capacity.
pub(super) struct AdmissionReservation {
    _guard: tokio::sync::OwnedMutexGuard<()>,
}

impl AdmissionQueue {
    /// Reserves capacity until the allocation or registry update completes.
    pub(super) async fn reserve(&self) -> AdmissionReservation {
        AdmissionReservation {
            _guard: self.0.clone().lock_owned().await,
        }
    }
}

/// Reuses or admits a participant using the experiment's matchmaking policy.
pub(super) async fn create_session<A: Game>(
    State(state): State<Arc<AppState<A>>>,
    principal: Option<Extension<ParticipantPrincipal>>,
    Json(_request): Json<CreateSessionRequest>,
) -> Result<Json<CreateSessionResponse>, AppError> {
    let participant_session_id = authenticated_participant_id(principal)?;
    tokio::spawn(admit_session(state, participant_session_id))
        .await
        .map_err(|error| AppError::from(anyhow!(error)))?
}

/// Completes admission independently of the lifetime of the requesting HTTP connection.
async fn admit_session<A: Game>(
    state: Arc<AppState<A>>,
    participant_session_id: String,
) -> Result<Json<CreateSessionResponse>, AppError> {
    if let Some(snapshot) = existing_assignment(&state, &participant_session_id).await? {
        return Ok(Json(snapshot));
    }
    let _intake_guard = require_open_experiment(&state).await?;
    require_consent(&state, &participant_session_id).await?;
    require_session_storage_reserve(&state).await?;
    let requested_mode = "direct".to_string();
    let prepared_agent = (state.context.config.agents.mode == AgentsMode::HumanVsAgent)
        .then(|| prepare_agent_construction(&state))
        .transpose()?;
    let admission = state.session_admission.reserve().await;
    if let Some(snapshot) = existing_assignment(&state, &participant_session_id).await? {
        return Ok(Json(snapshot));
    }
    let paired_or_created: Result<(String, PlayerRole, bool), AppError> = {
        if state.context.config.agents.mode == AgentsMode::HumanVsHuman {
            let existing = {
                let memory = state.memory.read().await;
                open_human_session_for_pairing(
                    &memory,
                    &requested_mode,
                    &memory
                        .participants
                        .get(&participant_session_id)
                        .ok_or_else(|| AppError::not_found("Participant session not found."))?
                        .purpose,
                )
            };
            if let Some(public_session_id) = existing {
                let role = join_existing_room(
                    &admission,
                    &state,
                    &public_session_id,
                    &participant_session_id,
                )
                .await?;
                Ok((public_session_id, role, false))
            } else {
                ensure_session_capacity(&state, SessionAdmission::Waiting, 1).await?;
                let (public_session_id, role) = create_live_session_locked(
                    &admission,
                    &state,
                    participant_session_id.clone(),
                    requested_mode.clone(),
                    PlayerRole::A,
                    rand::random::<u64>(),
                )
                .await?;
                Ok((public_session_id, role, true))
            }
        } else {
            ensure_session_capacity(&state, SessionAdmission::Active, 1).await?;
            let session_seed = prepared_agent
                .as_ref()
                .expect("agent was prepared")
                .session_seed;
            let (public_session_id, role) = create_live_session_locked(
                &admission,
                &state,
                participant_session_id.clone(),
                requested_mode.clone(),
                PlayerRole::A,
                session_seed,
            )
            .await?;
            Ok((public_session_id, role, true))
        }
    };
    drop(admission);
    let (public_session_id, role, created) = paired_or_created?;
    persist_session_event(
        &state,
        &public_session_id,
        Some(&participant_session_id),
        "participant_joined",
        json!({"role": role.as_str()}),
        None,
    )
    .await;
    if created
        && state.context.config.recruitment.prolific.enabled
        && state
            .memory
            .read()
            .await
            .sessions
            .get(&public_session_id)
            .is_some_and(|session| session.purpose == "research")
    {
        let refresh_state = state.clone();
        tokio::spawn(async move {
            let experiment_id = refresh_state.experiment_id.clone();
            if let Err(error) = refresh_prolific_progress(&refresh_state, &experiment_id).await {
                tracing::warn!(?error, %experiment_id, "could not refresh Prolific progress target");
            }
        });
    }
    if let Some(prepared_agent) = prepared_agent {
        tokio::spawn(construct_and_join_agent(
            state.clone(),
            public_session_id.clone(),
            prepared_agent,
        ));
    }
    let response = participant_state_response(&state, &public_session_id, role).await?;
    Ok(Json(response))
}

/// Pauses only new session admission before SQLite exhausts its filesystem.
async fn require_session_storage_reserve<A: Game>(
    state: &Arc<AppState<A>>,
) -> Result<(), AppError> {
    let Some(capacity) = state.context.installation.store.storage_capacity().await? else {
        return Ok(());
    };
    let reserve = state
        .context
        .config
        .capacity
        .storage_reserve_megabytes
        .saturating_mul(1024 * 1024);
    if capacity.available_bytes <= reserve {
        return Err(AppError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "New sessions are paused because storage reserve has been reached",
        ));
    }
    Ok(())
}

/// Admission class used by the single session-capacity policy.
#[derive(Clone, Copy)]
enum SessionAdmission {
    /// A human-human session still waiting for its second participant.
    Waiting,
    /// A fully allocated human-human or human-agent research session.
    Active,
}

/// Reserves session and ASR capacity before any durable session is created.
async fn ensure_session_capacity<G: Game>(
    state: &Arc<AppState<G>>,
    admission: SessionAdmission,
    transcription_streams_to_add: usize,
) -> Result<(), AppError> {
    let config = &state.context.config;
    let usage = experiment_occupancy(state).await;
    match admission {
        SessionAdmission::Waiting if usage.waiting >= config.capacity.max_waiting_sessions => {
            return Err(AppError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "Waiting-session capacity is temporarily full",
            ));
        }
        SessionAdmission::Active if usage.active >= config.capacity.max_active_sessions => {
            return Err(AppError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "Active-session capacity is temporarily full",
            ));
        }
        _ => {}
    }
    if speechmatics_readiness_required(config)
        && usage.transcription + transcription_streams_to_add
            > config.capacity.max_transcription_streams
    {
        return Err(AppError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "Transcription capacity is temporarily full",
        ));
    }
    Ok(())
}

/// Finds an existing human-human waiting session that can accept one more human.
fn open_human_session_for_pairing<G: Game>(
    memory: &MemoryState<G>,
    mode: &str,
    purpose: &str,
) -> Option<String> {
    memory
        .sessions
        .iter()
        .find(|(_, session)| {
            session.lifecycle() == &SessionLifecycle::Forming
                && session.mode == mode
                && session.purpose == purpose
                && next_role(session) == Some(PlayerRole::B)
                && session
                    .participants
                    .values()
                    .all(|participant| participant.source != "agent")
        })
        .map(|(public_session_id, _)| public_session_id.clone())
}

/// Commits the second human seat while the admission guard reserves capacity and the
/// session guard excludes concurrent expiry, leave, and activation.
async fn join_existing_room<A: Game>(
    _reservation: &AdmissionReservation,
    state: &Arc<AppState<A>>,
    public_session_id: &str,
    participant_session_id: &str,
) -> Result<PlayerRole, AppError> {
    let Some(lock) = session_transition_lock(state, public_session_id).await else {
        return Err(AppError::not_found("Session not found."));
    };
    let _guard = lock.lock().await;
    ensure_session_capacity(state, SessionAdmission::Active, 1).await?;
    let (membership, session_id) = {
        let memory = state.memory.read().await;
        let participant = memory
            .participants
            .get(participant_session_id)
            .ok_or_else(|| AppError::not_found("Participant session not found."))?;
        let room = memory
            .sessions
            .get(public_session_id)
            .filter(|room| {
                room.lifecycle() == &SessionLifecycle::Forming
                    && room.purpose == participant.purpose
            })
            .ok_or_else(|| {
                AppError::new(StatusCode::CONFLICT, "Session is no longer available.")
            })?;
        let role = next_role(room)
            .ok_or_else(|| AppError::forbidden("Session already has two players."))?;
        (
            SessionParticipant::human(participant, role, &state.context.config),
            room.session_id,
        )
    };
    state
        .context
        .installation
        .store
        .add_session_participant(membership.record(&state.experiment_id, session_id))
        .await?;
    let role = membership.role;
    state
        .memory
        .write()
        .await
        .sessions
        .get_mut(public_session_id)
        .expect("session guard protects reserved room")
        .participants
        .insert(participant_session_id.to_string(), membership);
    Ok(role)
}

/// Finds the first unassigned role in a two-player session.
fn next_role<G: Game>(session: &LiveSession<G>) -> Option<PlayerRole> {
    let roles = session
        .participants
        .values()
        .map(|p| p.role)
        .collect::<HashSet<_>>();
    if !roles.contains(&PlayerRole::A) {
        Some(PlayerRole::A)
    } else if !roles.contains(&PlayerRole::B) {
        Some(PlayerRole::B)
    } else {
        None
    }
}
