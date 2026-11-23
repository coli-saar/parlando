use super::*;

/// Mints a real one-use game ticket and returns a URL for the local test server.
pub(super) async fn game_socket_url(
    router: &TestRouter,
    base_url: &str,
    public_session_id: &str,
    participant_session_id: &str,
) -> String {
    let response = reqwest::Client::new()
        .post(format!(
            "{base_url}/api/sessions/{public_session_id}/game-session"
        ))
        .bearer_auth(router.credential(participant_session_id))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let plan = response.json::<Value>().await.unwrap();
    let token = plan["token"].as_str().unwrap();
    let host = base_url.trim_start_matches("http://");
    format!("ws://{host}/ws/game/{public_session_id}?token={token}")
}

// Starts a real local HTTP server so tests exercise the WebSocket upgrade path.
pub(super) async fn spawn_test_server(router: TestRouter) -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        axum::serve(listener, router.service).await.unwrap();
    });
    (format!("http://{addr}"), handle)
}

// Reads WebSocket messages until the requested server-message type appears.
pub(super) async fn read_ws_type<S>(socket: &mut S, message_type: &str) -> Value
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

/// Reads lifecycle snapshots until the requested canonical participant state appears.
pub(super) async fn read_participant_state<S>(socket: &mut S, state: &str) -> Value
where
    S: futures_util::Stream<
            Item = Result<TungsteniteMessage, tokio_tungstenite::tungstenite::Error>,
        > + Unpin,
{
    loop {
        let message = read_ws_type(socket, "participant_state").await;
        if message["participant_state"]["state"] == state {
            return message["participant_state"].clone();
        }
    }
}

// Asserts that no message of the given type arrives within a short interval.
pub(super) async fn assert_no_ws_type<S>(socket: &mut S, message_type: &str)
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
pub(super) async fn send_ws_json<S>(socket: &mut S, value: Value)
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
pub(super) async fn create_joined_room(router: TestRouter) -> (String, String, String) {
    let a = create_direct_participant(router.clone(), "A").await;
    let b = create_direct_participant(router.clone(), "B").await;
    consent_participant(router.clone(), &a).await;
    consent_participant(router.clone(), &b).await;
    let (_, created) = participant_json_request(
        router.clone(),
        &a,
        http::Method::POST,
        "/api/sessions",
        json!({}),
    )
    .await;
    let public_session_id = created["participant_state"]["public_session_id"]
        .as_str()
        .unwrap()
        .to_string();
    let (_, joined) =
        participant_json_request(router, &b, http::Method::POST, "/api/sessions", json!({})).await;
    assert_eq!(
        joined["participant_state"]["public_session_id"],
        public_session_id
    );
    (a, b, public_session_id)
}

// Requests one participant-bound audio plan and verifies that it is enabled.
pub(super) async fn request_audio_plan(
    router: TestRouter,
    public_session_id: &str,
    participant_id: &str,
) -> Value {
    let (status, plan) = participant_json_request(
        router,
        participant_id,
        http::Method::POST,
        &format!("/api/sessions/{public_session_id}/audio-session"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(plan["enabled"], true);
    plan
}

// Reads past control messages until the next binary audio frame arrives.
pub(super) async fn read_audio_binary<S>(socket: &mut S) -> Vec<u8>
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
pub(super) async fn wait_for_audio_control<S>(socket: &mut S)
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
pub(super) async fn assert_no_audio_binary<S>(socket: &mut S)
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
