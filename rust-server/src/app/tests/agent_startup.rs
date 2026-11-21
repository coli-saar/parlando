use super::*;

#[tokio::test]
async fn human_vs_agent_direct_room_eventually_supplies_agent_role_b() {
    let router = build_router(
        live_game(),
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

    let (status, created) = participant_json_request(
        router.clone(),
        &human,
        http::Method::POST,
        "/api/sessions",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(created["participant_state"]["role"], "A");
    let mut export = Value::Null;
    for _ in 0..20 {
        let (export_status, current_export) = admin_json_request(
            router.clone(),
            http::Method::GET,
            "/api/admin/export?variant=full",
            Value::Null,
        )
        .await;
        assert_eq!(export_status, StatusCode::OK);
        export = current_export;
        if export["session_participants"]
            .as_array()
            .is_some_and(|rows| rows.iter().any(|row| row["role"] == "B"))
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
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
async fn human_enters_waiting_room_before_agent_construction_finishes() {
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let router = build_router(
        live_game(),
        human_vs_agent_config(),
        ServeOptions {
            agent_factory: Some(Arc::new(GatedAgentFactory {
                entered: entered.clone(),
                release: release.clone(),
            })),
            ..ServeOptions::default()
        },
    )
    .await
    .unwrap();
    let human = create_direct_participant(router.clone(), "Human").await;
    consent_participant(router.clone(), &human).await;

    let (status, created) = participant_json_request(
        router.clone(),
        &human,
        http::Method::POST,
        "/api/sessions",
        json!({}),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    entered.notified().await;
    assert!(created["participant_state"]["presence"]["B"].is_null());
    release.notify_one();
}

#[tokio::test]
async fn failed_agent_construction_ends_the_waiting_session_as_technical_failure() {
    let router = build_router(
        live_game(),
        human_vs_agent_config(),
        ServeOptions {
            agent_factory: Some(Arc::new(FailingAgentFactory)),
            ..ServeOptions::default()
        },
    )
    .await
    .unwrap();
    let human = create_direct_participant(router.clone(), "Human").await;
    consent_participant(router.clone(), &human).await;
    let (status, created) = participant_json_request(
        router.clone(),
        &human,
        http::Method::POST,
        "/api/sessions",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(created["participant_state"]["state"], "waiting");

    for _ in 0..20 {
        let (state_status, participant_state) = participant_json_request(
            router.clone(),
            &human,
            http::Method::GET,
            "/api/participant-state",
            json!({}),
        )
        .await;
        assert_eq!(state_status, StatusCode::OK);
        if participant_state["participant_state"]["state"] == "ended" {
            assert_eq!(
                participant_state["participant_state"]["result"]["outcome"],
                "technical_failure"
            );
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("failed agent construction did not terminate the waiting session");
}

#[tokio::test]
/// Activation must wait for initialization; the first public input must reach the agent.
async fn agent_readiness_activation_waits_for_start_and_delivers_first_input() {
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let (events, mut callbacks) = mpsc::unbounded_channel();
    let router = build_router(
        live_game(),
        human_vs_agent_config(),
        ServeOptions {
            agent_factory: Some(Arc::new(ReadinessAgentFactory {
                gate_construction: false,
                entered: entered.clone(),
                release: release.clone(),
                events,
            })),
            ..ServeOptions::default()
        },
    )
    .await
    .unwrap();
    let (human, room) = create_human_vs_agent_room(router.clone(), "Human").await;
    let (base_url, server) = spawn_test_server(router.clone()).await;
    let (mut socket, _) = connect_async(game_socket_url(&router, &base_url, &room, &human).await)
        .await
        .unwrap();
    readiness_signal(&entered).await;
    let (status, blocked) = participant_json_request(
        router.clone(),
        &human,
        http::Method::GET,
        "/api/participant-state",
        json!({}),
    )
    .await;
    release.notify_one();
    // Only the public active notification authorizes input; no internal event wait.
    let _ = read_participant_state(&mut socket, "active").await;
    send_ws_json(
        &mut socket,
        json!({"type":"action", "action":{"finish":false}}),
    )
    .await;
    let _ = read_ws_type(&mut socket, "transition").await;
    send_ws_json(
        &mut socket,
        json!({"type":"message", "text":"first message"}),
    )
    .await;
    let _ = read_ws_type(&mut socket, "message").await;
    let mut observed = Vec::new();
    // Confirm delivery before deliberately terminating the game.
    for _ in 0..3 {
        observed.push(readiness_event(&mut callbacks).await);
    }
    let (leave_status, _) = participant_json_request(
        router,
        &human,
        http::Method::POST,
        &format!("/api/sessions/{room}/leave"),
        json!({}),
    )
    .await;
    loop {
        let event = readiness_event(&mut callbacks).await;
        let stopped = event == "shutdown";
        observed.push(event);
        if stopped {
            break;
        }
    }
    server.abort();
    assert_eq!(status, StatusCode::OK);
    assert_eq!(leave_status, StatusCode::OK);
    assert_eq!(
        observed,
        [
            "started",
            "action:A:false",
            "message:A:first message",
            "shutdown"
        ]
    );
    assert_eq!(
        blocked["participant_state"]["state"], "waiting",
        "game advertised activation while Agent::start was still blocked"
    );
}

#[tokio::test]
/// A construction result arriving after leave must be shut down without activation.
async fn agent_readiness_leave_during_construction_preserves_terminal_result() {
    assert_agent_readiness_cancellation(true).await;
}

#[tokio::test]
/// Initialization finishing after leave must be shut down without activation.
async fn agent_readiness_leave_during_start_preserves_terminal_result() {
    assert_agent_readiness_cancellation(false).await;
}

#[tokio::test]
async fn failed_agent_start_ends_the_session_and_shuts_the_agent_down() {
    let shutdowns = Arc::new(AtomicUsize::new(0));
    let stopped = Arc::new(Notify::new());
    let router = build_router(
        live_game(),
        human_vs_agent_config(),
        ServeOptions {
            agent_factory: Some(Arc::new(StartFailingAgentFactory {
                shutdowns: shutdowns.clone(),
                stopped: stopped.clone(),
            })),
            ..ServeOptions::default()
        },
    )
    .await
    .unwrap();
    let (human, public_session_id) = create_human_vs_agent_room(router.clone(), "Human").await;
    let (base_url, server) = spawn_test_server(router.clone()).await;
    let (mut socket, _) =
        connect_async(game_socket_url(&router, &base_url, &public_session_id, &human).await)
            .await
            .unwrap();
    let mut advertised_active = false;
    let ended = loop {
        let message = read_ws_type(&mut socket, "participant_state").await;
        let participant = &message["participant_state"];
        advertised_active |= participant["state"] == "active";
        if participant["state"] == "ended" {
            break participant.clone();
        }
    };
    assert_eq!(ended["result"]["outcome"], "technical_failure");

    readiness_signal(&stopped).await;
    server.abort();
    assert_eq!(shutdowns.load(Ordering::SeqCst), 1);
    assert!(
        !advertised_active,
        "failed initialization advertised an active game"
    );
}

#[tokio::test]
async fn agent_construction_runs_after_the_human_waiting_session_exists() {
    let router = build_router(
        live_game(),
        human_vs_agent_config(),
        ServeOptions {
            agent_factory: Some(Arc::new(LifecycleOrderFactory)),
            ..ServeOptions::default()
        },
    )
    .await
    .unwrap();
    let human = create_direct_participant(router.clone(), "Human").await;
    consent_participant(router.clone(), &human).await;

    let (status, joined) = participant_json_request(
        router.clone(),
        &human,
        http::Method::POST,
        "/api/sessions",
        json!({}),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    let mut events = Vec::new();
    for _ in 0..20 {
        let (export_status, export) = admin_json_request(
            router.clone(),
            http::Method::GET,
            "/api/admin/export?variant=full",
            Value::Null,
        )
        .await;
        assert_eq!(export_status, StatusCode::OK);
        events = export["session_events"]
            .as_array()
            .into_iter()
            .flatten()
            .cloned()
            .collect();
        if events.iter().any(|event| event["event_type"] == "log") {
            break;
        }
        tokio::task::yield_now().await;
    }
    let created = events
        .iter()
        .find(|event| event["event_type"] == "session_created")
        .expect("stored initial frame");
    let agent_log = events
        .iter()
        .find(|event| event["event_type"] == "log" && event["payload"]["source"] == "agent")
        .expect("agent constructor log was persisted");
    assert!(created["event_index"].as_i64().unwrap() < agent_log["event_index"].as_i64().unwrap());
    assert_eq!(agent_log["actor_role"], "B");
    assert!(agent_log["actor_participant_id"].as_i64().is_some());
    assert_eq!(agent_log["payload"]["text"], "agent constructor log");
    // Allow the database clock to move past the constructor log before starting the game.
    tokio::time::sleep(Duration::from_millis(2)).await;
    let room = joined["participant_state"]["public_session_id"]
        .as_str()
        .unwrap();
    let (base_url, server) = spawn_test_server(router.clone()).await;
    let (mut socket, _) = connect_async(game_socket_url(&router, &base_url, room, &human).await)
        .await
        .unwrap();
    let _ = read_participant_state(&mut socket, "active").await;
    let corpus = assert_corpus_session(router.clone(), "playing").await;
    assert_eq!(corpus["data_inventory"]["logs"], 0);
    server.abort();
}

/// Ending a forming session must dispose of late setup without changing its durable result.
async fn assert_agent_readiness_cancellation(gate_construction: bool) {
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let (events, mut callbacks) = mpsc::unbounded_channel();
    let router = build_router(
        live_game(),
        human_vs_agent_config(),
        ServeOptions {
            agent_factory: Some(Arc::new(ReadinessAgentFactory {
                gate_construction,
                entered: entered.clone(),
                release: release.clone(),
                events,
            })),
            ..ServeOptions::default()
        },
    )
    .await
    .unwrap();
    let (human, room) = create_human_vs_agent_room(router.clone(), "Human").await;
    let (base_url, server) = spawn_test_server(router.clone()).await;
    let (mut socket, _) = connect_async(game_socket_url(&router, &base_url, &room, &human).await)
        .await
        .unwrap();
    readiness_signal(&entered).await;
    let (status, ended) = participant_json_request(
        router.clone(),
        &human,
        http::Method::POST,
        &format!("/api/sessions/{room}/leave"),
        json!({}),
    )
    .await;
    let terminal = read_participant_state(&mut socket, "ended").await;
    release.notify_one();
    let mut observed = Vec::new();
    loop {
        let event = readiness_event(&mut callbacks).await;
        let stopped = event == "shutdown";
        observed.push(event);
        if stopped {
            break;
        }
    }
    let (_, after) = participant_json_request(
        router,
        &human,
        http::Method::GET,
        "/api/participant-state",
        json!({}),
    )
    .await;
    server.abort();
    assert_eq!(status, StatusCode::OK);
    assert_eq!(after["participant_state"]["state"], "ended");
    assert_eq!(after["participant_state"]["result"], terminal["result"]);
    assert_eq!(ended["participant_state"]["result"], terminal["result"]);
    assert_eq!(
        terminal["result"]["outcome"], "left_waiting_room",
        "setup must remain forming until initialization succeeds"
    );
    if gate_construction {
        assert_eq!(
            observed,
            ["shutdown"],
            "late construction must never start its agent"
        );
    } else {
        assert_eq!(observed, ["started", "shutdown"]);
    }
}

struct LifecycleOrderAgent;

#[async_trait]
impl Agent for LifecycleOrderAgent {
    /// Declines to act; this agent exists only to verify initialization order.
    async fn respond(&mut self) -> Result<Option<AgentResponse>> {
        Ok(None)
    }
}

struct LifecycleOrderFactory;

#[async_trait]
impl AgentFactory for LifecycleOrderFactory {
    /// Describes the lifecycle-order test factory.
    fn definition(&self) -> AgentDefinition {
        test_agent_definition()
    }

    /// Records that agent construction completed before returning the instance.
    async fn create(&self, context: AgentContext) -> Result<Box<dyn Agent + Send>> {
        context.logger.log("agent constructor log")?;
        Ok(Box::new(LifecycleOrderAgent))
    }

    fn identity(&self, _settings: &Value) -> Result<AgentIdentity> {
        test_agent_identity()
    }
}
