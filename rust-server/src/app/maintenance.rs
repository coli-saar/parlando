//! Runtime cleanup and terminalization of interrupted sessions.
//!
//! Mounted runtimes schedule cleanup of transient credentials, expired sessions, and
//! ended rooms. The sweep obtains the session transition lock before applying an expiry.
//! Startup finalization separately records terminal outcomes for durable sessions
//! interrupted by a previous process.

use super::{
    broadcast_participant_states, expire_live_session_locked, prolific_handoff, room_cleanup,
    session_transition_lock, AppState, Arc, Duration, HashMap, HashSet, ParticipantOutcomeKind,
    ParticipantResult, Result, RoomCleanup, SessionEnd, SessionEndCause, SessionEventRecord,
};

/// Runs bounded periodic cleanup for transient credentials, sessions, and tickets.
pub(super) fn spawn_security_cleanup(state: Arc<AppState>, clean_admin_sessions: bool) {
    tokio::spawn(async move {
        let mut lifecycle_interval = tokio::time::interval(Duration::from_secs(1));
        lifecycle_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut security_interval = tokio::time::interval(Duration::from_secs(60));
        security_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            if state.retired.load(std::sync::atomic::Ordering::Acquire) {
                let _admission = state.session_admission.reserve().await;
                let memory = state.memory.read().await;
                let drained = memory.sessions.is_empty() && memory.participants.is_empty();
                drop(memory);
                if drained {
                    if let Some(cache) = state.context.installation.runtime_router_cache.upgrade() {
                        if let Some(mounts) = cache.write().await.get_mut(&state.experiment_id) {
                            mounts.finish_draining(state.mount_id);
                        }
                    }
                    state.participant_auth.release_mount(state.mount_id).await;
                    if let Some(owners) = state
                        .context
                        .installation
                        .runtime_registry
                        .write()
                        .await
                        .get_mut(&state.experiment_id)
                    {
                        owners.retain(|owner| {
                            owner
                                .upgrade()
                                .is_some_and(|owner| owner.mount_id != state.mount_id)
                        });
                    }
                    break;
                }
            }
            if Arc::strong_count(&state) == 1 && state.memory.read().await.sessions.is_empty() {
                break;
            }
            tokio::select! {
                _ = lifecycle_interval.tick() => cleanup_transient_rooms(&state).await,
                _ = security_interval.tick() => {
                    let expired_participants = state.participant_auth.cleanup(state.mount_id).await;
                    state.upgrade_tickets.cleanup().await;
                    if clean_admin_sessions {
                        if let Err(error) = state.context.installation.admin_auth.cleanup().await {
                            tracing::warn!(%error, "failed to clean expired administrator sessions");
                        }
                    }
                    cleanup_expired_participants(&state, expired_participants).await;
                }
            }
        }
    });
}

/// Removes expired unattached credentials without disturbing participants retained in rooms.
async fn cleanup_expired_participants(
    state: &Arc<AppState>,
    expired_participants: HashSet<String>,
) {
    let _admission = state.session_admission.reserve().await;
    let participant_ids_in_rooms = state
        .memory
        .read()
        .await
        .sessions
        .values()
        .flat_map(|session| session.participants.keys().cloned())
        .collect::<HashSet<_>>();
    state
        .memory
        .write()
        .await
        .participants
        .retain(|participant_id, _| {
            !expired_participants.contains(participant_id)
                || participant_ids_in_rooms.contains(participant_id)
        });
    let unattached_timeout = state
        .context
        .config
        .session
        .waiting_session_timeout_seconds
        .max(1);
    let stale_unattached = {
        let now = chrono::Utc::now();
        let mut memory = state.memory.write().await;
        let stale = memory
            .participants
            .iter()
            .filter(|(participant_id, participant)| {
                !participant_ids_in_rooms.contains(*participant_id)
                    && chrono::DateTime::parse_from_rfc3339(&participant.updated_at)
                        .ok()
                        .is_some_and(|updated| {
                            updated.with_timezone(&chrono::Utc)
                                < now - chrono::Duration::seconds(unattached_timeout)
                        })
            })
            .map(|(participant_id, _)| participant_id.clone())
            .collect::<Vec<_>>();
        for participant_id in &stale {
            memory.participants.remove(participant_id);
        }
        stale
    };
    for participant_id in &stale_unattached {
        state
            .participant_auth
            .revoke_participant_session(participant_id, state.mount_id)
            .await;
    }
    if !stale_unattached.is_empty() {
        let stale = stale_unattached.iter().collect::<HashSet<_>>();
        state
            .chat_submission_budgets
            .write()
            .await
            .retain(|participant_id, _| !stale.contains(participant_id));
        state.rejection_windows.write().await.retain(|key, _| {
            !stale_unattached
                .iter()
                .any(|participant_id| key.contains(participant_id))
        });
    }
}

