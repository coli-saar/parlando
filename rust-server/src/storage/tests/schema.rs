use super::*;

/// Confirms the schema contains both evaluation data and the isolated credential table.
#[tokio::test]
async fn sqlite_schema_has_evaluation_and_administrator_tables() {
    let store = SqliteExperimentStore::connect("sqlite:///:memory:")
        .await
        .unwrap();
    let tables = store.table_names().await.unwrap();

    assert_eq!(
        tables,
        vec![
            "administrator_credential",
            "administrator_sessions",
            "consent_declarations",
            "experiment_config_revisions",
            "experiment_secrets",
            "experiments",
            "game_secrets",
            "game_settings",
            "participants",
            "prolific_submissions",
            "schema_migrations",
            "session_events",
            "session_participants",
            "sessions",
        ]
    );
    let experiment_columns = sqlx::query_scalar::<_, String>(
        "select name from pragma_table_info('experiments') order by cid",
    )
    .fetch_all(&store.pool)
    .await
    .unwrap();
    assert!(!experiment_columns.iter().any(|column| column == "obsolete"));
    let session_columns = sqlx::query_scalar::<_, String>(
        "select name from pragma_table_info('sessions') order by cid",
    )
    .fetch_all(&store.pool)
    .await
    .unwrap();
    assert!(session_columns.iter().any(|column| column == "purpose"));
    let game_setting_columns = sqlx::query_scalar::<_, String>(
        "select name from pragma_table_info('game_settings') order by cid",
    )
    .fetch_all(&store.pool)
    .await
    .unwrap();
    assert!(!game_setting_columns
        .iter()
        .any(|column| column.contains("workspace")));
    let schema_version = sqlx::query_scalar::<_, i64>("select max(version) from schema_migrations")
        .fetch_one(&store.pool)
        .await
        .unwrap();
    assert_eq!(schema_version, 17);
}

/// Confirms first setup wins atomically and survives reopening the SQLite database.
#[tokio::test]
async fn sqlite_admin_setup_is_atomic_and_persistent() {
    let temp = tempdir().expect("tempdir");
    let database_url = format!("sqlite:///{}", temp.path().join("admin.sqlite").display());
    let store = SqliteExperimentStore::connect(&database_url).await.unwrap();
    let credential = StoredAdminCredential {
        username: "researcher".to_string(),
        password_hash: "$argon2id$test-hash".to_string(),
        role: "administrator".to_string(),
    };

    assert!(store
        .create_admin_credential(credential.clone())
        .await
        .unwrap());
    assert!(!store
        .create_admin_credential(StoredAdminCredential {
            username: "second".to_string(),
            ..credential.clone()
        })
        .await
        .unwrap());
    drop(store);

    let reopened = SqliteExperimentStore::connect(&database_url).await.unwrap();
    assert_eq!(reopened.admin_credential().await.unwrap(), Some(credential));
}

/// Rejects a database stamped below the deliberately supported schema baseline.
#[tokio::test]
async fn sqlite_rejects_prebaseline_schema_versions() {
    let temp = tempdir().expect("tempdir");
    let database_url = format!(
        "sqlite:///{}",
        temp.path().join("prebaseline.sqlite").display()
    );
    let store = SqliteExperimentStore::connect(&database_url).await.unwrap();
    sqlx::query("delete from schema_migrations")
        .execute(&store.pool)
        .await
        .unwrap();
    sqlx::query("insert into schema_migrations (version, applied_at) values (10, ?)")
        .bind(now_iso())
        .execute(&store.pool)
        .await
        .unwrap();
    store.pool.close().await;

    let error = match SqliteExperimentStore::connect(&database_url).await {
        Ok(_) => panic!("pre-baseline schema should fail"),
        Err(error) => error,
    };
    assert!(error
        .to_string()
        .contains("schema version 10 is unsupported"));
    assert!(error.to_string().contains("convert it explicitly"));
}

/// Rejects an existing unversioned database before adding any Parlando tables.
#[tokio::test]
async fn sqlite_does_not_mutate_an_unversioned_existing_database() {
    let temp = tempdir().expect("tempdir");
    let path = temp.path().join("unversioned.sqlite");
    let database_url = format!("sqlite:///{}", path.display());
    std::fs::File::create(&path).unwrap();
    let pool = SqlitePool::connect(&database_url).await.unwrap();
    sqlx::query("create table existing_data (value text not null)")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;

    let error = match SqliteExperimentStore::connect(&database_url).await {
        Ok(_) => panic!("unversioned existing database should fail"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("no Parlando schema version"));

    let pool = SqlitePool::connect(&database_url).await.unwrap();
    let tables = sqlx::query_scalar::<_, String>(
        "select name from sqlite_master where type = 'table' order by name",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(tables, vec!["existing_data"]);
}

/// Rejects storage URLs for backends that this installation does not implement.
#[tokio::test]
async fn unsupported_database_scheme_fails_clearly() {
    let error = match experiment_store_from_url("postgres://localhost/parlando").await {
        Ok(_) => panic!("unsupported scheme should fail"),
        Err(error) => error,
    };

    assert!(error
        .to_string()
        .contains("unsupported database url scheme"));
}

/// Rejects an absent database URL instead of silently creating an implicit store.
#[tokio::test]
async fn empty_database_url_is_rejected() {
    let error = match experiment_store_from_url("").await {
        Ok(_) => panic!("empty database url should fail"),
        Err(error) => error,
    };

    assert!(error.to_string().contains("database.url is required"));
}

/// Delayed touches cannot shorten a session, and either inclusive expiry cutoff revokes it.
#[tokio::test]
async fn administrator_session_expiry_uses_monotonic_activity_and_fixed_creation_time() {
    let store = SqliteExperimentStore::connect("sqlite:///:memory:")
        .await
        .unwrap();
    for (digest, created_at, last_seen_at) in
        [("absolute", 10, 99), ("idle", 11, 20), ("active", 11, 20)]
    {
        store
            .save_admin_session(StoredAdminSession {
                token_digest: digest.into(),
                role: "administrator".into(),
                csrf_token: "token".into(),
                created_at,
                last_seen_at,
            })
            .await
            .unwrap();
    }
    store.touch_admin_session("active", 21).await.unwrap();
    store.touch_admin_session("active", 19).await.unwrap();
    store.delete_expired_admin_sessions(20, 10).await.unwrap();
    for digest in ["absolute", "idle"] {
        assert!(store.admin_session(digest).await.unwrap().is_none());
    }
    let active = store.admin_session("active").await.unwrap().unwrap();
    assert_eq!((active.created_at, active.last_seen_at), (11, 21));
    store.delete_admin_session("active").await.unwrap();
    store.touch_admin_session("active", 100).await.unwrap();
    assert!(store.admin_session("active").await.unwrap().is_none());
}
