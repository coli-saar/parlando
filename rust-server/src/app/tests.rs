use anyhow::{Context as _, Result};
use async_trait::async_trait;
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use futures_util::{SinkExt as _, StreamExt as _};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    fs,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
};
use tokio::net::TcpListener;
use tokio::sync::Notify;
use tokio_tungstenite::{connect_async, tungstenite::Message as TungsteniteMessage};
use tower::ServiceExt;

use crate::agents::{Agent, AgentContext, AgentIdentity, AgentResponse};
use crate::config::{
    AgentsConfig, AgentsMode, ConsentItemConfig, DatabaseConfig, DirectConfig,
    ExperimentIdentityConfig,
};
use crate::game::{AgentConfigField, AgentConfigValue, AgentDefinition, PlayerRole, SecretPurpose};

use super::*;

mod test_server;
use test_server::{build_router, *};
mod game_fixtures;
use game_fixtures::*;
mod agent_fixtures;
use agent_fixtures::*;
mod agent_observers;
use agent_observers::*;
mod voice_fixtures;
use voice_fixtures::*;
mod participants;
use participants::*;
mod sockets;
use sockets::*;

// Creates one human-vs-agent waiting room and returns the human and room ids.
async fn create_human_vs_agent_room(router: TestRouter, name: &str) -> (String, String) {
    let human = create_direct_participant(router.clone(), name).await;
    consent_participant(router.clone(), &human).await;
    let (status, created) = participant_json_request(
        router,
        &human,
        http::Method::POST,
        "/api/sessions",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(created["participant_state"]["role"], "A");
    (
        human,
        created["participant_state"]["public_session_id"]
            .as_str()
            .unwrap()
            .to_string(),
    )
}

// Polls the evaluation export until an event type appears or the test times out.
async fn wait_for_export_event(router: TestRouter, event_type: &str) -> Value {
    let mut last_export = Value::Null;
    for _ in 0..20 {
        let (status, export) = admin_json_request(
            router.clone(),
            http::Method::GET,
            "/api/admin/export?variant=full",
            Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        if export["session_events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["event_type"] == event_type)
        {
            return export;
        }
        last_export = export;
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("timed out waiting for event type {event_type}; last export: {last_export}");
}

// Polls the evaluation export until a TTS diagnostic event appears.
async fn wait_for_tts_diagnostic(router: TestRouter, diagnostic_event: &str) -> Value {
    for _ in 0..30 {
        let (status, export) = admin_json_request(
            router.clone(),
            http::Method::GET,
            "/api/admin/export?variant=full",
            Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        if export["session_events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| {
                event["event_type"] == "tts_diagnostic"
                    && event["payload"]["event"] == diagnostic_event
            })
        {
            return export;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("timed out waiting for TTS diagnostic {diagnostic_event}");
}

async fn consent_participant(router: TestRouter, participant_session_id: &str) {
    let (status, _response) = participant_json_request(
        router,
        participant_session_id,
        http::Method::POST,
        "/api/consent",
        json!({

            "decisions": {"study": true}
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

/// Validates the real administrator response against the published corpus contract.
async fn assert_corpus_session(router: TestRouter, expected_status: &str) -> Value {
    let (status, corpus) =
        admin_json_request(router, http::Method::GET, "/api/admin/export", Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{corpus}");
    let schema = serde_json::from_str(CORPUS_EXPORT_SCHEMA_V1).unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    let errors = validator
        .iter_errors(&corpus)
        .map(|error| error.to_string())
        .collect::<Vec<_>>();
    assert!(errors.is_empty(), "{errors:?}\n{corpus}");
    let session = &corpus["experiment"]["sessions"][0];
    assert_eq!(session["metadata"]["status"], expected_status);
    if expected_status == "playing" {
        assert!(session["metadata"]["duration_ms"].is_null());
    } else {
        assert!(session["metadata"]["duration_ms"].as_u64().is_some());
    }
    corpus
}

mod mounts;
mod session_transitions;
mod speech_timing;

mod access;
mod admin_dashboard;
mod admission;
mod agent_play;
mod agent_speech;
mod agent_startup;
mod audio;
mod configuration;
mod corpus;
mod experiment_lifecycle;
mod gameplay;
mod hosting;
mod privacy;
mod reconnection;
mod recruitment;
mod transcription;