/// Returns whether a runtime session has reached a durable terminal lifecycle state.
pub(super) fn session_lifecycle_is_ended(lifecycle: &str) -> bool {
    lifecycle == "ended"
}

/// Removes or expires transient sessions after configured waiting, idle, and lifetime bounds.
async fn cleanup_transient_rooms(state: &Arc<AppState>) {
    let now = chrono::Utc::now();
    let grace = chrono::Duration::seconds(state.context.config.session.reconnect_grace_seconds);
    let candidates = {
        let memory = state.memory.read().await;
        memory
            .sessions
            .iter()
            .filter(|(_, room)| room_cleanup(room, now, grace).is_some())
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>()
    };
    let mut removed = Vec::new();
    for public_session_id in candidates {
        let Some(transition_lock) = session_transition_lock(state, &public_session_id).await else {
            continue;
        };
        // A room with an unresolved write retains its lock. Defer that room to
        // the next sweep so other rooms can still expire and release resources.
        let Ok(_transition_guard) = transition_lock.try_lock() else {
            continue;
        };
        let due = state
            .memory
            .read()
            .await
            .sessions
            .get(&public_session_id)
            .and_then(|room| room_cleanup(room, chrono::Utc::now(), grace));
        match due {
            Some(RoomCleanup::Expire(reason)) => {
                match expire_live_session_locked(state, &public_session_id, reason).await {
                    Ok(true) => broadcast_participant_states(state, &public_session_id).await,
                    Ok(false) => {}
                    Err(error) => {
                        tracing::error!(%error, %public_session_id, reason, "session expiry failed")
                    }
                }
                continue;
            }
            Some(RoomCleanup::CloseFarewell) => {
                state
                    .audio_connections
                    .shutdown_session(&public_session_id)
                    .await;
                if let Some(room) = state
                    .memory
                    .write()
                    .await
                    .sessions
                    .get_mut(&public_session_id)
                {
                    room.finish_farewell();
                }
                continue;
            }
            Some(RoomCleanup::Evict) => {}
            None => continue,
        }
        state
            .audio_connections
            .shutdown_session(&public_session_id)
            .await;
        let mut memory = state.memory.write().await;
        if let Some(room) = memory.sessions.remove(&public_session_id) {
            // Terminal reconciliation now reads SQLite. Keep issued credentials until their expiry.
            for id in room.participants.keys() {
                memory.participants.remove(id);
            }
        }
        removed.push(public_session_id);
    }
    if removed.is_empty() {
        return;
    }
    let removed_set = removed.iter().collect::<HashSet<_>>();
    state
        .session_buses
        .write()
        .await
        .retain(|public_session_id, _| !removed_set.contains(public_session_id));
    state.committed_transcripts.write().await.retain(|key| {
        !removed
            .iter()
            .any(|public_session_id| key.starts_with(&format!("{public_session_id}:")))
    });
    state.game_connections.remove_sessions(&removed).await;
    state.audio_connections.remove_sessions(&removed).await;
    state.rejection_windows.write().await.retain(|key, _| {
        !removed
            .iter()
            .any(|public_session_id| key.starts_with(&format!("{public_session_id}\0")))
    });
}

/// Finalizes sessions left nonterminal by an earlier process before opening new intake.
pub(super) async fn finalize_interrupted_sessions(state: &Arc<AppState>) -> Result<()> {
    finalize_interrupted_experiment(
        &state.context.installation.store,
        &state.experiment_id,
        &state.context.config,
    )
    .await
}

/// Records interrupted rooms from durable state without loading a game component.
pub(super) async fn finalize_interrupted_experiment(
    store: &crate::storage::SqliteExperimentStore,
    experiment_id: &str,
    config: &super::ExperimentConfig,
) -> Result<()> {
    let sessions = store.interrupted_sessions(experiment_id).await?;
    for (session_id, started) in sessions {
        let participants = store
            .session_participants(experiment_id, session_id)
            .await?;
        let participant_results = participants
            .into_iter()
            .filter(|participant| participant.participant_kind.as_deref() != Some("agent"))
            .map(|participant| {
                let outcome = ParticipantOutcomeKind::TechnicalFailure;
                (
                    participant.role,
                    ParticipantResult {
                        handoff: (participant.identity_provider.as_deref() == Some("prolific"))
                            .then(|| prolific_handoff(config, &outcome, started))
                            .flatten(),
                        outcome,
                        reason: "server_restart".to_string(),
                        completion: None,
                        final_observation: None,
                    },
                )
            })
            .collect::<HashMap<_, _>>();
        let session_end = SessionEnd {
            cause: SessionEndCause::TechnicalFailure,
            completion: None,
            participant_results,
        };
        let event = SessionEventRecord {
            experiment_id: experiment_id.to_string(),
            session_id,
            event_type: "session_ended".to_string(),
            actor_participant_id: None,
            actor_role: None,
            payload: serde_json::to_value(&session_end)?,
            game_state: None,
        };
        store.end_session(event, session_end).await?;
    }
    Ok(())
}
