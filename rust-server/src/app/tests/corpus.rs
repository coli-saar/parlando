use super::*;

/// Confirms test sessions and their dependent rows are absent from every export variant input.
#[test]
fn export_filter_removes_testing_session_graph() {
    let exported = json!({
        "participants": [
            {"participant_id": 1, "research_id": "test-only"},
            {"participant_id": 2, "research_id": "research"}
        ],
        "sessions": [
            {"experiment_id":"e","mode":"direct","lifecycle":"forming","config_revision":1,"game_version":"1.0.0","created_at":"2026-01-01T00:00:00Z","public_session_id":"r10","dialogue_id":"d10","session_id":10,"purpose":"testing"},
            {"experiment_id":"e","mode":"direct","lifecycle":"forming","config_revision":1,"game_version":"1.0.0","created_at":"2026-01-01T00:00:00Z","public_session_id":"r20","dialogue_id":"d20","session_id":20,"purpose":"research"}
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
            {"session_id": 10, "experiment_id": "e", "event_id": 1,"event_type":"log","event_index":1,"payload":{},"game_time_ms":0},
            {"session_id": 20, "experiment_id": "e", "event_id": 1,"event_type":"log","event_index":1,"payload":{},"game_time_ms":0}
        ]
    });

    let mut snapshot = serde_json::from_value(exported).unwrap();
    retain_export_sessions(&mut snapshot);
    let exported = serde_json::to_value(snapshot).unwrap();

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

/// Corrupt timing cannot silently enter a corpus with a false timestamp.
#[test]
fn corpus_tokens_reject_invalid_intervals() {
    assert!(export_transcript_tokens(
        &json!([{"kind":"word","text":"a","start_ms":20,"end_ms":10}])
    )
    .is_err());
    assert!(export_transcript_tokens(
        &json!([{"kind":"word","text":"a","start_ms":0,"end_ms":10,"confidence":2}])
    )
    .is_err());
}

/// Populates a file-backed experiment with one eligible session plus excluded research rows.
async fn seed_corpus_database(database_url: &str, event_count: i64) -> sqlx::SqlitePool {
    let writer = sqlx::SqlitePool::connect(database_url).await.unwrap();
    let mut tx = writer.begin().await.unwrap();
    sqlx::query(
        "insert into participants
          (participant_id,research_id,experiment_id,participant_kind,identity_provider,external_id,created_at)
         values (1001,'export-a','step5','human','direct','PRIVATE_RECRUITMENT_ID','now'),
                (1002,'export-b','step5','human','direct',null,'now'),
                (1003,'waiting-only','step5','human','direct',null,'now')",
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "insert into sessions (experiment_id,session_id,public_session_id,dialogue_id,
         mode,lifecycle,purpose,created_at,waiting_started_at,waiting_deadline_at,
         lifetime_deadline_at,started_at,game_version)
         select 'step5', session_id, 'secret-room-' || session_id, 'export-' || session_id,
                'direct','running',purpose,ts,ts,ts,ts,started_at,'0.6.0'
         from (select 100 as session_id,'research' as purpose,'2026-01-01T00:00:00Z' as started_at
               union all select 101,'testing','2026-01-01T00:00:00Z'
               union all select 102,'research',null)
         cross join (select '2026-01-01T00:00:00Z' as ts)",
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "insert into session_participants (experiment_id,session_id,participant_id,
            participant_session_id,role,joined_at,connection_status)
         values ('step5',100,1001,'private-a','A','now','connected'),
                ('step5',100,1002,'private-b','B','now','connected'),
                ('step5',101,1003,'private-c','A','now','connected'),
                ('step5',102,1003,'private-d','A','now','connected')",
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "insert into session_events(experiment_id,session_id,event_index,event_type,payload_json,game_time_ms)
         values ('step5',100,0,'log','{\"text\":\"PRIVATE_SETUP_LOG\"}',-1),
                ('step5',101,0,'log','{\"text\":\"PRIVATE_TEST_LOG\"}',0),
                ('step5',102,0,'log','{\"text\":\"PRIVATE_WAITING_LOG\"}',0)",
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "with recursive indices(i) as (
             select 1 where ?1 > 0 union all select i + 1 from indices where i < ?1
         )
         insert into session_events (experiment_id,session_id,event_index,event_type,
            actor_participant_id,actor_role,payload_json,game_state_json,game_time_ms)
         select 'step5',100,i,'game_action_accepted',1001,'A',?2,?3,i from indices",
    )
    .bind(event_count)
    .bind(
        json!({
            "action":{"password":"PRIVATE_GAME_PASSWORD","text":"a".repeat(260)},
            "metadata":{"client_secret":"PRIVATE_GAME_SECRET"}, "recovery_checkpoint":true
        })
        .to_string(),
    )
    .bind(json!({"state":"PRIVATE_RECOVERY_FRAME"}).to_string())
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    writer
}

