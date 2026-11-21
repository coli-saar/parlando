use super::*;

/// Confirms the protected privacy routes render one truthful installation-wide status.
#[tokio::test]
async fn admin_privacy_status_renders_and_downloads_current_facts() {
    let (config, _tmp) = sqlite_config();
    let router = build_router(live_game(), config, ServeOptions::default())
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

    let (status, privacy) = admin_json_request(
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

    let (experiment_status, experiment_privacy) = admin_json_request(
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
    let router = build_router(live_game(), config, ServeOptions::default())
        .await
        .unwrap();
    authenticate_test_admin(router.clone()).await.unwrap();

    let (status, privacy) = admin_json_request(
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
