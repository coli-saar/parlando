use std::{
    error::Error,
    fmt,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

use serde_json::json;
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
};

use crate::{
    game::PlayerRole,
    storage::{SessionEventRecord, SharedExperimentStore},
};

/// Maximum UTF-8 byte length accepted for one session log entry.
pub const MAX_SESSION_LOG_ENTRY_BYTES: usize = 64 * 1024;
/// Maximum UTF-8 bytes accepted from one session during its process lifetime.
pub const MAX_SESSION_LOG_BYTES: usize = 8 * 1024 * 1024;
/// Maximum number of accepted entries waiting for the asynchronous storage worker.
pub const SESSION_LOG_QUEUE_CAPACITY: usize = 256;

/// Failure returned when a session logger cannot accept another message.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionLogError {
    /// The message exceeds the per-entry UTF-8 byte limit.
    EntryTooLarge,
    /// The session has exhausted its total log byte allowance.
    SessionLimitExceeded,
    /// The bounded asynchronous persistence queue is currently full.
    QueueFull,
    /// The session log sink has already closed.
    Closed,
}

impl fmt::Display for SessionLogError {
    /// Formats a stable diagnostic without including rejected log contents.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::EntryTooLarge => "session log entry exceeds the byte limit",
            Self::SessionLimitExceeded => "session log byte limit is exhausted",
            Self::QueueFull => "session log persistence queue is full",
            Self::Closed => "session log sink is closed",
        })
    }
}

impl Error for SessionLogError {}

/// Runtime-owned attribution attached to a log entry.
#[derive(Clone)]
enum SessionLogSource {
    Game,
    Agent {
        participant_id: i64,
        role: PlayerRole,
    },
}

/// One accepted message waiting for its storage backend.
struct PendingSessionLog {
    source: SessionLogSource,
    text: String,
}

/// Shared bounded sink state used by every scoped handle for one session.
struct SessionLoggerInner {
    sender: Option<mpsc::Sender<PendingSessionLog>>,
    accepted_bytes: AtomicUsize,
}

/// Cloneable capability for writing arbitrary text to one execution session.
///
/// A successful [`SessionLogger::log`] call means that the bounded sink accepted
/// the entry. Live SQLite persistence happens asynchronously, so success does not
/// guarantee that the entry survives an immediate process failure. The runtime
/// assigns the session and source; callers supply only the text.
#[derive(Clone)]
pub struct SessionLogger {
    inner: Arc<SessionLoggerInner>,
    source: SessionLogSource,
}

/// Execution-owned worker which drains and closes one live session log.
pub(crate) struct SessionLogWriter {
    shutdown: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
}

impl fmt::Debug for SessionLogger {
    /// Omits the session sink and attribution from diagnostic formatting.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SessionLogger")
            .field("binding", &"[SESSION-BOUND]")
            .finish()
    }
}

impl SessionLogWriter {
    /// Stops accepting entries, drains the queue, and waits for persistence to finish.
    pub(crate) async fn shutdown(mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        let mut task = self.task;
        if tokio::time::timeout(std::time::Duration::from_secs(5), &mut task)
            .await
            .is_err()
        {
            task.abort();
            tracing::warn!("session log writer did not drain within five seconds");
        }
    }
}

impl SessionLogger {
    /// Creates a logger which validates limits and then deliberately discards entries.
    ///
    /// This constructor is compiled only for Parlando's test-support surface. Production
    /// game and agent values receive runtime-created, session-bound handles.
    #[cfg(any(test, feature = "internal-tools"))]
    pub fn testing() -> Self {
        Self {
            inner: Arc::new(SessionLoggerInner {
                sender: None,
                accepted_bytes: AtomicUsize::new(0),
            }),
            source: SessionLogSource::Game,
        }
    }

