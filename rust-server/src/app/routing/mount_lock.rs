//! Synchronization of runtime mounting with configuration changes.
//!
//! Mount construction and configuration invalidation serialize on the same
//! experiment-name slot. A fixed slot table bounds memory for unknown namespaces.
//! Invalidation makes a changed configuration visible on the next mount while the mount
//! registry manages existing rooms.
use super::*;

/// Selects a fixed lock slot for an experiment name, including unknown request namespaces.
pub(in crate::app) fn mount_lock_slot(experiment_id: &str) -> usize {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    experiment_id.hash(&mut hasher);
    hasher.finish() as usize % MOUNT_LOCK_COUNT
}

pub(in crate::app) const MOUNT_LOCK_COUNT: usize = 64;

/// Serializes mounting and configuration invalidation for the same experiment.
pub(in crate::app) async fn lock_runtime_router_build(
    state: &ServerContext,
    experiment_id: &str,
) -> tokio::sync::OwnedMutexGuard<()> {
    state.installation.runtime_router_build_locks[mount_lock_slot(experiment_id)]
        .clone()
        .lock_owned()
        .await
}

/// Removes one mounted runtime after a durable change makes its captured state stale.
pub(in crate::app) async fn invalidate_runtime_router(state: &ServerContext, experiment_id: &str) {
    if let Some(cache) = state.installation.runtime_router_cache.upgrade() {
        if let Some(mounts) = cache.write().await.get_mut(experiment_id) {
            mounts.retire();
        }
    }
}

/// Evicts cached inactive runtimes after installation-wide provider settings change.
pub(in crate::app) async fn invalidate_inactive_runtime_routers(state: &ServerContext) {
    let Some(cache) = state.installation.runtime_router_cache.upgrade() else {
        return;
    };
    let experiment_ids = cache.read().await.keys().cloned().collect::<Vec<_>>();
    for experiment_id in experiment_ids {
        let guard = lock_runtime_router_build(state, &experiment_id).await;
        let inactive = state
            .installation
            .store
            .experiment_definition(&experiment_id)
            .await
            .ok()
            .flatten()
            .is_some_and(|experiment| experiment.status == "inactive");
        if inactive {
            invalidate_runtime_router(state, &experiment_id).await;
        }
        drop(guard);
    }
}
