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
    collections::{BTreeMap, VecDeque},
    fs,
    sync::{
        atomic::{AtomicUsize, Ordering},
        LazyLock, Mutex,
    },
};
use tokio::net::TcpListener;
use tokio_tungstenite::{connect_async, tungstenite::Message as TungsteniteMessage};
use tower::ServiceExt;

use crate::agents::{AgentInitContext, AgentResponse, AgentUtteranceKind, GameAgent};
use crate::config::{
    AgentsConfig, AgentsMode, DatabaseConfig, DirectConfig, ExperimentIdentityConfig,
};
use crate::game::PlayerRole;

use super::*;

/// Participant credentials issued by test routers, indexed by their public session handles.
static TEST_PARTICIPANT_CREDENTIALS: LazyLock<Mutex<HashMap<String, String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Real administrator sessions issued by test routers.
static TEST_ADMIN_SESSIONS: LazyLock<Mutex<Vec<AdminTestSession>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));

/// Authentication material for one real administrator test session.
#[derive(Clone)]
struct AdminTestSession {
    cookie: String,
    csrf_token: String,
}

/// Confirms the coarse process ceiling tolerates a realistic crowdsourcing burst.
#[test]
fn participant_creation_rate_uses_only_a_high_process_ceiling() {
    let mut rate = ParticipantCreationRate::default();
    for _ in 0..300 {
        assert!(rate.record(1_000));
    }
    assert!(!rate.record(1_000));
    assert!(rate.record(1_061));
}

/// Confirms chat pacing permits realistic bursts and refills without limiting game actions.
#[test]
fn chat_submission_budget_allows_burst_then_refills() {
    let mut budget = TokenBucket::new(20.0, 1.0);
    for _ in 0..20 {
        assert!(budget.consume());
    }
    assert!(!budget.consume());
    budget.last_refill -= Duration::from_secs(1);
    assert!(budget.consume());
}

/// Confirms dashboard CIDRs match only direct peers inside a configured range.
#[test]
fn administrator_network_policy_matches_ipv4_and_ipv6_cidrs() {
    assert!(admin_network_allows(&[], None));
    let ranges = vec!["192.0.2.0/24".to_string(), "2001:db8::/32".to_string()];
    assert!(admin_network_allows(
        &ranges,
        Some("192.0.2.44".parse().unwrap())
    ));
    assert!(admin_network_allows(
        &ranges,
        Some("2001:db8::7".parse().unwrap())
    ));
    assert!(!admin_network_allows(
        &ranges,
        Some("198.51.100.4".parse().unwrap())
    ));
    assert!(!admin_network_allows(&ranges, None));
}

/// Builds an active test router so unrelated protocol tests retain their narrow setup.
async fn build_router<A: GameAdapter>(
    adapter: A,
    config: ExperimentConfig,
    options: ServeOptions<A>,
) -> Result<Router>
where
    A::State: Serialize,
{
    let router = super::build_router(adapter, config, options).await?;
    authenticate_test_admin(router.clone()).await?;
    let (status, _) = json_request(
        router.clone(),
        http::Method::POST,
        "/api/admin/experiment/status",
        json!({"status": "active"}),
    )
    .await;
    anyhow::ensure!(status == StatusCode::OK, "test activation failed");
    Ok(router)
}

/// Creates or signs in the fixed test administrator through public authentication routes.
async fn authenticate_test_admin(router: Router) -> Result<()> {
    let login_body = json!({
        "username": "unit-test-administrator",
        "password": "unit-test-password-long-enough"
    });
    let setup_response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(http::Method::POST)
                .uri("/api/admin/setup")
                .header(http::header::CONTENT_TYPE, "application/json")
                .body(Body::from(login_body.to_string()))
                .unwrap(),
        )
        .await?;
    let login_response = if setup_response.status() == StatusCode::CONFLICT {
        router
            .clone()
            .oneshot(
                Request::builder()
                    .method(http::Method::POST)
                    .uri("/api/admin/login")
                    .header(http::header::CONTENT_TYPE, "application/json")
                    .body(Body::from(login_body.to_string()))
                    .unwrap(),
            )
            .await?
    } else {
        setup_response
    };
    anyhow::ensure!(
        login_response.status() == StatusCode::OK,
        "test administrator authentication failed"
    );
    let cookie = login_response
        .headers()
        .get(SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .context("test administrator response omitted its session cookie")?
        .to_string();
    let body = to_bytes(login_response.into_body(), usize::MAX).await?;
    let response: Value = serde_json::from_slice(&body)?;
    let csrf_token = response["csrf_token"]
        .as_str()
        .context("test administrator response omitted its CSRF token")?
        .to_string();
    TEST_ADMIN_SESSIONS
        .lock()
        .unwrap()
        .push(AdminTestSession { cookie, csrf_token });
    Ok(())
}

fn admin_test_event(index: i64, event_type: &str, role: Option<&str>, payload: Value) -> Value {
    let stored = crate::storage::StoredSessionEvent {
        event_id: index,
        experiment_id: "experiment".to_string(),
        session_id: 1,
        event_index: index,
        event_type: event_type.to_string(),
        actor_participant_id: None,
        actor_role: role.map(str::to_string),
        payload,
        game_state: None,
        created_at: format!("2026-07-11T20:15:{index:02}.000000+00:00"),
    };
    admin_event_summary(stored)
}

