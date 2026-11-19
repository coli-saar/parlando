use super::*;

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

/// Confirms `/admin` reaches setup and the resulting login persists across router restarts.
#[tokio::test]
async fn admin_setup_page_creates_one_persistent_credential() {
    let (config, _tmp) = sqlite_config();
    let router = crate::app::build_router(TinyAdapter, config.clone(), ServeOptions::default())
        .await
        .map(TestRouter::new)
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

    let restarted = crate::app::build_router(TinyAdapter, config, ServeOptions::default())
        .await
        .map(TestRouter::new)
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
