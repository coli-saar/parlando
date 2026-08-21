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

use crate::agents::{Agent, AgentContext, AgentIdentity, AgentResponse};
use crate::config::{
    AgentsConfig, AgentsMode, DatabaseConfig, DirectConfig, ExperimentIdentityConfig,
};
use crate::game::{
    ActionRejection, AgentConfigField, AgentConfigValue, AgentDefinition, PlayerRole, SecretPurpose,
};

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
    let limit = crate::bundled_runtime_limits().participant_creation.clone();
    for _ in 0..limit.max_attempts {
        assert!(rate.record(1_000));
    }
    assert!(!rate.record(1_000));
    assert!(rate.record(1_001 + limit.window_seconds));
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
async fn build_router<G: Game, F: crate::GameFactory<Game = G>>(
    adapter: F,
    config: ExperimentConfig,
    options: ServeOptions<G>,
) -> Result<Router>
where
    G::State: Serialize,
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
        game_time_ms: index * 1_000,
    };
    admin_event_summary(stored)
}

#[test]
fn admin_dashboard_html_reflects_game_scoped_experiment_layout() {
    assert!(ADMIN_EXPERIMENT_HTML.contains("product-header"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("font-family: Inter, ui-sans-serif, system-ui"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("icon-sprite"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("#icon-flask"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("#icon-activity"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("#icon-settings"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("#icon-shield"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("gameName"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("gameVersion"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("gameGit"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("gameBuild"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("Experiment ID (unique and immutable after creation)"));
    assert!(!ADMIN_EXPERIMENT_HTML.contains("class=\"immutable-value\""));
    assert!(ADMIN_EXPERIMENT_HTML.contains(".simple-panel > * { width: 100%; }"));
    assert!(!ADMIN_EXPERIMENT_HTML.contains("Experiment name"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("escapeHtml(factory.name)"));
    assert!(!ADMIN_EXPERIMENT_HTML.contains("escapeHtml(factory.display_name)"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("id=\"emptyExperimentWorkspace\""));
    assert!(ADMIN_EXPERIMENT_HTML.contains("id=\"showLogs\" type=\"checkbox\""));
    assert!(ADMIN_EXPERIMENT_HTML.contains("<span>Show game/agent logs</span>"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("bundle.kind === 'log' && !showLogs.checked"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("showLogs.addEventListener('change'"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("<h1>No experiments yet</h1>"));
    assert!(!ADMIN_EXPERIMENT_HTML.contains("createFirstExperimentButton"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("data-scope=\"experiments\""));
    assert!(ADMIN_EXPERIMENT_HTML.contains("data-scope=\"operations\""));
    assert!(ADMIN_EXPERIMENT_HTML.contains("data-scope=\"game\""));
    assert!(!ADMIN_EXPERIMENT_HTML.contains("data-scope=\"privacy\""));
    assert!(ADMIN_EXPERIMENT_HTML.contains("data-tab=\"privacy\""));
    assert!(ADMIN_EXPERIMENT_HTML.contains("/privacy.md"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("checkbox-line"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("Speechmatics API key"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("ElevenLabs API key"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("Speechmatics realtime URL default"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("ElevenLabs base URL default"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("speechmatics.realtime_url"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("tts.base_url"));
    assert!(ADMIN_EXPERIMENT_HTML
        .contains("Changing these defaults never changes an existing experiment revision"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("gameProviderSecretUpdates"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("gameSettingsSaved"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("Settings saved"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("Configured ("));
    assert!(ADMIN_EXPERIMENT_HTML.contains("data-secret-placeholder"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("data-agent-secret-field"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("data-reveal-agent-secret"));
    assert!(!ADMIN_EXPERIMENT_HTML.contains("Select a game secret"));
    assert!(ADMIN_EXPERIMENT_HTML.contains(".session-list { align-content: start;"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("grid-auto-rows: max-content"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("Agent configuration identity"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("${escapeHtml(digest.slice(0, 8))}</summary>"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("refreshDraftActivationWarning"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("End-of-utterance silence (seconds)"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("inputmode=\"decimal\""));
    assert!(ADMIN_EXPERIMENT_HTML.contains("Use a decimal point, for example 1.2"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("Voice transport"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("Speech recognition"));
    assert!(!ADMIN_EXPERIMENT_HTML.contains("path: 'voice.sample_rate_hz'"));
    assert!(!ADMIN_EXPERIMENT_HTML.contains("path: 'voice.frame_duration_ms'"));
    assert!(!ADMIN_EXPERIMENT_HTML.contains("path: 'transcription.provider'"));
    assert!(!ADMIN_EXPERIMENT_HTML.contains("path: 'tts.output_format'"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("Data retained in Parlando's SQLite database"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("privacy.exports?.selection_rule"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("data-consent-editor"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("data-consent-field=\"id\""));
    assert!(ADMIN_EXPERIMENT_HTML.contains("data-consent-field=\"title\""));
    assert!(ADMIN_EXPERIMENT_HTML.contains("data-consent-field=\"body\""));
    assert!(ADMIN_EXPERIMENT_HTML.contains("Add template"));
    assert!(!ADMIN_EXPERIMENT_HTML.contains("Core research set (3 items)"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("eligibility_and_information_v1_0"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("voice_and_transcription_v1_0"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("Blank item"));
    assert!(!ADMIN_EXPERIMENT_HTML.contains("{{LOCAL_INFORMATION_VERSION}}"));
    assert!(!ADMIN_EXPERIMENT_HTML.contains("{{INSTITUTION_NAME}}"));
    assert!(!ADMIN_EXPERIMENT_HTML.contains("{{SPEECHMATICS_ENTITY_AND_SERVICE}}"));
    assert!(!ADMIN_EXPERIMENT_HTML.contains("JSON list of consent statements"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("eleven_flash_v2_5"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("Activate intake"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("Pause intake"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("Start testing"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("Complete"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("/api/admin/runtime/"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("fetch('/api/admin/load')"));
    assert!(!ADMIN_EXPERIMENT_HTML.contains("runtimeApi('load')"));
    assert!(!ADMIN_EXPERIMENT_HTML.contains("runningExperimentId"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("New experiment"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("createExperimentDialog"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("newExperimentNotes"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("data-tab=\"notes\""));
    assert!(ADMIN_EXPERIMENT_HTML.contains("experimentNotesSource"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("safeMarkdown"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("source-editor-lines"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("document.title"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("state.game?.name"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("experimentCapabilityBadges"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("align-content: start"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("html { height: 100%; overflow: hidden; }"));
    assert!(ADMIN_EXPERIMENT_HTML.contains(".simple-panel { flex: 1;"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("overflow-y: auto; overscroll-behavior: contain"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("'Unarchive'"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("/archive"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("Experiment unavailable"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("new-experiment"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("experimentStatusFilter"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("data-status-filter"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("initializeStatusFilter"));
    assert!(!ADMIN_EXPERIMENT_HTML.contains("All statuses"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("data-value=\"running\""));
    assert!(!ADMIN_EXPERIMENT_HTML.contains("data-value=\"playing\""));
    assert!(!ADMIN_EXPERIMENT_HTML.contains("🟠"));
    assert!(!ADMIN_EXPERIMENT_HTML.contains("🟢"));
    assert!(!ADMIN_EXPERIMENT_HTML.contains("🔵"));
    assert!(!ADMIN_EXPERIMENT_HTML.contains("🔴"));
    assert!(!ADMIN_EXPERIMENT_HTML.contains("experimentRuntimeState"));
    assert!(!ADMIN_EXPERIMENT_HTML.contains("statusLabel('not-running')"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("experiment.status === 'archived'"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("<time>${escapeHtml(fmtDate(item.created_at))}</time>"));
    assert!(!ADMIN_EXPERIMENT_HTML
        .contains("<time>Created ${escapeHtml(fmtDate(item.created_at))}</time>"));
    assert!(!ADMIN_EXPERIMENT_HTML.contains("item.participant_title"));
    assert!(!ADMIN_EXPERIMENT_HTML.contains("experiment.participant_title"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("catalogueResizer"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("sessionResizer"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("Object.assign(state.selectedSession, refreshed)"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("function fmtGameTime(value)"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("title=\"Game time\""));
    assert!(!ADMIN_EXPERIMENT_HTML.contains("fmtTime(bundle.created_at)"));
    assert!(!ADMIN_EXPERIMENT_HTML.contains("class=\"expand\""));
    assert!(!ADMIN_EXPERIMENT_HTML.contains("event-json"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("border-left: 1px solid var(--strong-line)"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("title=\"Delete participant data\""));
    assert!(ADMIN_EXPERIMENT_HTML.contains("aria-label=\"Delete participant data\""));
    assert!(!ADMIN_EXPERIMENT_HTML.contains(">Delete data</button>"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("participant-role-a"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("participant-role-b"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("participant-assignments::after"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("`${type} v ${version}`"));
    assert!(!ADMIN_EXPERIMENT_HTML.contains("experimentForm"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("id=\"configForm\""));
    assert!(ADMIN_EXPERIMENT_HTML.contains("configurationFromForm"));
    assert!(
        ADMIN_EXPERIMENT_HTML.contains("Object.assign(state.experiment, authoritativeExperiment)")
    );
    assert!(ADMIN_EXPERIMENT_HTML.contains("function updateConfigurationEditability"));
    assert!(
        ADMIN_EXPERIMENT_HTML.contains("updateConfigurationEditability(authoritativeExperiment)")
    );
    assert!(ADMIN_EXPERIMENT_HTML.contains("experiment.status !== 'inactive'"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("id=\"institutionInput\""));
    assert!(ADMIN_EXPERIMENT_HTML.contains("Build information"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("data-tab=\"sessions\""));
    assert!(!ADMIN_EXPERIMENT_HTML.contains("data-tab=\"load\""));
    assert!(ADMIN_EXPERIMENT_HTML.contains("data-tab=\"export\""));
    assert!(ADMIN_EXPERIMENT_HTML.contains("data-tab=\"details\""));
    assert!(ADMIN_EXPERIMENT_HTML.contains("id=\"sessionsPanel\""));
    assert!(ADMIN_EXPERIMENT_HTML.contains("id=\"loadPanel\""));
    assert!(ADMIN_EXPERIMENT_HTML.contains("Runtime session liveness"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("renderLoadChart"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("id=\"exportPanel\""));
    assert!(ADMIN_EXPERIMENT_HTML.contains("id=\"exportVariant\""));
    assert!(ADMIN_EXPERIMENT_HTML.contains("Session log"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("participant-assignments"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("id=\"icon-bot\""));
    assert!(ADMIN_EXPERIMENT_HTML.contains("id=\"icon-user\""));
    assert!(ADMIN_EXPERIMENT_HTML.contains("delete-participant"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("id=\"detailsPanel\""));
    assert!(ADMIN_EXPERIMENT_HTML.contains("id=\"sessionDetail\""));
    assert!(ADMIN_EXPERIMENT_HTML.contains("session-workspace"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("session-picker"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("id=\"menuButton\""));
    assert!(ADMIN_EXPERIMENT_HTML.contains("sidebar-open"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("session-facts"));
    assert!(ADMIN_EXPERIMENT_HTML.contains("events-header"));
    assert!(!ADMIN_EXPERIMENT_HTML.contains("▣"));
    assert!(!ADMIN_EXPERIMENT_HTML.contains("≡"));
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

/// Confirms legacy consent-template macros never reach participants or evidence hashes.
#[test]
fn consent_templates_expand_to_complete_participant_text() {
    let mut config = ExperimentConfig::default();
    config.direct.participant_information_version = "local-v3".to_string();
    config.direct.consents = vec![crate::config::ConsentItemConfig {
        id: "template".to_string(),
        title: "Template".to_string(),
        required: true,
        body: "Version {{LOCAL_INFORMATION_VERSION}}; controller {{INSTITUTION_NAME}}; provider {{SPEECHMATICS_ENTITY_AND_SERVICE}} in {{SPEECHMATICS_PROCESSING_REGION}}."
            .to_string(),
    }];
    let settings = StoredGameSettings {
        institution: "Saarland University".to_string(),
        speechmatics_realtime_url: "wss://us.rt.speechmatics.com/v2".to_string(),
        ..StoredGameSettings::default()
    };
    let expanded = expanded_consent_items(&config, &settings);
    assert_eq!(
        expanded[0].body,
        "Version local-v3; controller Saarland University; provider Speechmatics in the European Union."
    );
    assert!(!expanded[0].body.contains("{{"));
    let hash = consent_configuration_hash(&config, &settings).unwrap();
    assert!(hash.starts_with("sha256:"));
}

/// Confirms administrator cookies remain first-party and work on loopback HTTP in Brave.
#[test]
fn administrator_cookie_matches_transport_security() {
    let mut config = ExperimentConfig::default();
    config.server.public_base_url = "http://127.0.0.1:8000".to_string();
    let loopback = administrator_cookie("token", ADMIN_ABSOLUTE_SECONDS, &config);
    assert!(loopback.contains("Max-Age=2592000"));
    assert!(loopback.contains("HttpOnly; SameSite=Strict"));
    assert!(!loopback.contains("Secure"));
    assert!(!loopback.contains("Domain="));

    config.server.public_base_url = "https://experiments.example.edu".to_string();
    let public = administrator_cookie("token", ADMIN_ABSOLUTE_SECONDS, &config);
    assert!(public.contains("Secure; HttpOnly; SameSite=Strict"));
}

/// Confirms lifecycle transitions preserve the semantic boundary between results and deletion.
#[test]
fn experiment_lifecycle_rejects_ambiguous_shortcuts() {
    assert!(ExperimentLifecycle::Inactive.can_transition_to(ExperimentLifecycle::Testing));
    assert!(ExperimentLifecycle::Testing.can_transition_to(ExperimentLifecycle::Active));
    assert!(ExperimentLifecycle::Active.can_transition_to(ExperimentLifecycle::Completed));
    assert!(ExperimentLifecycle::Completed.can_transition_to(ExperimentLifecycle::Archived));
    assert!(ExperimentLifecycle::Archived.can_transition_to(ExperimentLifecycle::Inactive));
    assert!(!ExperimentLifecycle::Testing.can_transition_to(ExperimentLifecycle::Completed));
    assert!(!ExperimentLifecycle::Active.can_transition_to(ExperimentLifecycle::Archived));
    assert!(!ExperimentLifecycle::Archived.can_transition_to(ExperimentLifecycle::Active));
}

/// Confirms game YAML rejects embedded credentials in favor of the secret store.
#[test]
fn game_configuration_rejects_credential_shaped_fields() {
    validate_game_config_contains_no_secrets(&json!({"difficulty": 2})).unwrap();
    let error = validate_game_config_contains_no_secrets(&json!({
        "provider": {"access_token": "sentinel"}
    }))
    .unwrap_err();
    assert!(error.to_string().contains("provider.access_token"));
    assert!(error.to_string().contains("Game secrets"));
}

/// Preserves semantic references while erasing actual values under credential-shaped keys.
#[test]
fn persisted_configuration_distinguishes_secret_references_from_values() {
    let mut reference = json!({"api_key": "game.agent_secret"});
    redact_secret_fields(&mut reference);
    assert_eq!(reference["api_key"], "game.agent_secret");

    let mut credential = json!({"api_key": "sentinel-secret-value"});
    redact_secret_fields(&mut credential);
    assert_eq!(credential["api_key"], "");
}

/// Repairs a historically blanked reference when its separately stored secret survives.
#[test]
fn hydrated_configuration_repairs_legacy_agent_secret_references() {
    let mut config = ExperimentConfig::default();
    config.agents = AgentsConfig {
        mode: AgentsMode::HumanVsAgent,
        human_vs_agent: Some(crate::config::HumanVsAgentConfig {
            factory: Some("secret.agent".to_string()),
            config: json!({"api_key": ""}),
            ..Default::default()
        }),
    };
    config.game_secrets.insert(
        "agent_secret_agent_api_key".to_string(),
        "sentinel-secret-value".to_string(),
    );

    repair_legacy_agent_secret_references(&[SecretAgentFactory.definition()], &mut config);

    assert_eq!(
        config.agents.human_vs_agent.unwrap().config["api_key"],
        "game.agent_secret_agent_api_key"
    );
}

/// Confirms provider credentials come only from the explicit game secret store.
#[tokio::test]
async fn administrator_can_store_and_explicitly_reveal_experiment_secrets() {
    let (mut config, _tmp) = sqlite_config();
    config.experiment.id = Some("secret-config".to_string());
    config.speechmatics.api_key = "in-memory-speech-secret".to_string();
    let descriptor = GameMetadata {
        id: "tiny-game".to_string(),
        name: "Tiny Game".to_string(),
        version: semver::Version::parse("0.4.0").unwrap(),
        build_manifest: json!({}),
    };
    let router = super::build_game_router(TinyAdapter, config, descriptor, |_| {
        Ok(ServeOptions::default())
    })
    .await
    .unwrap();
    authenticate_test_admin(router.clone()).await.unwrap();

    let (status, _) = json_request(
        router.clone(),
        http::Method::POST,
        "/api/admin/experiments",
        json!({"experiment_id": "secret-config"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, current) = json_request(
        router.clone(),
        http::Method::GET,
        "/api/admin/experiments/secret-config/config",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!serde_json::to_string(&current)
        .unwrap()
        .contains("in-memory-speech-secret"));
    assert_eq!(current["configured_secrets"], json!([]));

    let (status, catalogue) = json_request(
        router.clone(),
        http::Method::GET,
        "/api/admin/experiments",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(catalogue["game_provider_secrets"][0]["source"], "missing");

    let (status, _) = json_request(
        router.clone(),
        http::Method::POST,
        "/api/admin/game/secrets/reveal",
        json!({"key": "speechmatics.api_key"}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, updated) = json_request(
        router.clone(),
        http::Method::POST,
        "/api/admin/game/settings",
        json!({
            "expected_revision": 1,
            "institution": "Test University",
            "admin_allowed_ip_ranges": [],
            "speechmatics_realtime_url": "wss://eu.rt.speechmatics.com/v2",
            "tts_base_url": "wss://api.elevenlabs.io",
            "secret_updates": {
                "speechmatics.api_key": "stored-speechmatics-secret",
                "tts.api_key": "stored-elevenlabs-secret"
            },
            "secret_deletions": []
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["revision"], 2);
    let (status, catalogue) = json_request(
        router.clone(),
        http::Method::GET,
        "/api/admin/experiments",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(catalogue["game_provider_secrets"][0]["source"], "game");
    assert_eq!(catalogue["game_provider_secrets"][1]["source"], "game");
    let (status, revealed) = json_request(
        router.clone(),
        http::Method::POST,
        "/api/admin/game/secrets/reveal",
        json!({"key": "tts.api_key"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(revealed["value"], "stored-elevenlabs-secret");

    let (status, saved) = json_request(
        router.clone(),
        http::Method::POST,
        "/api/admin/experiments/secret-config/config",
        json!({
            "expected_revision": 1,
            "config": current["experiment"]["config"].clone(),
            "game_yaml": "difficulty: 2\n",
            "secret_updates": {"game.copy_key": "copy-me"},
            "secret_deletions": [],
            "change_summary": "Configure game"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    let (status, revealed) = json_request(
        router,
        http::Method::POST,
        "/api/admin/experiments/secret-config/secrets/reveal",
        json!({"key": "game.copy_key"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(revealed["value"], "copy-me");
}

/// Confirms endpoint defaults are copied once and explicit experiment endpoints always win.
#[tokio::test]
async fn experiment_provider_endpoints_are_revisioned_and_override_game_defaults() {
    let (config, _tmp) = sqlite_config();
    let descriptor = GameMetadata {
        id: "tiny-game".to_string(),
        name: "Tiny Game".to_string(),
        version: semver::Version::parse("0.4.0").unwrap(),
        build_manifest: json!({}),
    };
    let router = super::build_game_router(TinyAdapter, config, descriptor, |_| {
        Ok(ServeOptions::default())
    })
    .await
    .unwrap();
    authenticate_test_admin(router.clone()).await.unwrap();

    let (status, first_defaults) = json_request(
        router.clone(),
        http::Method::POST,
        "/api/admin/game/settings",
        json!({
            "expected_revision": 1,
            "institution": "",
            "admin_allowed_ip_ranges": [],
            "speechmatics_realtime_url": "wss://speech-default-one.test/v2",
            "tts_base_url": "wss://tts-default-one.test",
            "secret_updates": {},
            "secret_deletions": []
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{first_defaults}");

    let mut sparse = serde_json::to_value(ExperimentConfig::default()).unwrap();
    sparse["speechmatics"]
        .as_object_mut()
        .unwrap()
        .remove("realtime_url");
    sparse["tts"].as_object_mut().unwrap().remove("base_url");
    let (status, created) = json_request(
        router.clone(),
        http::Method::POST,
        "/api/admin/experiments",
        json!({"experiment_id": "inherited-endpoints", "config": sparse}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{created}");

    let (status, inherited) = json_request(
        router.clone(),
        http::Method::GET,
        "/api/admin/experiments/inherited-endpoints/config",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        inherited["experiment"]["config"]["speechmatics"]["realtime_url"],
        "wss://speech-default-one.test/v2"
    );
    assert_eq!(
        inherited["experiment"]["config"]["tts"]["base_url"],
        "wss://tts-default-one.test"
    );

    let (status, second_defaults) = json_request(
        router.clone(),
        http::Method::POST,
        "/api/admin/game/settings",
        json!({
            "expected_revision": 2,
            "institution": "",
            "admin_allowed_ip_ranges": [],
            "speechmatics_realtime_url": "wss://speech-default-two.test/v2",
            "tts_base_url": "wss://tts-default-two.test",
            "secret_updates": {},
            "secret_deletions": []
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{second_defaults}");

    let (status, unchanged) = json_request(
        router.clone(),
        http::Method::GET,
        "/api/admin/experiments/inherited-endpoints/config",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        unchanged["experiment"]["config"]["speechmatics"]["realtime_url"],
        "wss://speech-default-one.test/v2"
    );
    assert_eq!(
        unchanged["experiment"]["config"]["tts"]["base_url"],
        "wss://tts-default-one.test"
    );

    let mut explicit = serde_json::to_value(ExperimentConfig::default()).unwrap();
    explicit["speechmatics"]["realtime_url"] = json!("wss://speech-experiment.test/v2");
    explicit["tts"]["base_url"] = json!("wss://tts-experiment.test");
    let (status, created) = json_request(
        router.clone(),
        http::Method::POST,
        "/api/admin/experiments",
        json!({"experiment_id": "explicit-endpoints", "config": explicit}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let (status, selected) = json_request(
        router,
        http::Method::GET,
        "/api/admin/experiments/explicit-endpoints/config",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        selected["experiment"]["config"]["speechmatics"]["realtime_url"],
        "wss://speech-experiment.test/v2"
    );
    assert_eq!(
        selected["experiment"]["config"]["tts"]["base_url"],
        "wss://tts-experiment.test"
    );
}

/// Confirms the first experiment in a new game starts with editable provider URLs populated.
#[tokio::test]
async fn new_experiment_populates_default_provider_endpoints() {
    let (config, _tmp) = sqlite_config();
    let descriptor = GameMetadata {
        id: "tiny-game".to_string(),
        name: "Tiny Game".to_string(),
        version: semver::Version::parse("0.4.0").unwrap(),
        build_manifest: json!({}),
    };
    let router = super::build_game_router(TinyAdapter, config, descriptor, |_| {
        Ok(ServeOptions::default())
    })
    .await
    .unwrap();
    authenticate_test_admin(router.clone()).await.unwrap();

    let (status, created) = json_request(
        router.clone(),
        http::Method::POST,
        "/api/admin/experiments",
        json!({"experiment_id": "default-provider-endpoints"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{created}");

    let (status, response) = json_request(
        router,
        http::Method::GET,
        "/api/admin/experiments/default-provider-endpoints/config",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        response["experiment"]["config"]["speechmatics"]["realtime_url"],
        "wss://eu.rt.speechmatics.com/v2"
    );
    assert_eq!(
        response["experiment"]["config"]["tts"]["base_url"],
        "wss://api.elevenlabs.io"
    );
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
    authenticate_test_admin(router.clone()).await.unwrap();

    let page = admin_raw_request(router.clone(), http::Method::GET, "/admin/privacy").await;
    assert_eq!(page.status(), StatusCode::OK);
    let page_html = to_bytes(page.into_body(), usize::MAX).await.unwrap();
    let page_html = String::from_utf8_lossy(&page_html);
    assert!(page_html.contains("Installation-wide privacy behavior"));
    assert!(!page_html.contains("DPO platform assessment"));
    assert!(page_html.contains("href=\"/admin/experiments\""));

    let (status, privacy) = json_request(
        router.clone(),
        http::Method::GET,
        "/api/admin/privacy",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(privacy["privacy_contract_version"], "2");
    assert!(privacy.get("experiment_count").is_none());
    assert_eq!(privacy["raw_audio_stored_by_parlando"], false);
    assert!(privacy["overview"]["primary_storage"]
        .as_str()
        .unwrap()
        .contains("SQLite database file"));
    assert_eq!(
        privacy["not_retained"][0]["category"],
        "Participant IP addresses"
    );
    assert_eq!(privacy["exports"]["available"], true);
    assert_eq!(privacy["exports"]["variant"], "corpus");
    assert_eq!(privacy["exports"]["schema_id"], "parlando.corpus.v1");
    assert!(privacy["exports"]["timing"]
        .as_str()
        .unwrap()
        .contains("game_time_ms, measured in milliseconds from game start"));
    assert!(privacy["exports"]["timing"]
        .as_str()
        .unwrap()
        .contains("speech start and end positions use the same game clock"));
    assert!(privacy["exports"]["selection_rule"]
        .as_str()
        .unwrap()
        .contains("writes only the categories listed below"));
    assert_eq!(
        privacy["exports"]["included_fields"][0]["section"],
        "Manifest"
    );
    assert_eq!(
        privacy["exports"]["included_fields"][0]["fields"][0],
        "export_schema_version"
    );
    assert!(privacy["exports"].get("excluded").is_none());
    assert!(privacy["exports"]["not_written"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item.as_str().unwrap().contains("Consent declarations")));
    assert_eq!(privacy["participant_deletion"]["available"], true);
    assert_eq!(privacy["consent_evidence"]["available"], true);
    assert_eq!(privacy["external_services"], json!([]));
    assert_eq!(privacy["not_retained"].as_array().unwrap().len(), 1);
    assert!(privacy["configuration"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["setting"] == "Voice communication" && item["status"] == "Disabled"));
    assert!(privacy["configuration"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["setting"] == "Speech transcription" && item["status"] == "Disabled"));
    assert!(!privacy["storage"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["category"] == "Final voice transcripts"));

    let (experiment_status, experiment_privacy) = json_request(
        router.clone(),
        http::Method::GET,
        "/api/admin/experiments/step5/privacy",
        Value::Null,
    )
    .await;
    assert_eq!(experiment_status, StatusCode::OK);
    assert_eq!(experiment_privacy["experiment_id"], "step5");
    assert!(experiment_privacy
        .get("experiment_config_revision")
        .is_none());
    assert!(experiment_privacy.get("game_version").is_none());
    assert!(experiment_privacy.get("software").is_none());
    assert!(experiment_privacy["consent_evidence"]["detail"]
        .as_str()
        .unwrap()
        .contains("cryptographic fingerprint"));

    let markdown = admin_raw_request(
        router,
        http::Method::GET,
        "/api/admin/experiments/step5/privacy.md",
    )
    .await;
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
    let markdown_body = String::from_utf8_lossy(&markdown_body);
    assert!(markdown_body.contains("# Data processing record — experiment step5"));
    assert!(markdown_body.contains("## Scope and responsibility"));
    assert!(markdown_body.contains("## Privacy-relevant experiment settings"));
    assert!(!markdown_body.contains("Experiment configuration revision"));
    assert!(!markdown_body.contains("Game version"));
    assert!(!markdown_body.contains("Technical provenance"));
    assert!(markdown_body.contains("| Voice communication | Disabled |"));
    assert!(markdown_body.contains("| Speech transcription | Disabled |"));
    assert!(markdown_body.contains("## Data retained in Parlando's SQLite database"));
    assert!(markdown_body.contains("| Information | Why it is processed |"));
    assert!(markdown_body.contains("randomly generates a three-word pseudonym"));
    assert!(markdown_body.contains("adverb-adjective-animal"));
    assert!(markdown_body.contains("not derived from a name, contact detail, participant IP address, or other external identifier"));
    assert!(markdown_body.contains("## Data not retained by Parlando"));
    assert!(markdown_body.contains("Participant IP addresses"));
    assert!(markdown_body.contains("Parlando does not write participant network addresses"));
    assert!(markdown_body.contains("web server, hosting service, firewall"));
    assert!(markdown_body.contains("## Corpus export"));
    assert!(markdown_body.contains("The corpus is pseudonymized, not anonymous"));
    assert!(markdown_body.contains("complete game-specific configuration"));
    assert!(
        !markdown_body[..markdown_body.find("## Corpus export").unwrap()]
            .contains("complete game-specific configuration")
    );
    assert!(markdown_body.contains("| Part of the corpus | What it contains |"));
    assert!(markdown_body.contains("Stored or used by Parlando but not written to the corpus"));
    assert!(
        markdown_body.contains("does not copy a database-shaped document and then remove columns")
    );
    assert!(!markdown_body.contains("The export excludes"));
    assert!(markdown_body.contains("## Consent evidence and deletion"));
    assert!(markdown_body.contains("## What the institution must add"));
    assert!(!markdown_body.contains("identity source"));
    assert!(!markdown_body.contains("external participant linkage"));
    assert!(!markdown_body.contains("external recruitment"));
    assert!(!markdown_body.to_lowercase().contains("speechmatics"));
    assert!(!markdown_body.contains("streamed to the configured transcription provider"));
    assert!(!markdown_body.contains("requires content review"));
    assert!(!markdown_body.contains("inspect game configuration"));
    assert!(!markdown_body.contains("rare behavior"));
    assert!(markdown_body.contains(
        "review participant-authored typed messages and remove explicit identifying information"
    ));
    assert!(!markdown_body.contains("Final voice transcripts"));
    assert!(!markdown_body.contains("Raw microphone audio"));
    assert!(!markdown_body.contains("Browser voice diagnostics"));
    assert!(!markdown_body.contains("stored state snapshot(s)"));
    assert!(!markdown_body.contains("stored typed message(s)"));
    assert!(!markdown_body.contains("historical diagnostic event(s)"));
    assert!(!markdown_body.contains("Recognition model"));
    assert!(!markdown_body.contains("Synthesis model"));
}

/// Confirms an enabled transcription path reports its exact processor destination.
#[tokio::test]
async fn experiment_privacy_reports_only_the_enabled_speech_path() {
    let (mut config, _tmp) = sqlite_config();
    config.voice.enabled = true;
    config.transcription.enabled = true;
    config.speechmatics.api_key = "test-key".to_string();
    let router = build_router(TinyAdapter, config, ServeOptions::default())
        .await
        .unwrap();
    authenticate_test_admin(router.clone()).await.unwrap();

    let (status, privacy) = json_request(
        router,
        http::Method::GET,
        "/api/admin/experiments/step5/privacy",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(privacy["configuration"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["setting"] == "Speech transcription"
            && item["status"] == "Enabled"
            && item["detail"]
                .as_str()
                .unwrap()
                .contains("wss://eu.rt.speechmatics.com/v2")));
    let configuration = privacy["configuration"].to_string();
    assert!(!configuration.contains("enhanced"));
    assert!(!configuration.contains("en-US"));
    assert!(privacy["storage"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["category"] == "Final voice transcripts"));
    assert!(privacy["storage"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["category"] == "Final voice transcripts")
        .unwrap()["detail"]
        .as_str()
        .unwrap()
        .contains("speech start and end positions on the same game clock"));
    assert!(privacy["not_retained"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["category"] == "Raw microphone audio"));
    assert_eq!(privacy["external_services"][0]["service"], "speechmatics");
    assert!(privacy["exports"]["detail"]
        .as_str()
        .unwrap()
        .contains("final voice transcripts"));
}

/// Confirms the corpus is one experiment document with dashboard IDs and relative timing.
#[test]
fn corpus_export_is_nested_informative_and_structurally_minimized() {
    let full = json!({
        "participants": [
            {"participant_id": 7, "research_id": "calm-blue-otter", "participant_kind": "human", "external_id": "recruitment-id"},
            {"participant_id": 8, "research_id": "agent:tiny@1", "participant_kind": "agent", "metadata": {"agent_name": "tiny", "agent_version": "1"}}
        ],
        "experiment": {"experiment_id": "study", "game_version": "0.4.0", "config_revision": 2, "config": {"game": {"condition": "example"}, "direct": {"participant_information_url": "secret"}}},
        "sessions": [{"session_id": 3, "dialogue_id": "softly-amber-harbor", "config_revision": 2, "game_version": "0.4.0", "mode": "direct", "status": "completed", "created_at": "2026-01-01T00:00:00Z", "started_at": "2026-01-01T00:00:01Z", "completed_at": "2026-01-01T00:00:05Z", "completion": {"outcome": "success"}}],
        "session_participants": [
            {"session_id": 3, "participant_id": 7, "participant_session_id": "ps_secret", "role": "A"},
            {"session_id": 3, "participant_id": 8, "participant_session_id": "ps_agent", "role": "B"}
        ],
        "session_events": [
            {
                "session_id": 3,
                "event_index": 4,
                "event_type": "conversation_message",
                "actor_participant_id": 7,
                "actor_role": "A",
                "payload": {"text": "hello", "origin": "typed", "sender_participant_session_id": "ps_secret"},
                "game_time_ms": 3000
            },
            {
                "session_id": 3,
                "event_index": 5,
                "event_type": "conversation_message",
                "actor_participant_id": 7,
                "actor_role": "A",
                "payload": {"text": "spoken hello", "origin": "voice_transcript", "metadata": {"start_game_time_ms": 3200, "end_game_time_ms": 3900}},
                "game_time_ms": 4000
            },
            {
                "session_id": 3,
                "event_index": 6,
                "event_type": "log",
                "actor_participant_id": null,
                "actor_role": null,
                "payload": {"source": "game", "text": "game diagnostic"},
                "game_time_ms": 4100
            },
            {
                "session_id": 3,
                "event_index": 7,
                "event_type": "log",
                "actor_participant_id": 8,
                "actor_role": "B",
                "payload": {"source": "agent", "text": "agent diagnostic"},
                "game_time_ms": 4200
            }
        ]
    });
    let corpus = corpus_experiment_export(full.clone(), "2").unwrap();
    let encoded = serde_json::to_string(&corpus).unwrap();
    assert_eq!(corpus["release_status"], "corpus_candidate");
    assert!(encoded.contains("calm-blue-otter"));
    assert!(encoded.contains("softly-amber-harbor"));
    assert!(!encoded.contains("2026-01-01"));
    assert!(!encoded.contains("recruitment-id"));
    assert!(!encoded.contains("ps_secret"));
    assert!(!encoded.contains("2026-01-01"));
    assert!(!encoded.contains("participant_information_url"));
    assert_eq!(
        corpus["experiment"]["configuration"]["condition"],
        "example"
    );
    assert_eq!(
        corpus["experiment"]["sessions"][0]["events"][0]["text"],
        "hello"
    );
    assert_eq!(
        corpus["experiment"]["sessions"][0]["events"][0]["game_time_ms"],
        3_000
    );
    assert_eq!(
        corpus["experiment"]["sessions"][0]["events"][1]["utterance_timing"],
        json!({"origin": "game_clock", "start_ms": 3_200, "end_ms": 3_900})
    );
    assert_eq!(corpus["data_inventory"]["logs"], 2);
    assert_eq!(
        corpus["experiment"]["sessions"][0]["events"][2],
        json!({
            "index": 6,
            "game_time_ms": 4_100,
            "kind": "log",
            "source": "game",
            "participant_id": null,
            "role": null,
            "text": "game diagnostic",
        })
    );
    assert_eq!(
        corpus["experiment"]["sessions"][0]["events"][3],
        json!({
            "index": 7,
            "game_time_ms": 4_200,
            "kind": "log",
            "source": "agent",
            "participant_id": "agent:tiny@1",
            "role": "B",
            "text": "agent diagnostic",
        })
    );
    assert_eq!(
        corpus["experiment"]["sessions"][0]["metadata"]["time_to_start_ms"],
        1_000
    );
    assert_eq!(
        corpus["experiment"]["sessions"][0]["metadata"]["duration_ms"],
        4_000
    );

    let csv = export_csv(&corpus);
    assert_eq!(
        csv.lines()
            .filter(|line| line.starts_with("manifest,"))
            .count(),
        1
    );
    assert_eq!(
        csv.lines()
            .filter(|line| line.starts_with("experiment,"))
            .count(),
        1
    );
    assert_eq!(
        csv.lines()
            .filter(|line| line.starts_with("session,"))
            .count(),
        1
    );

    let mut invalid = full;
    invalid["session_events"][0]["game_time_ms"] = json!(-1);
    assert!(corpus_experiment_export(invalid, "2")
        .unwrap_err()
        .message
        .contains("without valid game time"));

    let schema: Value = serde_json::from_str(CORPUS_EXPORT_SCHEMA_V1).unwrap();
    assert_eq!(
        schema["$id"],
        "https://parlando.dev/schemas/parlando.corpus.v1.json"
    );
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

/// Confirms lifecycle changes gate intake and persist across restarts.
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
    let (status, testing) = json_request(
        router.clone(),
        http::Method::POST,
        "/api/admin/experiment/status",
        json!({"status": "testing"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(testing["status"], "testing");
    let (status, _) = json_request(
        router.clone(),
        http::Method::POST,
        "/api/participants",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
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
    assert!(participant["participant_id"].is_string());
    assert!(participant.get("participant_session_id").is_none());

    let (status, _) = json_request(
        router.clone(),
        http::Method::POST,
        "/api/admin/experiment/status",
        json!({"status": "inactive"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = json_request(
        router.clone(),
        http::Method::POST,
        "/api/participants",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    let (status, archived) = json_request(
        router.clone(),
        http::Method::POST,
        "/api/admin/experiment/status",
        json!({"status": "archived"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(archived["status"], "archived");

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
    assert_eq!(experiment["experiment"]["status"], "archived");
}

/// Confirms incomplete hosted voice services remain editable but block testing and active intake.
#[tokio::test]
async fn hosted_voice_services_without_credentials_block_experiment_start() {
    let (mut config, _tmp) = sqlite_config();
    config.transcription.enabled = true;
    config.speechmatics.api_key.clear();
    let router = super::build_router(TinyAdapter, config, ServeOptions::default())
        .await
        .expect("an incomplete inactive draft still builds");
    authenticate_test_admin(router.clone()).await.unwrap();

    let (config_status, dashboard_config) = json_request(
        router.clone(),
        http::Method::GET,
        "/api/admin/experiments/step5/config",
        Value::Null,
    )
    .await;
    assert_eq!(config_status, StatusCode::OK);
    assert!(dashboard_config["activation_issues"]
        .as_array()
        .unwrap()
        .iter()
        .any(|issue| issue.as_str().unwrap().contains("Speechmatics API key")));
    assert_eq!(
        dashboard_config["experiment"]["config"]["session"]["waiting_session_timeout_seconds"],
        600
    );
    assert_eq!(
        dashboard_config["experiment"]["config"]["session"]["session_max_lifetime_seconds"],
        14_400
    );
    assert_eq!(
        dashboard_config["experiment"]["config"]["capacity"]["max_active_sessions"],
        30
    );
    assert_eq!(
        dashboard_config["experiment"]["config"]["capacity"]["storage_reserve_megabytes"],
        256
    );

    let (status, problem) = json_request(
        router,
        http::Method::POST,
        "/api/admin/experiment/status",
        json!({"status": "testing"}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(problem["raw"]
        .as_str()
        .unwrap()
        .contains("Speechmatics API key"));
}

/// Confirms catalogue badges and lifecycle controls receive identical agent readiness.
#[tokio::test]
async fn catalogue_readiness_includes_missing_agent_secret_references() {
    let (mut config, _tmp) = sqlite_config();
    config.agents = AgentsConfig {
        mode: AgentsMode::HumanVsAgent,
        human_vs_agent: Some(crate::config::HumanVsAgentConfig {
            factory: Some("secret.agent".to_string()),
            config: json!({"api_key": "game.agent_secret"}),
            ..Default::default()
        }),
    };
    let router = super::build_router(
        TinyAdapter,
        config,
        ServeOptions {
            agent_factory: Some(Arc::new(SecretAgentFactory)),
            ..ServeOptions::default()
        },
    )
    .await
    .expect("a missing agent secret remains an editable inactive draft");
    authenticate_test_admin(router.clone()).await.unwrap();

    let (_, catalogue) = json_request(
        router.clone(),
        http::Method::GET,
        "/api/admin/experiments",
        Value::Null,
    )
    .await;
    let experiment = &catalogue["experiments"][0];
    assert_eq!(experiment["configuration_valid"], true);
    assert_eq!(experiment["runnable"], false);
    assert!(
        experiment["runnable_issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|issue| issue.as_str().unwrap().contains("game.agent_secret")),
        "unexpected catalogue readiness: {experiment:#}"
    );

    let (_, dashboard_config) = json_request(
        router,
        http::Method::GET,
        "/api/admin/experiments/step5/config",
        Value::Null,
    )
    .await;
    assert_eq!(
        experiment["runnable_issues"], dashboard_config["activation_issues"],
        "catalogue badges and lifecycle controls must share readiness diagnostics"
    );
}

/// Confirms test sessions and their dependent rows are absent from every export variant input.
#[test]
fn export_filter_removes_testing_session_graph() {
    let mut exported = json!({
        "participants": [
            {"participant_id": 1, "research_id": "test-only"},
            {"participant_id": 2, "research_id": "research"}
        ],
        "sessions": [
            {"session_id": 10, "purpose": "testing"},
            {"session_id": 20, "purpose": "research"}
        ],
        "session_participants": [
            {"session_id": 10, "participant_id": 1},
            {"session_id": 20, "participant_id": 2}
        ],
        "consent_declarations": [
            {"session_id": 10, "participant_id": 1, "purpose": "testing"},
            {"session_id": null, "participant_id": 1, "purpose": "testing"},
            {"session_id": null, "participant_id": 2, "purpose": "research"}
        ],
        "session_events": [
            {"session_id": 10},
            {"session_id": 20}
        ]
    });

    exclude_testing_sessions(&mut exported);

    assert_eq!(exported["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(
        exported["session_participants"].as_array().unwrap().len(),
        1
    );
    assert_eq!(exported["session_events"].as_array().unwrap().len(), 1);
    assert_eq!(exported["participants"].as_array().unwrap().len(), 1);
    assert_eq!(exported["participants"][0]["participant_id"], 2);
    assert_eq!(
        exported["consent_declarations"].as_array().unwrap().len(),
        1
    );
    assert_eq!(exported["consent_declarations"][0]["participant_id"], 2);
}

/// Confirms one compiled-game router can activate and isolate two experiment runtimes.
#[tokio::test]
async fn compiled_game_router_hosts_multiple_experiments() {
    let mut config = step_five_config();
    config.experiment.id = Some("primary".to_string());
    let descriptor = GameMetadata {
        id: "tiny-game".to_string(),
        name: "Tiny Game".to_string(),
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

    let (_, empty_catalogue) = json_request(
        router.clone(),
        http::Method::GET,
        "/api/admin/experiments",
        Value::Null,
    )
    .await;
    assert!(empty_catalogue["experiments"]
        .as_array()
        .unwrap()
        .is_empty());

    let (create_status, created) = json_request(
        router.clone(),
        http::Method::POST,
        "/api/admin/experiments",
        json!({"experiment_id": "primary", "notes": "# Primary\n\nPrivate catalogue note"}),
    )
    .await;
    assert_eq!(create_status, StatusCode::OK);
    assert_eq!(created["experiment_id"], "primary");

    let (catalogue_status, catalogue) = json_request(
        router.clone(),
        http::Method::GET,
        "/api/admin/experiments",
        Value::Null,
    )
    .await;
    assert_eq!(catalogue_status, StatusCode::OK);
    assert_eq!(
        catalogue["experiments"][0]["notes"],
        "# Primary\n\nPrivate catalogue note"
    );
    assert_eq!(catalogue["game"]["name"], "Tiny Game");
    assert_eq!(catalogue["experiments"][0]["configuration_valid"], true);
    assert_eq!(catalogue["experiments"][0]["runnable"], true);

    let (clone_status, _) = json_request(
        router.clone(),
        http::Method::POST,
        "/api/admin/experiments/primary/clone",
        json!({"experiment_id": "primary-clone"}),
    )
    .await;
    assert_eq!(clone_status, StatusCode::OK);
    let (clone_definition_status, clone_definition) = json_request(
        router.clone(),
        http::Method::GET,
        "/api/admin/experiments/primary-clone/config",
        Value::Null,
    )
    .await;
    assert_eq!(clone_definition_status, StatusCode::OK);
    assert!(clone_definition["experiment"]["notes"].is_null());

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
        json!({"experiment_id": "secondary"}),
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
    stored_config["experiment"]["config"]["session"]["waiting_session_timeout_seconds"] =
        Value::Number(300.into());
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

    for (experiment_id, lifecycle) in [("primary", "active"), ("secondary", "testing")] {
        let (status, body) = json_request(
            router.clone(),
            http::Method::POST,
            &format!("/api/admin/runtime/{experiment_id}/experiment/status"),
            json!({"status": lifecycle}),
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
        primary_participant["participant_id"],
        secondary_participant["participant_id"]
    );
    for participant in [&primary_participant, &secondary_participant] {
        TEST_PARTICIPANT_CREDENTIALS.lock().unwrap().insert(
            participant["participant_id"].as_str().unwrap().to_string(),
            participant["participant_credential"]
                .as_str()
                .unwrap()
                .to_string(),
        );
    }
    let primary_id = primary_participant["participant_id"].as_str().unwrap();
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
        "/e/primary/api/sessions",
        json!({"participant_session_id": primary_id}),
    )
    .await;
    assert_eq!(room_status, StatusCode::OK);
    let public_session_id = room["public_session_id"].as_str().unwrap();
    let (session_status, session) = json_request(
        router.clone(),
        http::Method::POST,
        &format!("/e/primary/api/sessions/{public_session_id}/game-session"),
        json!({"participant_session_id": primary_id}),
    )
    .await;
    assert_eq!(session_status, StatusCode::OK);
    assert_eq!(
        session["websocket_url"],
        format!("/e/primary/ws/game/{public_session_id}")
    );
    let game_ticket = session["token"].as_str().unwrap();
    let (base_url, server) = spawn_test_server(router.clone()).await;
    let host = base_url.trim_start_matches("http://");
    let (mut game_socket, _) = connect_async(format!(
        "ws://{host}/e/primary/ws/game/{public_session_id}?token={game_ticket}"
    ))
    .await
    .expect("experiment dispatcher must preserve the WebSocket upgrade handle");
    let presence = read_ws_type(&mut game_socket, "presence").await;
    assert_eq!(presence["presence"]["A"]["connected"], true);
    game_socket.close(None).await.unwrap();
    server.abort();
    let (_, process_load) = json_request(
        router.clone(),
        http::Method::GET,
        "/api/admin/load",
        Value::Null,
    )
    .await;
    assert_eq!(process_load["current"]["capacity"]["waiting_sessions"], 1);
    assert!(process_load["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["experiment_id"] == "primary"));
    let (_, secondary_config) = json_request(
        router.clone(),
        http::Method::GET,
        "/e/secondary/api/config",
        Value::Null,
    )
    .await;
    assert_eq!(secondary_config["institution"], "Test University");
    assert_eq!(secondary_config["experiment_status"], "testing");
    assert!(secondary_config.get("participant_title").is_none());

    let (_, catalogue) = json_request(
        router.clone(),
        http::Method::GET,
        "/api/admin/experiments",
        Value::Null,
    )
    .await;
    assert_eq!(catalogue["game"]["name"], "Tiny Game");
    assert_eq!(catalogue["experiments"].as_array().unwrap().len(), 3);
    let (_, privacy) = json_request(
        router.clone(),
        http::Method::GET,
        "/api/admin/privacy",
        Value::Null,
    )
    .await;
    assert!(privacy.get("experiment_count").is_none());

    let restarted = build_game_router(TinyAdapter, config, descriptor, |_config| {
        Ok(ServeOptions::default())
    })
    .await
    .unwrap();
    authenticate_test_admin(restarted.clone()).await.unwrap();
    let (_, restarted_catalogue) = json_request(
        restarted,
        http::Method::GET,
        "/api/admin/experiments",
        Value::Null,
    )
    .await;
    assert!(restarted_catalogue["experiments"]
        .as_array()
        .unwrap()
        .iter()
        .all(|experiment| experiment["status"] == "inactive"));
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
    let descriptor = GameMetadata {
        id: "tiny-game".to_string(),
        name: "Tiny Game".to_string(),
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
    assert!(catalogue.get("running_experiment_id").is_none());
    let experiments = catalogue["experiments"].as_array().unwrap();
    assert_eq!(
        experiments
            .iter()
            .find(|experiment| experiment["experiment_id"] == "historical-on-0.4")
            .unwrap()["status"],
        "inactive"
    );
    assert_eq!(
        experiments
            .iter()
            .find(|experiment| experiment["experiment_id"] == "historical")
            .unwrap()["status"],
        "inactive"
    );
}

#[test]
fn admin_event_bundles_close_ready_before_later_disconnect() {
    let events = vec![
        admin_test_event(
            1,
            "session_created",
            None,
            json!({"public_session_id": "571EBA"}),
        ),
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
fn admin_timeline_includes_logs_with_readable_attribution() {
    let stored = crate::storage::StoredSessionEvent {
        event_id: 73,
        experiment_id: "experiment".to_string(),
        session_id: 1,
        event_index: 73,
        event_type: "log".to_string(),
        actor_participant_id: None,
        actor_role: None,
        payload: json!({
            "source": "game",
            "text": "accepted Great Tree action from role A: SetSun"
        }),
        game_state: None,
        game_time_ms: 73_000,
    };

    let events = important_admin_events(vec![stored]);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["title"], "Game log");
    assert_eq!(
        events[0]["text"],
        "accepted Great Tree action from role A: SetSun"
    );

    let bundles = admin_event_bundles(&events);
    assert_eq!(bundles.len(), 1);
    assert_eq!(bundles[0]["title"], "Game log");
    assert_eq!(
        bundles[0]["text"],
        "accepted Great Tree action from role A: SetSun"
    );
    assert_eq!(bundles[0]["housekeeping"], false);
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

impl crate::GameFactory for TinyAdapter {
    type Game = TinyAdapter;

    /// Creates one stateless tiny game for a test session.
    fn create(&self, _context: crate::GameSessionContext) -> Result<TinyAdapter> {
        Ok(TinyAdapter)
    }
}

impl crate::GameFactory for NoAvailableActionsAdapter {
    type Game = NoAvailableActionsAdapter;

    /// Creates one stateless affordance game for a test session.
    fn create(&self, _context: crate::GameSessionContext) -> Result<NoAvailableActionsAdapter> {
        Ok(NoAvailableActionsAdapter)
    }
}

impl crate::GameFactory for LossSummaryAdapter {
    type Game = LossSummaryAdapter;

    /// Creates one stateless loss-summary game for a test session.
    fn create(&self, _context: crate::GameSessionContext) -> Result<LossSummaryAdapter> {
        Ok(LossSummaryAdapter)
    }
}

struct NoopAgent;

#[async_trait]
impl Agent<TinyAdapter> for NoopAgent {
    async fn respond(
        &mut self,
        _available_actions: Option<Vec<TinyAction>>,
    ) -> Result<Option<AgentResponse<TinyAction>>> {
        Ok(None)
    }
}

struct NoopAgentFactory;

struct SecretAgentFactory;

/// Returns the stable identity shared by runtime-only test factories.
fn test_agent_identity() -> Result<AgentIdentity> {
    Ok(AgentIdentity {
        name: "TestAgent".to_string(),
        version: "1".to_string(),
    })
}

#[async_trait]
impl AgentFactory<TinyAdapter> for SecretAgentFactory {
    /// Declares one required factory-purpose credential for readiness tests.
    fn definition(&self) -> AgentDefinition {
        AgentDefinition {
            id: "secret.agent".to_string(),
            name: "Secret agent".to_string(),
            description: "Agent with a required credential.".to_string(),
            config_fields: vec![AgentConfigField {
                key: "api_key".to_string(),
                label: "API key".to_string(),
                help: "Required test credential.".to_string(),
                value: AgentConfigValue::SecretReference {
                    purpose: SecretPurpose::Factory,
                },
                required: true,
                default_value: Value::Null,
            }],
        }
    }

    /// Creates the existing no-op agent; readiness is tested before construction.
    async fn create(&self, _context: AgentContext) -> Result<Box<dyn Agent<TinyAdapter> + Send>> {
        Ok(Box::new(NoopAgent))
    }

    fn identity(&self, _settings: &Value) -> Result<AgentIdentity> {
        test_agent_identity()
    }
}

/// Returns minimal metadata for runtime-only test agents.
fn test_agent_definition() -> AgentDefinition {
    AgentDefinition {
        id: "test.agent".to_string(),
        name: "Test agent".to_string(),
        description: "Agent used by runtime tests.".to_string(),
        config_fields: Vec::new(),
    }
}

#[async_trait]
impl AgentFactory<TinyAdapter> for NoopAgentFactory {
    fn definition(&self) -> AgentDefinition {
        test_agent_definition()
    }

    async fn create(&self, _context: AgentContext) -> Result<Box<dyn Agent<TinyAdapter> + Send>> {
        Ok(Box::new(NoopAgent))
    }

    fn identity(&self, _settings: &Value) -> Result<AgentIdentity> {
        test_agent_identity()
    }
}

struct ScriptedAgent {
    script: VecDeque<Option<AgentResponse<TinyAction>>>,
}

#[async_trait]
impl Agent<TinyAdapter> for ScriptedAgent {
    async fn respond(
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
    match (message, action) {
        (Some(message), Some(action)) => Some(AgentResponse::action_and_message(action, message)),
        (Some(message), None) => Some(AgentResponse::message(message)),
        (None, Some(action)) => Some(AgentResponse::action(action)),
        (None, None) => None,
    }
}

#[async_trait]
impl AgentFactory<TinyAdapter> for ScriptedAgentFactory {
    fn definition(&self) -> AgentDefinition {
        test_agent_definition()
    }

    async fn create(&self, _context: AgentContext) -> Result<Box<dyn Agent<TinyAdapter> + Send>> {
        self.created.fetch_add(1, Ordering::SeqCst);
        let script = self.scripts.lock().unwrap().pop_front().unwrap_or_default();
        Ok(Box::new(ScriptedAgent {
            script: script.into(),
        }))
    }

    fn identity(&self, _settings: &Value) -> Result<AgentIdentity> {
        test_agent_identity()
    }
}

struct RecordingActionsAgent {
    seen_actions: Arc<Mutex<Vec<Option<Vec<TinyAction>>>>>,
}

#[async_trait]
impl Agent<TinyAdapter> for RecordingActionsAgent {
    async fn respond(
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

#[async_trait]
impl AgentFactory<TinyAdapter> for RecordingActionsAgentFactory {
    fn definition(&self) -> AgentDefinition {
        test_agent_definition()
    }

    async fn create(&self, _context: AgentContext) -> Result<Box<dyn Agent<TinyAdapter> + Send>> {
        Ok(Box::new(RecordingActionsAgent {
            seen_actions: self.seen_actions.clone(),
        }))
    }

    fn identity(&self, _settings: &Value) -> Result<AgentIdentity> {
        test_agent_identity()
    }
}

struct RecordingObservationsAgent {
    observations: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl Agent<TinyAdapter> for RecordingObservationsAgent {
    async fn start(&mut self, current_observation: TinyObservation) -> Result<()> {
        self.observations.lock().unwrap().push(format!(
            "state:{}:{}",
            current_observation.role, current_observation.done
        ));
        Ok(())
    }

    async fn observe_transition(
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

    async fn observe_message(&mut self, speaker: PlayerRole, text: String) -> Result<()> {
        self.observations
            .lock()
            .unwrap()
            .push(format!("message:{}:{text}", speaker.as_str()));
        Ok(())
    }

    async fn finish(&mut self, completion: TinySummary) -> Result<()> {
        self.observations.lock().unwrap().push(format!(
            "finish:{}:{}",
            completion.outcome, completion.dyad_score
        ));
        Ok(())
    }

    async fn respond(
        &mut self,
        _available_actions: Option<Vec<TinyAction>>,
    ) -> Result<Option<AgentResponse<TinyAction>>> {
        Ok(None)
    }
}

struct RecordingObservationsAgentFactory {
    observations: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl AgentFactory<TinyAdapter> for RecordingObservationsAgentFactory {
    fn definition(&self) -> AgentDefinition {
        test_agent_definition()
    }

    async fn create(&self, _context: AgentContext) -> Result<Box<dyn Agent<TinyAdapter> + Send>> {
        Ok(Box::new(RecordingObservationsAgent {
            observations: self.observations.clone(),
        }))
    }

    fn identity(&self, _settings: &Value) -> Result<AgentIdentity> {
        test_agent_identity()
    }
}

struct SequencedDecisionAgent {
    log: Arc<Mutex<Vec<String>>>,
    decisions: usize,
}

#[async_trait]
impl Agent<TinyAdapter> for SequencedDecisionAgent {
    async fn observe_transition(
        &mut self,
        actor: PlayerRole,
        _action: TinyAction,
        _resulting_observation: TinyObservation,
    ) -> Result<()> {
        self.log
            .lock()
            .unwrap()
            .push(format!("observe_transition:{}", actor.as_str()));
        Ok(())
    }

    async fn respond(
        &mut self,
        _available_actions: Option<Vec<TinyAction>>,
    ) -> Result<Option<AgentResponse<TinyAction>>> {
        self.decisions += 1;
        self.log
            .lock()
            .unwrap()
            .push(format!("respond:{}", self.decisions));
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

#[async_trait]
impl AgentFactory<TinyAdapter> for SequencedDecisionAgentFactory {
    fn definition(&self) -> AgentDefinition {
        test_agent_definition()
    }

    async fn create(&self, _context: AgentContext) -> Result<Box<dyn Agent<TinyAdapter> + Send>> {
        Ok(Box::new(SequencedDecisionAgent {
            log: self.log.clone(),
            decisions: 0,
        }))
    }

    fn identity(&self, _settings: &Value) -> Result<AgentIdentity> {
        test_agent_identity()
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
        Ok(vec![crate::tts::AudioChunk {
            data: vec![1, 2, 3],
            sample_rate: 24000,
            channels: 1,
        }])
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

impl Game for TinyAdapter {
    type Config = Value;
    type State = TinyState;
    type Action = TinyAction;
    type Observation = TinyObservation;
    type Completion = TinySummary;

    fn initial_state(
        &self,
        _context: GameInitializationContext<'_, Self::Config>,
    ) -> Result<Self::State> {
        Ok(TinyState { done: false })
    }

    fn apply_action(
        &self,
        _state: &Self::State,
        action: &Self::Action,
        _actor: PlayerRole,
    ) -> std::result::Result<Self::State, ActionRejection> {
        if action.invalid {
            return Err(ActionRejection::new("invalid_tiny_action"));
        }
        Ok(TinyState {
            done: action.finish,
        })
    }

    fn observation(&self, state: &Self::State, player: PlayerRole) -> Self::Observation {
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

    fn completion(&self, state: &Self::State) -> Option<Self::Completion> {
        state.done.then(|| TinySummary {
            done: state.done,
            outcome: "success".to_string(),
            dyad_score: 10,
            player_scores: BTreeMap::from([("A".to_string(), 6), ("B".to_string(), 4)]),
        })
    }
}

impl Game for NoAvailableActionsAdapter {
    type Config = Value;
    type State = TinyState;
    type Action = TinyAction;
    type Observation = TinyObservation;
    type Completion = TinySummary;

    fn initial_state(
        &self,
        context: GameInitializationContext<'_, Self::Config>,
    ) -> Result<Self::State> {
        TinyAdapter.initial_state(context)
    }

    fn apply_action(
        &self,
        state: &Self::State,
        action: &Self::Action,
        actor: PlayerRole,
    ) -> std::result::Result<Self::State, ActionRejection> {
        TinyAdapter.apply_action(state, action, actor)
    }

    fn observation(&self, state: &Self::State, player: PlayerRole) -> Self::Observation {
        TinyAdapter.observation(state, player)
    }

    fn completion(&self, state: &Self::State) -> Option<Self::Completion> {
        TinyAdapter.completion(state)
    }
}

impl Game for LossSummaryAdapter {
    type Config = Value;
    type State = TinyState;
    type Action = TinyAction;
    type Observation = TinyObservation;
    type Completion = TinySummary;

    fn initial_state(
        &self,
        context: GameInitializationContext<'_, Self::Config>,
    ) -> Result<Self::State> {
        TinyAdapter.initial_state(context)
    }

    fn apply_action(
        &self,
        state: &Self::State,
        action: &Self::Action,
        actor: PlayerRole,
    ) -> std::result::Result<Self::State, ActionRejection> {
        TinyAdapter.apply_action(state, action, actor)
    }

    fn observation(&self, state: &Self::State, player: PlayerRole) -> Self::Observation {
        TinyAdapter.observation(state, player)
    }

    fn available_actions(
        &self,
        state: &Self::State,
        player: PlayerRole,
    ) -> Option<Vec<Self::Action>> {
        TinyAdapter.available_actions(state, player)
    }

    fn completion(&self, state: &Self::State) -> Option<Self::Completion> {
        TinyAdapter.completion(state).map(|mut completion| {
            completion.outcome = "loss".to_string();
            completion.dyad_score = 0;
            completion
                .player_scores
                .extend([("A".to_string(), 0), ("B".to_string(), 0)]);
            completion
        })
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
    let participant_session_id = response["participant_id"].as_str().unwrap().to_string();
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
async fn game_socket_url(
    base_url: &str,
    public_session_id: &str,
    participant_session_id: &str,
) -> String {
    let credential = TEST_PARTICIPANT_CREDENTIALS
        .lock()
        .unwrap()
        .get(participant_session_id)
        .cloned()
        .expect("test participant credential was not recorded");
    let response = reqwest::Client::new()
        .post(format!(
            "{base_url}/api/sessions/{public_session_id}/game-session"
        ))
        .bearer_auth(credential)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let plan: Value = response.json().await.unwrap();
    let token = plan["token"].as_str().unwrap();
    let host = base_url.trim_start_matches("http://");
    format!("ws://{host}/ws/game/{public_session_id}?token={token}")
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
        "/api/sessions",
        json!({"participant_session_id": a}),
    )
    .await;
    let public_session_id = created["public_session_id"].as_str().unwrap().to_string();
    let (_, joined) = json_request(
        router,
        http::Method::POST,
        "/api/sessions",
        json!({"participant_session_id": b}),
    )
    .await;
    assert_eq!(joined["public_session_id"], public_session_id);
    (a, b, public_session_id)
}

// Requests one participant-bound audio plan and verifies that it is enabled.
async fn request_audio_plan(
    router: Router,
    public_session_id: &str,
    participant_id: &str,
) -> Value {
    let (status, plan) = json_request(
        router,
        http::Method::POST,
        &format!("/api/sessions/{public_session_id}/audio-session"),
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
        "/api/sessions",
        json!({"participant_session_id": human}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(created["role"], "A");
    (
        human,
        created["public_session_id"].as_str().unwrap().to_string(),
    )
}

// Polls the evaluation export until an event type appears or the test times out.
async fn wait_for_export_event(router: Router, event_type: &str) -> Value {
    let mut last_export = Value::Null;
    for _ in 0..20 {
        let (status, export) = json_request(
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
async fn wait_for_tts_diagnostic(router: Router, diagnostic_event: &str) -> Value {
    for _ in 0..30 {
        let (status, export) = json_request(
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
    assert_eq!(public_config["game_name"], "Embedded game");
    assert!(public_config.get("participant_title").is_none());
    assert!(public_config["institution"].is_null());
    assert_eq!(public_config["consents"][0]["id"], "study");
    assert_eq!(public_config["voice"]["enabled"], true);
    assert!(public_config.get("transcription").is_none());
    assert!(public_config.get("tts").is_none());
    assert!(public_config.get("agents").is_none());
    assert!(public_config.get("privacy").is_none());
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
    assert!(api_body.contains("experiment_status"));
    assert!(!api_body.contains("participant_title"));
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
    let (a, _b, public_session_id) = create_joined_room(router.clone()).await;

    let (audio_status, audio) = json_request(
        router,
        http::Method::POST,
        &format!("/api/sessions/{public_session_id}/audio-session"),
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
    let (a, _b, public_session_id) = create_joined_room(router.clone()).await;

    let (audio_status, audio) = json_request(
        router,
        http::Method::POST,
        &format!("/api/sessions/{public_session_id}/audio-session"),
        json!({"participant_session_id": a}),
    )
    .await;
    assert_eq!(audio_status, StatusCode::OK);
    assert_eq!(audio["enabled"], true);
    assert_eq!(audio["protocol_version"], 1);
    assert_eq!(audio["sample_rate_hz"], 24000);
    assert_eq!(
        audio["websocket_url"],
        format!("/ws/audio/{public_session_id}"),
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

/// Records canonical frames received from each role during the JavaScript mute contract.
#[derive(Clone)]
struct MuteContractTranscriptionProvider {
    frames: Arc<Mutex<Vec<(String, u8)>>>,
}

#[async_trait]
impl TranscriptionProvider for MuteContractTranscriptionProvider {
    /// Starts a provider-neutral session that records the first byte of every input frame.
    async fn start_session(
        &self,
        context: TranscriptionSessionContext,
    ) -> Result<crate::transcription::TranscriptionSessionHandle> {
        let (input, mut inputs) = mpsc::channel(32);
        let (events, event_receiver) = mpsc::channel(4);
        let frames = self.frames.clone();
        tokio::spawn(async move {
            let _ = events.send(TranscriptionEvent::Ready).await;
            while let Some(message) = inputs.recv().await {
                match message {
                    TranscriptionInput::Audio(frame) => frames.lock().unwrap().push((
                        context.role.clone(),
                        frame.pcm.first().copied().unwrap_or(0),
                    )),
                    TranscriptionInput::Finish => break,
                }
            }
        });
        Ok(crate::transcription::TranscriptionSessionHandle {
            input,
            events: event_receiver,
        })
    }
}

/// Sends one line-delimited command to the JavaScript driver and reads its response.
async fn javascript_mute_command(
    input: &mut tokio::process::ChildStdin,
    output: &mut tokio::io::Lines<tokio::io::BufReader<tokio::process::ChildStdout>>,
    command: Value,
) -> Value {
    use tokio::io::AsyncWriteExt as _;

    input
        .write_all(format!("{command}\n").as_bytes())
        .await
        .unwrap();
    input.flush().await.unwrap();
    let line = tokio::time::timeout(Duration::from_secs(5), output.next_line())
        .await
        .expect("timed out waiting for JavaScript mute driver")
        .expect("could not read JavaScript mute driver output")
        .expect("JavaScript mute driver exited without a response");
    serde_json::from_str(&line).expect("JavaScript mute driver returned invalid JSON")
}

/// Waits until transcription has observed all expected role-A frame markers.
async fn wait_for_transcription_markers(frames: &Arc<Mutex<Vec<(String, u8)>>>, expected: &[u8]) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let observed = frames
                .lock()
                .unwrap()
                .iter()
                .filter_map(|(role, marker)| (role == "A").then_some(*marker))
                .collect::<Vec<_>>();
            if observed == expected {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("transcription did not receive the expected JavaScript frames");
}

/// Exercises the built JavaScript sink against the production Rust audio WebSocket.
///
/// This is ignored in ordinary Rust-only package tests because it requires Node and a freshly
/// built `js-client/dist`. The repository `test-client-server` target owns those prerequisites.
#[tokio::test]
#[ignore = "run through make test-client-server after building js-client"]
async fn javascript_sink_mute_contract_blocks_relay_and_transcription() {
    use std::process::Stdio;
    use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};

    let frames = Arc::new(Mutex::new(Vec::new()));
    let mut config = voice_enabled_config();
    config.transcription.enabled = true;
    config.speechmatics.api_key = "contract-test-key".to_string();
    let router = build_router(
        TinyAdapter,
        config,
        ServeOptions {
            transcription_provider: Some(Arc::new(MuteContractTranscriptionProvider {
                frames: frames.clone(),
            })),
            ..ServeOptions::default()
        },
    )
    .await
    .unwrap();
    let (participant_a, participant_b, public_session_id) =
        create_joined_room(router.clone()).await;
    let plan_a = request_audio_plan(router.clone(), &public_session_id, &participant_a).await;
    let plan_b = request_audio_plan(router.clone(), &public_session_id, &participant_b).await;
    let (base_url, server) = spawn_test_server(router).await;
    let host = base_url.trim_start_matches("http://");
    let (mut socket_b, _) = connect_async(format!(
        "ws://{host}/ws/audio/{public_session_id}?token={}",
        plan_b["token"].as_str().unwrap()
    ))
    .await
    .unwrap();

    let repository = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let fixture = json!({
        "origin": base_url,
        "public_session_id": public_session_id,
        "participant_session_id": participant_a,
        "plan": plan_a,
    });
    let mut child = tokio::process::Command::new("node")
        .arg(repository.join("js-client/tests/clientServerMuteContract.mjs"))
        .current_dir(repository)
        .env("PARLANDO_MUTE_CONTRACT", fixture.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("could not start JavaScript mute contract driver");
    let mut child_input = child.stdin.take().unwrap();
    let mut child_output = BufReader::new(child.stdout.take().unwrap()).lines();
    let ready = tokio::time::timeout(Duration::from_secs(10), child_output.next_line())
        .await
        .expect("timed out connecting JavaScript audio sink")
        .unwrap()
        .expect("JavaScript audio sink exited before connecting");
    assert_eq!(
        serde_json::from_str::<Value>(&ready).unwrap()["type"],
        "ready"
    );

    let (mut game_a, _) = connect_async(
        game_socket_url(&base_url, &public_session_id, &participant_a).await,
    )
    .await
    .unwrap();
    let (mut game_b, _) = connect_async(
        game_socket_url(&base_url, &public_session_id, &participant_b).await,
    )
    .await
    .unwrap();
    let _ = read_ws_type(&mut game_a, "session_started").await;
    let _ = read_ws_type(&mut game_b, "session_started").await;

    let live = javascript_mute_command(
        &mut child_input,
        &mut child_output,
        json!({"type":"capture", "marker":1}),
    )
    .await;
    assert_eq!(
        live,
        json!({"type":"captured", "marker":1, "track_enabled":true})
    );
    let live_frame = AudioFrame::decode(&read_audio_binary(&mut socket_b).await).unwrap();
    assert_eq!(live_frame.pcm[0], 1);
    wait_for_transcription_markers(&frames, &[1]).await;

    let muted = javascript_mute_command(
        &mut child_input,
        &mut child_output,
        json!({"type":"mute", "muted":true}),
    )
    .await;
    assert_eq!(
        muted,
        json!({"type":"muteChanged", "muted":true, "track_enabled":false})
    );
    let muted_capture = javascript_mute_command(
        &mut child_input,
        &mut child_output,
        json!({"type":"capture", "marker":2}),
    )
    .await;
    assert_eq!(muted_capture["track_enabled"], false);
    assert_no_audio_binary(&mut socket_b).await;
    assert_eq!(
        frames
            .lock()
            .unwrap()
            .iter()
            .filter_map(|(role, marker)| (role == "A").then_some(*marker))
            .collect::<Vec<_>>(),
        vec![1]
    );

    let partner_frame = AudioFrame {
        sequence: 0,
        timestamp_ms: 0,
        pcm: vec![9; crate::audio::AUDIO_FRAME_BYTES],
    }
    .encode();
    socket_b
        .send(TungsteniteMessage::Binary(partner_frame))
        .await
        .unwrap();
    let playback = javascript_mute_command(
        &mut child_input,
        &mut child_output,
        json!({"type":"waitForPlayback", "count":2}),
    )
    .await;
    assert_eq!(playback, json!({"type":"playback", "count":2}));

    let unmuted = javascript_mute_command(
        &mut child_input,
        &mut child_output,
        json!({"type":"mute", "muted":false}),
    )
    .await;
    assert_eq!(
        unmuted,
        json!({"type":"muteChanged", "muted":false, "track_enabled":true})
    );
    javascript_mute_command(
        &mut child_input,
        &mut child_output,
        json!({"type":"capture", "marker":3}),
    )
    .await;
    let resumed_frame = AudioFrame::decode(&read_audio_binary(&mut socket_b).await).unwrap();
    assert_eq!(resumed_frame.pcm[0], 3);
    wait_for_transcription_markers(&frames, &[1, 3]).await;

    let disconnected = javascript_mute_command(
        &mut child_input,
        &mut child_output,
        json!({"type":"disconnect"}),
    )
    .await;
    assert_eq!(disconnected["type"], "disconnected");
    child_input.shutdown().await.unwrap();
    assert!(tokio::time::timeout(Duration::from_secs(5), child.wait())
        .await
        .expect("JavaScript mute driver did not exit")
        .unwrap()
        .success());
    server.abort();
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
    let (a, b, public_session_id) = create_joined_room(router.clone()).await;
    let (_, plan_a) = json_request(
        router.clone(),
        http::Method::POST,
        &format!("/api/sessions/{public_session_id}/audio-session"),
        json!({"participant_session_id":a}),
    )
    .await;
    let (_, plan_b) = json_request(
        router.clone(),
        http::Method::POST,
        &format!("/api/sessions/{public_session_id}/audio-session"),
        json!({"participant_session_id":b}),
    )
    .await;
    let (base_url, server) = spawn_test_server(router.clone()).await;
    let host = base_url.trim_start_matches("http://");
    let (mut socket_b, _) = connect_async(format!(
        "ws://{host}/ws/audio/{public_session_id}?token={}",
        plan_b["token"].as_str().unwrap()
    ))
    .await
    .unwrap();
    let (mut socket_a, _) = connect_async(format!(
        "ws://{host}/ws/audio/{public_session_id}?token={}",
        plan_a["token"].as_str().unwrap()
    ))
    .await
    .unwrap();
    wait_for_audio_control(&mut socket_a).await;
    wait_for_audio_control(&mut socket_b).await;
    let waiting_frame = AudioFrame {
        sequence: 0,
        timestamp_ms: 0,
        pcm: vec![0; crate::audio::AUDIO_FRAME_BYTES],
    }
    .encode();
    socket_a
        .send(TungsteniteMessage::Binary(waiting_frame.clone()))
        .await
        .unwrap();
    assert_eq!(read_audio_binary(&mut socket_b).await, waiting_frame);
    tokio::time::sleep(Duration::from_millis(50)).await;
    let (_, waiting_export) = json_request(
        router.clone(),
        http::Method::GET,
        "/api/admin/export?variant=full",
        Value::Null,
    )
    .await;
    assert!(!waiting_export["session_events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|event| event["event_type"] == "conversation_message"));
    let (mut game_a, _) = connect_async(game_socket_url(&base_url, &public_session_id, &a).await)
        .await
        .unwrap();
    let (mut game_b, _) = connect_async(game_socket_url(&base_url, &public_session_id, &b).await)
        .await
        .unwrap();
    let _ = read_ws_type(&mut game_a, "session_started").await;
    let _ = read_ws_type(&mut game_b, "session_started").await;
    let frame = AudioFrame {
        sequence: 1,
        timestamp_ms: 20,
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
    let connect = |public_session_id: &str, token: &str| {
        connect_async(format!(
            "ws://{host}/ws/audio/{public_session_id}?token={token}"
        ))
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
    let (a, b, public_session_id) = create_joined_room(router.clone()).await;
    let old_a = request_audio_plan(router.clone(), &public_session_id, &a).await;
    let plan_b = request_audio_plan(router.clone(), &public_session_id, &b).await;
    let (base_url, server) = spawn_test_server(router.clone()).await;
    let host = base_url.trim_start_matches("http://");
    let (mut socket_b, _) = connect_async(format!(
        "ws://{host}/ws/audio/{public_session_id}?token={}",
        plan_b["token"].as_str().unwrap()
    ))
    .await
    .unwrap();
    let (mut old_socket_a, _) = connect_async(format!(
        "ws://{host}/ws/audio/{public_session_id}?token={}",
        old_a["token"].as_str().unwrap()
    ))
    .await
    .unwrap();
    wait_for_audio_control(&mut old_socket_a).await;
    let new_a = request_audio_plan(router, &public_session_id, &a).await;
    let (mut new_socket_a, _) = connect_async(format!(
        "ws://{host}/ws/audio/{public_session_id}?token={}",
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
        "/api/sessions",
        json!({"participant_session_id": participant_session_id}),
    )
    .await;
    assert_eq!(blocked_status, StatusCode::FORBIDDEN);

    consent_participant(router.clone(), &participant_session_id).await;
    let (status, room) = json_request(
        router,
        http::Method::POST,
        "/api/sessions",
        json!({"participant_session_id": participant_session_id}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(room.get("participant_session_id").is_none());
    assert_eq!(room["role"], "A");
    assert!(room["public_session_id"]
        .as_str()
        .is_some_and(|id| !id.is_empty()));
}

#[tokio::test]
async fn room_response_uses_null_when_game_does_not_enumerate_actions() {
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
        "/api/sessions",
        json!({"participant_session_id": participant_session_id}),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(room["available_actions"].is_null());
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
        "/api/sessions",
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
        "/api/sessions",
        json!({"participant_session_id": first}),
    )
    .await;
    let (second_status, second_room) = json_request(
        router.clone(),
        http::Method::POST,
        "/api/sessions",
        json!({"participant_session_id": second}),
    )
    .await;

    assert_eq!(first_status, StatusCode::OK);
    assert_eq!(second_status, StatusCode::OK);
    assert_eq!(first_room["role"], "A");
    assert_eq!(second_room["role"], "B");
    assert_eq!(
        first_room["public_session_id"],
        second_room["public_session_id"]
    );
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
    let roles = export["session_participants"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["role"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(roles.contains(&"A"));
    assert!(roles.contains(&"B"));
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
        "/api/sessions",
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
        "/api/sessions",
        json!({"participant_session_id": a}),
    )
    .await;
    assert_eq!(created["role"], "A");
    let public_session_id = created["public_session_id"].as_str().unwrap().to_string();
    let (_, joined) = json_request(
        router.clone(),
        http::Method::POST,
        "/api/sessions",
        json!({"participant_session_id": b}),
    )
    .await;
    assert_eq!(joined["role"], "B");
    assert_eq!(joined["public_session_id"], public_session_id);

    let (export_status, export) = json_request(
        router,
        http::Method::GET,
        "/api/admin/export?variant=full",
        Value::Null,
    )
    .await;
    assert_eq!(export_status, StatusCode::OK);
    assert_eq!(export["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(
        export["sessions"][0]["public_session_id"],
        public_session_id
    );
    assert_eq!(export["session_participants"].as_array().unwrap().len(), 2);
    let roles = export["session_participants"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["role"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(roles.contains(&"A"));
    assert!(roles.contains(&"B"));
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
    let (a, b, public_session_id) = create_joined_room(router.clone()).await;
    let (base_url, server) = spawn_test_server(router.clone()).await;
    let (mut socket_a, _) = connect_async(game_socket_url(&base_url, &public_session_id, &a).await)
        .await
        .unwrap();
    assert_no_ws_type(&mut socket_a, "session_started").await;

    let (mut socket_b, _) = connect_async(game_socket_url(&base_url, &public_session_id, &b).await)
        .await
        .unwrap();
    let assigned_a = read_ws_type(&mut socket_a, "session_started").await;
    assert_eq!(assigned_a["role"], "A");
    let assigned_b = read_ws_type(&mut socket_b, "session_started").await;
    assert_eq!(assigned_b["role"], "B");
    assert_no_ws_type(&mut socket_a, "session_started").await;
    server.abort();
}

#[tokio::test]
async fn explicit_leave_abandons_session_and_notifies_partner() {
    let router = build_router(TinyAdapter, step_five_config(), ServeOptions::default())
        .await
        .unwrap();
    let (a, b, public_session_id) = create_joined_room(router.clone()).await;
    let (base_url, server) = spawn_test_server(router.clone()).await;
    let (mut socket_a, _) = connect_async(game_socket_url(&base_url, &public_session_id, &a).await)
        .await
        .unwrap();
    let (mut socket_b, _) = connect_async(game_socket_url(&base_url, &public_session_id, &b).await)
        .await
        .unwrap();
    let _assigned_a = read_ws_type(&mut socket_a, "session_started").await;
    let _assigned_b = read_ws_type(&mut socket_b, "session_started").await;

    let (export_status, running_export) = json_request(
        router.clone(),
        http::Method::GET,
        "/api/admin/export?variant=full",
        Value::Null,
    )
    .await;
    assert_eq!(export_status, StatusCode::OK);
    assert_eq!(running_export["sessions"][0]["status"], "running");
    assert!(running_export["sessions"][0]["started_at"].is_string());

    send_ws_json(&mut socket_a, json!({"type": "leave"})).await;
    let abandoned = read_ws_type(&mut socket_b, "abandoned").await;
    assert_eq!(abandoned["public_session_id"], public_session_id);
    assert_eq!(abandoned["code"], "participant_left");

    let export = wait_for_export_event(router, "session_abandoned").await;
    assert_eq!(export["sessions"][0]["status"], "abandoned");
    let event = export["session_events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|event| event["event_type"] == "session_abandoned")
        .unwrap();
    assert_eq!(event["actor_role"], "A");
    assert_eq!(event["payload"]["reason"], "participant_left");
    server.abort();
}

#[tokio::test]
async fn websocket_rejects_actions_until_both_players_are_connected() {
    let router = build_router(TinyAdapter, step_five_config(), ServeOptions::default())
        .await
        .unwrap();
    let (a, _b, public_session_id) = create_joined_room(router.clone()).await;
    let (base_url, server) = spawn_test_server(router.clone()).await;
    let (mut socket_a, _) = connect_async(game_socket_url(&base_url, &public_session_id, &a).await)
        .await
        .unwrap();

    send_ws_json(
        &mut socket_a,
        json!({"type": "action", "action": {"finish": false}}),
    )
    .await;
    let error = read_ws_type(&mut socket_a, "action_rejected").await;
    assert_eq!(error["code"], "players_not_ready");

    let (export_status, export) = json_request(
        router,
        http::Method::GET,
        "/api/admin/export?variant=full",
        Value::Null,
    )
    .await;
    assert_eq!(export_status, StatusCode::OK);
    assert!(export["session_events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|event| {
            event["event_type"] == "game_action_rejected"
                && event["payload"]["reason_code"] == "players_not_ready"
        }));
    server.abort();
}

/// Rejected oversized actions retain bounded analytical metadata, not attacker input.
#[tokio::test]
async fn oversized_action_rejection_is_bounded_and_analyzable() {
    let router = build_router(TinyAdapter, step_five_config(), ServeOptions::default())
        .await
        .unwrap();
    let (a, b, public_session_id) = create_joined_room(router.clone()).await;
    let (base_url, server) = spawn_test_server(router.clone()).await;
    let (mut socket_a, _) = connect_async(game_socket_url(&base_url, &public_session_id, &a).await)
        .await
        .unwrap();
    let (mut socket_b, _) = connect_async(game_socket_url(&base_url, &public_session_id, &b).await)
        .await
        .unwrap();
    let _assigned_a = read_ws_type(&mut socket_a, "session_started").await;
    let _assigned_b = read_ws_type(&mut socket_b, "session_started").await;

    send_ws_json(
        &mut socket_a,
        json!({"type": "action", "action": {"padding": "x".repeat(5_000)}}),
    )
    .await;
    let error = read_ws_type(&mut socket_a, "error").await;
    assert_eq!(error["code"], "action_too_large");
    assert_no_ws_type(&mut socket_b, "error").await;

    let (export_status, export) = json_request(
        router,
        http::Method::GET,
        "/api/admin/export?variant=full",
        Value::Null,
    )
    .await;
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

/// Accepted actions always retain the authoritative resulting state.
#[tokio::test]
async fn accepted_actions_always_store_resulting_game_state() {
    let router = build_router(TinyAdapter, step_five_config(), ServeOptions::default())
        .await
        .unwrap();
    let (a, b, public_session_id) = create_joined_room(router.clone()).await;
    let (base_url, server) = spawn_test_server(router.clone()).await;
    let (mut socket_a, _) = connect_async(game_socket_url(&base_url, &public_session_id, &a).await)
        .await
        .unwrap();
    let (mut socket_b, _) = connect_async(game_socket_url(&base_url, &public_session_id, &b).await)
        .await
        .unwrap();
    let _assigned_a = read_ws_type(&mut socket_a, "session_started").await;
    let _assigned_b = read_ws_type(&mut socket_b, "session_started").await;
    send_ws_json(
        &mut socket_a,
        json!({"type": "action", "action": {"finish": true}}),
    )
    .await;
    let _completed = read_ws_type(&mut socket_a, "completed").await;

    let (export_status, export) =
        json_request(router, http::Method::GET, "/api/admin/export", Value::Null).await;
    assert_eq!(export_status, StatusCode::OK);
    let accepted = export["experiment"]["sessions"][0]["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|event| event["kind"] == "action")
        .unwrap();
    assert!(!accepted["state"].is_null());
    server.abort();
}

#[tokio::test]
async fn human_human_game_accepts_actions_after_second_human_connects() {
    let router = build_router(TinyAdapter, step_five_config(), ServeOptions::default())
        .await
        .unwrap();
    let (a, b, public_session_id) = create_joined_room(router.clone()).await;
    let (base_url, server) = spawn_test_server(router.clone()).await;
    let (mut socket_a, _) = connect_async(game_socket_url(&base_url, &public_session_id, &a).await)
        .await
        .unwrap();
    assert_no_ws_type(&mut socket_a, "session_started").await;

    send_ws_json(
        &mut socket_a,
        json!({"type": "action", "action": {"finish": false}}),
    )
    .await;
    let waiting_error = read_ws_type(&mut socket_a, "action_rejected").await;
    assert_eq!(waiting_error["code"], "players_not_ready");

    let (mut socket_b, _) = connect_async(game_socket_url(&base_url, &public_session_id, &b).await)
        .await
        .unwrap();
    let _assigned_a = read_ws_type(&mut socket_a, "session_started").await;
    let _assigned_b = read_ws_type(&mut socket_b, "session_started").await;
    send_ws_json(
        &mut socket_a,
        json!({"type": "action", "action": {"finish": true}}),
    )
    .await;
    let completed = read_ws_type(&mut socket_a, "completed").await;
    assert_eq!(completed["completion"]["done"], true);
    server.abort();
}

#[tokio::test]
async fn websocket_accepts_actions_chat_completion_and_persists_state_changes() {
    let router = build_router(TinyAdapter, step_five_config(), ServeOptions::default())
        .await
        .unwrap();
    let (a, b, public_session_id) = create_joined_room(router.clone()).await;
    let (base_url, server) = spawn_test_server(router.clone()).await;
    let (mut socket_a, _) = connect_async(game_socket_url(&base_url, &public_session_id, &a).await)
        .await
        .unwrap();
    let (mut socket_b, _) = connect_async(game_socket_url(&base_url, &public_session_id, &b).await)
        .await
        .unwrap();
    let _assigned_a = read_ws_type(&mut socket_a, "session_started").await;
    let _assigned_b = read_ws_type(&mut socket_b, "session_started").await;

    send_ws_json(
        &mut socket_a,
        json!({"type": "message", "text": "hello from A"}),
    )
    .await;
    let chat_a = read_ws_type(&mut socket_a, "message").await;
    let chat_b = read_ws_type(&mut socket_b, "message").await;
    assert_eq!(chat_a["message"]["text"], "hello from A");
    assert_eq!(chat_b["message"]["input"], "text");

    send_ws_json(
        &mut socket_a,
        json!({"type": "action", "action": {"finish": false}}),
    )
    .await;
    let state_a = read_ws_type(&mut socket_a, "transition").await;
    let state_b = read_ws_type(&mut socket_b, "transition").await;
    assert_eq!(state_a["actor"], "A");
    assert_eq!(state_b["actor"], "A");
    assert_eq!(state_a["action"]["finish"], false);
    assert_eq!(state_b["action"], state_a["action"]);
    assert_eq!(state_a["observation"]["done"], false);

    send_ws_json(
        &mut socket_a,
        json!({"type": "action", "action": {"finish": true}}),
    )
    .await;
    let completed_a = read_ws_type(&mut socket_a, "completed").await;
    let completed_b = read_ws_type(&mut socket_b, "completed").await;
    assert_eq!(completed_a["completion"]["done"], true);
    assert_eq!(completed_a["completion"]["outcome"], "success");
    assert_eq!(completed_a["completion"]["dyad_score"], 10);
    assert_eq!(completed_a["completion"]["player_scores"]["A"], 6);
    assert_eq!(completed_a["completion"]["player_scores"]["B"], 4);
    assert_eq!(completed_b["completion"]["done"], true);
    assert_eq!(completed_b["completion"], completed_a["completion"]);

    let (export_status, export) = json_request(
        router,
        http::Method::GET,
        "/api/admin/export?variant=full",
        Value::Null,
    )
    .await;
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
async fn loss_completion_is_broadcast_and_exported() {
    let router = build_router(
        LossSummaryAdapter,
        step_five_config(),
        ServeOptions::default(),
    )
    .await
    .unwrap();
    let (a, b, public_session_id) = create_joined_room(router.clone()).await;
    let (base_url, server) = spawn_test_server(router.clone()).await;
    let (mut socket_a, _) = connect_async(game_socket_url(&base_url, &public_session_id, &a).await)
        .await
        .unwrap();
    let (mut socket_b, _) = connect_async(game_socket_url(&base_url, &public_session_id, &b).await)
        .await
        .unwrap();
    let _assigned_a = read_ws_type(&mut socket_a, "session_started").await;
    let _assigned_b = read_ws_type(&mut socket_b, "session_started").await;

    send_ws_json(
        &mut socket_a,
        json!({"type": "action", "action": {"finish": true}}),
    )
    .await;
    let completed = read_ws_type(&mut socket_a, "completed").await;
    assert_eq!(completed["completion"]["done"], true);
    assert_eq!(completed["completion"]["outcome"], "loss");
    assert_eq!(completed["completion"]["dyad_score"], 0);
    assert_eq!(completed["completion"]["player_scores"]["A"], 0);
    assert_eq!(completed["completion"]["player_scores"]["B"], 0);

    let (export_status, export) = json_request(
        router,
        http::Method::GET,
        "/api/admin/export?variant=full",
        Value::Null,
    )
    .await;
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
    let (a, b, public_session_id) = create_joined_room(router.clone()).await;
    let (base_url, server) = spawn_test_server(router.clone()).await;
    let (mut socket_a, _) = connect_async(game_socket_url(&base_url, &public_session_id, &a).await)
        .await
        .unwrap();
    let (mut socket_b, _) = connect_async(game_socket_url(&base_url, &public_session_id, &b).await)
        .await
        .unwrap();
    let _assigned_a = read_ws_type(&mut socket_a, "session_started").await;
    let _assigned_b = read_ws_type(&mut socket_b, "session_started").await;

    send_ws_json(
        &mut socket_a,
        json!({"type": "action", "action": {"finish": true}}),
    )
    .await;
    let _completed_a = read_ws_type(&mut socket_a, "completed").await;

    send_ws_json(
        &mut socket_a,
        json!({"type": "action", "action": {"finish": false}}),
    )
    .await;
    let action_error = read_ws_type(&mut socket_a, "action_rejected").await;
    assert_eq!(action_error["code"], "session_complete");

    send_ws_json(
        &mut socket_a,
        json!({"type": "message", "text": "late hello"}),
    )
    .await;
    let chat_error = read_ws_type(&mut socket_a, "error").await;
    assert_eq!(chat_error["code"], "message_rejected");
    assert_no_ws_type(&mut socket_b, "message").await;

    let (transcript_status, transcript_response) = json_request(
        router.clone(),
        http::Method::POST,
        &format!("/api/sessions/{public_session_id}/transcripts"),
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

    let (export_status, export) = json_request(
        router,
        http::Method::GET,
        "/api/admin/export?variant=full",
        Value::Null,
    )
    .await;
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
async fn transcript_endpoints_are_private_and_diagnostics_are_not_persisted() {
    let (config, _temp) = sqlite_config();
    let router = build_router(TinyAdapter, config, ServeOptions::default())
        .await
        .unwrap();
    let (a, _b, public_session_id) = create_joined_room(router.clone()).await;

    let (conversation_get_status, _) = json_request(
        router.clone(),
        http::Method::GET,
        &format!("/api/sessions/{public_session_id}/conversation"),
        Value::Null,
    )
    .await;
    assert_eq!(conversation_get_status, StatusCode::NOT_FOUND);
    let (conversation_post_status, _) = json_request(
        router.clone(),
        http::Method::POST,
        &format!("/api/sessions/{public_session_id}/conversation"),
        json!({"text": "typed hello"}),
    )
    .await;
    assert_eq!(conversation_post_status, StatusCode::NOT_FOUND);
    let (transcript_get_status, _) = json_request(
        router.clone(),
        http::Method::GET,
        &format!("/api/sessions/{public_session_id}/transcripts"),
        Value::Null,
    )
    .await;
    assert_eq!(transcript_get_status, StatusCode::NOT_FOUND);
    let (transcript_stream_status, _) = json_request(
        router.clone(),
        http::Method::GET,
        &format!("/api/sessions/{public_session_id}/transcripts/stream"),
        Value::Null,
    )
    .await;
    assert_eq!(transcript_stream_status, StatusCode::NOT_FOUND);
    let (transcription_context_status, _) = json_request(
        router.clone(),
        http::Method::GET,
        &format!("/api/sessions/{public_session_id}/transcription-context"),
        Value::Null,
    )
    .await;
    assert_eq!(transcription_context_status, StatusCode::NOT_FOUND);

    let (transcript_status, _transcript) = json_request(
        router.clone(),
        http::Method::POST,
        &format!("/api/sessions/{public_session_id}/transcripts"),
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
        &format!("/api/sessions/{public_session_id}/voice-diagnostics"),
        json!({
            "participant_session_id": a,
            "event": "mic_started",
            "metadata": {"device": "test"}
        }),
    )
    .await;
    assert_eq!(diagnostic_status, StatusCode::OK);
    assert_eq!(diagnostic["stored"], false);

    let (export_status, export) =
        json_request(router, http::Method::GET, "/api/admin/export", Value::Null).await;
    assert_eq!(export_status, StatusCode::OK);
    let events = export["experiment"]["sessions"][0]["events"]
        .as_array()
        .unwrap();
    assert!(events.is_empty());
}

#[tokio::test]
async fn admin_sessions_api_reads_actions_from_database() {
    let (config, _temp) = sqlite_config();
    let router = build_router(TinyAdapter, config, ServeOptions::default())
        .await
        .unwrap();
    let (a, b, public_session_id) = create_joined_room(router.clone()).await;
    let (base_url, server) = spawn_test_server(router.clone()).await;
    let (mut socket_a, _) = connect_async(game_socket_url(&base_url, &public_session_id, &a).await)
        .await
        .unwrap();
    let (mut socket_b, _) = connect_async(game_socket_url(&base_url, &public_session_id, &b).await)
        .await
        .unwrap();
    let _assigned_a = read_ws_type(&mut socket_a, "session_started").await;
    let _assigned_b = read_ws_type(&mut socket_b, "session_started").await;

    send_ws_json(
        &mut socket_a,
        json!({"type": "action", "action": {"finish": false}}),
    )
    .await;
    let _state_a = read_ws_type(&mut socket_a, "transition").await;
    let _state_b = read_ws_type(&mut socket_b, "transition").await;
    let (transcript_status, _transcript) = json_request(
        router.clone(),
        http::Method::POST,
        &format!("/api/sessions/{public_session_id}/transcripts"),
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
        "/api/sessions",
        json!({"participant_session_id": human}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(created["role"], "A");
    assert_eq!(created["presence"]["B"]["connected"], true);
    assert_eq!(created["presence"]["B"]["audioReady"], true);

    let (export_status, export) = json_request(
        router,
        http::Method::GET,
        "/api/admin/export?variant=full",
        Value::Null,
    )
    .await;
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
    let _ = read_ws_type(&mut socket_1, "session_started").await;
    let _ = read_ws_type(&mut socket_2, "session_started").await;

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
    let (human, public_session_id) = create_human_vs_agent_room(router.clone(), "Human").await;
    let (base_url, server) = spawn_test_server(router).await;
    let (mut socket, _) =
        connect_async(game_socket_url(&base_url, &public_session_id, &human).await)
            .await
            .unwrap();
    let assigned = read_ws_type(&mut socket, "session_started").await;
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
    let (human, public_session_id) = create_human_vs_agent_room(router.clone(), "Human").await;
    let (base_url, server) = spawn_test_server(router.clone()).await;
    let (mut socket, _) =
        connect_async(game_socket_url(&base_url, &public_session_id, &human).await)
            .await
            .unwrap();
    let _ = read_ws_type(&mut socket, "session_started").await;
    let _ = wait_for_export_event(router.clone(), "agent_started").await;

    send_ws_json(
        &mut socket,
        json!({"type": "message", "text": "typed hello"}),
    )
    .await;
    let _ = read_ws_type(&mut socket, "message").await;
    let (status, _) = json_request(
        router,
        http::Method::POST,
        &format!("/api/sessions/{public_session_id}/transcripts"),
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
        if captured.contains(&"message:A:typed hello".to_string()) {
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
    let (human, public_session_id) = create_human_vs_agent_room(router.clone(), "Human").await;
    let (base_url, server) = spawn_test_server(router.clone()).await;
    let (mut socket, _) =
        connect_async(game_socket_url(&base_url, &public_session_id, &human).await)
            .await
            .unwrap();
    let _ = read_ws_type(&mut socket, "session_started").await;
    let _ = wait_for_export_event(router.clone(), "agent_started").await;

    send_ws_json(
        &mut socket,
        json!({"type": "action", "action": {"finish": false}}),
    )
    .await;
    let _ = read_ws_type(&mut socket, "transition").await;

    for _ in 0..20 {
        let captured = observations.lock().unwrap().clone();
        if captured.contains(&"action:A:false:false".to_string()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(observations
        .lock()
        .unwrap()
        .contains(&"action:A:false:false".to_string()));

    send_ws_json(
        &mut socket,
        json!({"type": "action", "action": {"finish": true}}),
    )
    .await;
    let _ = read_ws_type(&mut socket, "completed").await;
    for _ in 0..20 {
        let captured = observations.lock().unwrap().clone();
        if captured.contains(&"finish:success:10".to_string()) {
            assert!(captured.contains(&"action:A:true:true".to_string()));
            server.abort();
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    server.abort();
    panic!("expected agent to observe the terminal action and completion");
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
    let (human, public_session_id) = create_human_vs_agent_room(router.clone(), "Human").await;
    let (base_url, server) = spawn_test_server(router.clone()).await;
    let (mut socket, _) =
        connect_async(game_socket_url(&base_url, &public_session_id, &human).await)
            .await
            .unwrap();
    let _ = read_ws_type(&mut socket, "session_started").await;
    let first_update = read_next_ws_value(&mut socket).await;
    assert_eq!(first_update["type"], "transition");
    let message = read_ws_type(&mut socket, "message").await;
    assert_eq!(message["message"]["sender"], "B");
    assert_eq!(message["message"]["input"], "text");
    assert_eq!(message["message"]["text"], "agent says hello");
    let completed = read_ws_type(&mut socket, "completed").await;
    assert_eq!(completed["completion"]["done"], true);

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
    let (human, public_session_id) = create_human_vs_agent_room(router.clone(), "Human").await;
    let (base_url, server) = spawn_test_server(router).await;
    let (mut socket, _) =
        connect_async(game_socket_url(&base_url, &public_session_id, &human).await)
            .await
            .unwrap();
    let _ = read_ws_type(&mut socket, "session_started").await;

    for _ in 0..20 {
        let snapshot = log.lock().unwrap().clone();
        if snapshot.iter().any(|entry| entry == "respond:2") {
            assert_eq!(
                snapshot,
                vec!["respond:1", "observe_transition:B", "respond:2"]
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
    let (human, public_session_id) = create_human_vs_agent_room(router.clone(), "Human").await;
    let (base_url, server) = spawn_test_server(router.clone()).await;
    let (mut socket, _) =
        connect_async(game_socket_url(&base_url, &public_session_id, &human).await)
            .await
            .unwrap();
    let _ = read_ws_type(&mut socket, "session_started").await;

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
                .contains("invalid_tiny_action")
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
    let (human, public_session_id) = create_human_vs_agent_room(router.clone(), "Human").await;
    let (base_url, server) = spawn_test_server(router.clone()).await;
    let (mut socket, _) =
        connect_async(game_socket_url(&base_url, &public_session_id, &human).await)
            .await
            .unwrap();
    let _ = read_ws_type(&mut socket, "session_started").await;

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
    let (human, public_session_id) = create_human_vs_agent_room(router.clone(), "Human").await;
    let (base_url, server) = spawn_test_server(router.clone()).await;
    let (mut socket, _) =
        connect_async(game_socket_url(&base_url, &public_session_id, &human).await)
            .await
            .unwrap();
    let _ = read_ws_type(&mut socket, "session_started").await;

    let _ = wait_for_tts_diagnostic(router.clone(), "tts_message_failed").await;
    send_ws_json(
        &mut socket,
        json!({"type": "message", "text": "please try again"}),
    )
    .await;
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
    config.speechmatics.realtime_url = "wss://speech-experiment.test/v2".to_string();
    config.speechmatics.api_key = "speechmatics-sentinel-secret".to_string();
    config.tts.base_url = "wss://tts-experiment.test".to_string();
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
    assert!(serialized.contains("wss://speech-experiment.test/v2"));
    assert!(serialized.contains("wss://tts-experiment.test"));
    assert!(!serialized.contains("\"experiment\""));
    assert!(!serialized.contains("\"server\""));
    assert!(!serialized.contains("\"database\""));
}

#[derive(Clone)]
struct LifecycleOrderGame {
    events: Arc<Mutex<Vec<&'static str>>>,
    logger: crate::SessionLogger,
}

#[derive(Clone)]
struct LifecycleOrderGameFactory {
    events: Arc<Mutex<Vec<&'static str>>>,
}

impl crate::GameFactory for LifecycleOrderGameFactory {
    type Game = LifecycleOrderGame;

    /// Creates a session-local lifecycle recorder which shares the test event sink.
    fn create(&self, context: crate::GameSessionContext) -> Result<LifecycleOrderGame> {
        Ok(LifecycleOrderGame {
            events: self.events.clone(),
            logger: context.logger,
        })
    }
}

struct LifecycleOrderAgent;

#[async_trait]
impl Agent<LifecycleOrderGame> for LifecycleOrderAgent {
    /// Declines to act; this agent exists only to verify initialization order.
    async fn respond(
        &mut self,
        _available_actions: Option<Vec<TinyAction>>,
    ) -> Result<Option<AgentResponse<TinyAction>>> {
        Ok(None)
    }
}

struct LifecycleOrderFactory {
    events: Arc<Mutex<Vec<&'static str>>>,
}

#[async_trait]
impl AgentFactory<LifecycleOrderGame> for LifecycleOrderFactory {
    /// Describes the lifecycle-order test factory.
    fn definition(&self) -> AgentDefinition {
        test_agent_definition()
    }

    /// Records that agent construction completed before returning the instance.
    async fn create(
        &self,
        context: AgentContext,
    ) -> Result<Box<dyn Agent<LifecycleOrderGame> + Send>> {
        self.events.lock().unwrap().push("agent_created");
        context.logger.log("agent constructor log")?;
        Ok(Box::new(LifecycleOrderAgent))
    }

    fn identity(&self, _settings: &Value) -> Result<AgentIdentity> {
        test_agent_identity()
    }
}

impl Game for LifecycleOrderGame {
    type Config = Value;
    type State = TinyState;
    type Action = TinyAction;
    type Observation = TinyObservation;
    type Completion = TinySummary;

    /// Records initial-state construction after the agent factory has completed.
    fn initial_state(
        &self,
        _context: GameInitializationContext<'_, Self::Config>,
    ) -> Result<Self::State> {
        self.events.lock().unwrap().push("initial_state");
        let _ = self.logger.log("initial state constructed by game code");
        Ok(TinyState { done: false })
    }

    /// Applies the tiny test transition.
    fn apply_action(
        &self,
        state: &Self::State,
        action: &Self::Action,
        actor: PlayerRole,
    ) -> std::result::Result<Self::State, ActionRejection> {
        TinyAdapter.apply_action(state, action, actor)
    }

    /// Projects the tiny role-specific observation.
    fn observation(&self, state: &Self::State, role: PlayerRole) -> Self::Observation {
        TinyAdapter.observation(state, role)
    }

    /// Returns no terminal result for the lifecycle-order setup path.
    fn completion(&self, state: &Self::State) -> Option<Self::Completion> {
        TinyAdapter.completion(state)
    }
}

#[tokio::test]
async fn agent_construction_precedes_initial_state_construction() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let router = build_router(
        LifecycleOrderGameFactory {
            events: events.clone(),
        },
        human_vs_agent_config(),
        ServeOptions {
            agent_factory: Some(Arc::new(LifecycleOrderFactory {
                events: events.clone(),
            })),
            ..ServeOptions::default()
        },
    )
    .await
    .unwrap();
    let human = create_direct_participant(router.clone(), "Human").await;
    consent_participant(router.clone(), &human).await;

    let (status, _) = json_request(
        router.clone(),
        http::Method::POST,
        "/api/sessions",
        json!({"participant_session_id": human}),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        events.lock().unwrap().as_slice(),
        &["agent_created", "initial_state"]
    );

    let mut logs = Vec::new();
    for _ in 0..20 {
        let (export_status, export) = json_request(
            router.clone(),
            http::Method::GET,
            "/api/admin/export?variant=full",
            Value::Null,
        )
        .await;
        assert_eq!(export_status, StatusCode::OK);
        logs = export["session_events"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|event| event["event_type"] == "log")
            .cloned()
            .collect();
        if logs.len() == 2 {
            break;
        }
        tokio::task::yield_now().await;
    }
    let game_log = logs
        .iter()
        .find(|event| event["payload"]["source"] == "game")
        .expect("game log was persisted");
    assert_eq!(game_log["payload"]["source"], "game");
    assert_eq!(
        game_log["payload"]["text"],
        "initial state constructed by game code"
    );
    let agent_log = logs
        .iter()
        .find(|event| event["payload"]["source"] == "agent")
        .expect("agent constructor log was persisted");
    assert_eq!(agent_log["actor_role"], "B");
    assert!(agent_log["actor_participant_id"].as_i64().is_some());
    assert_eq!(agent_log["payload"]["text"], "agent constructor log");
}
