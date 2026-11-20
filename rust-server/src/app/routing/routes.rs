//! HTTP route declarations and middleware for experiment and installation surfaces.
//!
//! Runtime construction supplies initialized state for the experiment handlers. Catalogue
//! routes use installation context directly. The route trees attach authority checks and
//! shared HTTP limits at the surface where each operation is exposed.

use super::*;

/// Wires participant and admin HTTP surfaces around an initialized experiment.
pub(super) fn experiment_routes<A: Game>(
    state: Arc<AppState<A>>,
    mounted_runtime: bool,
    cors: CorsLayer,
    client_dist: Option<PathBuf>,
) -> BuiltRouters<A> {
    let context = state.context.clone();
    let participant_prefix = if mounted_runtime {
        format!("/e/{}", state.experiment_id)
    } else {
        String::new()
    };
    let runtime_admin_prefix = if mounted_runtime {
        format!("/api/admin/runtime/{}", state.experiment_id)
    } else {
        String::new()
    };
    let participant_path = |path: &str| format!("{participant_prefix}{path}");
    let public_routes = Router::new()
        .route(&participant_path("/health"), get(health::<A>))
        .route(&participant_path("/api/config"), get(public_config::<A>))
        .route(
            &participant_path("/api/participants"),
            post(create_participant::<A>),
        );
    let participant_routes = Router::new()
        .route(&participant_path("/api/consent"), post(consent::<A>))
        .route(
            &participant_path("/api/participant-state"),
            get(get_participant_state::<A>),
        )
        .route(
            &participant_path("/api/sessions"),
            post(create_session::<A>),
        )
        .route(
            &participant_path("/api/sessions/:public_session_id/leave"),
            post(leave_session::<A>),
        )
        .route(
            &participant_path("/api/sessions/:public_session_id/game-session"),
            post(game_session::<A>),
        )
        .route(
            &participant_path("/api/sessions/:public_session_id/audio-session"),
            post(audio_session::<A>),
        )
        .route(
            &participant_path("/api/sessions/:public_session_id/voice-diagnostics"),
            post(add_voice_diagnostic::<A>),
        )
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_participant_auth::<A>,
        ));
    let websocket_routes = Router::new()
        .route(
            &participant_path("/ws/game/:public_session_id"),
            get(game_socket::<A>),
        )
        .route(
            &participant_path("/ws/audio/:public_session_id"),
            get(audio_socket::<A>),
        );
    let experiment_admin = Router::new()
        .route(
            &participant_path("/api/admin/experiment"),
            get(admin_experiment::<A>),
        )
        .merge(experiment_admin_routes(&format!(
            "{participant_prefix}/api/admin"
        )))
        .route_layer(middleware::from_fn_with_state(
            context.clone(),
            require_admin_auth::<A>,
        ))
        .route_layer(middleware::from_fn_with_state(
            context.clone(),
            require_admin_network::<A>,
        ));
    let api = Router::new()
        .merge(public_routes)
        .merge(participant_routes)
        .merge(experiment_admin)
        .merge(websocket_routes)
        .with_state(state.clone())
        .merge(dashboard_routes(&context, &participant_prefix));
    let api = http_layers(api, &context, cors);
    spawn_security_cleanup(state.clone(), !mounted_runtime);
    if !mounted_runtime {
        spawn_load_sampler(context.clone());
    }

    let api = if let Some(dist) = client_dist.filter(|path| path.join("index.html").is_file()) {
        let index = dist.join("index.html");
        api.route_service(&participant_path("/"), ServeFile::new(index))
            .nest_service(
                &participant_path("/assets"),
                ServeDir::new(dist.join("assets")),
            )
    } else {
        api
    };
    let runtime_admin = {
        experiment_admin_routes(&runtime_admin_prefix)
            .route_layer(middleware::from_fn_with_state(
                context.clone(),
                require_admin_auth::<A>,
            ))
            .route_layer(middleware::from_fn_with_state(
                context.clone(),
                require_admin_network::<A>,
            ))
            .layer(RequestBodyLimitLayer::new(64 * 1024))
            .layer(middleware::from_fn(security_headers))
            .layer(middleware::from_fn_with_state(
                context.clone(),
                track_request_load::<A>,
            ))
            .with_state(state.clone())
    };
    BuiltRouters {
        state,
        primary: api,
        runtime_admin,
    }
}

/// Defines the live experiment operations served under both administrator URL prefixes.
pub(super) fn experiment_admin_routes<A: Game>(prefix: &str) -> Router<Arc<AppState<A>>> {
    let path = |suffix: &str| format!("{prefix}{suffix}");
    Router::new()
        .route(
            &path("/experiment/status"),
            post(admin_update_experiment_status::<A>),
        )
        .route(&path("/sessions"), get(admin_sessions::<A>))
        .route(
            &path("/progress"),
            get(admin_progress::<A>).post(admin_refresh_progress::<A>),
        )
        .route(
            &path("/sessions/:session_id"),
            get(admin_session_detail::<A>),
        )
        .route(
            &path("/sessions/:session_id/events"),
            get(admin_session_events::<A>),
        )
        .route(
            &path("/sessions/:session_id/replay"),
            get(admin_session_replay::<A>),
        )
        .route(&path("/export"), get(admin_export::<A>))
        .route(
            &path("/participants/:research_id/deletion"),
            get(admin_participant_deletion_preview::<A>).post(admin_delete_participant_data::<A>),
        )
}

