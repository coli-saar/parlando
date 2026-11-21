use super::*;

/// An isolated server and the credentials issued by that server during a test.
#[derive(Clone)]
pub(super) struct TestRouter {
    pub(super) service: Router,
    pub(super) participants: Arc<Mutex<HashMap<String, String>>>,
    pub(super) admin: Arc<Mutex<Option<AdminTestSession>>>,
}

impl TestRouter {
    /// Starts a fixture without any authenticated clients.
    pub(super) fn new(service: Router) -> Self {
        Self {
            service,
            participants: Default::default(),
            admin: Default::default(),
        }
    }

    /// Sends one unmodified request directly to this server.
    pub(super) async fn oneshot(
        &self,
        request: Request<Body>,
    ) -> Result<Response, std::convert::Infallible> {
        self.service.clone().oneshot(request).await
    }

    /// Looks up a bearer credential issued by this server for the specified participant.
    pub(super) fn credential(&self, id: &str) -> String {
        self.participants
            .lock()
            .unwrap()
            .get(id)
            .expect("participant was registered on this server")
            .clone()
    }

    /// Saves the credential returned by a successful participant registration.
    pub(super) fn remember(&self, id: String, credential: String) {
        self.participants.lock().unwrap().insert(id, credential);
    }
}

/// Authentication material for one real administrator test session.
#[derive(Clone)]
pub(super) struct AdminTestSession {
    pub(super) cookie: String,
    pub(super) csrf_token: String,
}

/// Builds an active test router so unrelated protocol tests retain their narrow setup.
pub(super) async fn build_router(
    adapter: LoadedGame,
    config: ExperimentConfig,
    options: ServeOptions,
) -> Result<TestRouter> {
    let router = crate::app::build_router(adapter, config, options)
        .await
        .map(TestRouter::new)?;
    authenticate_test_admin(router.clone()).await?;
    let (status, _) = admin_json_request(
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
pub(super) async fn authenticate_test_admin(router: TestRouter) -> Result<()> {
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
    *router.admin.lock().unwrap() = Some(AdminTestSession { cookie, csrf_token });
    Ok(())
}

/// Sends one anonymous request with exactly the supplied payload.
pub(super) async fn json_request(
    router: TestRouter,
    method: http::Method,
    path: &str,
    body: Value,
) -> (StatusCode, Value) {
    send_json_request(&router, Request::builder().method(method).uri(path), body).await
}

/// Sends one request as the named participant on this server.
pub(super) async fn participant_json_request(
    router: TestRouter,
    participant: &str,
    method: http::Method,
    path: &str,
    body: Value,
) -> (StatusCode, Value) {
    let request = Request::builder().method(method).uri(path).header(
        AUTHORIZATION,
        format!("Bearer {}", router.credential(participant)),
    );
    send_json_request(&router, request, body).await
}

/// Sends one authenticated JSON request using this server's administrator session.
pub(super) async fn admin_json_request(
    router: TestRouter,
    method: http::Method,
    path: &str,
    body: Value,
) -> (StatusCode, Value) {
    let request = admin_request_builder(&router, method, path);
    send_json_request(&router, request, body).await
}

/// Builds request headers for the authenticated administrator of this test server.
pub(super) fn admin_request_builder(
    router: &TestRouter,
    method: http::Method,
    path: &str,
) -> http::request::Builder {
    let session = router.admin.lock().unwrap();
    let session = session.as_ref().expect("test administrator is signed in");
    Request::builder()
        .method(method)
        .uri(path)
        .header(http::header::COOKIE, &session.cookie)
        .header("x-csrf-token", &session.csrf_token)
}

/// Sends the exact supplied JSON payload and decodes the single response.
pub(super) async fn send_json_request(
    router: &TestRouter,
    request: http::request::Builder,
    body: Value,
) -> (StatusCode, Value) {
    let response = router
        .oneshot(
            request
                .header(http::header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
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

/// Sends one unauthenticated request and returns its raw body and content type.
pub(super) async fn raw_request(
    router: TestRouter,
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
pub(super) async fn admin_raw_request(
    router: TestRouter,
    method: http::Method,
    path: &str,
) -> Response {
    router
        .oneshot(
            admin_request_builder(&router, method, path)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}