/// Verifies all public formats agree on registry, event order, speech timing, and redaction.
#[tokio::test]
async fn corpus_download_formats_preserve_projection() {
    let (config, _directory) = sqlite_config();
    let database_url = config.database.url.clone();
    let router = build_router(live_game(), config, ServeOptions::default())
        .await
        .unwrap();
    let (status, empty) = admin_json_request(
        router.clone(),
        http::Method::GET,
        "/api/admin/export",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(empty["experiment"]["participants"], json!([]));
    assert_eq!(empty["experiment"]["sessions"], json!([]));

    let writer = seed_corpus_database(&database_url, 1).await;
    sqlx::query("update experiments set config_json = json_set(config_json, '$.game', json(?)) where experiment_id = 'step5'")
        .bind(json!({"condition":"example","api_key":"PRIVATE_CONFIG_KEY"}).to_string())
        .execute(&writer).await.unwrap();
    sqlx::query(
        "update sessions set lifecycle = 'ended', started_at = '2026-01-01T00:00:01Z',
            ended_at = '2026-01-01T00:00:05Z', completion_json = ?, session_end_json = ?
         where experiment_id = 'step5' and session_id = 100",
    )
    .bind(json!({"outcome":"success","token":"PRIVATE_COMPLETION_TOKEN"}).to_string())
    .bind(
        json!({"cause":{"type":"game_completed"},"participant_results":{},"completion":null})
            .to_string(),
    )
    .execute(&writer)
    .await
    .unwrap();
    sqlx::query("update participants set participant_kind = 'agent', metadata_json = ? where participant_id = 1002")
        .bind(json!({"factory":"fixture","agent_name":"Example","agent_version":"2","PRIVATE_NOT_EXPORTED":1}).to_string())
        .execute(&writer).await.unwrap();

    for (index, event_type, payload) in [
        (
            2,
            "conversation_message",
            json!({"id":"m1","text":"First,\"line\"\nsecond\n","origin":"typed"}),
        ),
        (
            3,
            "conversation_speech_timing",
            json!({"message_id":"m1","metadata":{
                "start_game_time_ms":1,"end_game_time_ms":2
            }}),
        ),
        (
            4,
            "conversation_speech_timing",
            json!({"message_id":"m1","metadata":{
                "start_game_time_ms":2,"end_game_time_ms":5,
                "tokens":[{"kind":"word","text":"First","start_ms":2,"end_ms":3,"provider_field":"PRIVATE_PROVIDER"}]
            }}),
        ),
        (
            5,
            "log",
            json!({"source":"agent","text":"a useful diagnostic"}),
        ),
    ] {
        sqlx::query(
            "insert into session_events (experiment_id,session_id,event_index,event_type,
                 actor_participant_id,actor_role,payload_json,game_time_ms)
             values ('step5',100,?,?,1002,'B',?,5)",
        )
        .bind(index)
        .bind(event_type)
        .bind(payload.to_string())
        .execute(&writer)
        .await
        .unwrap();
    }

    let mut downloads = Vec::new();
    for (format, media_type) in [
        ("json", "application/json"),
        ("yml", "application/yaml"),
        ("csv", "text/csv"),
    ] {
        let response = admin_raw_request(
            router.clone(),
            http::Method::GET,
            &format!("/api/admin/export?format={format}"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()["content-type"],
            format!("{media_type}; charset=utf-8")
        );
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(response.headers()["x-content-type-options"], "nosniff");
        let extension = if format == "yml" { "yaml" } else { format };
        assert_eq!(
            response.headers()["content-disposition"],
            format!("attachment; filename=\"step5-corpus-v1.{extension}\"")
        );
        let bytes: usize = response.headers()["content-length"]
            .to_str()
            .unwrap()
            .parse()
            .unwrap();
        let body = to_bytes(response.into_body(), 8192).await.unwrap();
        assert_eq!(bytes, body.len());
        let text = std::str::from_utf8(&body).unwrap().to_owned();
        assert!(!text.contains("PRIVATE_"));
        assert!(!text.contains("secret-room"));
        assert!(!text.contains("waiting-only"));
        assert!(!text.contains("2026-01-01"));
        downloads.push(text);
    }
    let corpus: Value = serde_json::from_str(&downloads[0]).unwrap();
    let schema: Value = serde_json::from_str(CORPUS_EXPORT_SCHEMA_V1).unwrap();
    assert_eq!(
        schema["$id"],
        "https://parlando.dev/schemas/parlando.corpus.v1.json"
    );
    assert!(jsonschema::validator_for(&schema)
        .unwrap()
        .is_valid(&corpus));
    assert_eq!(corpus["release_status"], "corpus_candidate");
    assert_eq!(
        serde_yaml::from_str::<Value>(&downloads[1]).unwrap(),
        corpus
    );
    let csv = downloads[2]
        .lines()
        .skip(1)
        .map(|line| {
            let (table, encoded) = line.split_once(',').unwrap();
            let json = encoded
                .strip_prefix('"')
                .unwrap()
                .strip_suffix('"')
                .unwrap()
                .replace("\"\"", "\"");
            (table, serde_json::from_str::<Value>(&json).unwrap())
        })
        .collect::<Vec<_>>();
    assert_eq!(
        csv.iter().map(|(name, _)| *name).collect::<Vec<_>>(),
        ["manifest", "experiment", "session"]
    );
    let mut reconstructed = csv[0].1.clone();
    reconstructed["experiment"] = csv[1].1.clone();
    reconstructed["experiment"]["sessions"] = json!([csv[2].1]);
    assert_eq!(reconstructed, corpus);
    assert_eq!(
        corpus["data_inventory"],
        json!({"participants":2,"sessions":1,"events":3,"actions":1,"messages":1,"logs":1})
    );
    let experiment = &corpus["experiment"];
    assert_eq!(
        experiment["configuration"],
        json!({"condition":"example","api_key":""})
    );
    assert_eq!(
        experiment["participants"][1]["agent_identity"]["agent_name"],
        "Example"
    );
    let session = &experiment["sessions"][0];
    assert_eq!(session["session_id"], "export-100");
    assert_eq!(
        session["completion"],
        json!({"outcome":"success","token":""})
    );
    assert_eq!(session["metadata"]["status"], "completed");
    assert_eq!(session["metadata"]["time_to_start_ms"], 1_000);
    assert_eq!(session["metadata"]["duration_ms"], 4_000);
    let events = &session["events"];
    assert_eq!(
        events[2],
        json!({"index":5,"game_time_ms":5,"kind":"log","source":"agent",
        "participant_id":"export-b","role":"B","text":"a useful diagnostic"})
    );
    assert_eq!(events[0]["index"], 1);
    assert_eq!(events[0]["action"]["password"], "");
    assert!(events[0]["state"].is_null());
    assert_eq!(events[1]["index"], 2);
    assert_eq!(events[1]["text"], "First,\"line\"\nsecond\n");
    assert_eq!(events[1]["utterance_timing"]["start_ms"], 2);
    assert_eq!(
        events[1]["tokens"],
        json!([{"kind":"word","text":"First","start_ms":2,"end_ms":3}])
    );

    // A failure after generation has started still returns a conflict without a partial attachment.
    sqlx::query(
        "update session_events set game_time_ms = -1 where session_id = 100 and event_index = 2",
    )
    .execute(&writer)
    .await
    .unwrap();
    let response = admin_raw_request(router.clone(), http::Method::GET, "/api/admin/export").await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert!(!response.headers().contains_key("content-disposition"));
    let body = to_bytes(response.into_body(), 1024).await.unwrap();
    assert!(std::str::from_utf8(&body)
        .unwrap()
        .contains("without valid game time"));

    sqlx::query(
        "delete from session_participants where session_id = 100 and participant_id = 1002",
    )
    .execute(&writer)
    .await
    .unwrap();
    let response = admin_raw_request(router, http::Method::GET, "/api/admin/export").await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body = to_bytes(response.into_body(), 1024).await.unwrap();
    assert!(std::str::from_utf8(&body)
        .unwrap()
        .contains("expected exactly two"));
}

/// Holds two simultaneous snapshots over a corpus beyond both old limits and writes during them.
#[tokio::test]
async fn corpus_download_large_snapshots_do_not_block_live_writes() {
    use crate::app::corpus_download::{download_corpus, CorpusFormat};
    use crate::storage::SqliteExperimentStore;

    let (config, _directory) = sqlite_config();
    let database_url = config.database.url.clone();
    let _router = build_router(live_game(), config, ServeOptions::default())
        .await
        .unwrap();
    let writer = seed_corpus_database(&database_url, 100_001).await;
    let store = SqliteExperimentStore::connect(&database_url).await.unwrap();
    let mut left = store.corpus_read("step5").await.unwrap();
    left.experiment().await.unwrap().unwrap();
    let mut right = tokio::time::timeout(Duration::from_secs(5), store.corpus_read("step5"))
        .await
        .unwrap()
        .unwrap();
    right.experiment().await.unwrap().unwrap();

    let (left, right, live_write) = tokio::join!(
        download_corpus(left, "step5", "2", CorpusFormat::Json),
        download_corpus(right, "step5", "2", CorpusFormat::Csv),
        async {
            tokio::time::timeout(
                Duration::from_secs(5),
                store.append_session_event(SessionEventRecord {
                    experiment_id: "step5".to_owned(),
                    session_id: 100,
                    actor_participant_id: None,
                    actor_role: None,
                    event_type: "log".to_owned(),
                    payload: json!({"text":"later-write"}),
                    game_state: None,
                }),
            )
            .await
            .unwrap()
        },
    );
    assert_eq!(live_write.unwrap(), 100_002);

    // A slow consumer has not read even one chunk. WAL truncation can already proceed.
    let (busy, _, _): (i64, i64, i64) = sqlx::query_as("pragma wal_checkpoint(truncate)")
        .fetch_one(&writer)
        .await
        .unwrap();
    assert_eq!(busy, 0);
    for (result, count_field) in [
        (left, "\"events\":100001"),
        (right, "\"\"events\"\":100001"),
    ] {
        let (expected_size, body) = result.unwrap();
        let mut stream = body.into_data_stream();
        let first = stream.next().await.unwrap().unwrap();
        assert!(std::str::from_utf8(&first).unwrap().contains(count_field));
        let mut size = first.len();
        let mut last = first;
        while let Some(chunk) = stream.next().await {
            last = chunk.unwrap();
            assert!(last.len() <= 64 * 1024);
            size += last.len();
        }
        assert_eq!(size as u64, expected_size);
        assert!(size > 32 * 1024 * 1024, "{size} bytes");
        assert!(!std::str::from_utf8(&last).unwrap().contains("later-write"));
    }
    let mut fresh = store.corpus_read("step5").await.unwrap();
    assert_eq!(fresh.inventory().await.unwrap().logs, 1);
}
