//! Exercises the deployment host over real HTTP connections and WebSockets.

use super::*;
use parlando::deployment::{self, Deployment, ExperimentSelection, HostOptions};
use reqwest::{Client, Method, Response};
use tokio::{task::JoinHandle, time::timeout};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

const UPLOAD_A: &str = "fixture-a-upload-credential-for-deployment-tests";
const UPLOAD_B: &str = "fixture-b-upload-credential-for-deployment-tests";
const ADMIN: &str = "operator-only-credential-for-deployment-tests";
const PATH: &str = "/api/hosting";
// Selection and restart can compile several real components in an unoptimized test build.
// Use the production control client's request budget during parallel CI runs.
const HOST_OPERATION_TIMEOUT: Duration = Duration::from_secs(180);

struct TestHost {
    client: Client,
    address: SocketAddr,
    origin: String,
    task: JoinHandle<Result<()>>,
}

impl TestHost {
    /// Starts a deployment host using the same public entry point as the CLI.
    async fn start(address: SocketAddr, store: &Path) -> Self {
        let origin = format!("http://{address}");
        let client = Client::builder()
            .pool_max_idle_per_host(0)
            .timeout(HOST_OPERATION_TIMEOUT)
            .build()
            .unwrap();
        let task = tokio::spawn(deployment::serve(
            address,
            HostOptions {
                store: store.into(),
                public_url: origin.clone(),
                grants: BTreeMap::from([
                    ("fixture-a".to_owned(), UPLOAD_A.to_owned()),
                    ("fixture-b".to_owned(), UPLOAD_B.to_owned()),
                ]),
                admin_token: ADMIN.into(),
            },
        ));
        timeout(HOST_OPERATION_TIMEOUT, async {
            loop {
                assert!(!task.is_finished(), "deployment host failed to start");
                if client.get(format!("{origin}/health")).send().await.is_ok() {
                    break;
                }
                time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("deployment host health timed out");
        Self {
            client,
            address,
            origin,
            task,
        }
    }

    /// Sends an optionally authenticated JSON request to the running host.
    async fn request(
        &self,
        method: Method,
        path: &str,
        credential: Option<&str>,
        body: Option<Value>,
    ) -> Response {
        let mut request = self
            .client
            .request(method, format!("{}{path}", self.origin));
        if let Some(credential) = credential {
            request = request.bearer_auth(credential);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        request.send().await.unwrap()
    }

    /// Pins the requested revision and materializes the response for assertion.
    async fn select(&self, id: &str, package: &Deployment, config: Value) -> Response {
        let selection = ExperimentSelection {
            game: package.game.clone(),
            revision: package.revision.clone(),
            config,
        };
        self.request(
            Method::PUT,
            &format!("{PATH}/experiments/{id}"),
            Some(ADMIN),
            Some(serde_json::to_value(selection).unwrap()),
        )
        .await
    }

    /// Opens test intake and creates a participant attached to one hosted room.
    async fn seat(&self, id: &str) -> (String, String) {
        let status = self
            .request(
                Method::POST,
                &format!("{PATH}/experiments/{id}/admin/experiment/status"),
                Some(ADMIN),
                Some(json!({"status": "testing"})),
            )
            .await;
        assert_eq!(
            status.status(),
            StatusCode::OK,
            "{}",
            status.text().await.unwrap()
        );
        let registration = check_json(
            self.request(
                Method::POST,
                &format!("/e/{id}/api/participants"),
                None,
                Some(json!({})),
            )
            .await,
        )
        .await;
        let token = registration["participant_credential"].as_str().unwrap();
        let consent = self
            .request(
                Method::POST,
                &format!("/e/{id}/api/consent"),
                Some(token),
                Some(json!({"decisions": {}})),
            )
            .await;
        assert_eq!(consent.status(), StatusCode::OK);
        let room = check_json(
            self.request(
                Method::POST,
                &format!("/e/{id}/api/sessions"),
                Some(token),
                Some(json!({})),
            )
            .await,
        )
        .await;
        (
            token.to_owned(),
            room["participant_state"]["public_session_id"]
                .as_str()
                .unwrap()
                .to_owned(),
        )
    }

    /// Obtains a scoped WebSocket ticket from the participant's owning runtime.
    async fn ticket(&self, id: &str, token: &str, room: &str) -> Value {
        check_json(
            self.request(
                Method::POST,
                &format!("/e/{id}/api/sessions/{room}/game-session"),
                Some(token),
                Some(json!({})),
            )
            .await,
        )
        .await
    }

    /// Returns the tested browser handshake, with one ticket and the configured origin.
    fn websocket(
        &self,
        path: &str,
        token: &str,
    ) -> tokio_tungstenite::tungstenite::http::Request<()> {
        let mut request = format!("ws://{}{path}?token={token}", self.address)
            .into_client_request()
            .unwrap();
        request
            .headers_mut()
            .insert("Origin", self.origin.parse().unwrap());
        request
    }

    /// Stops serving so the test can start a new host on the same durable registry.
    async fn stop(self) {
        self.task.abort();
        assert!(self.task.await.unwrap_err().is_cancelled());
    }
}

/// Checks an expected success response and parses its JSON body.
async fn check_json(response: Response) -> Value {
    let status = response.status();
    let bytes = response.bytes().await.unwrap();
    assert!(
        status.is_success(),
        "{status}: {}",
        String::from_utf8_lossy(&bytes)
    );
    serde_json::from_slice(&bytes).unwrap()
}

/// Writes one ordinary deployable fixture with revision-identifying participant assets.
fn create_package(path: &Path, game: &str, asset: &str) {
    std::fs::create_dir_all(path.join("client/assets")).unwrap();
    let fixture = std::env::var_os("PARLANDO_TEST_COMPONENT").expect("set PARLANDO_TEST_COMPONENT");
    std::fs::copy(fixture, path.join("game.wasm")).unwrap();
    std::fs::write(
        path.join("game.yaml"),
        format!(
            "id: {game}\nname: {game}\nversion: 1.0.0\ncomponent: game.wasm\nclient_dist: client\n"
        ),
    )
    .unwrap();
    std::fs::write(
        path.join("client/index.html"),
        "<script src='./assets/app.js'></script>",
    )
    .unwrap();
    std::fs::write(path.join("client/assets/app.js"), asset).unwrap();
    std::fs::write(path.join("client/favicon.ico"), "fixture icon").unwrap();
}

/// Checks concurrent publication, strict scope and validation, and durable per-experiment routing.
#[tokio::test]
#[ignore = "requires the component fixture built by make test-components"]
async fn deployment_publishes_immutable_packages_and_routes_sessions_to_their_pin() {
    let directory = tempfile::tempdir().unwrap();
    let store = directory.path().join("host");
    let package_a = directory.path().join("fixture-a");
    let package_b = directory.path().join("fixture-b");
    create_package(&package_a, "fixture-a", "first revision");
    create_package(&package_b, "fixture-b", "second game");
    let address = reserve_address().await.unwrap();
    let host = TestHost::start(address, &store).await;
    let uploads = format!("{PATH}/packages");
    let first_scope = format!("{uploads}/fixture-a");
    let other_scope = format!("{uploads}/fixture-b");

    for (scope, token) in [
        (&first_scope, None),
        (&other_scope, Some(UPLOAD_A)),
        (&first_scope, Some(ADMIN)),
    ] {
        let response = host
            .request(Method::PUT, scope, token, Some(json!({})))
            .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    for invalid in [
        json!({"files":{"../escape":"ZA=="}}),
        json!({"files":{"game.yaml":"!!!"}}),
        json!({"files":{"Client/a":"YQ==","client/b":"Yg=="}}),
        json!({"files":{
            "game.yaml": base64::engine::general_purpose::STANDARD.encode("id: fixture-a\nname: Fixture\nversion: 1.0.0\ncomponent: invalid.wasm\n"),
            "invalid.wasm":"aW52YWxpZA=="
        }}),
    ] {
        let response = host
            .request(Method::PUT, &first_scope, Some(UPLOAD_A), Some(invalid))
            .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
    assert!(!directory.path().join("escape").exists());

    for relative in [".env", "client/assets/.env", "unrelated.txt"] {
        let unwanted = package_a.join(relative);
        std::fs::write(&unwanted, "deployment-secret-sentinel").unwrap();
        // An unreachable target distinguishes local rejection from server validation.
        let error = deployment::deploy("http://127.0.0.1:1", &package_a, UPLOAD_A)
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("package path")
                || error.to_string().contains("unexpected file")
        );
        std::fs::remove_file(unwanted).unwrap();
    }

    let (first, retry) = tokio::join!(
        deployment::deploy(&host.origin, &package_a, UPLOAD_A),
        deployment::deploy(&host.origin, &package_a, UPLOAD_A),
    );
    let (first, retry) = (first.unwrap(), retry.unwrap());
    assert_eq!(first.revision, retry.revision);
    assert_eq!(first.component_sha256, retry.component_sha256);
    assert_eq!(
        std::fs::read_dir(store.join("packages/fixture-a"))
            .unwrap()
            .count(),
        1
    );

    for config in [
        json!({"speechmatics":{"api_key":"deployment-secret-sentinel"}}),
        json!({"game":{"access_token":"deployment-secret-sentinel"}}),
        json!({"session":{"session_max_lifetime_seconds":0}}),
    ] {
        assert_eq!(
            host.select("one", &first, config).await.status(),
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        std::fs::read_dir(store.join("selections")).unwrap().count(),
        0
    );
    assert_eq!(
        std::fs::read_dir(store.join("experiments"))
            .unwrap()
            .count(),
        0
    );
    check_json(host.select("one", &first, json!({})).await).await;
    check_json(host.select("one", &first, json!({})).await).await;

    for path in [
        "/e/one/api/admin/setup",
        "/api/hosting/experiments/one/admin/setup",
        "/api/hosting/experiments/one/admin/game/secrets/reveal",
        "/api/hosting/experiments/one/admin/game/settings",
    ] {
        assert_eq!(
            host.request(Method::POST, path, Some(ADMIN), Some(json!({})))
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
    }
    let sessions = format!("{PATH}/experiments/one/admin/sessions");
    assert_eq!(
        host.request(Method::GET, &sessions, Some(UPLOAD_A), None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        host.request(Method::GET, &sessions, Some(ADMIN), None)
            .await
            .status(),
        StatusCode::OK
    );

    let (participant, room) = host.seat("one").await;
    create_package(&package_a, "fixture-a", "changed revision");
    let replacement = deployment::deploy(&host.origin, &package_a, UPLOAD_A)
        .await
        .unwrap();
    assert_ne!(first.revision, replacement.revision);
    assert_eq!(first.component_sha256, replacement.component_sha256);
    assert_eq!(
        host.select("one", &replacement, json!({})).await.status(),
        StatusCode::CONFLICT
    );

    let second = deployment::deploy(&host.origin, &package_b, UPLOAD_B)
        .await
        .unwrap();
    assert_eq!(
        host.select("two", &second, json!({})).await.status(),
        StatusCode::OK
    );
    assert_eq!(
        host.request(
            Method::GET,
            "/e/two/api/participant-state",
            Some(&participant),
            None
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED,
    );
    let ticket = host.ticket("one", &participant, &room).await;
    let path = ticket["websocket_url"].as_str().unwrap();
    let token = ticket["token"].as_str().unwrap();
    assert!(path.starts_with("/e/one/ws/game/"));
    let wrong_path = path.replacen("/e/one/", "/e/two/", 1);
    let error = timeout(
        Duration::from_secs(5),
        connect_async(host.websocket(&wrong_path, token)),
    )
    .await
    .expect("wrong experiment handshake timed out")
    .unwrap_err();
    assert!(matches!(
        error,
        tokio_tungstenite::tungstenite::Error::Http(_)
    ));
    let (mut socket, _) = timeout(
        Duration::from_secs(5),
        connect_async(host.websocket(path, token)),
    )
    .await
    .expect("owning experiment handshake timed out")
    .unwrap();
    let message = timeout(Duration::from_secs(2), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(message.is_text());
    socket.close(None).await.unwrap();
    drop(socket);

    let response = host
        .request(Method::GET, "/e/one/assets/app.js", None, None)
        .await;
    assert_eq!(response.text().await.unwrap(), "first revision");
    let response = host
        .request(Method::GET, "/e/two/favicon.ico", None, None)
        .await;
    assert_eq!(response.text().await.unwrap(), "fixture icon");
    host.stop().await;

    let restarted = TestHost::start(address, &store).await;
    let config = check_json(
        restarted
            .request(Method::GET, "/e/one/api/config", None, None)
            .await,
    )
    .await;
    assert_eq!(config["experiment_status"], "inactive");
    assert_eq!(config["game_name"], "fixture-a");
    assert_eq!(
        restarted.select("one", &first, json!({})).await.status(),
        StatusCode::OK
    );
    let response = restarted
        .request(Method::GET, "/e/one/assets/app.js", None, None)
        .await;
    assert_eq!(response.text().await.unwrap(), "first revision");
    let response = restarted
        .request(Method::GET, "/e/two/assets/app.js", None, None)
        .await;
    assert_eq!(response.text().await.unwrap(), "second game");
    restarted.stop().await;
}
