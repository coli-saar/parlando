//! Counts live seats and transcription capacity from the active session cache.
//!
//! Admission and monitoring share the same counts for the experiment's live revisions.
//! Each revision supplies its own session cache and transcription settings, so an older,
//! draining revision continues to contribute its occupied seats and speech resources.

use super::{
    speechmatics_readiness_required, AppState, Arc, Game, HashSet, MemoryState, SessionLifecycle,
    Weak,
};

/// Runtime occupancy used by both admission checks and dashboard reporting.
#[derive(Default)]
pub(super) struct Occupancy {
    pub(super) waiting: usize,
    pub(super) active: usize,
    pub(super) completed: usize,
    pub(super) unattached: usize,
    pub(super) transcription: usize,
}

impl Occupancy {
    /// Adds one revision's usage under its own speech configuration.
    ///
    /// A forming room with one seat consumes waiting capacity. Once both seats are
    /// assigned it consumes active capacity even before play starts. Ended rooms
    /// contribute only to retention. Speech capacity counts non-agent seats in
    /// unfinished rooms on revisions that require transcription.
    pub(super) fn include<G: Game>(
        &mut self,
        memory: &MemoryState<G>,
        transcription_enabled: bool,
    ) {
        let mut attached = HashSet::new();
        for session in memory.sessions.values() {
            match session.lifecycle() {
                SessionLifecycle::Ended(_) => self.completed += 1,
                SessionLifecycle::Forming if session.participants.len() < 2 => self.waiting += 1,
                SessionLifecycle::Forming | SessionLifecycle::Running => self.active += 1,
            }
            attached.extend(session.participants.keys());
            if transcription_enabled && !session.lifecycle().is_ended() {
                self.transcription += session
                    .participants
                    .values()
                    .filter(|p| p.source != "agent")
                    .count();
            }
        }
        self.unattached += memory
            .participants
            .keys()
            .filter(|id| !attached.contains(*id))
            .count();
    }
}

/// Counts occupied resources across all live revisions of this experiment.
pub(super) async fn experiment_occupancy<G: Game>(state: &Arc<AppState<G>>) -> Occupancy {
    let owners = state
        .context
        .installation
        .runtime_registry
        .read()
        .await
        .get(&state.experiment_id)
        .map(|mounts| mounts.iter().filter_map(Weak::upgrade).collect::<Vec<_>>())
        .unwrap_or_default();
    let mut usage = Occupancy::default();
    for owner in owners {
        let memory = owner.memory.read().await;
        usage.include(
            &memory,
            speechmatics_readiness_required(&owner.context.config),
        );
    }
    usage
}