#[test]
fn admin_dashboard_html_reflects_game_scoped_experiment_layout() {
    assert!(ADMIN_EXPERIMENT_HTML.contains("app-header"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("gameName"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("href=\"/admin/privacy\""));
    assert!(ADMIN_EXPERIMENT_HTML.contains("Activate experiment"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("Deactivate intake"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("/api/admin/runtime/"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("New experiment"));
    assert!(!ADMIN_EXPERIMENT_HTML.contains("experimentForm"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("id=\"configForm\""));
    assert!(ADMIN_EXPERIMENT_HTML.contains("configurationFromForm"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("id=\"institutionInput\""));
    assert!(ADMIN_EXPERIMENT_HTML.contains("Experiment Details"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("data-tab=\"sessions\""));
    assert!(ADMIN_EXPERIMENT_HTML.contains("data-tab=\"load\""));
    assert!(ADMIN_EXPERIMENT_HTML.contains("data-tab=\"export\""));
    assert!(ADMIN_EXPERIMENT_HTML.contains("data-tab=\"details\""));
    assert!(ADMIN_EXPERIMENT_HTML.contains("id=\"sessionsPanel\""));
    assert!(ADMIN_EXPERIMENT_HTML.contains("id=\"loadPanel\""));
    assert!(ADMIN_EXPERIMENT_HTML.contains("Runtime session liveness"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("renderLoadChart"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("id=\"exportPanel\""));
    assert!(ADMIN_EXPERIMENT_HTML.contains("id=\"exportVariant\""));
    assert!(ADMIN_EXPERIMENT_HTML.contains("Dialogue ID"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("Participant ID"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("Delete participant data"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("id=\"detailsPanel\""));
    assert!(ADMIN_EXPERIMENT_HTML.contains("id=\"sessionDetail\""));
    assert!(ADMIN_EXPERIMENT_HTML.contains("sessions-layout"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("session-picker"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("participant-collapse"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("participant-summary-list"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("session-participants"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("id=\"menuButton\""));
    assert!(ADMIN_EXPERIMENT_HTML.contains("sidebar-open"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("summary-line"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("events-header"));
    assert!(!ADMIN_EXPERIMENT_HTML.contains("refreshSessions"));
    assert!(!ADMIN_EXPERIMENT_HTML.contains(">Reproducibility<"));
    let sidebar = ADMIN_EXPERIMENT_HTML
        .split("id=\"experimentSidebar\"")
        .nth(1)
        .and_then(|html| html.split("</aside>").next())
        .unwrap();
    assert!(!sidebar.contains("<h2>Sessions</h2>"));
    let sessions_panel = ADMIN_EXPERIMENT_HTML
        .split("id=\"sessionsPanel\"")
        .nth(1)
        .and_then(|html| html.split("id=\"exportPanel\"").next())
        .unwrap();
    assert!(!sessions_panel.contains("<h2>Players</h2>"));
}

/// Confirms the protected load endpoint exposes bounded operational telemetry.
#[tokio::test]
async fn admin_load_exposes_capacity_counters_and_liveness_policy() {
    let (config, _tmp) = sqlite_config();
    let router = build_router(TinyAdapter, config, ServeOptions::default())
        .await
        .unwrap();
    let response = admin_raw_request(router, http::Method::GET, "/api/admin/load").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let load: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(load["sampling"]["interval_seconds"], 5);
    assert_eq!(load["sampling"]["retention_seconds"], 3_600);
    assert_eq!(load["liveness_thresholds_ms"]["live"], 5_000);
    assert_eq!(load["liveness_thresholds_ms"]["socket_timeout"], 90_000);
    assert_eq!(load["current"]["capacity"]["active_reserved_sessions"], 0);
    assert!(load["current"]["counters"]["requests_total"].is_number());
    assert!(load["history"].is_array());
    assert!(load["sessions"].is_array());
}

/// Confirms the protected privacy routes render one truthful installation-wide status.
#[tokio::test]
async fn admin_privacy_status_renders_and_downloads_current_facts() {
    let (config, _tmp) = sqlite_config();
    let router = build_router(TinyAdapter, config, ServeOptions::default())
        .await
        .unwrap();

    let page = admin_raw_request(router.clone(), http::Method::GET, "/admin/privacy").await;
    assert_eq!(page.status(), StatusCode::OK);
    let page_html = to_bytes(page.into_body(), usize::MAX).await.unwrap();
    let page_html = String::from_utf8_lossy(&page_html);
    assert!(page_html.contains("Installation-wide facts"));
    assert!(page_html.contains("Not yet bound to a completed DPO platform assessment"));
    assert!(page_html.contains("href=\"/admin/experiments\""));

    let (status, privacy) = json_request(
        router.clone(),
        http::Method::GET,
        "/api/admin/privacy",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(privacy["privacy_contract_version"], "1");
    assert_eq!(privacy["raw_audio_stored_by_parlando"], false);
    assert_eq!(privacy["exports"]["full_internal_export"], true);
    assert_eq!(privacy["exports"]["research_export"], true);
    assert_eq!(privacy["exports"]["corpus_export"], true);
    assert_eq!(privacy["participant_deletion"]["available"], true);
    assert_eq!(privacy["consent_evidence"]["available"], true);
    assert_eq!(privacy["external_services"], json!([]));

    let markdown = admin_raw_request(router, http::Method::GET, "/api/admin/privacy.md").await;
    assert_eq!(markdown.status(), StatusCode::OK);
    assert_eq!(
        markdown
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("text/markdown; charset=utf-8")
    );
    assert!(markdown.headers().contains_key(CONTENT_DISPOSITION));
    let markdown_body = to_bytes(markdown.into_body(), usize::MAX).await.unwrap();
    assert!(String::from_utf8_lossy(&markdown_body).contains("# Parlando privacy status"));
}

/// Confirms fixed public exports exclude direct identity while retaining readable ids.
#[test]
fn research_and_corpus_exports_apply_fixed_identity_boundaries() {
    let full = json!({
        "participants": [{
            "participant_id": 7,
            "research_id": "calm-blue-otter",
            "participant_kind": "human",
            "external_id": "recruitment-id"
        }],
        "experiment": {"experiment_id": "study", "game_version": "0.4.0", "config_revision": 2},
        "sessions": [{"session_id": 3, "dialogue_id": "softly-amber-harbor", "config_revision": 2, "game_version": "0.4.0", "mode": "direct", "status": "completed", "created_at": "2026-01-01T00:00:00Z"}],
        "session_participants": [{"session_id": 3, "participant_id": 7, "participant_session_id": "ps_secret", "role": "A"}],
        "session_events": [{
            "session_id": 3,
            "event_index": 4,
            "event_type": "conversation_message",
            "actor_participant_id": 7,
            "actor_role": "A",
            "payload": {"text": "hello", "origin": "typed", "sender_participant_session_id": "ps_secret"},
            "created_at": "2026-01-01T00:00:04Z"
        }]
    });
    let research = research_export(full, "1");
    let encoded = serde_json::to_string(&research).unwrap();
    assert!(!encoded.contains("recruitment-id"));
    assert!(!encoded.contains("ps_secret"));
    assert!(encoded.contains("calm-blue-otter"));
    assert!(encoded.contains("softly-amber-harbor"));
    assert_eq!(research["sessions"][0]["config_revision"], 2);
    assert_eq!(research["experiment"]["game_version"], "0.4.0");

    let corpus = corpus_export(research);
    let encoded = serde_json::to_string(&corpus).unwrap();
    assert_eq!(corpus["release_status"], "corpus_candidate");
    assert!(encoded.contains("calm-blue-otter"));
    assert!(encoded.contains("softly-amber-harbor"));
    assert!(!encoded.contains("2026-01-01"));
    assert_eq!(corpus["messages"][0]["text"], "hello");
    assert_eq!(corpus["messages"][0]["time_from_session_start_ms"], 4_000);
    assert_eq!(corpus["sessions"][0]["config_revision"], 2);
}

/// Confirms `/admin` reaches setup and the resulting login persists across router restarts.
#[tokio::test]
async fn admin_setup_page_creates_one_persistent_credential() {
    let (config, _tmp) = sqlite_config();
    let router = super::build_router(TinyAdapter, config.clone(), ServeOptions::default())
        .await
        .unwrap();

    let admin_entry = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/admin")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(admin_entry.status().is_redirection());
    assert_eq!(
        admin_entry
            .headers()
            .get(http::header::LOCATION)
            .and_then(|value| value.to_str().ok()),
        Some("/admin/login")
    );

    let setup_page = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/admin/login")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let setup_html = to_bytes(setup_page.into_body(), usize::MAX).await.unwrap();
    assert!(String::from_utf8_lossy(&setup_html).contains("Create administrator"));

    let setup_response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(http::Method::POST)
                .uri("/api/admin/setup")
                .header(http::header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"username": "researcher", "password": "a-long-test-password"})
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(setup_response.status(), StatusCode::OK);
    assert!(setup_response.headers().contains_key(SET_COOKIE));

    let (second_status, _) = json_request(
        router,
        http::Method::POST,
        "/api/admin/setup",
        json!({"username": "second", "password": "another-test-password"}),
    )
    .await;
    assert_eq!(second_status, StatusCode::CONFLICT);

    let restarted = super::build_router(TinyAdapter, config, ServeOptions::default())
        .await
        .unwrap();
    let login_page = restarted
        .clone()
        .oneshot(
            Request::builder()
                .uri("/admin/login")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let login_html = to_bytes(login_page.into_body(), usize::MAX).await.unwrap();
    assert!(String::from_utf8_lossy(&login_html).contains("Sign in"));
    let (login_status, _) = json_request(
        restarted,
        http::Method::POST,
        "/api/admin/login",
        json!({"username": "researcher", "password": "a-long-test-password"}),
    )
    .await;
    assert_eq!(login_status, StatusCode::OK);
}

/// Confirms startup is closed, activation opens intake, and restart closes it again.
#[tokio::test]
async fn experiment_starts_inactive_and_admin_controls_intake() {
    let (mut config, _tmp) = sqlite_config();
    config.experiment.id = Some("bootstrap".to_string());
    let router = super::build_router(TinyAdapter, config.clone(), ServeOptions::default())
        .await
        .unwrap();

    let (status, public) = json_request(
        router.clone(),
        http::Method::GET,
        "/api/config",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(public["experiment_status"], "inactive");
    let (status, _) = json_request(
        router.clone(),
        http::Method::POST,
        "/api/participants",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);

    authenticate_test_admin(router.clone()).await.unwrap();

    let (settings_status, _) = json_request(
        router.clone(),
        http::Method::POST,
        "/api/admin/game/settings",
        json!({"expected_revision": 1, "institution": "Test University", "admin_allowed_ip_ranges": []}),
    )
    .await;
    assert_eq!(settings_status, StatusCode::OK);
    let (status, activation) = json_request(
        router.clone(),
        http::Method::POST,
        "/api/admin/experiment/status",
        json!({"status": "active"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(activation["status"], "active");
    let (status, participant) = json_request(
        router.clone(),
        http::Method::POST,
        "/api/participants",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(participant["participant_session_id"].is_string());

    let (status, _) = json_request(
        router.clone(),
        http::Method::POST,
        "/api/admin/experiment/status",
        json!({"status": "inactive"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) =
        json_request(router, http::Method::POST, "/api/participants", json!({})).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);

    let restarted = super::build_router(TinyAdapter, config, ServeOptions::default())
        .await
        .unwrap();
    authenticate_test_admin(restarted.clone()).await.unwrap();
    let (status, experiment) = json_request(
        restarted,
        http::Method::GET,
        "/api/admin/experiment",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(experiment["experiment"]["status"], "inactive");
}

/// Confirms one compiled-game router can activate and isolate two experiment runtimes.
#[tokio::test]
async fn compiled_game_router_hosts_multiple_experiments() {
    let mut config = step_five_config();
    config.experiment.id = Some("primary".to_string());
    let descriptor = GameDescriptor {
        id: "tiny-game".to_string(),
        display_name: "Tiny Game".to_string(),
        version: semver::Version::parse("0.4.0").unwrap(),
        build_manifest: json!({"name": "tiny-game", "version": "0.4.0"}),
    };
    let router = build_game_router(TinyAdapter, config.clone(), descriptor.clone(), |_config| {
        Ok(ServeOptions::default())
    })
    .await
    .unwrap();

    let root = router
        .clone()
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert!(root.status().is_redirection());
    assert_eq!(
        root.headers()
            .get(http::header::LOCATION)
            .and_then(|value| value.to_str().ok()),
        Some("/admin/experiments")
    );
    let unauthenticated_dashboard = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/admin/experiments")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(unauthenticated_dashboard.status().is_redirection());
    assert_eq!(
        unauthenticated_dashboard
            .headers()
            .get(http::header::LOCATION)
            .and_then(|value| value.to_str().ok()),
        Some("/admin/login")
    );
    let unauthenticated_api = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/admin/experiments")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauthenticated_api.status(), StatusCode::UNAUTHORIZED);
    authenticate_test_admin(router.clone()).await.unwrap();

    let (settings_status, _) = json_request(
        router.clone(),
        http::Method::POST,
        "/api/admin/game/settings",
        json!({"expected_revision": 1, "institution": "Test University", "admin_allowed_ip_ranges": []}),
    )
    .await;
    assert_eq!(settings_status, StatusCode::OK);

    let (create_status, created) = json_request(
        router.clone(),
        http::Method::POST,
        "/api/admin/experiments",
        json!({"experiment_id": "secondary", "study_name": "Second condition"}),
    )
    .await;
    assert_eq!(create_status, StatusCode::OK);
    assert_eq!(created["game_version"], "0.4.0");
    let (config_status, mut stored_config) = json_request(
        router.clone(),
        http::Method::GET,
        "/api/admin/experiments/secondary/config",
        Value::Null,
    )
    .await;
    assert_eq!(config_status, StatusCode::OK);
    stored_config["experiment"]["config"]["study"]["name"] =
        Value::String("Edited second condition".to_string());
    let (save_status, saved) = json_request(
        router.clone(),
        http::Method::POST,
        "/api/admin/experiments/secondary/config",
        json!({
            "expected_revision": stored_config["experiment"]["config_revision"],
            "config": stored_config["experiment"]["config"],
            "change_summary": "Edited in the dashboard"
        }),
    )
    .await;
    assert_eq!(save_status, StatusCode::OK, "config save failed: {saved}");
    assert_eq!(saved["revision"], 2);

    for experiment_id in ["primary", "secondary"] {
        let (status, body) = json_request(
            router.clone(),
            http::Method::POST,
            &format!("/api/admin/runtime/{experiment_id}/experiment/status"),
            json!({"status": "active"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "activation failed: {body}");
    }

    let (_, primary_participant) = json_request(
        router.clone(),
        http::Method::POST,
        "/e/primary/api/participants",
        json!({}),
    )
    .await;
    let (_, secondary_participant) = json_request(
        router.clone(),
        http::Method::POST,
        "/e/secondary/api/participants",
        json!({}),
    )
    .await;
    assert_ne!(
        primary_participant["participant_session_id"],
        secondary_participant["participant_session_id"]
    );
    for participant in [&primary_participant, &secondary_participant] {
        TEST_PARTICIPANT_CREDENTIALS.lock().unwrap().insert(
            participant["participant_session_id"]
                .as_str()
                .unwrap()
                .to_string(),
            participant["participant_credential"]
                .as_str()
                .unwrap()
                .to_string(),
        );
    }
    let primary_id = primary_participant["participant_session_id"]
        .as_str()
        .unwrap();
    let (consent_status, _) = json_request(
        router.clone(),
        http::Method::POST,
        "/e/primary/api/consent",
        json!({"participant_session_id": primary_id, "decisions": {"study": true}}),
    )
    .await;
    assert_eq!(consent_status, StatusCode::OK);
    let (room_status, room) = json_request(
        router.clone(),
        http::Method::POST,
        "/e/primary/api/rooms",
        json!({"participant_session_id": primary_id}),
    )
    .await;
    assert_eq!(room_status, StatusCode::OK);
    let room_id = room["room_id"].as_str().unwrap();
    let (session_status, session) = json_request(
        router.clone(),
        http::Method::POST,
        &format!("/e/primary/api/rooms/{room_id}/game-session"),
        json!({"participant_session_id": primary_id}),
    )
    .await;
    assert_eq!(session_status, StatusCode::OK);
    assert_eq!(
        session["websocket_url"],
        format!("/e/primary/ws/game/{room_id}")
    );
    let (_, secondary_config) = json_request(
        router.clone(),
        http::Method::GET,
        "/e/secondary/api/config",
        Value::Null,
    )
    .await;
    assert_eq!(secondary_config["institution"], "Test University");
    assert_eq!(secondary_config["study_name"], "Edited second condition");

    let (_, catalogue) = json_request(
        router,
        http::Method::GET,
        "/api/admin/experiments",
        Value::Null,
    )
    .await;
    assert_eq!(catalogue["game"]["display_name"], "Tiny Game");
    assert_eq!(catalogue["experiments"].as_array().unwrap().len(), 2);
}

/// Confirms old game versions remain inspectable but must be cloned before activation.
#[tokio::test]
async fn compiled_game_router_requires_exact_version_and_clones_forward() {
    let (mut config, _tmp) = sqlite_config();
    config.experiment.id = Some("current".to_string());
    let store = experiment_store_from_url(&config.database.url)
        .await
        .unwrap();
    let mut old_config = config.clone();
    old_config.experiment.id = Some("historical".to_string());
    store
        .create_experiment(ExperimentRecord {
            experiment_id: "historical".to_string(),
            game_version: "0.3.0".to_string(),
            config: persistable_config_json(&old_config).unwrap(),
            server_version: Some("0.2.0".to_string()),
            version_manifest: Some(json!({"game": "tiny-game", "version": "0.3.0"})),
            status: "inactive".to_string(),
            notes: None,
        })
        .await
        .unwrap();
    let descriptor = GameDescriptor {
        id: "tiny-game".to_string(),
        display_name: "Tiny Game".to_string(),
        version: semver::Version::parse("0.4.0").unwrap(),
        build_manifest: json!({"name": "tiny-game", "version": "0.4.0"}),
    };
    let router = build_game_router(TinyAdapter, config.clone(), descriptor.clone(), |_config| {
        Ok(ServeOptions::default())
    })
    .await
    .unwrap();
    authenticate_test_admin(router.clone()).await.unwrap();

    let (activation_status, _) = json_request(
        router.clone(),
        http::Method::POST,
        "/api/admin/runtime/historical/experiment/status",
        json!({"status": "active"}),
    )
    .await;
    assert_eq!(activation_status, StatusCode::CONFLICT);

    let (clone_status, clone) = json_request(
        router.clone(),
        http::Method::POST,
        "/api/admin/experiments/historical/clone",
        json!({"experiment_id": "historical-on-0.4"}),
    )
    .await;
    assert_eq!(clone_status, StatusCode::OK, "clone failed: {clone}");
    assert_eq!(clone["game_version"], "0.4.0");
    let (activation_status, _) = json_request(
        router,
        http::Method::POST,
        "/api/admin/runtime/historical-on-0.4/experiment/status",
        json!({"status": "active"}),
    )
    .await;
    assert_eq!(activation_status, StatusCode::OK);

    let restarted = build_game_router(TinyAdapter, config, descriptor, |_config| {
        Ok(ServeOptions::default())
    })
    .await
    .unwrap();
    authenticate_test_admin(restarted.clone()).await.unwrap();
    let (catalogue_status, catalogue) = json_request(
        restarted,
        http::Method::GET,
        "/api/admin/experiments",
        Value::Null,
    )
    .await;
    assert_eq!(catalogue_status, StatusCode::OK);
    assert!(catalogue["experiments"]
        .as_array()
        .unwrap()
        .iter()
        .all(|experiment| experiment["status"] == "inactive"));
}

#[test]
fn admin_event_bundles_close_ready_before_later_disconnect() {
    let events = vec![
        admin_test_event(1, "session_created", None, json!({"room_id": "571EBA"})),
        admin_test_event(2, "participant_joined", Some("A"), json!({"role": "A"})),
        admin_test_event(3, "participant_joined", Some("B"), json!({"role": "B"})),
        admin_test_event(4, "participant_connected", Some("A"), Value::Null),
        admin_test_event(5, "ready", Some("A"), Value::Null),
        admin_test_event(
            8,
            "participant_connected",
            Some("B"),
            json!({"source": "agent"}),
        ),
        admin_test_event(133, "participant_disconnected", Some("A"), Value::Null),
    ];

    let bundles = admin_event_bundles(&events);

    let a_ready = bundles
        .iter()
        .find(|bundle| {
            bundle["kind"] == "participant" && bundle["role"] == "A" && bundle["first_index"] == 2
        })
        .unwrap();
    assert_eq!(a_ready["first_index"], 2);
    assert_eq!(a_ready["last_index"], 5);
    assert_eq!(a_ready["problem"], false);
    assert_eq!(a_ready["housekeeping"], true);

    let a_disconnect = bundles
        .iter()
        .find(|bundle| {
            bundle["kind"] == "participant" && bundle["role"] == "A" && bundle["first_index"] == 133
        })
        .unwrap();
    assert_eq!(a_disconnect["first_index"], 133);
    assert_eq!(a_disconnect["last_index"], 133);
    assert_eq!(a_disconnect["problem"], true);
    assert_eq!(a_disconnect["title"], "Participant");
}

#[test]
fn admin_event_bundles_merge_interleaved_action_events_by_action() {
    let action = json!({"type": "moveStep", "player": "B", "direction": "down"});
    let events = vec![
        admin_test_event(70, "agent_action", Some("B"), json!({"action": action})),
        admin_test_event(
            71,
            "game_action_accepted",
            Some("B"),
            json!({"action": {"type": "moveStep", "player": "B", "direction": "down"}}),
        ),
        admin_test_event(
            72,
            "conversation_message",
            Some("A"),
            json!({"origin": "voice_transcript", "sender_role": "A", "text": "Guten Abend."}),
        ),
    ];

    let bundles = admin_event_bundles(&events);

    let accepted = bundles
        .iter()
        .find(|bundle| bundle["title"] == "Action" && bundle["role"] == "B")
        .unwrap();
    assert_eq!(accepted["first_index"], 70);
    assert_eq!(accepted["last_index"], 71);
    assert_eq!(accepted["problem"], false);
    assert_eq!(accepted["housekeeping"], false);
    assert_eq!(accepted["action"]["type"], "moveStep");
    assert_eq!(accepted["action"]["direction"], "down");
}

#[test]
fn admin_event_bundles_hide_routine_voice_rows_but_keep_setup_status() {
    let events = vec![
        admin_test_event(
            6,
            "voice_diagnostic",
            Some("A"),
            json!({"event": "voice_connect_requested"}),
        ),
        admin_test_event(
            7,
            "voice_diagnostic",
            Some("A"),
            json!({"event": "stt_initialized"}),
        ),
        admin_test_event(
            10,
            "voice_diagnostic",
            Some("A"),
            json!({"event": "voice_token_received"}),
        ),
        admin_test_event(
            11,
            "voice_diagnostic",
            Some("A"),
            json!({"event": "transcription_stream_connecting"}),
        ),
        admin_test_event(
            15,
            "voice_diagnostic",
            Some("A"),
            json!({"event": "transcription_stream_started"}),
        ),
        admin_test_event(
            18,
            "voice_diagnostic",
            Some("A"),
            json!({"event": "local_track_published"}),
        ),
    ];

    let bundles = admin_event_bundles(&events);

    assert!(bundles.iter().all(|bundle| !bundle["steps"]
        .as_str()
        .unwrap_or("")
        .contains("voice_token_received")));
    assert!(bundles.iter().all(|bundle| !bundle["steps"]
        .as_str()
        .unwrap_or("")
        .contains("local_track_published")));
    let voice_bundles = bundles
        .iter()
        .filter(|bundle| bundle["kind"] == "voice" && bundle["role"] == "A")
        .collect::<Vec<_>>();
    assert_eq!(voice_bundles.len(), 1);
    let stream_bundle = voice_bundles[0];
    assert_eq!(stream_bundle["first_index"], 6);
    assert_eq!(stream_bundle["last_index"], 15);
    assert_eq!(stream_bundle["title"], "Voice");
    assert_eq!(stream_bundle["problem"], false);
    assert_eq!(stream_bundle["problem_reason"], Value::Null);
    assert_eq!(stream_bundle["housekeeping"], true);
}

#[test]
fn admin_event_bundles_keep_late_teardown_after_completion_non_problematic() {
    let events = vec![
        admin_test_event(131, "session_completed", Some("B"), json!({"done": true})),
        admin_test_event(133, "participant_disconnected", Some("A"), Value::Null),
        admin_test_event(
            134,
            "voice_diagnostic",
            Some("A"),
            json!({"event": "transcription_stream_disconnected"}),
        ),
        admin_test_event(
            135,
            "voice_diagnostic",
            Some("A"),
            json!({"event": "audio_transport_disconnected"}),
        ),
    ];

    let bundles = admin_event_bundles(&events);

    let participant_teardown = bundles
        .iter()
        .find(|bundle| bundle["kind"] == "participant" && bundle["first_index"] == 133)
        .unwrap();
    assert_eq!(participant_teardown["problem"], false);
    assert_eq!(participant_teardown["problem_reason"], Value::Null);

    let voice_teardown = bundles
        .iter()
        .find(|bundle| bundle["kind"] == "voice" && bundle["first_index"] == 134)
        .unwrap();
    assert_eq!(voice_teardown["problem"], false);
    assert_eq!(voice_teardown["problem_reason"], Value::Null);
}

#[test]
fn admin_event_bundles_do_not_flag_pending_agent_actions_after_disconnect() {
    let events = vec![
        admin_test_event(133, "participant_disconnected", Some("A"), Value::Null),
        admin_test_event(
            136,
            "agent_action",
            Some("B"),
            json!({"action": {"type": "moveStep", "player": "B", "direction": "down"}}),
        ),
        admin_test_event(
            137,
            "agent_action",
            Some("B"),
            json!({"action": {"type": "moveStep", "player": "B", "direction": "up"}}),
        ),
        admin_test_event(
            138,
            "agent_action",
            Some("B"),
            json!({"action": {"type": "moveStep", "player": "B", "direction": "down"}}),
        ),
    ];

    let bundles = admin_event_bundles(&events);

    let disconnect = bundles
        .iter()
        .find(|bundle| bundle["first_index"] == 133)
        .unwrap();
    assert_eq!(disconnect["problem"], true);

    let late_actions = bundles
        .iter()
        .filter(|bundle| bundle["kind"] == "action")
        .collect::<Vec<_>>();
    assert_eq!(late_actions.len(), 3);
    assert!(late_actions
        .iter()
        .all(|bundle| bundle["problem"] == false && bundle["problem_reason"] == Value::Null));
}

#[test]
fn admin_event_bundles_merge_transcript_storage_and_display_rows() {
    let events = vec![
        admin_test_event(
            73,
            "transcript_segment",
            Some("A"),
            json!({"player": "A", "text": "Guten Abend."}),
        ),
        admin_test_event(
            74,
            "conversation_message",
            Some("A"),
            json!({"origin": "voice_transcript", "sender_role": "A", "text": "Guten Abend"}),
        ),
    ];

    let bundles = admin_event_bundles(&events);

    assert_eq!(bundles.len(), 1);
    assert_eq!(bundles[0]["title"], "Voice Message");
    assert_eq!(bundles[0]["first_index"], 73);
    assert_eq!(bundles[0]["last_index"], 74);
    assert_eq!(bundles[0]["text"], "Guten Abend.");
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct TinyState {
    done: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
struct TinyAction {
    finish: bool,
    #[serde(default)]
    invalid: bool,
}

#[derive(Clone, Debug, Serialize)]
struct TinyObservation {
    done: bool,
    role: String,
}

#[derive(Clone, Debug, Serialize)]
struct TinyEvent {
    name: String,
}

#[derive(Clone, Debug, Serialize)]
struct TinySummary {
    done: bool,
    outcome: String,
    dyad_score: i64,
    player_scores: BTreeMap<String, i64>,
}

#[derive(Clone)]
struct TinyAdapter;

#[derive(Clone)]
struct NoAvailableActionsAdapter;

#[derive(Clone)]
struct LossSummaryAdapter;

struct NoopAgent;

#[async_trait]
impl GameAgent<TinyAdapter> for NoopAgent {
    async fn maybe_act(
        &mut self,
        _available_actions: Option<Vec<TinyAction>>,
    ) -> Result<Option<AgentResponse<TinyAction>>> {
        Ok(None)
    }
}

struct NoopAgentFactory;

impl AgentFactory<TinyAdapter> for NoopAgentFactory {
    fn create(&self, _context: AgentInitContext) -> Result<Box<dyn GameAgent<TinyAdapter> + Send>> {
        Ok(Box::new(NoopAgent))
    }
}

struct ScriptedAgent {
    script: VecDeque<Option<AgentResponse<TinyAction>>>,
}

#[async_trait]
impl GameAgent<TinyAdapter> for ScriptedAgent {
    async fn maybe_act(
        &mut self,
        _available_actions: Option<Vec<TinyAction>>,
    ) -> Result<Option<AgentResponse<TinyAction>>> {
        Ok(self.script.pop_front().unwrap_or(None))
    }
}

struct ScriptedAgentFactory {
    created: AtomicUsize,
    scripts: Mutex<VecDeque<Vec<Option<AgentResponse<TinyAction>>>>>,
}

impl ScriptedAgentFactory {
    // Creates a factory that hands one script to each fresh agent instance.
    fn new(scripts: Vec<Vec<Option<AgentResponse<TinyAction>>>>) -> Self {
        Self {
            created: AtomicUsize::new(0),
            scripts: Mutex::new(scripts.into()),
        }
    }

    // Returns how many agent instances were created by the server runtime.
    fn created_count(&self) -> usize {
        self.created.load(Ordering::SeqCst)
    }
}

// Creates a scripted optional response.
fn scripted_response(
    message: Option<&str>,
    action: Option<TinyAction>,
) -> Option<AgentResponse<TinyAction>> {
    Some(AgentResponse {
        message: message.map(str::to_string),
        action,
    })
}

impl AgentFactory<TinyAdapter> for ScriptedAgentFactory {
    fn create(&self, _context: AgentInitContext) -> Result<Box<dyn GameAgent<TinyAdapter> + Send>> {
        self.created.fetch_add(1, Ordering::SeqCst);
        let script = self.scripts.lock().unwrap().pop_front().unwrap_or_default();
        Ok(Box::new(ScriptedAgent {
            script: script.into(),
        }))
    }
}

struct RecordingActionsAgent {
    seen_actions: Arc<Mutex<Vec<Option<Vec<TinyAction>>>>>,
}

#[async_trait]
impl GameAgent<TinyAdapter> for RecordingActionsAgent {
    async fn maybe_act(
        &mut self,
        available_actions: Option<Vec<TinyAction>>,
    ) -> Result<Option<AgentResponse<TinyAction>>> {
        self.seen_actions.lock().unwrap().push(available_actions);
        Ok(None)
    }
}

struct RecordingActionsAgentFactory {
    seen_actions: Arc<Mutex<Vec<Option<Vec<TinyAction>>>>>,
}

impl AgentFactory<TinyAdapter> for RecordingActionsAgentFactory {
    fn create(&self, _context: AgentInitContext) -> Result<Box<dyn GameAgent<TinyAdapter> + Send>> {
        Ok(Box::new(RecordingActionsAgent {
            seen_actions: self.seen_actions.clone(),
        }))
    }
}

struct RecordingObservationsAgent {
    observations: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl GameAgent<TinyAdapter> for RecordingObservationsAgent {
    async fn observe_state(&mut self, current_observation: TinyObservation) -> Result<()> {
        self.observations.lock().unwrap().push(format!(
            "state:{}:{}",
            current_observation.role, current_observation.done
        ));
        Ok(())
    }

    async fn observe_action(
        &mut self,
        actor: PlayerRole,
        action: TinyAction,
        resulting_observation: TinyObservation,
    ) -> Result<()> {
        self.observations.lock().unwrap().push(format!(
            "action:{}:{}:{}",
            actor.as_str(),
            action.finish,
            resulting_observation.done
        ));
        Ok(())
    }

    async fn observe_message(
        &mut self,
        speaker: PlayerRole,
        kind: AgentUtteranceKind,
        text: String,
    ) -> Result<()> {
        self.observations
            .lock()
            .unwrap()
            .push(format!("message:{}:{kind:?}:{text}", speaker.as_str()));
        Ok(())
    }

    async fn maybe_act(
        &mut self,
        _available_actions: Option<Vec<TinyAction>>,
    ) -> Result<Option<AgentResponse<TinyAction>>> {
        Ok(None)
    }
}

struct RecordingObservationsAgentFactory {
    observations: Arc<Mutex<Vec<String>>>,
}

impl AgentFactory<TinyAdapter> for RecordingObservationsAgentFactory {
    fn create(&self, _context: AgentInitContext) -> Result<Box<dyn GameAgent<TinyAdapter> + Send>> {
        Ok(Box::new(RecordingObservationsAgent {
            observations: self.observations.clone(),
        }))
    }
}

struct SequencedDecisionAgent {
    log: Arc<Mutex<Vec<String>>>,
    decisions: usize,
}

#[async_trait]
impl GameAgent<TinyAdapter> for SequencedDecisionAgent {
    async fn observe_action(
        &mut self,
        actor: PlayerRole,
        _action: TinyAction,
        _resulting_observation: TinyObservation,
    ) -> Result<()> {
        self.log
            .lock()
            .unwrap()
            .push(format!("observe_action:{}", actor.as_str()));
        Ok(())
    }

    async fn maybe_act(
        &mut self,
        _available_actions: Option<Vec<TinyAction>>,
    ) -> Result<Option<AgentResponse<TinyAction>>> {
        self.decisions += 1;
        self.log
            .lock()
            .unwrap()
            .push(format!("maybe_act:{}", self.decisions));
        if self.decisions == 1 {
            return Ok(scripted_response(
                None,
                Some(TinyAction {
                    finish: false,
                    invalid: false,
                }),
            ));
        }
        Ok(None)
    }
}

struct SequencedDecisionAgentFactory {
    log: Arc<Mutex<Vec<String>>>,
}

impl AgentFactory<TinyAdapter> for SequencedDecisionAgentFactory {
    fn create(&self, _context: AgentInitContext) -> Result<Box<dyn GameAgent<TinyAdapter> + Send>> {
        Ok(Box::new(SequencedDecisionAgent {
            log: self.log.clone(),
            decisions: 0,
        }))
    }
}

struct MockTtsProvider {
    calls: AtomicUsize,
    fail_first: bool,
}

struct MockAudioPublisher {
    calls: AtomicUsize,
}

#[async_trait]
impl StreamingTtsProvider for MockTtsProvider {
    async fn synthesize(
        &self,
        _text: &str,
        _message_id: &str,
    ) -> Result<Vec<crate::tts::AudioChunk>> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail_first && call == 0 {
            return Err(anyhow!("mock tts failure"));
        }
        Ok(vec![
            crate::tts::AudioChunk {
                data: vec![1, 2, 3],
                sample_rate: 24000,
                channels: 1,
                final_chunk: false,
            },
            crate::tts::AudioChunk {
                data: vec![],
                sample_rate: 24000,
                channels: 1,
                final_chunk: true,
            },
        ])
    }
}

#[async_trait]
impl crate::audio_publisher::AgentAudioPublisher for MockAudioPublisher {
    async fn publish(
        &self,
        _room_id: &str,
        _message_id: &str,
        chunks: &[crate::tts::AudioChunk],
    ) -> Result<crate::audio_publisher::AudioPublishSummary> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(crate::audio_publisher::AudioPublishSummary {
            chunks_published: chunks.iter().filter(|chunk| !chunk.data.is_empty()).count(),
            bytes_published: chunks.iter().map(|chunk| chunk.data.len()).sum(),
            sample_rate: 24000,
            channels: 1,
        })
    }
}

impl GameAdapter for TinyAdapter {
    type State = TinyState;
    type Action = TinyAction;
    type Observation = TinyObservation;
    type Event = TinyEvent;
    type Summary = TinySummary;

    fn initial_state(&self) -> Self::State {
        TinyState { done: false }
    }

    fn validate_action(
        &self,
        _state: &Self::State,
        action: &Self::Action,
        _player: PlayerRole,
    ) -> Result<()> {
        if action.invalid {
            return Err(anyhow!("invalid tiny action"));
        }
        Ok(())
    }

    fn apply_action(&self, _state: &Self::State, action: &Self::Action) -> Result<Self::State> {
        Ok(TinyState {
            done: action.finish,
        })
    }

    fn observe_state(&self, state: &Self::State, player: PlayerRole) -> Self::Observation {
        TinyObservation {
            done: state.done,
            role: player.as_str().to_string(),
        }
    }

    fn available_actions(
        &self,
        _state: &Self::State,
        _player: PlayerRole,
    ) -> Option<Vec<Self::Action>> {
        Some(vec![TinyAction {
            finish: true,
            invalid: false,
        }])
    }

    fn events_for_action(
        &self,
        _before: &Self::State,
        _after: &Self::State,
        _action: &Self::Action,
        _player: PlayerRole,
    ) -> Vec<Self::Event> {
        vec![TinyEvent {
            name: "acted".to_string(),
        }]
    }

    fn is_complete(&self, state: &Self::State) -> bool {
        state.done
    }

    fn completion_summary(&self, state: &Self::State) -> Self::Summary {
        TinySummary {
            done: state.done,
            outcome: if state.done { "success" } else { "in_progress" }.to_string(),
            dyad_score: if state.done { 10 } else { 0 },
            player_scores: BTreeMap::from([
                ("A".to_string(), if state.done { 6 } else { 0 }),
                ("B".to_string(), if state.done { 4 } else { 0 }),
            ]),
        }
    }
}

impl GameAdapter for NoAvailableActionsAdapter {
    type State = TinyState;
    type Action = TinyAction;
    type Observation = TinyObservation;
    type Event = TinyEvent;
    type Summary = TinySummary;

    fn initial_state(&self) -> Self::State {
        TinyAdapter.initial_state()
    }

    fn validate_action(
        &self,
        state: &Self::State,
        action: &Self::Action,
        player: PlayerRole,
    ) -> Result<()> {
        TinyAdapter.validate_action(state, action, player)
    }

    fn apply_action(&self, state: &Self::State, action: &Self::Action) -> Result<Self::State> {
        TinyAdapter.apply_action(state, action)
    }

    fn observe_state(&self, state: &Self::State, player: PlayerRole) -> Self::Observation {
        TinyAdapter.observe_state(state, player)
    }

    fn events_for_action(
        &self,
        before: &Self::State,
        after: &Self::State,
        action: &Self::Action,
        player: PlayerRole,
    ) -> Vec<Self::Event> {
        TinyAdapter.events_for_action(before, after, action, player)
    }

    fn is_complete(&self, state: &Self::State) -> bool {
        TinyAdapter.is_complete(state)
    }

    fn completion_summary(&self, state: &Self::State) -> Self::Summary {
        TinyAdapter.completion_summary(state)
    }
}

impl GameAdapter for LossSummaryAdapter {
    type State = TinyState;
    type Action = TinyAction;
    type Observation = TinyObservation;
    type Event = TinyEvent;
    type Summary = TinySummary;

    fn initial_state(&self) -> Self::State {
        TinyAdapter.initial_state()
    }

    fn validate_action(
        &self,
        state: &Self::State,
        action: &Self::Action,
        player: PlayerRole,
    ) -> Result<()> {
        TinyAdapter.validate_action(state, action, player)
    }

    fn apply_action(&self, state: &Self::State, action: &Self::Action) -> Result<Self::State> {
        TinyAdapter.apply_action(state, action)
    }

    fn observe_state(&self, state: &Self::State, player: PlayerRole) -> Self::Observation {
        TinyAdapter.observe_state(state, player)
    }

    fn available_actions(
        &self,
        state: &Self::State,
        player: PlayerRole,
    ) -> Option<Vec<Self::Action>> {
        TinyAdapter.available_actions(state, player)
    }

    fn events_for_action(
        &self,
        before: &Self::State,
        after: &Self::State,
        action: &Self::Action,
        player: PlayerRole,
    ) -> Vec<Self::Event> {
        TinyAdapter.events_for_action(before, after, action, player)
    }

    fn is_complete(&self, state: &Self::State) -> bool {
        TinyAdapter.is_complete(state)
    }

    fn completion_summary(&self, state: &Self::State) -> Self::Summary {
        let mut summary = TinyAdapter.completion_summary(state);
        if state.done {
            summary.outcome = "loss".to_string();
            summary.dyad_score = 0;
            summary
                .player_scores
                .extend([("A".to_string(), 0), ("B".to_string(), 0)]);
        }
        summary
    }
}

#[tokio::test]
async fn reusable_router_builds_with_typed_adapter() {
    let mut config = ExperimentConfig::default();
    config.database.url = "sqlite:///:memory:".to_string();
    let _router = build_router(
        TinyAdapter,
        config,
        ServeOptions {
            agent_factory: None,
            ..ServeOptions::default()
        },
    )
    .await
    .expect("router builds with a typed adapter");
}

fn step_five_config() -> ExperimentConfig {
    ExperimentConfig {
        experiment: ExperimentIdentityConfig {
            id: Some("step5".to_string()),
        },
        database: DatabaseConfig {
            url: "sqlite:///:memory:".to_string(),
        },
        direct: DirectConfig {
            enabled: true,
            participant_information_version: "test-v1".to_string(),
            participant_information_url: "https://example.test/privacy".to_string(),
            consents: vec![crate::config::ConsentItemConfig {
                id: "study".to_string(),
                title: "Study".to_string(),
                body: "Agree?".to_string(),
                required: true,
            }],
        },
        ..ExperimentConfig::default()
    }
}

fn sqlite_config() -> (ExperimentConfig, tempfile::TempDir) {
    let temp = tempfile::tempdir().unwrap();
    let mut config = step_five_config();
    config.database.url = format!(
        "sqlite:///{}",
        temp.path().join("server-core.sqlite").display()
    );
    (config, temp)
}

fn voice_enabled_config() -> ExperimentConfig {
    let mut config = step_five_config();
    config.voice.enabled = true;
    config
}

fn human_vs_agent_config() -> ExperimentConfig {
    let mut config = step_five_config();
    config.agents = AgentsConfig {
        mode: AgentsMode::HumanVsAgent,
        human_vs_agent: Some(crate::config::HumanVsAgentConfig {
            act_timeout_seconds: 1.0,
            invalid_action_limit: 2,
            ..Default::default()
        }),
    };
    config
}

async fn json_request(
    router: Router,
    method: http::Method,
    path: &str,
    mut body: Value,
) -> (StatusCode, Value) {
    let participant_session_id = body.as_object_mut().and_then(|object| {
        object.remove("mode");
        object.remove("participant_session_id")
    });
    let participant_credential = participant_session_id
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .and_then(|participant_session_id| {
            TEST_PARTICIPANT_CREDENTIALS
                .lock()
                .unwrap()
                .get(&participant_session_id)
                .cloned()
        });
    let admin_sessions = if path.starts_with("/api/admin/")
        && path != "/api/admin/setup"
        && path != "/api/admin/login"
    {
        TEST_ADMIN_SESSIONS.lock().unwrap().clone()
    } else {
        Vec::new()
    };
    let mut response = None;
    for admin_session in admin_sessions
        .iter()
        .rev()
        .map(Some)
        .chain(std::iter::once(None))
    {
        let mut request = Request::builder()
            .method(method.clone())
            .uri(path)
            .header(http::header::CONTENT_TYPE, "application/json");
        if let Some(credential) = &participant_credential {
            request = request.header(AUTHORIZATION, format!("Bearer {credential}"));
        }
        if let Some(admin_session) = admin_session {
            request = request
                .header(http::header::COOKIE, &admin_session.cookie)
                .header("x-csrf-token", &admin_session.csrf_token);
        }
        let candidate = router
            .clone()
            .oneshot(request.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap();
        if candidate.status() != StatusCode::UNAUTHORIZED || admin_session.is_none() {
            response = Some(candidate);
            break;
        }
    }
    let response = response.expect("test request produced no response");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| json!({"raw": String::from_utf8_lossy(&bytes).to_string()}))
    };
    (status, value)
}

// Sends a request and returns the raw response body for static-file assertions.
async fn raw_request(
    router: Router,
    method: http::Method,
    path: &str,
    body: Body,
) -> (StatusCode, String, Option<String>) {
    let response = router
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .body(body)
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let content_type = response
        .headers()
        .get(http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (
        status,
        String::from_utf8_lossy(&bytes).to_string(),
        content_type,
    )
}

/// Sends a protected administrator request with a real server-issued cookie and CSRF token.
async fn admin_raw_request(router: Router, method: http::Method, path: &str) -> Response {
    let sessions = TEST_ADMIN_SESSIONS.lock().unwrap().clone();
    for session in sessions.iter().rev() {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(method.clone())
                    .uri(path)
                    .header(http::header::COOKIE, &session.cookie)
                    .header("x-csrf-token", &session.csrf_token)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        if response.status() != StatusCode::UNAUTHORIZED {
            return response;
        }
    }
    panic!("no administrator test session authenticated for {path}");
}

async fn create_direct_participant(router: Router, _name: &str) -> String {
    let (status, response) =
        json_request(router, http::Method::POST, "/api/participants", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let participant_session_id = response["participant_session_id"]
        .as_str()
        .unwrap()
        .to_string();
    let credential = response["participant_credential"]
        .as_str()
        .unwrap()
        .to_string();
    TEST_PARTICIPANT_CREDENTIALS
        .lock()
        .unwrap()
        .insert(participant_session_id.clone(), credential);
    participant_session_id
}

/// Mints a real one-use game ticket and returns a URL for the local test server.
async fn game_socket_url(base_url: &str, room_id: &str, participant_session_id: &str) -> String {
    let credential = TEST_PARTICIPANT_CREDENTIALS
        .lock()
        .unwrap()
        .get(participant_session_id)
        .cloned()
        .expect("test participant credential was not recorded");
    let response = reqwest::Client::new()
        .post(format!("{base_url}/api/rooms/{room_id}/game-session"))
        .bearer_auth(credential)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let plan: Value = response.json().await.unwrap();
    let token = plan["token"].as_str().unwrap();
    let host = base_url.trim_start_matches("http://");
    format!("ws://{host}/ws/game/{room_id}?token={token}")
}

// Starts a real local HTTP server so tests exercise the WebSocket upgrade path.
async fn spawn_test_server(router: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (format!("http://{addr}"), handle)
}

// Reads WebSocket messages until the requested server-message type appears.
async fn read_ws_type<S>(socket: &mut S, message_type: &str) -> Value
where
    S: futures_util::Stream<
            Item = Result<TungsteniteMessage, tokio_tungstenite::tungstenite::Error>,
        > + Unpin,
{
    let deadline = Duration::from_secs(2);
    loop {
        let message = tokio::time::timeout(deadline, socket.next())
            .await
            .expect("timed out waiting for WebSocket message")
            .expect("WebSocket closed before expected message")
            .expect("WebSocket read failed");
        let TungsteniteMessage::Text(text) = message else {
            continue;
        };
        let value: Value = serde_json::from_str(&text).unwrap();
        if value["type"] == message_type {
            return value;
        }
    }
}

// Reads the next JSON WebSocket server message without filtering by type.
async fn read_next_ws_value<S>(socket: &mut S) -> Value
where
    S: futures_util::Stream<
            Item = Result<TungsteniteMessage, tokio_tungstenite::tungstenite::Error>,
        > + Unpin,
{
    loop {
        let message = tokio::time::timeout(Duration::from_secs(2), socket.next())
            .await
            .expect("timed out waiting for WebSocket message")
            .expect("WebSocket closed before next message")
            .expect("WebSocket read failed");
        let TungsteniteMessage::Text(text) = message else {
            continue;
        };
        return serde_json::from_str(&text).unwrap();
    }
}

// Asserts that no message of the given type arrives within a short interval.
async fn assert_no_ws_type<S>(socket: &mut S, message_type: &str)
where
    S: futures_util::Stream<
            Item = Result<TungsteniteMessage, tokio_tungstenite::tungstenite::Error>,
        > + Unpin,
{
    let result = tokio::time::timeout(Duration::from_millis(200), async {
        loop {
            let Some(message) = socket.next().await else {
                return false;
            };
            let Ok(TungsteniteMessage::Text(text)) = message else {
                continue;
            };
            let value: Value = serde_json::from_str(&text).unwrap();
            if value["type"] == message_type {
                return true;
            }
        }
    })
    .await;
    assert!(
        !matches!(result, Ok(true)),
        "unexpected {message_type} WebSocket message"
    );
}

// Sends a JSON client message through a WebSocket connection.
async fn send_ws_json<S>(socket: &mut S, value: Value)
where
    S: futures_util::Sink<TungsteniteMessage, Error = tokio_tungstenite::tungstenite::Error>
        + Unpin,
{
    socket
        .send(TungsteniteMessage::Text(value.to_string()))
        .await
        .unwrap();
}

// Creates a two-player room and returns the participant sessions plus room id.
async fn create_joined_room(router: Router) -> (String, String, String) {
    let a = create_direct_participant(router.clone(), "A").await;
    let b = create_direct_participant(router.clone(), "B").await;
    consent_participant(router.clone(), &a).await;
    consent_participant(router.clone(), &b).await;
    let (_, created) = json_request(
        router.clone(),
        http::Method::POST,
        "/api/rooms",
        json!({"participant_session_id": a}),
    )
    .await;
    let room_id = created["room_id"].as_str().unwrap().to_string();
    let (_, joined) = json_request(
        router,
        http::Method::POST,
        "/api/rooms",
        json!({"participant_session_id": b}),
    )
    .await;
    assert_eq!(joined["room_id"], room_id);
    (
        created["participant_session_id"]
            .as_str()
            .unwrap()
            .to_string(),
        joined["participant_session_id"]
            .as_str()
            .unwrap()
            .to_string(),
        room_id,
    )
}

// Requests one participant-bound audio plan and verifies that it is enabled.
async fn request_audio_plan(router: Router, room_id: &str, participant_id: &str) -> Value {
    let (status, plan) = json_request(
        router,
        http::Method::POST,
        &format!("/api/rooms/{room_id}/audio-session"),
        json!({"participant_session_id": participant_id}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(plan["enabled"], true);
    plan
}

// Reads past control messages until the next binary audio frame arrives.
async fn read_audio_binary<S>(socket: &mut S) -> Vec<u8>
where
    S: futures_util::Stream<
            Item = Result<TungsteniteMessage, tokio_tungstenite::tungstenite::Error>,
        > + Unpin,
{
    loop {
        let message = tokio::time::timeout(Duration::from_secs(2), socket.next())
            .await
            .expect("timed out waiting for binary audio")
            .expect("audio WebSocket closed before binary audio")
            .expect("audio WebSocket read failed");
        if let TungsteniteMessage::Binary(bytes) = message {
            return bytes.to_vec();
        }
    }
}

// Waits until an audio socket reports its initial transcription state.
async fn wait_for_audio_control<S>(socket: &mut S)
where
    S: futures_util::Stream<
            Item = Result<TungsteniteMessage, tokio_tungstenite::tungstenite::Error>,
        > + Unpin,
{
    loop {
        let message = tokio::time::timeout(Duration::from_secs(2), socket.next())
            .await
            .expect("timed out waiting for audio control state")
            .expect("audio WebSocket closed before control state")
            .expect("audio WebSocket read failed");
        if matches!(message, TungsteniteMessage::Text(_)) {
            return;
        }
    }
}

// Verifies that no binary audio reaches a socket during a short isolation window.
async fn assert_no_audio_binary<S>(socket: &mut S)
where
    S: futures_util::Stream<
            Item = Result<TungsteniteMessage, tokio_tungstenite::tungstenite::Error>,
        > + Unpin,
{
    let received = tokio::time::timeout(Duration::from_millis(200), async {
        loop {
            let Some(message) = socket.next().await else {
                return false;
            };
            match message {
                Ok(TungsteniteMessage::Binary(_)) => return true,
                Ok(_) => continue,
                Err(_) => return false,
            }
        }
    })
    .await;
    assert!(!matches!(received, Ok(true)), "audio leaked across rooms");
}

// Creates one human-vs-agent waiting room and returns the human and room ids.
async fn create_human_vs_agent_room(router: Router, name: &str) -> (String, String) {
    let human = create_direct_participant(router.clone(), name).await;
    consent_participant(router.clone(), &human).await;
    let (status, created) = json_request(
        router,
        http::Method::POST,
        "/api/rooms",
        json!({"participant_session_id": human}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(created["role"], "A");
    (
        created["participant_session_id"]
            .as_str()
            .unwrap()
            .to_string(),
        created["room_id"].as_str().unwrap().to_string(),
    )
}

// Polls the evaluation export until an event type appears or the test times out.
async fn wait_for_export_event(router: Router, event_type: &str) -> Value {
    for _ in 0..20 {
        let (status, export) = json_request(
            router.clone(),
            http::Method::GET,
            "/api/admin/export",
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
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("timed out waiting for event type {event_type}");
}

// Polls the evaluation export until a TTS diagnostic event appears.
async fn wait_for_tts_diagnostic(router: Router, diagnostic_event: &str) -> Value {
    for _ in 0..30 {
        let (status, export) = json_request(
            router.clone(),
            http::Method::GET,
            "/api/admin/export",
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

async fn consent_participant(router: Router, participant_session_id: &str) {
    let (status, _response) = json_request(
        router,
        http::Method::POST,
        "/api/consent",
        json!({
            "participant_session_id": participant_session_id,
            "decisions": {"study": true}
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn health_and_public_config_expose_client_bootstrap_shape() {
    let mut config = step_five_config();
    config.study.name = "Bootstrap Study".to_string();
    config.voice.enabled = true;
    config.transcription.enabled = true;
    config.speechmatics.api_key = "test-key".to_string();
    config.tts.enabled = true;
    config.tts.voice_id = "voice-1".to_string();
    config.tts.api_key = "tts-secret".to_string();
    config.tts.voice_name = "Agent Voice".to_string();
    let router = build_router(TinyAdapter, config, ServeOptions::default())
        .await
        .unwrap();

    let (health_status, health) =
        json_request(router.clone(), http::Method::GET, "/health", Value::Null).await;
    assert_eq!(health_status, StatusCode::OK);
    assert_eq!(health["status"], "ok");
    assert_eq!(health["storage"], "read_write");

    let (config_status, public_config) =
        json_request(router, http::Method::GET, "/api/config", Value::Null).await;
    assert_eq!(config_status, StatusCode::OK);
    assert_eq!(public_config["study_name"], "Bootstrap Study");
    assert!(public_config["institution"].is_null());
    assert_eq!(public_config["consents"][0]["id"], "study");
    assert_eq!(public_config["voice"]["enabled"], true);
    assert_eq!(public_config["voice"]["transport"], "websocket");
    assert_eq!(public_config["transcription"]["enabled"], true);
    assert_eq!(public_config["tts"]["voice_name"], "Agent Voice");
    assert_eq!(public_config["agents"]["mode"], "human_vs_human");
    assert_eq!(public_config["agents"]["human_vs_agent"], false);
}

#[tokio::test]
async fn static_serving_returns_assets_spa_fallback_and_preserves_api_prefixes() {
    let temp = tempfile::tempdir().unwrap();
    let dist = temp.path().join("dist");
    fs::create_dir_all(dist.join("assets")).unwrap();
    fs::write(dist.join("index.html"), "<main>Parlando client</main>").unwrap();
    fs::write(dist.join("assets/app.js"), "console.log('asset');").unwrap();
    fs::write(temp.path().join("secret.txt"), "do not serve me").unwrap();

    let mut config = step_five_config();
    config.server.client_dist_path = Some(dist.display().to_string());
    let router = build_router(TinyAdapter, config, ServeOptions::default())
        .await
        .unwrap();

    let (root_status, root_body, root_type) =
        raw_request(router.clone(), http::Method::GET, "/", Body::empty()).await;
    assert_eq!(root_status, StatusCode::OK);
    assert!(root_body.contains("Parlando client"));
    assert!(root_type
        .as_deref()
        .is_some_and(|value| value.contains("text/html")));

    let (asset_status, asset_body, asset_type) = raw_request(
        router.clone(),
        http::Method::GET,
        "/assets/app.js",
        Body::empty(),
    )
    .await;
    assert_eq!(asset_status, StatusCode::OK);
    assert_eq!(asset_body, "console.log('asset');");
    assert!(asset_type
        .as_deref()
        .is_some_and(|value| value.contains("javascript")));

    let (fallback_status, fallback_body, _) = raw_request(
        router.clone(),
        http::Method::GET,
        "/room/abc",
        Body::empty(),
    )
    .await;
    assert_eq!(fallback_status, StatusCode::OK);
    assert!(fallback_body.contains("Parlando client"));

    let (api_status, api_body, _) = raw_request(
        router.clone(),
        http::Method::GET,
        "/api/config",
        Body::empty(),
    )
    .await;
    assert_eq!(api_status, StatusCode::OK);
    assert!(api_body.contains("study_name"));
    assert!(!api_body.contains("Parlando client"));

    let (_traversal_status, traversal_body, _) = raw_request(
        router,
        http::Method::GET,
        "/assets/../secret.txt",
        Body::empty(),
    )
    .await;
    assert!(!traversal_body.contains("do not serve me"));
}

#[tokio::test]
async fn audio_session_is_disabled_when_voice_is_disabled() {
    let router = build_router(TinyAdapter, step_five_config(), ServeOptions::default())
        .await
        .unwrap();
    let (a, _b, room_id) = create_joined_room(router.clone()).await;

    let (audio_status, audio) = json_request(
        router,
        http::Method::POST,
        &format!("/api/rooms/{room_id}/audio-session"),
        json!({"participant_session_id": a}),
    )
    .await;
    assert_eq!(audio_status, StatusCode::OK);
    assert_eq!(audio["enabled"], false);
    assert!(audio["token"].is_null());
}

#[tokio::test]
async fn enabled_audio_session_returns_parlando_websocket_contract() {
    let router = build_router(TinyAdapter, voice_enabled_config(), ServeOptions::default())
        .await
        .unwrap();
    let (a, _b, room_id) = create_joined_room(router.clone()).await;

    let (audio_status, audio) = json_request(
        router,
        http::Method::POST,
        &format!("/api/rooms/{room_id}/audio-session"),
        json!({"participant_session_id": a}),
    )
    .await;
    assert_eq!(audio_status, StatusCode::OK);
    assert_eq!(audio["enabled"], true);
    assert_eq!(audio["protocol_version"], 1);
    assert_eq!(audio["sample_rate_hz"], 24000);
    assert_eq!(
        audio["websocket_url"],
        format!("/ws/audio/{room_id}"),
        "the browser must resolve the path against its actual origin"
    );
    assert!(audio["token"]
        .as_str()
        .is_some_and(|value| !value.is_empty()));
}

/// Deterministic provider that finalizes the first received audio frame twice.
struct DuplicateFinalTranscriptionProvider;

#[async_trait]
impl TranscriptionProvider for DuplicateFinalTranscriptionProvider {
    /// Starts a test session whose duplicate finals exercise idempotent persistence.
    async fn start_session(
        &self,
        _context: TranscriptionSessionContext,
    ) -> Result<crate::transcription::TranscriptionSessionHandle> {
        let (input, mut inputs) = mpsc::channel(4);
        let (events, event_receiver) = mpsc::channel(4);
        tokio::spawn(async move {
            let _ = events.send(TranscriptionEvent::Ready).await;
            while let Some(message) = inputs.recv().await {
                if matches!(message, TranscriptionInput::Audio(_)) {
                    let utterance = FinalTranscriptUtterance {
                        start_time_ms: 0,
                        end_time_ms: 20,
                        text: "relay transcript".to_string(),
                        result_ids: vec!["stable-result".to_string()],
                    };
                    let _ = events
                        .send(TranscriptionEvent::FinalUtterance(utterance.clone()))
                        .await;
                    let _ = events
                        .send(TranscriptionEvent::FinalUtterance(utterance))
                        .await;
                    break;
                }
            }
        });
        Ok(crate::transcription::TranscriptionSessionHandle {
            input,
            events: event_receiver,
        })
    }
}

#[tokio::test]
async fn audio_websocket_relays_pcm_and_commits_one_final_utterance() {
    let mut config = voice_enabled_config();
    config.transcription.enabled = true;
    config.speechmatics.api_key = "server-only-test-key".to_string();
    let router = build_router(
        TinyAdapter,
        config,
        ServeOptions {
            transcription_provider: Some(Arc::new(DuplicateFinalTranscriptionProvider)),
            ..ServeOptions::default()
        },
    )
    .await
    .unwrap();
    let (a, b, room_id) = create_joined_room(router.clone()).await;
    let (_, plan_a) = json_request(
        router.clone(),
        http::Method::POST,
        &format!("/api/rooms/{room_id}/audio-session"),
        json!({"participant_session_id":a}),
    )
    .await;
    let (_, plan_b) = json_request(
        router.clone(),
        http::Method::POST,
        &format!("/api/rooms/{room_id}/audio-session"),
        json!({"participant_session_id":b}),
    )
    .await;
    let (base_url, server) = spawn_test_server(router.clone()).await;
    let host = base_url.trim_start_matches("http://");
    let (mut socket_b, _) = connect_async(format!(
        "ws://{host}/ws/audio/{room_id}?token={}",
        plan_b["token"].as_str().unwrap()
    ))
    .await
    .unwrap();
    let (mut socket_a, _) = connect_async(format!(
        "ws://{host}/ws/audio/{room_id}?token={}",
        plan_a["token"].as_str().unwrap()
    ))
    .await
    .unwrap();
    let frame = AudioFrame {
        sequence: 0,
        timestamp_ms: 0,
        pcm: vec![0; crate::audio::AUDIO_FRAME_BYTES],
    }
    .encode();
    socket_a
        .send(TungsteniteMessage::Binary(frame.clone()))
        .await
        .unwrap();
    loop {
        let message = tokio::time::timeout(Duration::from_secs(2), socket_b.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        if matches!(message, TungsteniteMessage::Binary(bytes) if bytes == frame) {
            break;
        }
    }
    let export = wait_for_export_event(router, "conversation_message").await;
    assert_eq!(
        export["session_events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|event| {
                event["event_type"] == "conversation_message"
                    && event["payload"]["origin"] == "voice_transcript"
            })
            .count(),
        1
    );
    assert!(!export["session_events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|event| event["event_type"] == "transcript_segment"));
    server.abort();
}

/// Proves that two simultaneous player pairs receive audio only inside their own rooms.
#[tokio::test]
async fn audio_websockets_isolate_two_simultaneous_player_pairs() {
    let router = build_router(TinyAdapter, voice_enabled_config(), ServeOptions::default())
        .await
        .unwrap();
    let (a_one, b_one, room_one) = create_joined_room(router.clone()).await;
    let (a_two, b_two, room_two) = create_joined_room(router.clone()).await;
    assert_ne!(room_one, room_two);

    let plan_a_one = request_audio_plan(router.clone(), &room_one, &a_one).await;
    let plan_b_one = request_audio_plan(router.clone(), &room_one, &b_one).await;
    let plan_a_two = request_audio_plan(router.clone(), &room_two, &a_two).await;
    let plan_b_two = request_audio_plan(router.clone(), &room_two, &b_two).await;
    let tokens = [
        plan_a_one["token"].as_str().unwrap(),
        plan_b_one["token"].as_str().unwrap(),
        plan_a_two["token"].as_str().unwrap(),
        plan_b_two["token"].as_str().unwrap(),
    ];
    for (index, token) in tokens.iter().enumerate() {
        assert!(tokens[..index].iter().all(|existing| existing != token));
    }

    let (base_url, server) = spawn_test_server(router).await;
    let host = base_url.trim_start_matches("http://");
    let connect = |room_id: &str, token: &str| {
        connect_async(format!("ws://{host}/ws/audio/{room_id}?token={token}"))
    };
    let (mut socket_a_one, _) = connect(&room_one, tokens[0]).await.unwrap();
    let (mut socket_b_one, _) = connect(&room_one, tokens[1]).await.unwrap();
    let (mut socket_a_two, _) = connect(&room_two, tokens[2]).await.unwrap();
    let (mut socket_b_two, _) = connect(&room_two, tokens[3]).await.unwrap();

    let frame_one = AudioFrame {
        sequence: 11,
        timestamp_ms: 220,
        pcm: vec![1; crate::audio::AUDIO_FRAME_BYTES],
    }
    .encode();
    let frame_two = AudioFrame {
        sequence: 22,
        timestamp_ms: 440,
        pcm: vec![2; crate::audio::AUDIO_FRAME_BYTES],
    }
    .encode();
    socket_a_one
        .send(TungsteniteMessage::Binary(frame_one.clone()))
        .await
        .unwrap();
    assert_eq!(read_audio_binary(&mut socket_b_one).await, frame_one);
    assert_no_audio_binary(&mut socket_a_two).await;
    assert_no_audio_binary(&mut socket_b_two).await;

    socket_a_two
        .send(TungsteniteMessage::Binary(frame_two.clone()))
        .await
        .unwrap();
    assert_eq!(read_audio_binary(&mut socket_b_two).await, frame_two);
    assert_no_audio_binary(&mut socket_a_one).await;
    assert_no_audio_binary(&mut socket_b_one).await;

    server.abort();
}

/// Ensures a replacement connection cannot leave an older socket injecting room audio.
#[tokio::test]
async fn replacement_audio_connection_revokes_the_older_socket_generation() {
    let router = build_router(TinyAdapter, voice_enabled_config(), ServeOptions::default())
        .await
        .unwrap();
    let (a, b, room_id) = create_joined_room(router.clone()).await;
    let old_a = request_audio_plan(router.clone(), &room_id, &a).await;
    let plan_b = request_audio_plan(router.clone(), &room_id, &b).await;
    let (base_url, server) = spawn_test_server(router.clone()).await;
    let host = base_url.trim_start_matches("http://");
    let (mut socket_b, _) = connect_async(format!(
        "ws://{host}/ws/audio/{room_id}?token={}",
        plan_b["token"].as_str().unwrap()
    ))
    .await
    .unwrap();
    let (mut old_socket_a, _) = connect_async(format!(
        "ws://{host}/ws/audio/{room_id}?token={}",
        old_a["token"].as_str().unwrap()
    ))
    .await
    .unwrap();
    wait_for_audio_control(&mut old_socket_a).await;
    let new_a = request_audio_plan(router, &room_id, &a).await;
    let (mut new_socket_a, _) = connect_async(format!(
        "ws://{host}/ws/audio/{room_id}?token={}",
        new_a["token"].as_str().unwrap()
    ))
    .await
    .unwrap();
    wait_for_audio_control(&mut new_socket_a).await;

    let stale_frame = AudioFrame {
        sequence: 1,
        timestamp_ms: 20,
        pcm: vec![3; crate::audio::AUDIO_FRAME_BYTES],
    }
    .encode();
    old_socket_a
        .send(TungsteniteMessage::Binary(stale_frame))
        .await
        .unwrap();
    assert_no_audio_binary(&mut socket_b).await;

    let current_frame = AudioFrame {
        sequence: 2,
        timestamp_ms: 40,
        pcm: vec![4; crate::audio::AUDIO_FRAME_BYTES],
    }
    .encode();
    new_socket_a
        .send(TungsteniteMessage::Binary(current_frame.clone()))
        .await
        .unwrap();
    assert_eq!(read_audio_binary(&mut socket_b).await, current_frame);
    server.abort();
}

/// Verifies that audio credentials are single-use and cannot cross room boundaries.
#[tokio::test]
async fn audio_tokens_are_single_use_and_room_bound() {
    let router = build_router(TinyAdapter, voice_enabled_config(), ServeOptions::default())
        .await
        .unwrap();
    let (a_one, _b_one, room_one) = create_joined_room(router.clone()).await;
    let (_a_two, _b_two, room_two) = create_joined_room(router.clone()).await;
    let wrong_room_plan = request_audio_plan(router.clone(), &room_one, &a_one).await;
    let reusable_plan = request_audio_plan(router.clone(), &room_one, &a_one).await;
    let (base_url, server) = spawn_test_server(router).await;
    let host = base_url.trim_start_matches("http://");

    let wrong_room = connect_async(format!(
        "ws://{host}/ws/audio/{room_two}?token={}",
        wrong_room_plan["token"].as_str().unwrap()
    ))
    .await;
    assert!(matches!(
        wrong_room,
        Err(tokio_tungstenite::tungstenite::Error::Http(response))
            if response.status() == StatusCode::FORBIDDEN
    ));

    let url = format!(
        "ws://{host}/ws/audio/{room_one}?token={}",
        reusable_plan["token"].as_str().unwrap()
    );
    let (_socket, _) = connect_async(url.clone()).await.unwrap();
    let replay = connect_async(url).await;
    assert!(matches!(
        replay,
        Err(tokio_tungstenite::tungstenite::Error::Http(response))
            if response.status() == StatusCode::FORBIDDEN
    ));
    server.abort();
}

#[tokio::test]
async fn direct_participant_creation_respects_direct_enabled_flag() {
    let mut config = step_five_config();
    config.direct.enabled = false;
    let router = build_router(TinyAdapter, config, ServeOptions::default())
        .await
        .unwrap();

    let (status, response) =
        json_request(router, http::Method::POST, "/api/participants", json!({})).await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(response["raw"], "Direct mode is disabled.");
}

#[tokio::test]
async fn public_participant_creation_rejects_external_identity_sources() {
    let router = build_router(TinyAdapter, step_five_config(), ServeOptions::default())
        .await
        .unwrap();
    let (status, response) = json_request(
        router.clone(),
        http::Method::POST,
        "/api/participants",
        json!({
            "source": "prolific",
            "external_id": "PROLIFIC-1",
            "metadata": {"cohort": "pilot"}
        }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(response["raw"].as_str().unwrap().contains("unknown field"));
}

#[tokio::test]
async fn direct_room_creation_requires_consent_then_assigns_role_a() {
    let router = build_router(TinyAdapter, step_five_config(), ServeOptions::default())
        .await
        .unwrap();
    let participant_session_id = create_direct_participant(router.clone(), "Direct Player").await;

    let (blocked_status, _blocked) = json_request(
        router.clone(),
        http::Method::POST,
        "/api/rooms",
        json!({"participant_session_id": participant_session_id}),
    )
    .await;
    assert_eq!(blocked_status, StatusCode::FORBIDDEN);

    consent_participant(router.clone(), &participant_session_id).await;
    let (status, room) = json_request(
        router,
        http::Method::POST,
        "/api/rooms",
        json!({"participant_session_id": participant_session_id}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(room["participant_session_id"], participant_session_id);
    assert_eq!(room["role"], "A");
    assert!(room["room_id"].as_str().is_some_and(|id| !id.is_empty()));
}

#[tokio::test]
async fn room_response_omits_available_actions_when_game_does_not_provide_affordance() {
    let router = build_router(
        NoAvailableActionsAdapter,
        step_five_config(),
        ServeOptions::default(),
    )
    .await
    .unwrap();
    let participant_session_id = create_direct_participant(router.clone(), "No Actions").await;
    consent_participant(router.clone(), &participant_session_id).await;

    let (status, room) = json_request(
        router,
        http::Method::POST,
        "/api/rooms",
        json!({"participant_session_id": participant_session_id}),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(room.get("available_actions").is_none());
}

#[tokio::test]
async fn required_consent_must_be_accepted_not_only_declared() {
    let router = build_router(TinyAdapter, step_five_config(), ServeOptions::default())
        .await
        .unwrap();
    let participant_session_id = create_direct_participant(router.clone(), "Nope").await;
    let (consent_status, _consent_response) = json_request(
        router.clone(),
        http::Method::POST,
        "/api/consent",
        json!({
            "participant_session_id": participant_session_id,
            "decisions": {"study": false}
        }),
    )
    .await;
    assert_eq!(consent_status, StatusCode::OK);

    let (room_status, response) = json_request(
        router,
        http::Method::POST,
        "/api/rooms",
        json!({"participant_session_id": participant_session_id}),
    )
    .await;

    assert_eq!(room_status, StatusCode::FORBIDDEN);
    assert!(response["raw"]
        .as_str()
        .unwrap()
        .contains("Consent is required"));
}

#[tokio::test]
async fn two_independent_waiting_room_entries_pair_into_one_room() {
    let router = build_router(TinyAdapter, step_five_config(), ServeOptions::default())
        .await
        .unwrap();
    let first = create_direct_participant(router.clone(), "First tab").await;
    let second = create_direct_participant(router.clone(), "Second tab").await;
    consent_participant(router.clone(), &first).await;
    consent_participant(router.clone(), &second).await;

    let (first_status, first_room) = json_request(
        router.clone(),
        http::Method::POST,
        "/api/rooms",
        json!({"participant_session_id": first}),
    )
    .await;
    let (second_status, second_room) = json_request(
        router.clone(),
        http::Method::POST,
        "/api/rooms",
        json!({"participant_session_id": second}),
    )
    .await;

    assert_eq!(first_status, StatusCode::OK);
    assert_eq!(second_status, StatusCode::OK);
    assert_eq!(first_room["role"], "A");
    assert_eq!(second_room["role"], "B");
    assert_eq!(first_room["room_id"], second_room["room_id"]);
    assert!(first_room["presence"]["A"]
        .get("participantSessionId")
        .is_none());
    assert!(first_room["presence"].get("B").is_none());
    assert!(second_room["presence"]["A"]
        .get("participantSessionId")
        .is_none());
    assert!(second_room["presence"]["B"]
        .get("participantSessionId")
        .is_none());

    let (export_status, export) = json_request(
        router,
        http::Method::GET,
        "/api/admin/export?variant=full",
        Value::Null,
    )
    .await;
    assert_eq!(export_status, StatusCode::OK);
    assert_eq!(export["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(export["session_participants"].as_array().unwrap().len(), 2);
    assert!(export["session_participants"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["participant_session_id"] == first && row["role"] == "A"));
    assert!(export["session_participants"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["participant_session_id"] == second && row["role"] == "B"));
}

#[tokio::test]
async fn adversarial_room_routes_reject_caller_controlled_role_fields() {
    let router = build_router(TinyAdapter, step_five_config(), ServeOptions::default())
        .await
        .unwrap();
    let a = create_direct_participant(router.clone(), "A").await;
    consent_participant(router.clone(), &a).await;
    let (create_status, create_response) = json_request(
        router.clone(),
        http::Method::POST,
        "/api/rooms",
        json!({"participant_session_id": a, "role": "B"}),
    )
    .await;
    assert_eq!(create_status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(create_response["raw"]
        .as_str()
        .unwrap()
        .contains("unknown field `role`"));
}

/// Confirms authenticated APIs reject the removed caller-controlled participant field.
#[tokio::test]
async fn authenticated_routes_reject_participant_identity_in_request_bodies() {
    let router = build_router(TinyAdapter, step_five_config(), ServeOptions::default())
        .await
        .unwrap();
    let participant_session_id = create_direct_participant(router.clone(), "A").await;
    let credential = TEST_PARTICIPANT_CREDENTIALS
        .lock()
        .unwrap()
        .get(&participant_session_id)
        .cloned()
        .unwrap();
    let response = router
        .oneshot(
            Request::builder()
                .method(http::Method::POST)
                .uri("/api/consent")
                .header(http::header::CONTENT_TYPE, "application/json")
                .header(AUTHORIZATION, format!("Bearer {credential}"))
                .body(Body::from(
                    json!({
                        "participant_session_id": participant_session_id,
                        "decisions": {"study": true}
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn room_routes_persist_evaluation_session_and_join_events() {
    let router = build_router(TinyAdapter, step_five_config(), ServeOptions::default())
        .await
        .unwrap();
    let a = create_direct_participant(router.clone(), "A").await;
    let b = create_direct_participant(router.clone(), "B").await;
    consent_participant(router.clone(), &a).await;
    consent_participant(router.clone(), &b).await;
    let (_, created) = json_request(
        router.clone(),
        http::Method::POST,
        "/api/rooms",
        json!({"participant_session_id": a}),
    )
    .await;
    assert_eq!(created["role"], "A");
    let room_id = created["room_id"].as_str().unwrap().to_string();
    let (_, joined) = json_request(
        router.clone(),
        http::Method::POST,
        "/api/rooms",
        json!({"participant_session_id": b}),
    )
    .await;
    assert_eq!(joined["role"], "B");
    assert_eq!(joined["room_id"], room_id);

    let (export_status, export) = json_request(
        router,
        http::Method::GET,
        "/api/admin/export?variant=full",
        Value::Null,
    )
    .await;
    assert_eq!(export_status, StatusCode::OK);
    assert_eq!(export["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(export["sessions"][0]["room_id"], room_id);
    assert_eq!(export["session_participants"].as_array().unwrap().len(), 2);
    assert!(export["session_participants"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["participant_session_id"] == a && row["role"] == "A"));
    assert!(export["session_participants"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["participant_session_id"] == b && row["role"] == "B"));
    let event_types = export["session_events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| event["event_type"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        event_types,
        vec![
            "session_created",
            "participant_joined",
            "participant_joined"
        ]
    );
}

#[tokio::test]
async fn websocket_role_assignment_is_targeted_to_one_connection() {
    let router = build_router(TinyAdapter, step_five_config(), ServeOptions::default())
        .await
        .unwrap();
    let (a, b, room_id) = create_joined_room(router.clone()).await;
    let (base_url, server) = spawn_test_server(router.clone()).await;
    let (mut socket_a, _) = connect_async(game_socket_url(&base_url, &room_id, &a).await)
        .await
        .unwrap();
    assert_no_ws_type(&mut socket_a, "roleAssigned").await;

    let (mut socket_b, _) = connect_async(game_socket_url(&base_url, &room_id, &b).await)
        .await
        .unwrap();
    let assigned_a = read_ws_type(&mut socket_a, "roleAssigned").await;
    assert_eq!(assigned_a["participant_session_id"], a);
    assert_eq!(assigned_a["role"], "A");
    let assigned_b = read_ws_type(&mut socket_b, "roleAssigned").await;
    assert_eq!(assigned_b["participant_session_id"], b);
    assert_eq!(assigned_b["role"], "B");
    assert_no_ws_type(&mut socket_a, "roleAssigned").await;
    server.abort();
}

#[tokio::test]
async fn websocket_rejects_actions_until_both_players_are_connected() {
    let router = build_router(TinyAdapter, step_five_config(), ServeOptions::default())
        .await
        .unwrap();
    let (a, _b, room_id) = create_joined_room(router.clone()).await;
    let (base_url, server) = spawn_test_server(router.clone()).await;
    let (mut socket_a, _) = connect_async(game_socket_url(&base_url, &room_id, &a).await)
        .await
        .unwrap();

    send_ws_json(
        &mut socket_a,
        json!({"type": "submitAction", "action": {"finish": false}}),
    )
    .await;
    let error = read_ws_type(&mut socket_a, "error").await;
    assert!(error["message"]
        .as_str()
        .unwrap()
        .contains("waiting for both players"));

    let (export_status, export) =
        json_request(router, http::Method::GET, "/api/admin/export", Value::Null).await;
    assert_eq!(export_status, StatusCode::OK);
    assert!(export["session_events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|event| {
            event["event_type"] == "game_action_rejected"
                && event["payload"]["error"]
                    .as_str()
                    .unwrap()
                    .contains("waiting for both players")
        }));
    server.abort();
}

/// Rejected oversized actions retain bounded analytical metadata, not attacker input.
#[tokio::test]
async fn oversized_action_rejection_is_bounded_and_analyzable() {
    let router = build_router(TinyAdapter, step_five_config(), ServeOptions::default())
        .await
        .unwrap();
    let (a, b, room_id) = create_joined_room(router.clone()).await;
    let (base_url, server) = spawn_test_server(router.clone()).await;
    let (mut socket_a, _) = connect_async(game_socket_url(&base_url, &room_id, &a).await)
        .await
        .unwrap();
    let (mut socket_b, _) = connect_async(game_socket_url(&base_url, &room_id, &b).await)
        .await
        .unwrap();
    let _assigned_a = read_ws_type(&mut socket_a, "roleAssigned").await;
    let _assigned_b = read_ws_type(&mut socket_b, "roleAssigned").await;

    send_ws_json(
        &mut socket_a,
        json!({"type": "submitAction", "action": {"padding": "x".repeat(5_000)}}),
    )
    .await;
    let error = read_ws_type(&mut socket_a, "error").await;
    assert!(error["message"].as_str().unwrap().contains("too large"));

    let (export_status, export) =
        json_request(router, http::Method::GET, "/api/admin/export", Value::Null).await;
    assert_eq!(export_status, StatusCode::OK);
    let rejected = export["session_events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|event| event["event_type"] == "game_action_rejected")
        .unwrap();
    assert_eq!(rejected["payload"]["reason_code"], "action_too_large");
    assert!(rejected["payload"]["submitted_bytes"].as_u64().unwrap() > 4_096);
    assert_eq!(
        rejected["payload"]["action_sha256"].as_str().unwrap().len(),
        64
    );
    assert!(!rejected["payload"].to_string().contains(&"x".repeat(100)));
    server.abort();
}

/// Disabling full-state storage removes both the state column and embedded pre-state.
#[tokio::test]
async fn privacy_switch_removes_all_full_game_state_copies() {
    let mut config = step_five_config();
    config.privacy.store_full_game_state = false;
    let router = build_router(TinyAdapter, config, ServeOptions::default())
        .await
        .unwrap();
    let (a, b, room_id) = create_joined_room(router.clone()).await;
    let (base_url, server) = spawn_test_server(router.clone()).await;
    let (mut socket_a, _) = connect_async(game_socket_url(&base_url, &room_id, &a).await)
        .await
        .unwrap();
    let (mut socket_b, _) = connect_async(game_socket_url(&base_url, &room_id, &b).await)
        .await
        .unwrap();
    let _assigned_a = read_ws_type(&mut socket_a, "roleAssigned").await;
    let _assigned_b = read_ws_type(&mut socket_b, "roleAssigned").await;
    send_ws_json(
        &mut socket_a,
        json!({"type": "submitAction", "action": {"finish": true}}),
    )
    .await;
    let _completed = read_ws_type(&mut socket_a, "completed").await;

    let (export_status, export) =
        json_request(router, http::Method::GET, "/api/admin/export", Value::Null).await;
    assert_eq!(export_status, StatusCode::OK);
    let accepted = export["session_events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|event| event["event_type"] == "game_action_accepted")
        .unwrap();
    assert!(accepted["game_state"].is_null());
    assert!(accepted["payload"].get("before").is_none());
    server.abort();
}

#[tokio::test]
async fn human_human_game_accepts_actions_after_second_human_connects() {
    let router = build_router(TinyAdapter, step_five_config(), ServeOptions::default())
        .await
        .unwrap();
    let (a, b, room_id) = create_joined_room(router.clone()).await;
    let (base_url, server) = spawn_test_server(router.clone()).await;
    let (mut socket_a, _) = connect_async(game_socket_url(&base_url, &room_id, &a).await)
        .await
        .unwrap();
    assert_no_ws_type(&mut socket_a, "roleAssigned").await;

    send_ws_json(
        &mut socket_a,
        json!({"type": "submitAction", "action": {"finish": false}}),
    )
    .await;
    let waiting_error = read_ws_type(&mut socket_a, "error").await;
    assert!(waiting_error["message"]
        .as_str()
        .unwrap()
        .contains("waiting for both players"));

    let (mut socket_b, _) = connect_async(game_socket_url(&base_url, &room_id, &b).await)
        .await
        .unwrap();
    let _assigned_a = read_ws_type(&mut socket_a, "roleAssigned").await;
    let _assigned_b = read_ws_type(&mut socket_b, "roleAssigned").await;
    send_ws_json(
        &mut socket_a,
        json!({"type": "submitAction", "action": {"finish": true}}),
    )
    .await;
    let completed = read_ws_type(&mut socket_a, "completed").await;
    assert_eq!(completed["summary"]["done"], true);
    server.abort();
}

#[tokio::test]
async fn websocket_accepts_actions_chat_completion_and_persists_state_changes() {
    let router = build_router(TinyAdapter, step_five_config(), ServeOptions::default())
        .await
        .unwrap();
    let (a, b, room_id) = create_joined_room(router.clone()).await;
    let (base_url, server) = spawn_test_server(router.clone()).await;
    let (mut socket_a, _) = connect_async(game_socket_url(&base_url, &room_id, &a).await)
        .await
        .unwrap();
    let (mut socket_b, _) = connect_async(game_socket_url(&base_url, &room_id, &b).await)
        .await
        .unwrap();
    let _assigned_a = read_ws_type(&mut socket_a, "roleAssigned").await;
    let _assigned_b = read_ws_type(&mut socket_b, "roleAssigned").await;

    send_ws_json(
        &mut socket_a,
        json!({"type": "sendChatMessage", "text": "hello from A"}),
    )
    .await;
    let chat_a = read_ws_type(&mut socket_a, "conversationMessageAdded").await;
    let chat_b = read_ws_type(&mut socket_b, "conversationMessageAdded").await;
    assert_eq!(chat_a["conversation_message"]["text"], "hello from A");
    assert_eq!(chat_b["conversation_message"]["origin"], "typed");

    send_ws_json(
        &mut socket_a,
        json!({"type": "submitAction", "action": {"finish": false}}),
    )
    .await;
    let state_a = read_ws_type(&mut socket_a, "stateChanged").await;
    let state_b = read_ws_type(&mut socket_b, "stateChanged").await;
    assert_eq!(state_a["participant_session_id"], a);
    assert_eq!(state_a["role"], "A");
    assert_eq!(state_b["participant_session_id"], b);
    assert_eq!(state_b["role"], "B");
    assert_eq!(state_a["observation"]["done"], false);

    send_ws_json(
        &mut socket_a,
        json!({"type": "submitAction", "action": {"finish": true}}),
    )
    .await;
    let completed_a = read_ws_type(&mut socket_a, "completed").await;
    let completed_b = read_ws_type(&mut socket_b, "completed").await;
    assert_eq!(completed_a["summary"]["done"], true);
    assert_eq!(completed_a["summary"]["outcome"], "success");
    assert_eq!(completed_a["summary"]["dyad_score"], 10);
    assert_eq!(completed_a["summary"]["player_scores"]["A"], 6);
    assert_eq!(completed_a["summary"]["player_scores"]["B"], 4);
    assert_eq!(completed_b["summary"]["done"], true);
    assert_eq!(completed_b["summary"], completed_a["summary"]);

    let (export_status, export) =
        json_request(router, http::Method::GET, "/api/admin/export", Value::Null).await;
    assert_eq!(export_status, StatusCode::OK);
    let event_types = export["session_events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| event["event_type"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(event_types.contains(&"conversation_message"));
    assert!(event_types.contains(&"game_action_accepted"));
    assert!(!event_types.contains(&"state_changed"));
    assert!(event_types.contains(&"session_completed"));
    server.abort();
}

#[tokio::test]
async fn loss_completion_summary_is_broadcast_and_exported() {
    let router = build_router(
        LossSummaryAdapter,
        step_five_config(),
        ServeOptions::default(),
    )
    .await
    .unwrap();
    let (a, b, room_id) = create_joined_room(router.clone()).await;
    let (base_url, server) = spawn_test_server(router.clone()).await;
    let (mut socket_a, _) = connect_async(game_socket_url(&base_url, &room_id, &a).await)
        .await
        .unwrap();
    let (mut socket_b, _) = connect_async(game_socket_url(&base_url, &room_id, &b).await)
        .await
        .unwrap();
    let _assigned_a = read_ws_type(&mut socket_a, "roleAssigned").await;
    let _assigned_b = read_ws_type(&mut socket_b, "roleAssigned").await;

    send_ws_json(
        &mut socket_a,
        json!({"type": "submitAction", "action": {"finish": true}}),
    )
    .await;
    let completed = read_ws_type(&mut socket_a, "completed").await;
    assert_eq!(completed["summary"]["done"], true);
    assert_eq!(completed["summary"]["outcome"], "loss");
    assert_eq!(completed["summary"]["dyad_score"], 0);
    assert_eq!(completed["summary"]["player_scores"]["A"], 0);
    assert_eq!(completed["summary"]["player_scores"]["B"], 0);

    let (export_status, export) =
        json_request(router, http::Method::GET, "/api/admin/export", Value::Null).await;
    assert_eq!(export_status, StatusCode::OK);
    let completed_events = export["session_events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| event["event_type"] == "session_completed")
        .collect::<Vec<_>>();
    assert_eq!(completed_events.len(), 1);
    assert_eq!(export["sessions"][0]["completion"]["outcome"], "loss");
    server.abort();
}

#[tokio::test]
async fn completed_rooms_reject_late_game_channel_input() {
    let router = build_router(TinyAdapter, step_five_config(), ServeOptions::default())
        .await
        .unwrap();
    let (a, b, room_id) = create_joined_room(router.clone()).await;
    let (base_url, server) = spawn_test_server(router.clone()).await;
    let (mut socket_a, _) = connect_async(game_socket_url(&base_url, &room_id, &a).await)
        .await
        .unwrap();
    let (mut socket_b, _) = connect_async(game_socket_url(&base_url, &room_id, &b).await)
        .await
        .unwrap();
    let _assigned_a = read_ws_type(&mut socket_a, "roleAssigned").await;
    let _assigned_b = read_ws_type(&mut socket_b, "roleAssigned").await;

    send_ws_json(
        &mut socket_a,
        json!({"type": "submitAction", "action": {"finish": true}}),
    )
    .await;
    let _completed_a = read_ws_type(&mut socket_a, "completed").await;

    send_ws_json(
        &mut socket_a,
        json!({"type": "submitAction", "action": {"finish": false}}),
    )
    .await;
    let action_error = read_ws_type(&mut socket_a, "error").await;
    assert!(action_error["message"]
        .as_str()
        .unwrap()
        .contains("no longer accepts game messages"));

    send_ws_json(
        &mut socket_a,
        json!({"type": "sendChatMessage", "text": "late hello"}),
    )
    .await;
    let chat_error = read_ws_type(&mut socket_a, "error").await;
    assert!(chat_error["message"]
        .as_str()
        .unwrap()
        .contains("no longer accepts game messages"));
    assert_no_ws_type(&mut socket_b, "conversationMessageAdded").await;

    let (transcript_status, transcript_response) = json_request(
        router.clone(),
        http::Method::POST,
        &format!("/api/rooms/{room_id}/transcripts"),
        json!({
            "participant_session_id": a,
            "player": "A",
            "text": "late transcript",
            "metadata": {}
        }),
    )
    .await;
    assert_eq!(transcript_status, StatusCode::NOT_FOUND);
    assert!(transcript_response["raw"].as_str().is_none());

    let (export_status, export) =
        json_request(router, http::Method::GET, "/api/admin/export", Value::Null).await;
    assert_eq!(export_status, StatusCode::OK);
    let events = export["session_events"].as_array().unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| event["event_type"] == "session_completed")
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event["event_type"] == "conversation_message")
            .count(),
        0
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event["event_type"] == "transcript_segment")
            .count(),
        0
    );
    server.abort();
}

#[tokio::test]
async fn transcript_endpoints_are_private_and_diagnostics_persist() {
    let (mut config, _temp) = sqlite_config();
    config.privacy.store_voice_diagnostics = true;
    let router = build_router(TinyAdapter, config, ServeOptions::default())
        .await
        .unwrap();
    let (a, _b, room_id) = create_joined_room(router.clone()).await;

    let (conversation_get_status, _) = json_request(
        router.clone(),
        http::Method::GET,
        &format!("/api/rooms/{room_id}/conversation"),
        Value::Null,
    )
    .await;
    assert_eq!(conversation_get_status, StatusCode::NOT_FOUND);
    let (conversation_post_status, _) = json_request(
        router.clone(),
        http::Method::POST,
        &format!("/api/rooms/{room_id}/conversation"),
        json!({"text": "typed hello"}),
    )
    .await;
    assert_eq!(conversation_post_status, StatusCode::NOT_FOUND);
    let (transcript_get_status, _) = json_request(
        router.clone(),
        http::Method::GET,
        &format!("/api/rooms/{room_id}/transcripts"),
        Value::Null,
    )
    .await;
    assert_eq!(transcript_get_status, StatusCode::NOT_FOUND);
    let (transcript_stream_status, _) = json_request(
        router.clone(),
        http::Method::GET,
        &format!("/api/rooms/{room_id}/transcripts/stream"),
        Value::Null,
    )
    .await;
    assert_eq!(transcript_stream_status, StatusCode::NOT_FOUND);
    let (transcription_context_status, _) = json_request(
        router.clone(),
        http::Method::GET,
        &format!("/api/rooms/{room_id}/transcription-context"),
        Value::Null,
    )
    .await;
    assert_eq!(transcription_context_status, StatusCode::NOT_FOUND);

    let (transcript_status, _transcript) = json_request(
        router.clone(),
        http::Method::POST,
        &format!("/api/rooms/{room_id}/transcripts"),
        json!({
            "participant_session_id": a,
            "player": "B",
            "start_time_ms": 10,
            "end_time_ms": 40,
            "text": "spoken hello",
            "metadata": {"confidence": 0.9}
        }),
    )
    .await;
    assert_eq!(transcript_status, StatusCode::NOT_FOUND);

    let (diagnostic_status, diagnostic) = json_request(
        router.clone(),
        http::Method::POST,
        &format!("/api/rooms/{room_id}/voice-diagnostics"),
        json!({
            "participant_session_id": a,
            "event": "mic_started",
            "metadata": {"device": "test"}
        }),
    )
    .await;
    assert_eq!(diagnostic_status, StatusCode::OK);
    assert_eq!(diagnostic["event"], "mic_started");

    let (export_status, export) = json_request(
        router,
        http::Method::GET,
        "/api/admin/export?variant=full",
        Value::Null,
    )
    .await;
    assert_eq!(export_status, StatusCode::OK);
    let events = export["session_events"].as_array().unwrap();
    assert!(!events
        .iter()
        .any(|event| event["event_type"] == "transcript_segment"));
    assert!(events
        .iter()
        .any(|event| event["event_type"] == "voice_diagnostic"));
}

#[tokio::test]
async fn admin_sessions_api_reads_actions_from_database() {
    let (config, _temp) = sqlite_config();
    let router = build_router(TinyAdapter, config, ServeOptions::default())
        .await
        .unwrap();
    let (a, b, room_id) = create_joined_room(router.clone()).await;
    let (base_url, server) = spawn_test_server(router.clone()).await;
    let (mut socket_a, _) = connect_async(game_socket_url(&base_url, &room_id, &a).await)
        .await
        .unwrap();
    let (mut socket_b, _) = connect_async(game_socket_url(&base_url, &room_id, &b).await)
        .await
        .unwrap();
    let _assigned_a = read_ws_type(&mut socket_a, "roleAssigned").await;
    let _assigned_b = read_ws_type(&mut socket_b, "roleAssigned").await;

    send_ws_json(
        &mut socket_a,
        json!({"type": "submitAction", "action": {"finish": false}}),
    )
    .await;
    let _state_a = read_ws_type(&mut socket_a, "stateChanged").await;
    let _state_b = read_ws_type(&mut socket_b, "stateChanged").await;
    let (transcript_status, _transcript) = json_request(
        router.clone(),
        http::Method::POST,
        &format!("/api/rooms/{room_id}/transcripts"),
        json!({
            "participant_session_id": a,
            "player": "A",
            "start_time_ms": 10,
            "end_time_ms": 40,
            "text": "admin-visible transcript",
            "metadata": {"confidence": 0.95}
        }),
    )
    .await;
    assert_eq!(transcript_status, StatusCode::NOT_FOUND);

    let (sessions_status, sessions) = json_request(
        router.clone(),
        http::Method::GET,
        "/api/admin/sessions",
        Value::Null,
    )
    .await;
    assert_eq!(sessions_status, StatusCode::OK);
    let session_id = sessions["sessions"][0]["session_id"].as_i64().unwrap();
    assert!(sessions["sessions"][0]["event_count"].as_i64().unwrap() >= 6);

    let (detail_status, detail) = json_request(
        router.clone(),
        http::Method::GET,
        &format!("/api/admin/sessions/{session_id}"),
        Value::Null,
    )
    .await;
    assert_eq!(detail_status, StatusCode::OK);
    assert_eq!(detail["participants"].as_array().unwrap().len(), 2);
    let events = detail["events"].as_array().unwrap();
    assert!(events.iter().any(|event| {
        event["event_type"] == "game_action_accepted"
            && event["text"].as_str().unwrap().contains("\"finish\":false")
    }));

    let after_action = events
        .iter()
        .find(|event| event["event_type"] == "game_action_accepted")
        .and_then(|event| event["event_index"].as_i64())
        .unwrap();
    let (poll_status, poll) = json_request(
        router,
        http::Method::GET,
        &format!("/api/admin/sessions/{session_id}/events?after={after_action}"),
        Value::Null,
    )
    .await;
    assert_eq!(poll_status, StatusCode::OK);
    assert!(poll["events"].as_array().unwrap().is_empty());
    server.abort();
}

#[tokio::test]
async fn human_vs_agent_direct_room_supplies_agent_role_b_immediately() {
    let router = build_router(
        TinyAdapter,
        human_vs_agent_config(),
        ServeOptions {
            agent_factory: Some(Arc::new(NoopAgentFactory)),
            ..ServeOptions::default()
        },
    )
    .await
    .unwrap();
    let human = create_direct_participant(router.clone(), "Human").await;
    consent_participant(router.clone(), &human).await;

    let (status, created) = json_request(
        router.clone(),
        http::Method::POST,
        "/api/rooms",
        json!({"participant_session_id": human}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(created["role"], "A");
    assert_eq!(created["presence"]["B"]["connected"], true);
    assert_eq!(created["presence"]["B"]["audioReady"], true);

    let (export_status, export) =
        json_request(router, http::Method::GET, "/api/admin/export", Value::Null).await;
    assert_eq!(export_status, StatusCode::OK);
    let roles = export["session_participants"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["role"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(roles.contains(&"A"));
    assert!(roles.contains(&"B"));
    assert!(export["participants"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["participant_kind"] == "agent"));
}

#[tokio::test]
async fn agent_runtime_creates_one_fresh_agent_per_room() {
    let factory = Arc::new(ScriptedAgentFactory::new(vec![vec![], vec![]]));
    let router = build_router(
        TinyAdapter,
        human_vs_agent_config(),
        ServeOptions {
            agent_factory: Some(factory.clone()),
            ..ServeOptions::default()
        },
    )
    .await
    .unwrap();
    let (human_1, room_1) = create_human_vs_agent_room(router.clone(), "Human 1").await;
    let (human_2, room_2) = create_human_vs_agent_room(router.clone(), "Human 2").await;
    let (base_url, server) = spawn_test_server(router).await;
    let (mut socket_1, _) = connect_async(game_socket_url(&base_url, &room_1, &human_1).await)
        .await
        .unwrap();
    let (mut socket_2, _) = connect_async(game_socket_url(&base_url, &room_2, &human_2).await)
        .await
        .unwrap();
    let _ = read_ws_type(&mut socket_1, "roleAssigned").await;
    let _ = read_ws_type(&mut socket_2, "roleAssigned").await;

    for _ in 0..20 {
        if factory.created_count() == 2 {
            server.abort();
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    server.abort();
    panic!("expected one fresh agent for each room");
}

#[tokio::test]
async fn agent_runtime_receives_same_available_action_affordance_as_ui() {
    let seen_actions = Arc::new(Mutex::new(Vec::new()));
    let router = build_router(
        TinyAdapter,
        human_vs_agent_config(),
        ServeOptions {
            agent_factory: Some(Arc::new(RecordingActionsAgentFactory {
                seen_actions: seen_actions.clone(),
            })),
            ..ServeOptions::default()
        },
    )
    .await
    .unwrap();
    let (human, room_id) = create_human_vs_agent_room(router.clone(), "Human").await;
    let (base_url, server) = spawn_test_server(router).await;
    let (mut socket, _) = connect_async(game_socket_url(&base_url, &room_id, &human).await)
        .await
        .unwrap();
    let assigned = read_ws_type(&mut socket, "roleAssigned").await;
    let ui_available_actions = assigned["available_actions"].as_array().unwrap();
    assert_eq!(ui_available_actions.len(), 1);
    assert_eq!(ui_available_actions[0]["finish"], true);

    for _ in 0..20 {
        let captured = seen_actions.lock().unwrap().clone();
        if let Some(first_turn_actions) = captured.first() {
            assert_eq!(
                first_turn_actions,
                &Some(vec![TinyAction {
                    finish: true,
                    invalid: false,
                }])
            );
            server.abort();
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    server.abort();
    panic!("expected agent to receive available actions");
}

#[tokio::test]
async fn agent_runtime_observes_messages_with_speaker_and_modality() {
    let observations = Arc::new(Mutex::new(Vec::new()));
    let router = build_router(
        TinyAdapter,
        human_vs_agent_config(),
        ServeOptions {
            agent_factory: Some(Arc::new(RecordingObservationsAgentFactory {
                observations: observations.clone(),
            })),
            ..ServeOptions::default()
        },
    )
    .await
    .unwrap();
    let (human, room_id) = create_human_vs_agent_room(router.clone(), "Human").await;
    let (base_url, server) = spawn_test_server(router.clone()).await;
    let (mut socket, _) = connect_async(game_socket_url(&base_url, &room_id, &human).await)
        .await
        .unwrap();
    let _ = read_ws_type(&mut socket, "roleAssigned").await;
    let _ = wait_for_export_event(router.clone(), "agent_started").await;

    send_ws_json(
        &mut socket,
        json!({"type": "sendChatMessage", "text": "typed hello"}),
    )
    .await;
    let _ = read_ws_type(&mut socket, "conversationMessageAdded").await;
    let (status, _) = json_request(
        router,
        http::Method::POST,
        &format!("/api/rooms/{room_id}/transcripts"),
        json!({
            "participant_session_id": human,
            "player": "A",
            "start_time_ms": 0,
            "end_time_ms": 10,
            "text": "spoken hello",
            "metadata": {}
        }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    for _ in 0..20 {
        let captured = observations.lock().unwrap().clone();
        if captured.contains(&"message:A:Typed:typed hello".to_string()) {
            server.abort();
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    server.abort();
    panic!("expected agent to observe typed messages");
}

#[tokio::test]
async fn agent_runtime_observes_actions_with_resulting_observation() {
    let observations = Arc::new(Mutex::new(Vec::new()));
    let router = build_router(
        TinyAdapter,
        human_vs_agent_config(),
        ServeOptions {
            agent_factory: Some(Arc::new(RecordingObservationsAgentFactory {
                observations: observations.clone(),
            })),
            ..ServeOptions::default()
        },
    )
    .await
    .unwrap();
    let (human, room_id) = create_human_vs_agent_room(router.clone(), "Human").await;
    let (base_url, server) = spawn_test_server(router.clone()).await;
    let (mut socket, _) = connect_async(game_socket_url(&base_url, &room_id, &human).await)
        .await
        .unwrap();
    let _ = read_ws_type(&mut socket, "roleAssigned").await;
    let _ = wait_for_export_event(router.clone(), "agent_started").await;

    send_ws_json(
        &mut socket,
        json!({"type": "submitAction", "action": {"finish": false}}),
    )
    .await;
    let _ = read_ws_type(&mut socket, "stateChanged").await;

    for _ in 0..20 {
        let captured = observations.lock().unwrap().clone();
        if captured.contains(&"action:A:false:false".to_string()) {
            server.abort();
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    server.abort();
    panic!("expected agent to observe accepted action");
}

#[tokio::test]
async fn agent_runtime_persists_messages_and_validated_actions() {
    let factory = Arc::new(ScriptedAgentFactory::new(vec![vec![
        scripted_response(
            Some("agent says hello"),
            Some(TinyAction {
                finish: false,
                invalid: false,
            }),
        ),
        scripted_response(
            None,
            Some(TinyAction {
                finish: true,
                invalid: false,
            }),
        ),
    ]]));
    let router = build_router(
        TinyAdapter,
        human_vs_agent_config(),
        ServeOptions {
            agent_factory: Some(factory),
            ..ServeOptions::default()
        },
    )
    .await
    .unwrap();
    let (human, room_id) = create_human_vs_agent_room(router.clone(), "Human").await;
    let (base_url, server) = spawn_test_server(router.clone()).await;
    let (mut socket, _) = connect_async(game_socket_url(&base_url, &room_id, &human).await)
        .await
        .unwrap();
    let _ = read_ws_type(&mut socket, "roleAssigned").await;
    let first_update = read_next_ws_value(&mut socket).await;
    assert_eq!(first_update["type"], "stateChanged");
    let message = read_ws_type(&mut socket, "conversationMessageAdded").await;
    assert_eq!(message["conversation_message"]["origin"], "agent");
    assert_eq!(message["conversation_message"]["text"], "agent says hello");
    let completed = read_ws_type(&mut socket, "completed").await;
    assert_eq!(completed["summary"]["done"], true);

    let export = wait_for_export_event(router, "session_completed").await;
    let events = export["session_events"].as_array().unwrap();
    assert!(events
        .iter()
        .any(|event| event["event_type"] == "agent_started"));
    assert!(events
        .iter()
        .any(|event| event["event_type"] == "agent_action"));
    assert!(events.iter().any(|event| {
        event["event_type"] == "conversation_message" && event["payload"]["origin"] == "agent"
    }));
    assert!(events
        .iter()
        .any(|event| event["event_type"] == "game_action_accepted"));
    server.abort();
}

#[tokio::test]
async fn agent_runtime_observes_accepted_action_before_next_decision() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let router = build_router(
        TinyAdapter,
        human_vs_agent_config(),
        ServeOptions {
            agent_factory: Some(Arc::new(SequencedDecisionAgentFactory { log: log.clone() })),
            ..ServeOptions::default()
        },
    )
    .await
    .unwrap();
    let (human, room_id) = create_human_vs_agent_room(router.clone(), "Human").await;
    let (base_url, server) = spawn_test_server(router).await;
    let (mut socket, _) = connect_async(game_socket_url(&base_url, &room_id, &human).await)
        .await
        .unwrap();
    let _ = read_ws_type(&mut socket, "roleAssigned").await;

    for _ in 0..20 {
        let snapshot = log.lock().unwrap().clone();
        if snapshot.iter().any(|entry| entry == "maybe_act:2") {
            assert_eq!(
                snapshot,
                vec!["maybe_act:1", "observe_action:B", "maybe_act:2"]
            );
            server.abort();
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    server.abort();
    panic!("timed out waiting for the second agent decision");
}

#[tokio::test]
async fn agent_runtime_stops_invalid_agents_cleanly() {
    let factory = Arc::new(ScriptedAgentFactory::new(vec![vec![
        scripted_response(
            None,
            Some(TinyAction {
                finish: false,
                invalid: true,
            }),
        ),
        scripted_response(
            None,
            Some(TinyAction {
                finish: false,
                invalid: true,
            }),
        ),
        scripted_response(
            None,
            Some(TinyAction {
                finish: false,
                invalid: false,
            }),
        ),
    ]]));
    let router = build_router(
        TinyAdapter,
        human_vs_agent_config(),
        ServeOptions {
            agent_factory: Some(factory),
            ..ServeOptions::default()
        },
    )
    .await
    .unwrap();
    let (human, room_id) = create_human_vs_agent_room(router.clone(), "Human").await;
    let (base_url, server) = spawn_test_server(router.clone()).await;
    let (mut socket, _) = connect_async(game_socket_url(&base_url, &room_id, &human).await)
        .await
        .unwrap();
    let _ = read_ws_type(&mut socket, "roleAssigned").await;

    let export = wait_for_export_event(router, "agent_error").await;
    let events = export["session_events"].as_array().unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| event["event_type"] == "agent_action")
            .count(),
        2
    );
    assert!(!events
        .iter()
        .any(|event| event["event_type"] == "game_action_accepted"));
    assert!(events.iter().any(|event| {
        event["event_type"] == "agent_error"
            && event["payload"]["last_error"]
                .as_str()
                .unwrap()
                .contains("invalid tiny action")
    }));
    server.abort();
}

#[tokio::test]
async fn agent_runtime_rejects_empty_responses() {
    let empty_response = Some(AgentResponse {
        message: None,
        action: None,
    });
    let factory = Arc::new(ScriptedAgentFactory::new(vec![vec![
        empty_response.clone(),
        empty_response,
    ]]));
    let router = build_router(
        TinyAdapter,
        human_vs_agent_config(),
        ServeOptions {
            agent_factory: Some(factory),
            ..ServeOptions::default()
        },
    )
    .await
    .unwrap();
    let (human, room_id) = create_human_vs_agent_room(router.clone(), "Human").await;
    let (base_url, server) = spawn_test_server(router.clone()).await;
    let (mut socket, _) = connect_async(game_socket_url(&base_url, &room_id, &human).await)
        .await
        .unwrap();
    let _ = read_ws_type(&mut socket, "roleAssigned").await;

    let export = wait_for_export_event(router, "agent_error").await;
    assert!(export["session_events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|event| {
            event["event_type"] == "agent_error"
                && event["payload"]["last_error"]
                    .as_str()
                    .unwrap()
                    .contains("empty response")
        }));
    server.abort();
}

#[tokio::test]
async fn agent_tts_records_diagnostics_for_agent_messages() {
    let factory = Arc::new(ScriptedAgentFactory::new(vec![vec![scripted_response(
        Some("speak this"),
        None,
    )]]));
    let tts_provider = Arc::new(MockTtsProvider {
        calls: AtomicUsize::new(0),
        fail_first: false,
    });
    let audio_publisher = Arc::new(MockAudioPublisher {
        calls: AtomicUsize::new(0),
    });
    let router = build_router(
        TinyAdapter,
        human_vs_agent_config(),
        ServeOptions {
            agent_factory: Some(factory),
            tts_provider: Some(tts_provider),
            audio_publisher: Some(audio_publisher.clone()),
            ..ServeOptions::default()
        },
    )
    .await
    .unwrap();
    let (human, room_id) = create_human_vs_agent_room(router.clone(), "Human").await;
    let (base_url, server) = spawn_test_server(router.clone()).await;
    let (mut socket, _) = connect_async(game_socket_url(&base_url, &room_id, &human).await)
        .await
        .unwrap();
    let _ = read_ws_type(&mut socket, "roleAssigned").await;

    let export = wait_for_tts_diagnostic(router, "tts_message_completed").await;
    let diagnostics = export["session_events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| event["event_type"] == "tts_diagnostic")
        .map(|event| event["payload"]["event"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(diagnostics.contains(&"tts_message_started"));
    assert!(diagnostics.contains(&"tts_first_audio"));
    assert!(diagnostics.contains(&"tts_audio_summary"));
    assert!(diagnostics.contains(&"tts_publish_started"));
    assert!(diagnostics.contains(&"tts_publish_completed"));
    assert!(diagnostics.contains(&"tts_message_completed"));
    assert_eq!(audio_publisher.calls.load(Ordering::SeqCst), 1);
    server.abort();
}

#[tokio::test]
async fn agent_tts_continues_after_provider_failure() {
    let factory = Arc::new(ScriptedAgentFactory::new(vec![vec![
        scripted_response(Some("first fails"), None),
        scripted_response(Some("second succeeds"), None),
    ]]));
    let tts_provider = Arc::new(MockTtsProvider {
        calls: AtomicUsize::new(0),
        fail_first: true,
    });
    let router = build_router(
        TinyAdapter,
        human_vs_agent_config(),
        ServeOptions {
            agent_factory: Some(factory),
            tts_provider: Some(tts_provider),
            ..ServeOptions::default()
        },
    )
    .await
    .unwrap();
    let (human, room_id) = create_human_vs_agent_room(router.clone(), "Human").await;
    let (base_url, server) = spawn_test_server(router.clone()).await;
    let (mut socket, _) = connect_async(game_socket_url(&base_url, &room_id, &human).await)
        .await
        .unwrap();
    let _ = read_ws_type(&mut socket, "roleAssigned").await;

    let export = wait_for_tts_diagnostic(router, "tts_message_completed").await;
    let diagnostics = export["session_events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| event["event_type"] == "tts_diagnostic")
        .map(|event| event["payload"]["event"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(diagnostics.contains(&"tts_message_failed"));
    assert!(diagnostics.contains(&"tts_message_completed"));
    server.abort();
}

/// Proves sentinel provider credentials cannot cross the durable configuration boundary.
#[test]
fn persisted_configuration_redacts_provider_credentials() {
    let mut config = step_five_config();
    config.speechmatics.api_key = "speechmatics-sentinel-secret".to_string();
    config.tts.api_key = "elevenlabs-sentinel-secret".to_string();
    config.agents.human_vs_agent = Some(crate::config::HumanVsAgentConfig {
        config: json!({
            "auth_token": "nested-auth-sentinel",
            "client_secret": "nested-client-sentinel",
            "private_key": "nested-key-sentinel",
        }),
        ..Default::default()
    });

    let serialized = serde_json::to_string(&persistable_config_json(&config).unwrap()).unwrap();

    assert!(!serialized.contains("speechmatics-sentinel-secret"));
    assert!(!serialized.contains("elevenlabs-sentinel-secret"));
    assert!(!serialized.contains("nested-auth-sentinel"));
    assert!(!serialized.contains("nested-client-sentinel"));
    assert!(!serialized.contains("nested-key-sentinel"));
}
