//! Exercises the deployment host over real HTTP connections and WebSockets.

use super::*;
use parlando::deployment::{self, HostOptions};
use reqwest::header::{CONTENT_TYPE, COOKIE, ORIGIN, SET_COOKIE};
use reqwest::{Client, Method, Response};
use std::io::{Cursor, Write};
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
    async fn start(address: SocketAddr, store: &Path, admin_url: Option<String>) -> Self {
        let origin = format!("http://{address}");
        let client = Client::builder()
            .pool_max_idle_per_host(0)
            .timeout(HOST_OPERATION_TIMEOUT)
            .build()
            .unwrap();
        let options = HostOptions {
            store: store.into(),
            public_url: origin.clone(),
            grants: BTreeMap::from([
                ("fixture-a".to_owned(), UPLOAD_A.to_owned()),
                ("fixture-b".to_owned(), UPLOAD_B.to_owned()),
            ]),
            admin_token: Some(ADMIN.into()),
        };
        let task = match admin_url {
            Some(admin_url) => tokio::spawn(deployment::serve_with_dashboard(
                address, options, admin_url,
            )),
            None => tokio::spawn(deployment::serve(address, options)),
        };
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

    /// Uploads an archive with a game-scoped credential and a caller-selected package path.
    async fn upload(&self, path: &str, credential: &str, bytes: Vec<u8>) -> Response {
        self.client
            .put(format!("{}{path}", self.origin))
            .bearer_auth(credential)
            .header(CONTENT_TYPE, "application/zip")
            .body(bytes)
            .send()
            .await
            .unwrap()
    }

    /// Registers a participant with open intake and attaches them to a room.
    async fn seat(&self, id: &str) -> (String, String) {
        let registration = check_json(
            self.request(
                Method::POST,
                &format!("/e/fixture-a~{id}/api/participants"),
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
                &format!("/e/fixture-a~{id}/api/consent"),
                Some(token),
                Some(json!({"decisions": {}})),
            )
            .await;
        assert_eq!(consent.status(), StatusCode::OK);
        let room = check_json(
            self.request(
                Method::POST,
                &format!("/e/fixture-a~{id}/api/sessions"),
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
                &format!("/e/fixture-a~{id}/api/sessions/{room}/game-session"),
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
fn create_package(path: &Path, game: &str, asset: &str) -> PathBuf {
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
    let archive = path.with_extension("zip");
    deployment::write_package_archive(path, &archive).unwrap();
    archive
}

/// Encodes crafted ZIP paths and inputs without the production writer's package checks.
fn crafted_zip(members: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (path, data) in members {
        writer
            .start_file(
                *path,
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Stored),
            )
            .unwrap();
        writer.write_all(data).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

/// Signs in on the admin origin and keeps the cookie separate from the session response.
async fn browser_login(
    client: &Client,
    admin_origin: &str,
    username: &str,
    password: &str,
) -> (String, Value) {
    let response = client
        .post(format!("{admin_origin}{PATH}/auth/login"))
        .header(ORIGIN, admin_origin)
        .json(&json!({"username": username, "password": password}))
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success(), "{}", response.status());
    let set_cookie = response.headers()[SET_COOKIE].to_str().unwrap();
    assert!(set_cookie.contains("HttpOnly"));
    assert!(set_cookie.contains("SameSite=Strict"));
    assert!(!set_cookie.to_ascii_lowercase().contains("domain="));
    let cookie = set_cookie.split(';').next().unwrap().to_owned();
    assert!(!cookie.is_empty());
    (cookie, check_json(response).await)
}

/// Exercises browser permissions across games and durable account changes on the admin origin.
#[tokio::test]
#[ignore = "requires the component fixture built by make test-components"]
async fn deployment_browser_accounts_manage_games_and_revoke_sessions() {
    let directory = tempfile::tempdir().unwrap();
    let store = directory.path().join("host");
    let address = reserve_address().await.unwrap();
    let admin_origin = format!("http://localhost:{}", address.port());
    let host = TestHost::start(address, &store, Some(admin_origin.clone())).await;
    let client = &host.client;
    let request = |method: Method, suffix: &str| {
        client.request(method, format!("{admin_origin}{PATH}{suffix}"))
    };
    for (origin, status) in [
        (admin_origin.as_str(), StatusCode::OK),
        (host.origin.as_str(), StatusCode::NOT_FOUND),
    ] {
        assert_eq!(
            client
                .get(format!("{origin}/admin"))
                .send()
                .await
                .unwrap()
                .status(),
            status
        );
    }
    let accounts = "/accounts";
    let password = "initial-experimenter-password";
    let replacement_password = "replacement-experimenter-password";
    let login_body = json!({"username":"researcher","password":password});
    let account = check_json(
        request(Method::POST, accounts)
            .bearer_auth(ADMIN)
            .json(&login_body)
            .send()
            .await
            .unwrap(),
    )
    .await;
    let account_id = account["id"].as_i64().unwrap();
    let account_path = format!("{accounts}/{account_id}");
    assert_eq!(account["role"], "experimenter");

    let (cookie, session) = browser_login(client, &admin_origin, "researcher", password).await;
    let csrf = session["csrf_token"].as_str().unwrap();
    assert_eq!(session["role"], "experimenter");
    assert_eq!(session["id"], account["id"]);
    assert_eq!(session["username"], "researcher");
    assert_eq!(
        check_json(
            request(Method::GET, "/auth/session")
                .header(COOKIE, &cookie)
                .send()
                .await
                .unwrap(),
        )
        .await,
        session
    );
    assert_eq!(
        client
            .get(format!("{}{PATH}/auth/session", host.origin))
            .header(COOKIE, &cookie)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        request(Method::POST, "/auth/login")
            .header(ORIGIN, &host.origin)
            .json(&login_body)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    for (method, path) in [(Method::POST, accounts), (Method::GET, "/audit")] {
        assert_eq!(
            request(method, path)
                .header(COOKIE, &cookie)
                .header(ORIGIN, &admin_origin)
                .header("x-csrf-token", csrf)
                .json(&json!({
                    "username": "unexpected-operator",
                    "password": password,
                    "role": "operator",
                }))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
    }

    let cli_package = create_package(&directory.path().join("fixture-a"), "fixture-a", "CLI game");
    deployment::deploy(&host.origin, &cli_package, UPLOAD_A)
        .await
        .unwrap();
    let browser_package = create_package(
        &directory.path().join("browser-fixture"),
        "browser-fixture",
        "browser game",
    );
    let upload = std::fs::read(browser_package).unwrap();
    let upload_path = "/packages";
    assert_eq!(
        request(Method::PUT, upload_path)
            .bearer_auth(UPLOAD_A)
            .header(CONTENT_TYPE, "application/zip")
            .body(upload.clone())
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED,
    );
    for (origin, csrf_token) in [
        (None, Some(csrf)),
        (Some(host.origin.as_str()), Some(csrf)),
        (Some(admin_origin.as_str()), None),
    ] {
        let mut rejected = request(Method::PUT, upload_path).header(COOKIE, &cookie);
        if let Some(origin) = origin {
            rejected = rejected.header(ORIGIN, origin);
        }
        if let Some(csrf_token) = csrf_token {
            rejected = rejected.header("x-csrf-token", csrf_token);
        }
        let response = rejected
            .header(CONTENT_TYPE, "application/zip")
            .body(upload.clone())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
    let browser_deployment = check_json(
        request(Method::PUT, upload_path)
            .header(COOKIE, &cookie)
            .header(ORIGIN, &admin_origin)
            .header("x-csrf-token", csrf)
            .header(CONTENT_TYPE, "application/zip")
            .body(upload)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(browser_deployment["game"], "browser-fixture");
    let dashboard = check_json(
        request(Method::GET, "/dashboard")
            .header(COOKIE, &cookie)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(dashboard["games"].as_array().unwrap().len(), 2);
    let operator_password = "browser-operator-credential";
    let operator = check_json(
        request(Method::POST, accounts)
            .bearer_auth(ADMIN)
            .json(&json!({
                "username": "operator",
                "password": operator_password,
                "role": "operator"
            }))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(operator["role"], "operator");
    let (operator_cookie, operator_session) =
        browser_login(client, &admin_origin, "operator", operator_password).await;
    let operator_csrf = operator_session["csrf_token"].as_str().unwrap();
    let colleague_password = "independent-experimenter-password";
    let colleague = check_json(
        request(Method::POST, accounts)
            .header(COOKIE, &operator_cookie)
            .header(ORIGIN, &admin_origin)
            .header("x-csrf-token", operator_csrf)
            .json(&json!({"username":"colleague","password":colleague_password}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(colleague["role"], "experimenter");
    let (colleague_cookie, _) =
        browser_login(client, &admin_origin, "colleague", colleague_password).await;
    let session_request =
        |cookie: &str| request(Method::GET, "/auth/session").header(COOKIE, cookie);
    let (logout_cookie, logout_session) =
        browser_login(client, &admin_origin, "researcher", password).await;
    assert!(request(Method::POST, "/auth/logout")
        .header(COOKIE, &logout_cookie)
        .header(ORIGIN, &admin_origin)
        .header(
            "x-csrf-token",
            logout_session["csrf_token"].as_str().unwrap()
        )
        .send()
        .await
        .unwrap()
        .status()
        .is_success());
    assert_eq!(
        session_request(&logout_cookie)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let (second_cookie, _) = browser_login(client, &admin_origin, "researcher", password).await;
    let update = |body: Value| {
        request(Method::PUT, &account_path)
            .bearer_auth(ADMIN)
            .json(&body)
    };
    assert!(update(json!({"password":replacement_password}))
        .send()
        .await
        .unwrap()
        .status()
        .is_success());
    for revoked in [&cookie, &second_cookie] {
        assert_eq!(
            session_request(revoked).send().await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        request(Method::POST, "/auth/login")
            .header(ORIGIN, &admin_origin)
            .json(&login_body)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let (replacement_cookie, _) =
        browser_login(client, &admin_origin, "researcher", replacement_password).await;
    assert!(request(Method::PUT, &account_path)
        .header(COOKIE, &operator_cookie)
        .header(ORIGIN, &admin_origin)
        .header("x-csrf-token", operator_csrf)
        .json(&json!({"disabled": true}))
        .send()
        .await
        .unwrap()
        .status()
        .is_success());
    assert_eq!(
        session_request(&replacement_cookie)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(Method::POST, "/auth/login")
            .header(ORIGIN, &admin_origin)
            .json(&json!({"username":"researcher","password":replacement_password}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let colleague_dashboard = check_json(
        request(Method::GET, "/dashboard")
            .header(COOKIE, &colleague_cookie)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(colleague_dashboard["games"], dashboard["games"]);
    let audit = check_json(
        request(Method::GET, "/audit")
            .header(COOKIE, &operator_cookie)
            .send()
            .await
            .unwrap(),
    )
    .await;
    let events = audit["events"].as_array().unwrap();
    let researcher = format!("account:{account_id}:researcher");
    let operator_id = operator["id"].as_i64().unwrap();
    let operator_actor = format!("account:{operator_id}:operator");
    let reset_target = account_id.to_string();
    for (actor, operation, target) in [
        (researcher.as_str(), "upload", "browser-fixture"),
        ("upload-token:fixture-a", "upload", "fixture-a"),
        ("host-operator-token", "reset-password", &reset_target),
        (operator_actor.as_str(), "create-account", "colleague"),
        (operator_actor.as_str(), "disable-account", &reset_target),
    ] {
        assert!(
            events.iter().any(|event| event["actor"] == actor
                && event["operation"] == operation
                && event["target"] == target
                && event["outcome"] == "success"),
            "audit has no successful {actor} entry for {operation} on {target}: {audit}"
        );
    }
    let audit = audit.to_string();
    for credential in [
        password,
        replacement_password,
        operator_password,
        colleague_password,
        ADMIN,
        csrf,
    ] {
        assert!(!audit.contains(credential));
    }
    host.stop().await;

    let restarted = TestHost::start(address, &store, Some(admin_origin.clone())).await;
    let (cookie, _) = browser_login(
        &restarted.client,
        &admin_origin,
        "operator",
        operator_password,
    )
    .await;
    let after = check_json(
        restarted
            .client
            .get(format!("{admin_origin}{PATH}/dashboard"))
            .header(COOKIE, cookie)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(after["games"], dashboard["games"]);
    restarted.stop().await;
}

/// Checks concurrent publication, strict scope and validation, and durable per-experiment routing.
#[tokio::test]
#[ignore = "requires the component fixture built by make test-components"]
async fn deployment_replaces_idle_games_and_protects_live_runs() {
    let directory = tempfile::tempdir().unwrap();
    let store = directory.path().join("host");
    let source_a = directory.path().join("fixture-a");
    let package_a = create_package(&source_a, "fixture-a", "first revision");
    let package_b = create_package(
        &directory.path().join("fixture-b"),
        "fixture-b",
        "second game",
    );
    let address = reserve_address().await.unwrap();
    let admin_origin = format!("http://localhost:{}", address.port());
    let host = TestHost::start(address, &store, Some(admin_origin.clone())).await;
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
    assert_eq!(
        host.request(Method::PUT, &first_scope, Some(UPLOAD_A), Some(json!({})))
            .await
            .status(),
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    );
    let mut duplicate = crafted_zip(&[("game.yaml", b"One"), ("fake.yaml", b"two")]);
    let mut offset = 0;
    while let Some(index) = duplicate[offset..]
        .windows(9)
        .position(|name| name == b"fake.yaml")
    {
        offset += index;
        duplicate[offset..offset + 9].copy_from_slice(b"game.yaml");
        offset += 9;
    }
    for invalid in [
        b"This is not a zip".to_vec(),
        crafted_zip(&[("../escape", b"contents")]),
        crafted_zip(&[("game.yaml", b"!!!")]),
        crafted_zip(&[("Client/a", b"One"), ("client/b", b"two")]),
        crafted_zip(&[("client", b"One"), ("client/b", b"two")]),
        crafted_zip(&[
            (
                "game.yaml",
                b"id: fixture-a\nname: Fixture\nversion: 1.0.0\ncomponent: invalid.wasm\n",
            ),
            ("invalid.wasm", b"invalid"),
        ]),
        duplicate,
        std::fs::read(&package_b).unwrap(),
    ] {
        let response = host.upload(&first_scope, UPLOAD_A, invalid).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
    assert!(!directory.path().join("escape").exists());

    for relative in [".env", "client/assets/.env", "unrelated.txt"] {
        let unwanted = source_a.join(relative);
        std::fs::write(&unwanted, "deployment-secret-sentinel").unwrap();
        let rejected_archive = directory.path().join("rejected.zip");
        let error = deployment::write_package_archive(&source_a, &rejected_archive).unwrap_err();
        assert!(
            error.to_string().contains("package path")
                || error.to_string().contains("unexpected file")
        );
        assert!(!rejected_archive.exists());
        std::fs::remove_file(unwanted).unwrap();
    }

    let (first, retry) = tokio::join!(
        deployment::deploy(&host.origin, &package_a, UPLOAD_A),
        deployment::deploy(&host.origin, &package_a, UPLOAD_A),
    );
    let (first, retry) = (first.unwrap(), retry.unwrap());
    assert_eq!(first.revision, retry.revision);
    assert_eq!(first.component_sha256, retry.component_sha256);
    let reordered_members = [
        "client/favicon.ico",
        "client/assets/app.js",
        "game.wasm",
        "client/index.html",
        "game.yaml",
    ]
    .map(|path| (path, std::fs::read(source_a.join(path)).unwrap()));
    let members = reordered_members
        .iter()
        .map(|(path, bytes)| (*path, bytes.as_slice()))
        .collect::<Vec<_>>();
    let reordered = check_json(
        host.upload(&first_scope, UPLOAD_A, crafted_zip(&members))
            .await,
    )
    .await;
    assert_eq!(reordered["revision"], first.revision);
    let create_user = |name: &str| {
        host.client
            .post(format!("{admin_origin}{PATH}/accounts"))
            .bearer_auth(ADMIN)
            .json(&json!({"username":name,"password":"two-user-race-password"}))
    };
    for username in ["researcher", "deployer"] {
        check_json(create_user(username).send().await.unwrap()).await;
    }
    let (researcher, identity) = browser_login(
        &host.client,
        &admin_origin,
        "researcher",
        "two-user-race-password",
    )
    .await;
    let (deployer, other) = browser_login(
        &host.client,
        &admin_origin,
        "deployer",
        "two-user-race-password",
    )
    .await;
    let manage = |path: &str| {
        host.client
            .post(format!("{admin_origin}/fixture-a/api/admin/{path}"))
            .header(COOKIE, &researcher)
            .header(ORIGIN, &admin_origin)
            .header("x-csrf-token", identity["csrf_token"].as_str().unwrap())
    };
    let activate = format!("{PATH}/games/fixture-a/activate");
    check_json(
        host.request(Method::POST, &activate, Some(ADMIN), None)
            .await,
    )
    .await;
    check_json(
        manage("experiments")
            .json(&json!({"experiment_id":"Beacon-Trial"}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    let lifecycle = "runtime/Beacon-Trial/experiment/status";
    let replace_package = create_package(
        &directory.path().join("replacement"),
        "fixture-a",
        "changed revision",
    );
    let replace_bytes = std::fs::read(&replace_package).unwrap();
    let replace = || {
        host.client
            .put(format!("{admin_origin}{first_scope}"))
            .header(COOKIE, &deployer)
            .header(ORIGIN, &admin_origin)
            .header("x-csrf-token", other["csrf_token"].as_str().unwrap())
            .header(CONTENT_TYPE, "application/zip")
            .body(replace_bytes.clone())
            .send()
    };
    let (starting, replacing) = tokio::join!(
        manage(lifecycle).json(&json!({"status":"testing"})).send(),
        replace()
    );
    check_json(starting.unwrap()).await;
    let replacing = replacing.unwrap();
    let installed = if replacing.status().is_success() {
        "changed revision"
    } else {
        assert_eq!(
            replacing.status(),
            StatusCode::CONFLICT,
            "{}",
            replacing.text().await.unwrap()
        );
        "first revision"
    };
    let different_bytes = if installed == "changed revision" {
        std::fs::read(&package_a).unwrap()
    } else {
        replace_bytes.clone()
    };
    assert_eq!(
        host.upload(&first_scope, UPLOAD_A, different_bytes.clone())
            .await
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        host.request(
            Method::GET,
            "/e/fixture-a~Beacon-Trial/assets/app.js",
            None,
            None
        )
        .await
        .text()
        .await
        .unwrap(),
        installed
    );
    let (credential, room) = host.seat("Beacon-Trial").await;
    let ticket = host.ticket("Beacon-Trial", &credential, &room).await;
    let ws_path = ticket["websocket_url"].as_str().unwrap();
    assert!(
        ws_path.starts_with("/e/fixture-a~Beacon-Trial/ws/game/"),
        "{ws_path}"
    );
    let mut socket = connect_async(host.websocket(ws_path, ticket["token"].as_str().unwrap()))
        .await
        .unwrap()
        .0;
    assert!(timeout(Duration::from_secs(5), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
        .is_text());
    check_json(
        manage(lifecycle)
            .json(&json!({"status":"inactive"}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    // Closing intake leaves existing sessions on their original engine.
    assert_eq!(
        host.upload(&first_scope, UPLOAD_A, different_bytes.clone())
            .await
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        host.request(
            Method::POST,
            &format!("{PATH}/games/fixture-a/deactivate"),
            Some(ADMIN),
            None
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    let leave = host
        .request(
            Method::POST,
            &format!("/e/fixture-a~Beacon-Trial/api/sessions/{room}/leave"),
            Some(&credential),
            Some(json!({})),
        )
        .await;
    assert!(
        leave.status().is_success(),
        "{}",
        leave.text().await.unwrap()
    );
    socket.close(None).await.unwrap();
    drop(socket);
    // Socket closure is acknowledged asynchronously by the host.
    let replaced = timeout(Duration::from_secs(15), async {
        loop {
            let response = host
                .upload(&first_scope, UPLOAD_A, different_bytes.clone())
                .await;
            if response.status() != StatusCode::CONFLICT {
                break check_json(response).await;
            }
            time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the idle game should accept replacement after disconnection");
    assert_eq!(replaced["game"], "fixture-a");
    // Publishing another game is independent and never gives it access to this one's sessions.
    deployment::deploy(&host.origin, &package_b, UPLOAD_B)
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_dir(store.join("packages/fixture-a"))
            .unwrap()
            .count(),
        2
    );
    host.stop().await;
    let restarted = TestHost::start(address, &store, Some(admin_origin)).await;
    let cfg = check_json(
        restarted
            .request(
                Method::GET,
                "/e/fixture-a~Beacon-Trial/api/config",
                None,
                None,
            )
            .await,
    )
    .await;
    assert_eq!(cfg["experiment_status"], "inactive");
    assert_eq!(cfg["game_name"], "fixture-a");
    check_json(
        restarted
            .request(
                Method::POST,
                &format!("{PATH}/games/fixture-a/deactivate"),
                Some(ADMIN),
                None,
            )
            .await,
    )
    .await;
    assert!(!restarted
        .request(
            Method::GET,
            "/e/fixture-a~Beacon-Trial/api/config",
            None,
            None
        )
        .await
        .status()
        .is_success());
    restarted.stop().await;
}
