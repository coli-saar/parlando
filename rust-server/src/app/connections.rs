//! Generation-checked ownership of participant transports.
//!
//! Game and audio transports keep separate registries. Registering a replacement signals
//! the previous owner and returns a generation that teardown must present. Monitoring
//! reads snapshots and session cleanup retires entries for the selected rooms.

use super::{new_id, ConnectionLiveness, ConnectionLivenessSnapshot};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::{mpsc, RwLock};

/// Cancellation handle for the socket generation currently serving a role.
struct ConnectionControl {
    generation: String,
    shutdown: mpsc::Sender<()>,
    liveness: Arc<ConnectionLiveness>,
}

/// One active owner per session and role for a particular transport (game or audio).
#[derive(Default)]
pub(super) struct ConnectionRegistry {
    owners: RwLock<HashMap<String, ConnectionControl>>,
}

impl ConnectionRegistry {
    /// Registers a role and immediately signals any previous owner to stop.
    pub(super) async fn register(
        &self,
        key: String,
        liveness: Arc<ConnectionLiveness>,
    ) -> (String, mpsc::Receiver<()>, bool) {
        let generation = new_id("connection");
        let (shutdown, receiver) = mpsc::channel(1);
        let control = ConnectionControl {
            generation: generation.clone(),
            shutdown,
            liveness,
        };
        let previous = self.owners.write().await.insert(key, control);
        let replaced = previous.is_some();
        if let Some(previous) = previous {
            let _ = previous.shutdown.try_send(());
        }
        (generation, receiver, replaced)
    }

    /// Removes the finishing socket only if its generation still owns the seat.
    pub(super) async fn unregister(&self, key: &str, generation: &str) -> bool {
        let mut owners = self.owners.write().await;
        if owners
            .get(key)
            .is_some_and(|control| control.generation == generation)
        {
            owners.remove(key);
            true
        } else {
            false
        }
    }

    /// Signals all roles of one session to stop their transport.
    pub(super) async fn shutdown_session(&self, public_session_id: &str) {
        let prefix = format!("{public_session_id}:");
        let owners = self.owners.read().await;
        for (_, control) in owners.iter().filter(|(key, _)| key.starts_with(&prefix)) {
            let _ = control.shutdown.try_send(());
        }
    }

    /// Drops both roles of each named session, closing their shutdown channels.
    /// Match the complete session ID so retiring one room preserves other rooms
    /// whose IDs share its prefix. Retiring the same room again is a no-op.
    pub(super) async fn remove_sessions(&self, removed: &[String]) {
        self.owners.write().await.retain(|key, _| {
            !removed.iter().any(|id| {
                key.strip_prefix(id.as_str())
                    .is_some_and(|rest| rest.starts_with(':'))
            })
        });
    }

    /// Copies liveness data for monitoring without exposing mutable registrations.
    pub(super) async fn snapshots(&self) -> HashMap<String, ConnectionLivenessSnapshot> {
        self.owners
            .read()
            .await
            .iter()
            .map(|(key, control)| (key.clone(), control.liveness.snapshot()))
            .collect()
    }

    /// Counts active owners of this transport.
    pub(super) async fn count(&self) -> usize {
        self.owners.read().await.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A replaced socket cannot unregister its replacement or cancel a neighboring session.
    #[tokio::test]
    async fn replacement_retains_its_generation_and_session_scope() {
        let registry = ConnectionRegistry::default();
        let (old, mut old_rx, _) = registry
            .register("room:A".into(), Arc::new(ConnectionLiveness::new()))
            .await;
        let (current, mut current_rx, replaced) = registry
            .register("room:A".into(), Arc::new(ConnectionLiveness::new()))
            .await;
        let (_, mut other_rx, _) = registry
            .register("room2:A".into(), Arc::new(ConnectionLiveness::new()))
            .await;
        assert!(replaced);
        assert_eq!(old_rx.try_recv(), Ok(()));
        assert!(!registry.unregister("room:A", &old).await);
        registry.shutdown_session("room").await;
        assert_eq!(current_rx.try_recv(), Ok(()));
        assert_eq!(other_rx.try_recv(), Err(mpsc::error::TryRecvError::Empty));
        assert!(registry.unregister("room:A", &current).await);
        assert_eq!(registry.count().await, 1);
    }
    /// Batch retirement drops both seats and keeps the liveness and owner of a similarly named room.
    #[tokio::test]
    async fn retiring_sessions_releases_only_their_transports() {
        let registry = ConnectionRegistry::default();
        let mut receivers = Vec::new();
        for key in ["room:A", "room:B", "elsewhere:B", "room2:A"] {
            let (_, receiver, replaced) = registry
                .register(key.into(), Arc::new(ConnectionLiveness::new()))
                .await;
            assert!(!replaced);
            receivers.push(receiver);
        }
        registry
            .remove_sessions(&["room".into(), "elsewhere".into()])
            .await;
        for receiver in &mut receivers[..3] {
            assert_eq!(
                receiver.try_recv(),
                Err(mpsc::error::TryRecvError::Disconnected)
            );
        }
        assert_eq!(
            receivers[3].try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        );
        let snapshots = registry.snapshots().await;
        assert_eq!(snapshots.len(), 1);
        assert!(snapshots.contains_key("room2:A"));
    }
}