    /// Enqueues one arbitrary UTF-8 string for this logger's session and source.
    ///
    /// Contents have no schema, but resource consumption is bounded. The method is
    /// synchronous so it can be called from every `Game` method without performing
    /// database I/O or blocking on storage.
    pub fn log(&self, message: impl Into<String>) -> Result<(), SessionLogError> {
        let text = message.into();
        let bytes = text.len();
        if bytes > MAX_SESSION_LOG_ENTRY_BYTES {
            return Err(SessionLogError::EntryTooLarge);
        }
        let accepted = self
            .inner
            .accepted_bytes
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                current
                    .checked_add(bytes)
                    .filter(|next| *next <= MAX_SESSION_LOG_BYTES)
            })
            .map_err(|_| SessionLogError::SessionLimitExceeded)?;
        let Some(sender) = self.inner.sender.as_ref() else {
            let _ = accepted;
            return Ok(());
        };
        match sender.try_send(PendingSessionLog {
            source: self.source.clone(),
            text,
        }) {
            Ok(()) => Ok(()),
            Err(mpsc::error::TrySendError::Full(pending)) => {
                self.inner
                    .accepted_bytes
                    .fetch_sub(pending.text.len(), Ordering::AcqRel);
                Err(SessionLogError::QueueFull)
            }
            Err(mpsc::error::TrySendError::Closed(pending)) => {
                self.inner
                    .accepted_bytes
                    .fetch_sub(pending.text.len(), Ordering::AcqRel);
                Err(SessionLogError::Closed)
            }
        }
    }

    /// Creates the game-scoped handle and asynchronous SQLite writer for one live session.
    pub(crate) fn live(
        store: SharedExperimentStore,
        experiment_id: String,
        session_id: i64,
    ) -> (Self, SessionLogWriter) {
        let (sender, mut receiver) = mpsc::channel::<PendingSessionLog>(SESSION_LOG_QUEUE_CAPACITY);
        let (shutdown_sender, mut shutdown_receiver) = oneshot::channel();
        let task = tokio::spawn(async move {
            let mut closing = false;
            loop {
                let entry = if closing {
                    receiver.recv().await
                } else {
                    tokio::select! {
                        _ = &mut shutdown_receiver => {
                            receiver.close();
                            closing = true;
                            continue;
                        }
                        entry = receiver.recv() => entry,
                    }
                };
                let Some(entry) = entry else { break };
                let (actor_participant_id, actor_role, source) = match entry.source {
                    SessionLogSource::Game => (None, None, "game"),
                    SessionLogSource::Agent {
                        participant_id,
                        role,
                    } => (
                        Some(participant_id),
                        Some(role.as_str().to_string()),
                        "agent",
                    ),
                };
                if let Err(error) = store
                    .append_session_event(SessionEventRecord {
                        experiment_id: experiment_id.clone(),
                        session_id,
                        event_type: "log".to_string(),
                        actor_participant_id,
                        actor_role,
                        payload: json!({"source": source, "text": entry.text}),
                        game_state: None,
                    })
                    .await
                {
                    tracing::warn!(%error, session_id, "failed to persist session log");
                }
            }
        });
        let logger = Self {
            inner: Arc::new(SessionLoggerInner {
                sender: Some(sender),
                accepted_bytes: AtomicUsize::new(0),
            }),
            source: SessionLogSource::Game,
        };
        (
            logger,
            SessionLogWriter {
                shutdown: Some(shutdown_sender),
                task,
            },
        )
    }

    /// Derives an agent-attributed handle while retaining the same session sink.
    pub(crate) fn for_agent(&self, participant_id: i64, role: PlayerRole) -> Self {
        Self {
            inner: self.inner.clone(),
            source: SessionLogSource::Agent {
                participant_id,
                role,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Confirms content is unconstrained while byte and session limits remain enforced.
    #[test]
    fn testing_logger_accepts_arbitrary_text_within_limits() {
        let logger = SessionLogger::testing();
        assert_eq!(logger.log("nul:\0 unicode: 🌳\njson: {not-json}"), Ok(()));
        assert_eq!(
            logger.log("x".repeat(MAX_SESSION_LOG_ENTRY_BYTES + 1)),
            Err(SessionLogError::EntryTooLarge)
        );
    }

    /// Confirms cloned handles share one session-wide byte allowance.
    #[test]
    fn cloned_loggers_share_the_session_limit() {
        let logger = SessionLogger::testing();
        let clone = logger.clone();
        for _ in 0..(MAX_SESSION_LOG_BYTES / MAX_SESSION_LOG_ENTRY_BYTES) {
            logger.log("x".repeat(MAX_SESSION_LOG_ENTRY_BYTES)).unwrap();
        }
        assert_eq!(
            clone.log("one more byte"),
            Err(SessionLogError::SessionLimitExceeded)
        );
    }
}
