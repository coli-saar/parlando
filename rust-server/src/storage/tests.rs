//! Shared SQLite fixtures for storage contract tests.

mod configuration;
mod export;
mod participants;
mod queries;
mod schema;
mod transitions;

use crate::protocol::SessionEndCause;
use tempfile::tempdir;

use super::*;
use crate::protocol::{ParticipantResult, SessionEnd};
use serde_json::{json, Value};
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::sync::Arc;

/// Builds a compact terminal value for storage-boundary tests.
fn test_session_end(cause: SessionEndCause, completion: Option<Value>) -> SessionEnd {
    SessionEnd {
        cause,
        completion,
        participant_results: HashMap::new(),
    }
}

/// Reads the raw rows through the production experiment snapshot for serialization assertions.
async fn experiment_rows(store: &SqliteExperimentStore, experiment_id: &str) -> Value {
    serde_json::to_value(store.export_snapshot(experiment_id).await.unwrap()).unwrap()
}

/// Creates one running session for storage-level lifecycle and logging tests.
async fn running_test_session(store: &SqliteExperimentStore, experiment_id: &str) -> i64 {
    store
        .create_experiment(ExperimentRecord {
            experiment_id: experiment_id.to_string(),
            game_version: "1.0.0".to_string(),
            config: json!({}),
            server_version: None,
            version_manifest: None,
            status: "inactive".to_string(),
            notes: None,
        })
        .await
        .unwrap();
    store
        .create_session(SessionRecord {
            experiment_id: experiment_id.to_string(),
            config_revision: 1,
            game_version: "1.0.0".to_string(),
            public_session_id: format!("{experiment_id}-public"),
            mode: "direct".to_string(),
            lifecycle: "running".to_string(),
            purpose: "research".to_string(),
            waiting_timeout_seconds: 600,
            maximum_lifetime_seconds: 14_400,
        })
        .await
        .unwrap()
}
