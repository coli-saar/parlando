//! Checks the boundaries between admission, durable lifecycle, and ordered publication.
use super::*;

/// Issues overlapping joins for one authenticated participant through the real handler.
#[tokio::test]
async fn concurrent_agent_room_joins_reserve_one_session() {
    let router = build_router(
        TinyAdapter,
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
    let router = build_router(TinyAdapter, config, ServeOptions::default())
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
    let router = build_router(TinyAdapter, config, ServeOptions::default())
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
struct CounterGame;

impl crate::GameFactory for CounterGame {
    type Game = Self;
    /// Creates one independent counter per session and replay.
    fn create(&self, _: GameSessionContext) -> Result<Self> {
        Ok(Self)
    }
}

impl Game for CounterGame {
    type State = u32;
    type Action = u32;
    type Observation = Value;
    type Completion = Value;
    type Config = Value;
    /// Starts the counter at zero.
    fn initial_state(&self, _: GameInitializationContext<'_, Value>) -> Result<u32> {
        Ok(0)
    }
    /// Adds the supplied increment to the previous revision.
    fn apply_action(
        &self,
        state: &u32,
        action: &u32,
        _: PlayerRole,
    ) -> std::result::Result<u32, ActionRejection> {
        Ok(state + action)
    }
    /// Identifies the private recipient as well as the resulting total.
    fn observation(&self, state: &u32, role: PlayerRole) -> Value {
        json!({"total":state,"role":role.as_str()})
    }
    /// Supplies an affordance that also depends on the accepted revision.
    fn available_actions(&self, state: &u32, _: PlayerRole) -> Option<Vec<u32>> {
        Some(vec![state + 1])
    }
    /// Allows all of the concurrent submissions to progress.
    fn completion(&self, _: &u32) -> Option<Value> {
        None
    }
}

/// Compares both clients' independently targeted transitions against durable replay.
#[tokio::test]
async fn simultaneous_actions_publish_role_private_snapshots_in_replay_order() {
    let router = build_router(CounterGame, step_five_config(), ServeOptions::default())
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
impl AgentFactory<TinyAdapter> for SlowFirstFactory {
    /// Shares the supported agent settings used by the active experiment.
    fn definition(&self) -> AgentDefinition {
        test_agent_definition()
    }
    /// Creates an independently responsive agent for the second room.
    async fn create(&self, _: AgentContext) -> Result<Box<dyn Agent<TinyAdapter> + Send>> {
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
impl Agent<TinyAdapter> for BlockedDecision {
    /// Holds the callback while the participant continues to make legal moves.
    async fn respond(
        &mut self,
        _: Option<Vec<TinyAction>>,
    ) -> Result<Option<AgentResponse<TinyAction>>> {
        self.entered.notify_one();
        self.release.notified().await;
        Ok(Some(AgentResponse::action(TinyAction {
            finish: true,
            invalid: false,
        })))
    }

    /// Reports each accepted transition delivered after the held callback returns.
    async fn observe_transition(
        &mut self,
        _: PlayerRole,
        _: TinyAction,
        _: TinyObservation,
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
        TinyAdapter,
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
    let router = build_router(TinyAdapter, config, ServeOptions::default())
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
        TinyAdapter,
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
    let router = build_router(TinyAdapter, config, ServeOptions::default())
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
    let router = build_router(TinyAdapter, config, ServeOptions::default())
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
