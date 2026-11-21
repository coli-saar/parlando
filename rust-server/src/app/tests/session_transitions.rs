//! Checks the boundaries between admission, durable lifecycle, and ordered publication.
use super::*;

/// Issues overlapping joins for one authenticated participant through the real handler.
#[tokio::test]
async fn concurrent_agent_room_joins_reserve_one_session() {
    let router = build_router(
        live_game(),
        human_vs_agent_config(),
        ServeOptions {
            agent_factory: Some(Arc::new(NoopAgentFactory)),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let human = create_direct_participant(router.clone(), "same human").await;
    consent_participant(router.clone(), &human).await;
    let replies = futures_util::future::join_all((0..8).map(|_| {
        participant_json_request(
            router.clone(),
            &human,
            http::Method::POST,
            "/api/sessions",
            json!({}),
        )
    }))
    .await;
    let room = replies[0].1["participant_state"]["public_session_id"]
        .as_str()
        .unwrap();
    for (status, body) in &replies {
        assert_eq!(*status, StatusCode::OK);
        assert_eq!(body["participant_state"]["public_session_id"], room);
    }
    let (_, sessions) = admin_json_request(
        router,
        http::Method::GET,
        "/api/admin/sessions",
        Value::Null,
    )
    .await;
    assert_eq!(sessions["sessions"].as_array().unwrap().len(), 1);
}

/// Fails the first membership insert, verifies it was never published, and retries.
#[tokio::test]
async fn failed_first_membership_can_retry_without_reusing_a_partial_room() {
    let (config, directory) = sqlite_config();
    let router = build_router(live_game(), config, ServeOptions::default())
        .await
        .unwrap();
    let pool = sqlx::SqlitePool::connect(
        directory
            .path()
            .join("server-core.sqlite")
            .to_str()
            .unwrap(),
    )
    .await
    .unwrap();
    let participant = create_direct_participant(router.clone(), "first seat").await;
    consent_participant(router.clone(), &participant).await;
    sqlx::query("create trigger fail_membership before insert on session_participants begin select raise(abort, 'injected failure'); end").execute(&pool).await.unwrap();
    let (status, _) = participant_json_request(
        router.clone(),
        &participant,
        http::Method::POST,
        "/api/sessions",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    let (_, state) = participant_json_request(
        router.clone(),
        &participant,
        http::Method::GET,
        "/api/participant-state",
        Value::Null,
    )
    .await;
    assert_eq!(state["participant_state"]["state"], "registered");
    sqlx::query("drop trigger fail_membership")
        .execute(&pool)
        .await
        .unwrap();
    let (status, state) = participant_json_request(
        router,
        &participant,
        http::Method::POST,
        "/api/sessions",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(state["participant_state"]["state"], "waiting");
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "select lifecycle, initialization_complete from sessions order by session_id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(rows, vec![("ended".into(), 1), ("forming".into(), 1)]);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("select count(*) from session_participants")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
}

/// Confirms that a prior terminal database commit dominates a stale in-memory action.
#[tokio::test]
async fn already_ended_storage_never_publishes_an_accepted_action() {
    let (config, _directory) = sqlite_config();
    let url = config.database.url.clone();
    let router = build_router(live_game(), config, ServeOptions::default())
        .await
        .unwrap();
    let (a, b, room) = create_joined_room(router.clone()).await;
    let (base, server) = spawn_test_server(router.clone()).await;
    let (mut first, _) = connect_async(game_socket_url(&router, &base, &room, &a).await)
        .await
        .unwrap();
    let (mut second, _) = connect_async(game_socket_url(&router, &base, &room, &b).await)
        .await
        .unwrap();
    read_participant_state(&mut first, "active").await;
    read_participant_state(&mut second, "active").await;
    let (_, list) = admin_json_request(
        router.clone(),
        http::Method::GET,
        "/api/admin/sessions",
        Value::Null,
    )
    .await;
    let id = list["sessions"][0]["session_id"].as_i64().unwrap();
    let store = crate::storage::SqliteExperimentStore::connect(&url)
        .await
        .unwrap();
    let end = SessionEnd {
        cause: SessionEndCause::IdleTimedOut,
        completion: None,
        participant_results: HashMap::new(),
    };
    store
        .end_session(
            SessionEventRecord {
                experiment_id: "step5".into(),
                session_id: id,
                event_type: "session_ended".into(),
                actor_participant_id: None,
                actor_role: None,
                payload: json!(end),
                game_state: None,
            },
            end,
        )
        .await
        .unwrap();
    send_ws_json(
        &mut first,
        json!({"type":"action","action":{"finish":true}}),
    )
    .await;
    assert_eq!(
        read_ws_type(&mut first, "action_rejected").await["code"],
        "session_complete"
    );
    assert_no_ws_type(&mut second, "transition").await;
    let events = store.session_events("step5", id, None).await.unwrap();
    assert!(!events
        .iter()
        .any(|e| e.event_type == "game_action_accepted"));
    server.abort();
}

/// A game whose observations and affordances identify the exact action revision.
/// Enables counter mechanics on the shared Wasm fixture.
fn counter_config(mut config: ExperimentConfig, checkpointed: bool) -> ExperimentConfig {
    config.game = json!({"counter": true, "checkpointed": checkpointed});
    config
}

/// Compares both clients' independently targeted transitions against durable replay.
#[tokio::test]
async fn simultaneous_actions_publish_role_private_snapshots_in_replay_order() {
    let router = build_router(
        live_game(),
        counter_config(step_five_config(), false),
        ServeOptions::default(),
    )
    .await
    .unwrap();
    let (a, b, room) = create_joined_room(router.clone()).await;
    let (base, server) = spawn_test_server(router.clone()).await;
    let (mut first, _) = connect_async(game_socket_url(&router, &base, &room, &a).await)
        .await
        .unwrap();
    let (mut second, _) = connect_async(game_socket_url(&router, &base, &room, &b).await)
        .await
        .unwrap();
    read_participant_state(&mut first, "active").await;
    read_participant_state(&mut second, "active").await;
    for i in 1..=12 {
        tokio::join!(
            send_ws_json(&mut first, json!({"type":"action","action":i})),
            send_ws_json(&mut second, json!({"type":"action","action":i+20}))
        );
    }
    let mut packets = vec![];
    for _ in 0..24 {
        packets.push(tokio::join!(
            read_ws_type(&mut first, "transition"),
            read_ws_type(&mut second, "transition")
        ));
    }
    let (_, list) = admin_json_request(
        router.clone(),
        http::Method::GET,
        "/api/admin/sessions",
        Value::Null,
    )
    .await;
    let id = list["sessions"][0]["session_id"].as_i64().unwrap();
    let (status, replay) = admin_json_request(
        router,
        http::Method::GET,
        &format!("/api/admin/sessions/{id}/replay"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    for (i, (a, b)) in packets.iter().enumerate() {
        for (role, packet) in [("A", a), ("B", b)] {
            let frame = &replay["roles"][role][i + 1];
            assert_eq!(packet["observation"], frame["observation"]);
            assert_eq!(packet["available_actions"], frame["available_actions"]);
            assert_eq!(packet["actor"], frame["transition"]["actor"]);
            assert_eq!(packet["action"], frame["transition"]["action"]);
        }
    }
    server.abort();
}

/// Refuses a prepared action in SQLite and checks that only a later durable action is visible.
#[tokio::test]
async fn failed_action_write_keeps_the_committed_checkpoint_for_reconnect_and_recovery() {
    let (config, _directory) = sqlite_config();
    let database_url = config.database.url.clone();
    let router = build_router(
        live_game(),
        counter_config(config, false),
        ServeOptions::default(),
    )
    .await
    .unwrap();
    let pool = sqlx::SqlitePool::connect(&database_url).await.unwrap();
    let (a, b, room) = create_joined_room(router.clone()).await;
    let (base, server) = spawn_test_server(router.clone()).await;
    let first_url = game_socket_url(&router, &base, &room, &a).await;
    let second_url = game_socket_url(&router, &base, &room, &b).await;
    let (mut first, _) = connect_async(&first_url).await.unwrap();
    let (mut second, _) = connect_async(&second_url).await.unwrap();
    read_participant_state(&mut first, "active").await;
    read_participant_state(&mut second, "active").await;
    sqlx::query(
        "create trigger refuse_action before insert on session_events
         when new.event_type = 'game_action_accepted'
         begin select raise(abort, 'refused action'); end",
    )
    .execute(&pool)
    .await
    .unwrap();

    send_ws_json(&mut first, json!({"type":"action","action":9})).await;
    assert_eq!(
        read_ws_type(&mut first, "error").await["code"],
        "internal_error"
    );
    assert_no_ws_type(&mut second, "transition").await;
    let accepted_count = sqlx::query_scalar::<_, i64>(
        "select count(*) from session_events where event_type = 'game_action_accepted'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(accepted_count, 0);
    let (mut replacement, _) = connect_async(game_socket_url(&router, &base, &room, &a).await)
        .await
        .unwrap();
    let snapshot = read_participant_state(&mut replacement, "active").await;
    assert_eq!(snapshot["observation"]["total"], 0);
    assert!(snapshot["available_actions"].is_null());

    sqlx::query("drop trigger refuse_action")
        .execute(&pool)
        .await
        .unwrap();
    send_ws_json(&mut replacement, json!({"type":"action","action":4})).await;
    let (first_transition, second_transition) = tokio::join!(
        read_ws_type(&mut replacement, "transition"),
        read_ws_type(&mut second, "transition"),
    );
    for transition in [first_transition, second_transition] {
        assert_eq!(transition["observation"]["total"], 4);
        assert!(transition["available_actions"].is_null());
    }
    server.abort();
}

/// Replays a recorded checkpoint and fails closed if the checkpoint disappears.
#[tokio::test]
async fn checkpointed_turn_replays_without_reexecution_or_corpus_disclosure() {
    let (config, _directory) = sqlite_config();
    let database_url = config.database.url.clone();
    let router = build_router(
        live_game(),
        counter_config(config, true),
        ServeOptions::default(),
    )
    .await
    .unwrap();
    let (a, b, room) = create_joined_room(router.clone()).await;
    let (base, server) = spawn_test_server(router.clone()).await;
    let (mut first, _) = connect_async(game_socket_url(&router, &base, &room, &a).await)
        .await
        .unwrap();
    let (mut second, _) = connect_async(game_socket_url(&router, &base, &room, &b).await)
        .await
        .unwrap();
    read_participant_state(&mut first, "active").await;
    read_participant_state(&mut second, "active").await;
    send_ws_json(&mut first, json!({"type":"action","action":4})).await;
    let transition = read_ws_type(&mut first, "transition").await;
    assert_eq!(transition["observation"]["total"], 4);

    let pool = sqlx::SqlitePool::connect(&database_url).await.unwrap();
    let (session_id, checkpoint, required): (i64, String, bool) = sqlx::query_as(
        "select session_id, game_state_json, json_extract(payload_json, '$.recovery_checkpoint')
         from session_events where event_type = 'game_action_accepted'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(required);
    assert_eq!(
        serde_json::from_str::<Value>(&checkpoint).unwrap()["views"][0]["total"],
        4
    );
    let corpus = assert_corpus_session(router.clone(), "playing").await;
    let events = corpus["experiment"]["sessions"][0]["events"]
        .as_array()
        .unwrap();
    let action = events
        .iter()
        .find(|event| event["kind"] == "action")
        .unwrap();
    assert_eq!(action["action"], 4);
    assert_eq!(action.get("state"), Some(&Value::Null));

    // A checkpoint must supply the views without asking the game to replay this action.
    sqlx::query(
        "update session_events set payload_json = json_set(payload_json, '$.action', 'invalid')
         where session_id = ? and event_type = 'game_action_accepted'",
    )
    .bind(session_id)
    .execute(&pool)
    .await
    .unwrap();
    let replay_url = format!("/api/admin/sessions/{session_id}/replay");
    let (status, replay) =
        admin_json_request(router.clone(), http::Method::GET, &replay_url, Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    for role in ["A", "B"] {
        let frame = &replay["roles"][role][1];
        assert_eq!(frame["observation"]["total"], 4);
        assert_eq!(frame["observation"]["role"], role);
        assert!(frame["available_actions"].is_null());
    }

    sqlx::query(
        "update session_events set game_state_json = null
         where session_id = ? and event_type = 'game_action_accepted'",
    )
    .bind(session_id)
    .execute(&pool)
    .await
    .unwrap();
    let (status, error) =
        admin_json_request(router, http::Method::GET, &replay_url, Value::Null).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(error.to_string().contains("Missing recovery checkpoint"));
    server.abort();
}

/// Cancels the submitter after COMMIT and confirms the lost acknowledgment from durable storage.
#[tokio::test]
async fn ambiguous_commit_survives_cancellation_and_publishes_once() {
    let (service, state) = super::super::routing::build_router_with_state(
        live_game(),
        counter_config(step_five_config(), false),
    )
    .await
    .unwrap();
    let router = TestRouter::new(service);
    authenticate_test_admin(router.clone()).await.unwrap();
    let (status, _) = admin_json_request(
        router.clone(),
        http::Method::POST,
        "/api/admin/experiment/status",
        json!({"status": "active"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (a, b, room) = create_joined_room(router.clone()).await;
    let (base, server) = spawn_test_server(router.clone()).await;
    let (mut first, _) = connect_async(game_socket_url(&router, &base, &room, &a).await)
        .await
        .unwrap();
    let (mut second, _) = connect_async(game_socket_url(&router, &base, &room, &b).await)
        .await
        .unwrap();
    read_participant_state(&mut first, "active").await;
    read_participant_state(&mut second, "active").await;

    let session_id = state.memory.read().await.sessions[&room].session_id;
    let store = &state.context.installation.store;
    let (committed, commit_ack) = tokio::sync::oneshot::channel();
    let (release, hold_ack) = tokio::sync::oneshot::channel();
    *store.transition_ack_fault.lock().unwrap() = Some(crate::storage::TransitionAckFault {
        committed,
        release: hold_ack,
    });
    store
        .fail_transition_probe_once
        .store(true, Ordering::Relaxed);
    let submitter = tokio::spawn({
        let (state, room, a) = (state.clone(), room.clone(), a.clone());
        async move {
            super::super::action::submit_action(
                state,
                &room,
                &a,
                PlayerRole::A,
                ComponentValue::from_typed(&4u32).unwrap(),
            )
            .await
        }
    });
    tokio::time::timeout(Duration::from_secs(3), commit_ack)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        state.memory.read().await.sessions[&room]
            .state()
            .observation(PlayerRole::A)
            .decode::<Value>()
            .unwrap()["total"],
        0
    );
    submitter.abort();
    assert!(submitter.await.unwrap_err().is_cancelled());

    let queued = tokio::spawn({
        let (state, room, b) = (state.clone(), room.clone(), b.clone());
        async move {
            super::super::action::submit_action(
                state,
                &room,
                &b,
                PlayerRole::B,
                ComponentValue::from_typed(&9u32).unwrap(),
            )
            .await
        }
    });
    release.send(()).unwrap();
    let queue_error = tokio::time::timeout(Duration::from_secs(3), queued)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert!(queue_error
        .to_string()
        .contains("awaiting storage confirmation"));
    assert_eq!(
        state.memory.read().await.sessions[&room]
            .state()
            .observation(PlayerRole::A)
            .decode::<Value>()
            .unwrap()["total"],
        0
    );

    let (first_transition, second_transition) =
        tokio::time::timeout(Duration::from_secs(8), async {
            tokio::join!(
                read_ws_type(&mut first, "transition"),
                read_ws_type(&mut second, "transition"),
            )
        })
        .await
        .unwrap();
    for transition in [first_transition, second_transition] {
        assert_eq!(transition["observation"]["total"], 4);
    }
    assert_no_ws_type(&mut second, "transition").await;
    assert_eq!(
        state.memory.read().await.sessions[&room]
            .state()
            .observation(PlayerRole::A)
            .decode::<Value>()
            .unwrap()["total"],
        4
    );
    let events = store
        .session_events("step5", session_id, None)
        .await
        .unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == "game_action_accepted")
            .count(),
        1
    );
    server.abort();
}

/// Holds only the first agent's first decision so that its session inbox can fill.
struct SlowFirstFactory {
    created: AtomicUsize,
    entered: Arc<Notify>,
    release: Arc<Notify>,
    events: mpsc::UnboundedSender<&'static str>,
}

struct BlockedDecision {
    entered: Arc<Notify>,
    release: Arc<Notify>,
    events: mpsc::UnboundedSender<&'static str>,
}

#[async_trait]
impl AgentFactory for SlowFirstFactory {
    /// Shares the supported agent settings used by the active experiment.
    fn definition(&self) -> AgentDefinition {
        test_agent_definition()
    }
    /// Creates an independently responsive agent for the second room.
    async fn create(&self, _: AgentContext) -> Result<Box<dyn Agent + Send>> {
        if self.created.fetch_add(1, Ordering::SeqCst) == 0 {
            Ok(Box::new(BlockedDecision {
                entered: self.entered.clone(),
                release: self.release.clone(),
                events: self.events.clone(),
            }))
        } else {
            Ok(Box::new(NoopAgent))
        }
    }
    /// Supplies the same identity across both instances.
    fn identity(&self, _: &Value) -> Result<AgentIdentity> {
        test_agent_identity()
    }
}

#[async_trait]
impl Agent for BlockedDecision {
    /// Holds the callback while the participant continues to make legal moves.
    async fn respond(&mut self) -> Result<Option<AgentResponse>> {
        self.entered.notify_one();
        self.release.notified().await;
        Ok(Some(AgentResponse::action(tiny_action(&TinyAction {
            finish: true,
            invalid: false,
        }))))
    }

    /// Reports each accepted transition delivered after the held callback returns.
    async fn observe_transition(
        &mut self,
        _: PlayerRole,
        _: ComponentValue,
        _: ComponentValue,
    ) -> Result<()> {
        self.events.send("observed")?;
        Ok(())
    }

    /// Marks the end of observation delivery.
    async fn shutdown(&mut self) -> Result<()> {
        self.events.send("shutdown")?;
        Ok(())
    }
}

/// A blocked agent can exhaust only its own room, before any input it cannot observe commits.
#[tokio::test]
async fn slow_agent_never_blocks_another_room_or_loses_an_accepted_action() {
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let (events, mut observed) = mpsc::unbounded_channel();
    let mut config = human_vs_agent_config();
    config
        .agents
        .human_vs_agent
        .as_mut()
        .unwrap()
        .act_timeout_seconds = 30.0;
    let router = build_router(
        live_game(),
        config,
        ServeOptions {
            agent_factory: Some(Arc::new(SlowFirstFactory {
                entered: entered.clone(),
                release: release.clone(),
                created: AtomicUsize::new(0),
                events,
            })),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let (human, room) = create_human_vs_agent_room(router.clone(), "slow").await;
    let (base, server) = spawn_test_server(router.clone()).await;
    let (mut slow, _) = connect_async(game_socket_url(&router, &base, &room, &human).await)
        .await
        .unwrap();
    read_participant_state(&mut slow, "active").await;
    tokio::time::timeout(Duration::from_secs(2), entered.notified())
        .await
        .unwrap();
    for _ in 0..64 {
        send_ws_json(
            &mut slow,
            json!({"type":"action","action":{"finish":false}}),
        )
        .await;
        read_ws_type(&mut slow, "transition").await;
    }
    send_ws_json(&mut slow, json!({"type":"action","action":{"finish":true}})).await;
    let end = read_participant_state(&mut slow, "ended").await;
    assert_eq!(end["result"]["outcome"], "technical_failure");
    let (human, room) = create_human_vs_agent_room(router.clone(), "healthy").await;
    let (mut healthy, _) = connect_async(game_socket_url(&router, &base, &room, &human).await)
        .await
        .unwrap();
    read_participant_state(&mut healthy, "active").await;
    send_ws_json(
        &mut healthy,
        json!({"type":"action","action":{"finish":true}}),
    )
    .await;
    assert_eq!(
        read_participant_state(&mut healthy, "ended").await["result"]["outcome"],
        "completed"
    );
    release.notify_one();
    tokio::time::timeout(Duration::from_secs(3), async {
        for _ in 0..64 {
            assert_eq!(observed.recv().await, Some("observed"));
        }
        assert_eq!(observed.recv().await, Some("shutdown"));
    })
    .await
    .unwrap();
    server.abort();
}

/// A replaced disconnect timer cannot expire a newer reconnect grace deadline.
#[tokio::test]
async fn second_disconnect_receives_its_own_grace_period() {
    let mut config = step_five_config();
    config.session.reconnect_grace_seconds = 2;
    let router = build_router(live_game(), config, ServeOptions::default())
        .await
        .unwrap();
    let (a, b, room) = create_joined_room(router.clone()).await;
    let (base, server) = spawn_test_server(router.clone()).await;
    let (mut first, _) = connect_async(game_socket_url(&router, &base, &room, &a).await)
        .await
        .unwrap();
    let (mut second, _) = connect_async(game_socket_url(&router, &base, &room, &b).await)
        .await
        .unwrap();
    read_participant_state(&mut first, "active").await;
    read_participant_state(&mut second, "active").await;
    first.close(None).await.unwrap();
    let old = read_participant_state(&mut second, "paused").await;
    let (mut first, _) = connect_async(game_socket_url(&router, &base, &room, &a).await)
        .await
        .unwrap();
    read_participant_state(&mut second, "active").await;
    tokio::time::sleep(Duration::from_millis(900)).await;
    first.close(None).await.unwrap();
    let current = read_participant_state(&mut second, "paused").await;
    let deadline = |snapshot: &Value| {
        chrono::DateTime::parse_from_rfc3339(snapshot["reason"]["deadline_at"].as_str().unwrap())
            .unwrap()
            .with_timezone(&chrono::Utc)
    };
    let target = deadline(&old) + chrono::Duration::milliseconds(120);
    let sleep = (target - chrono::Utc::now()).to_std().unwrap_or_default();
    tokio::time::sleep(sleep).await;
    assert!(chrono::Utc::now() < deadline(&current));
    let (_, state) = participant_json_request(
        router.clone(),
        &b,
        http::Method::GET,
        "/api/participant-state",
        Value::Null,
    )
    .await;
    assert_eq!(state["participant_state"]["state"], "paused");
    assert_eq!(
        read_participant_state(&mut second, "ended").await["result"]["outcome"],
        "partner_left"
    );
    server.abort();
}

/// Inspects the actual in-memory room count independently of durable reconciliation.
async fn retained_rooms(router: &TestRouter) -> u64 {
    let (status, load) = admin_json_request(
        router.clone(),
        http::Method::GET,
        "/api/admin/load",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    load["current"]["capacity"]["completed_retained_sessions"]
        .as_u64()
        .unwrap()
}

/// A software seat cannot pin a completed room after its last human socket closes.
#[tokio::test]
async fn terminal_agent_room_retains_only_human_game_connections_and_grace() {
    let mut config = human_vs_agent_config();
    config.session.reconnect_grace_seconds = 1;
    let router = build_router(
        live_game(),
        config,
        ServeOptions {
            agent_factory: Some(Arc::new(NoopAgentFactory)),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let (human, room) = create_human_vs_agent_room(router.clone(), "human").await;
    let (base, server) = spawn_test_server(router.clone()).await;
    let (mut socket, _) = connect_async(game_socket_url(&router, &base, &room, &human).await)
        .await
        .unwrap();
    read_participant_state(&mut socket, "active").await;
    send_ws_json(
        &mut socket,
        json!({"type":"action","action":{"finish":true}}),
    )
    .await;
    let ended = read_participant_state(&mut socket, "ended").await;
    tokio::time::sleep(Duration::from_millis(1250)).await;
    assert_eq!(
        retained_rooms(&router).await,
        1,
        "game transport is still connected"
    );
    socket.close(None).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        retained_rooms(&router).await,
        1,
        "human still has a grace period"
    );
    wait_for_completed_runtime_cleanup(router.clone()).await;
    let (status, result) = participant_json_request(
        router,
        &human,
        http::Method::GET,
        "/api/participant-state",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(result["participant_state"], ended);
    server.abort();
}

/// A participant can leave via HTTP before opening the first game socket.
#[tokio::test]
async fn terminal_waiting_room_with_no_game_socket_expires_after_grace() {
    let mut config = step_five_config();
    config.session.reconnect_grace_seconds = 1;
    let router = build_router(live_game(), config, ServeOptions::default())
        .await
        .unwrap();
    let human = create_direct_participant(router.clone(), "human").await;
    consent_participant(router.clone(), &human).await;
    let (_, joined) = participant_json_request(
        router.clone(),
        &human,
        http::Method::POST,
        "/api/sessions",
        json!({}),
    )
    .await;
    let room = joined["participant_state"]["public_session_id"]
        .as_str()
        .unwrap();
    let (status, ended) = participant_json_request(
        router.clone(),
        &human,
        http::Method::POST,
        &format!("/api/sessions/{room}/leave"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        ended["participant_state"]["result"]["outcome"],
        "left_waiting_room"
    );
    assert_eq!(retained_rooms(&router).await, 1);
    wait_for_completed_runtime_cleanup(router.clone()).await;
    let (status, result) = participant_json_request(
        router,
        &human,
        http::Method::POST,
        "/api/sessions",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(result, ended);
}

/// Reconnecting within the socket grace period still exposes the room's committed idle ending.
#[tokio::test]
async fn reconnect_within_grace_after_idle_deadline_returns_ended_room() {
    let mut config = step_five_config();
    config.session.session_idle_timeout_seconds = 2;
    config.session.reconnect_grace_seconds = 60;
    let router = build_router(live_game(), config, ServeOptions::default())
        .await
        .unwrap();
    let (a, b, room) = create_joined_room(router.clone()).await;
    let (base, server) = spawn_test_server(router.clone()).await;
    let (mut first, _) = connect_async(game_socket_url(&router, &base, &room, &a).await)
        .await
        .unwrap();
    let (mut second, _) = connect_async(game_socket_url(&router, &base, &room, &b).await)
        .await
        .unwrap();
    read_participant_state(&mut first, "active").await;
    read_participant_state(&mut second, "active").await;
    second.close(None).await.unwrap();
    let paused = read_participant_state(&mut first, "paused").await;
    let idle =
        chrono::DateTime::parse_from_rfc3339(paused["idle_deadline_at"].as_str().unwrap()).unwrap();
    let grace =
        chrono::DateTime::parse_from_rfc3339(paused["reason"]["deadline_at"].as_str().unwrap())
            .unwrap();
    let reconnect_url = game_socket_url(&router, &base, &room, &b).await;
    let until_idle = idle.with_timezone(&chrono::Utc) - chrono::Utc::now();
    tokio::time::sleep(until_idle.to_std().unwrap_or_default()).await;
    let (mut replacement, _) = connect_async(reconnect_url).await.unwrap();
    let ended = read_participant_state(&mut replacement, "ended").await;
    assert!(chrono::Utc::now() < grace);
    assert_eq!(ended["public_session_id"], room);
    assert_eq!(ended["result"]["reason"], "idle_timeout");
    assert_eq!(ended["result"]["outcome"], "idle_limit_reached");
    server.abort();
}