/// Mounts catalogue and installation routes without allocating a live experiment.
pub(super) fn dashboard_routes<A: Game>(state: &Arc<ServerContext<A>>, prefix: &str) -> Router {
    let participant_path = |path: &str| format!("{prefix}{path}");
    let public_admin_routes = Router::new()
        .route(
            &participant_path("/admin/assets/parlando-mark.svg"),
            get(parlando_mark),
        )
        .route(
            &participant_path("/admin/assets/parlando-wordmark.svg"),
            get(parlando_wordmark),
        )
        .route(
            &participant_path("/admin/assets/palette.css"),
            get(parlando_palette),
        )
        .route(&participant_path("/admin"), get(admin_entry))
        .route(&participant_path("/admin/"), get(admin_entry))
        .route(
            &participant_path("/admin/login"),
            get(admin_login_page::<A>),
        )
        .route(
            &participant_path("/api/admin/setup"),
            post(admin_setup::<A>),
        )
        .route(
            &participant_path("/api/admin/login"),
            post(admin_login::<A>),
        )
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_admin_network::<A>,
        ));
    let admin_routes = Router::new()
        .route(
            &participant_path("/api/admin/export-schema"),
            get(admin_corpus_export_schema),
        )
        .route(&participant_path("/api/admin/load"), get(admin_load::<A>))
        .route(
            &participant_path("/admin/experiments"),
            get(admin_experiments_page),
        )
        .route(
            &participant_path("/admin/assets/admin-dashboard.css"),
            get(admin_dashboard_css),
        )
        .route(
            &participant_path("/admin/assets/admin-dashboard.js"),
            get(admin_dashboard_javascript),
        )
        .route(
            &participant_path("/admin/assets/admin-dashboard-state.js"),
            get(admin_dashboard_state_javascript),
        )
        .route(
            &participant_path("/admin/assets/admin-dashboard-format.js"),
            get(admin_dashboard_format_javascript),
        )
        .route(
            &participant_path("/admin/assets/admin-dashboard-api.js"),
            get(admin_dashboard_api_javascript),
        )
        .route(
            &participant_path("/admin/privacy"),
            get(admin_privacy_page::<A>),
        )
        .route(
            &participant_path("/api/admin/privacy"),
            get(admin_privacy_json::<A>),
        )
        .route(
            &participant_path("/api/admin/privacy.json"),
            get(admin_privacy_json_download::<A>),
        )
        .route(
            &participant_path("/api/admin/privacy.md"),
            get(admin_privacy_markdown_download::<A>),
        )
        .route(
            &participant_path("/api/admin/experiments"),
            get(admin_experiments::<A>).post(admin_create_experiment::<A>),
        )
        .route(
            &participant_path("/api/admin/experiments/:experiment_id/clone"),
            post(admin_clone_experiment::<A>),
        )
        .route(
            &participant_path("/api/admin/experiments/:experiment_id/config"),
            get(admin_experiment_config::<A>).post(admin_save_experiment_config::<A>),
        )
        .route(
            &participant_path("/api/admin/experiments/:experiment_id/privacy"),
            get(admin_experiment_privacy_json::<A>),
        )
        .route(
            &participant_path("/api/admin/experiments/:experiment_id/privacy.json"),
            get(admin_experiment_privacy_json_download::<A>),
        )
        .route(
            &participant_path("/api/admin/experiments/:experiment_id/privacy.md"),
            get(admin_experiment_privacy_markdown_download::<A>),
        )
        .route(
            &participant_path("/api/admin/experiments/:experiment_id/secrets/reveal"),
            post(admin_reveal_experiment_secret::<A>),
        )
        .route(
            &participant_path("/api/admin/experiments/:experiment_id/config/validate"),
            post(admin_validate_game_config::<A>),
        )
        .route(
            &participant_path("/api/admin/experiments/:experiment_id/revisions"),
            get(admin_experiment_revisions::<A>),
        )
        .route(
            &participant_path("/api/admin/experiments/:experiment_id/catalogue"),
            post(admin_update_experiment_catalogue::<A>),
        )
        .route(
            &participant_path("/api/admin/experiments/:experiment_id/archive"),
            post(admin_archive_experiment::<A>),
        )
        .route(
            &participant_path("/api/admin/game/settings"),
            get(admin_game_settings::<A>).post(admin_update_game_settings::<A>),
        )
        .route(
            &participant_path("/api/admin/game/secrets/reveal"),
            post(admin_reveal_game_secret::<A>),
        )
        .route(
            &participant_path("/api/admin/logout"),
            post(admin_logout::<A>),
        )
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_admin_auth::<A>,
        ))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_admin_network::<A>,
        ));
    public_admin_routes
        .merge(admin_routes)
        .with_state(state.clone())
}

/// Applies the same HTTP limits and response policy to installation and experiment routes.
pub(super) fn http_layers<A: Game>(
    api: Router,
    state: &Arc<ServerContext<A>>,
    cors: CorsLayer,
) -> Router {
    api.layer(RequestBodyLimitLayer::new(64 * 1024))
        .layer(ConcurrencyLimitLayer::new(256))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(30),
        ))
        .layer(middleware::from_fn(security_headers))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            track_request_load::<A>,
        ))
        .layer(cors)
}
