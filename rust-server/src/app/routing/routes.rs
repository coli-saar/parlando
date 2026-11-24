//! HTTP route declarations and middleware for experiment and installation surfaces.
//!
//! Runtime construction supplies initialized state for the experiment handlers. Catalogue
//! routes use installation context directly. The route trees attach authority checks and
//! shared HTTP limits at the surface where each operation is exposed.

use super::*;

/// Wires participant and admin HTTP surfaces around an initialized experiment.
pub(super) fn experiment_routes(
    state: Arc<AppState>,
    mounted_runtime: bool,
    hosted: bool,
    cors: CorsLayer,
    client_dist: Option<PathBuf>,
) -> BuiltRouters {
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
        .route(&participant_path("/health"), get(health))
        .route(&participant_path("/api/config"), get(public_config))
        .route(
            &participant_path("/api/participants"),
            post(create_participant),
        );
    let participant_routes = Router::new()
        .route(&participant_path("/api/consent"), post(consent))
        .route(
            &participant_path("/api/participant-state"),
            get(get_participant_state),
        )
        .route(&participant_path("/api/sessions"), post(create_session))
        .route(
            &participant_path("/api/sessions/:public_session_id/leave"),
            post(leave_session),
        )
        .route(
            &participant_path("/api/sessions/:public_session_id/game-session"),
            post(game_session),
        )
        .route(
            &participant_path("/api/sessions/:public_session_id/audio-session"),
            post(audio_session),
        )
        .route(
            &participant_path("/api/sessions/:public_session_id/voice-diagnostics"),
            post(add_voice_diagnostic),
        )
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_participant_auth,
        ));
    let websocket_routes = Router::new()
        .route(
            &participant_path("/ws/game/:public_session_id"),
            get(game_socket),
        )
        .route(
            &participant_path("/ws/audio/:public_session_id"),
            get(audio_socket),
        );
    let experiment_admin = Router::new()
        .route(
            &participant_path("/api/admin/experiment"),
            get(admin_experiment),
        )
        .merge(experiment_admin_routes(&format!(
            "{participant_prefix}/api/admin"
        )))
        .route_layer(middleware::from_fn_with_state(
            context.clone(),
            require_admin_auth,
        ))
        .route_layer(middleware::from_fn_with_state(
            context.clone(),
            require_admin_network,
        ));
    let mut api = Router::new()
        .merge(public_routes)
        .merge(participant_routes)
        .merge(websocket_routes);
    if !hosted {
        api = api.merge(experiment_admin);
    }
    let mut api = api.with_state(state.clone());
    if !hosted {
        api = api.merge(dashboard_routes(&context, &participant_prefix));
    }
    spawn_security_cleanup(state.clone(), !mounted_runtime);
    if !mounted_runtime {
        spawn_load_sampler(context.clone());
    }

    if let Some(dist) = client_dist.filter(|path| path.join("index.html").is_file()) {
        api = api.route_service(
            &participant_path("/"),
            ServeFile::new(dist.join("index.html")),
        );
        if hosted {
            api = api.fallback_service(Router::new().nest_service(
                &participant_prefix,
                ServeDir::new(dist).append_index_html_on_directories(false),
            ));
        } else {
            api = api.nest_service(
                &participant_path("/assets"),
                ServeDir::new(dist.join("assets")),
            );
        }
    }
    let api = http_layers(api, &context, cors);
    let runtime_admin = experiment_admin_routes(&runtime_admin_prefix);
    let runtime_admin = if hosted {
        runtime_admin
    } else {
        runtime_admin
            .route_layer(middleware::from_fn_with_state(
                context.clone(),
                require_admin_auth,
            ))
            .route_layer(middleware::from_fn_with_state(
                context.clone(),
                require_admin_network,
            ))
    };
    let runtime_admin = runtime_admin
        .layer(RequestBodyLimitLayer::new(64 * 1024))
        .layer(middleware::from_fn(security_headers))
        .layer(middleware::from_fn_with_state(
            context.clone(),
            track_request_load,
        ))
        .with_state(state.clone());
    BuiltRouters {
        state,
        primary: api,
        runtime_admin,
    }
}

/// Defines the live experiment operations served under both administrator URL prefixes.
pub(super) fn experiment_admin_routes(prefix: &str) -> Router<Arc<AppState>> {
    let path = |suffix: &str| format!("{prefix}{suffix}");
    Router::new()
        .route(
            &path("/experiment/status"),
            post(admin_update_experiment_status),
        )
        .route(&path("/sessions"), get(admin_sessions))
        .route(
            &path("/progress"),
            get(admin_progress).post(admin_refresh_progress),
        )
        .route(&path("/sessions/:session_id"), get(admin_session_detail))
        .route(
            &path("/sessions/:session_id/events"),
            get(admin_session_events),
        )
        .route(
            &path("/sessions/:session_id/replay"),
            get(admin_session_replay),
        )
        .route(&path("/export"), get(admin_export))
        .route(
            &path("/participants/:research_id/deletion"),
            get(admin_participant_deletion_preview).post(admin_delete_participant_data),
        )
}

/// Mounts catalogue and installation routes without allocating a live experiment.
pub(super) fn dashboard_routes(state: &Arc<ServerContext>, prefix: &str) -> Router {
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
        .route(&participant_path("/admin/login"), get(admin_login_page))
        .route(&participant_path("/api/admin/setup"), post(admin_setup))
        .route(&participant_path("/api/admin/login"), post(admin_login))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_admin_network,
        ));
    let admin_routes = dashboard_feature_routes(prefix)
        .route(
            &participant_path("/admin/experiments"),
            get(admin_experiments_page),
        )
        .route(&participant_path("/admin/privacy"), get(admin_privacy_page))
        .route(&participant_path("/api/admin/logout"), post(admin_logout))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_admin_auth,
        ))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_admin_network,
        ));
    public_admin_routes
        .merge(admin_routes)
        .with_state(state.clone())
}

/// Serves the dashboard after the deployment host authenticates a game owner.
pub(super) fn hosted_dashboard_routes(state: &Arc<ServerContext>) -> Router {
    let cors = CorsLayer::new();
    let router = dashboard_feature_routes("")
        .route("/", get(hosted_admin_experiments_page))
        .route("/admin/assets/parlando-mark.svg", get(parlando_mark))
        .route(
            "/admin/assets/parlando-wordmark.svg",
            get(parlando_wordmark),
        )
        .route("/admin/assets/palette.css", get(parlando_palette))
        .with_state(state.clone());
    http_layers(router, state, cors)
}

/// Shares catalogue, settings, data, and dashboard assets across administration surfaces.
fn dashboard_feature_routes(prefix: &str) -> Router<Arc<ServerContext>> {
    let participant_path = |path: &str| format!("{prefix}{path}");
    Router::new()
        .route(
            &participant_path("/api/admin/experiments/:experiment_id/information/:page/download"),
            get(crate::app::study_sites::admin_information_site_download),
        )
        .route(
            &participant_path("/api/admin/export-schema"),
            get(admin_corpus_export_schema),
        )
        .route(&participant_path("/api/admin/load"), get(admin_load))
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
            &participant_path("/admin/assets/communication-config.js"),
            get(admin_communication_config_javascript),
        )
        .route(
            &participant_path("/admin/assets/imprint-config.js"),
            get(admin_imprint_config_javascript),
        )
        .route(
            &participant_path("/api/admin/privacy"),
            get(admin_privacy_json),
        )
        .route(
            &participant_path("/api/admin/privacy.json"),
            get(admin_privacy_json_download),
        )
        .route(
            &participant_path("/api/admin/privacy.md"),
            get(admin_privacy_markdown_download),
        )
        .route(
            &participant_path("/api/admin/experiments"),
            get(admin_experiments).post(admin_create_experiment),
        )
        .route(
            &participant_path("/api/admin/experiments/:experiment_id/clone"),
            post(admin_clone_experiment),
        )
        .route(
            &participant_path("/api/admin/experiments/:experiment_id/config"),
            get(admin_experiment_config).post(admin_save_experiment_config),
        )
        .route(
            &participant_path("/api/admin/experiments/:experiment_id/privacy"),
            get(admin_experiment_privacy_json),
        )
        .route(
            &participant_path("/api/admin/experiments/:experiment_id/privacy.json"),
            get(admin_experiment_privacy_json_download),
        )
        .route(
            &participant_path("/api/admin/experiments/:experiment_id/privacy.md"),
            get(admin_experiment_privacy_markdown_download),
        )
        .route(
            &participant_path("/api/admin/experiments/:experiment_id/secrets/reveal"),
            post(admin_reveal_experiment_secret),
        )
        .route(
            &participant_path("/api/admin/experiments/:experiment_id/config/validate"),
            post(admin_validate_game_config),
        )
        .route(
            &participant_path("/api/admin/experiments/:experiment_id/revisions"),
            get(admin_experiment_revisions),
        )
        .route(
            &participant_path("/api/admin/experiments/:experiment_id/catalogue"),
            post(admin_update_experiment_catalogue),
        )
        .route(
            &participant_path("/api/admin/experiments/:experiment_id/archive"),
            post(admin_archive_experiment),
        )
        .route(
            &participant_path("/api/admin/game/settings"),
            get(admin_game_settings).post(admin_update_game_settings),
        )
        .route(
            &participant_path("/api/admin/game/secrets/reveal"),
            post(admin_reveal_game_secret),
        )
}

/// Applies the same HTTP limits and response policy to installation and experiment routes.
pub(super) fn http_layers(api: Router, state: &Arc<ServerContext>, cors: CorsLayer) -> Router {
    api.layer(RequestBodyLimitLayer::new(64 * 1024))
        .layer(ConcurrencyLimitLayer::new(256))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(30),
        ))
        .layer(middleware::from_fn(security_headers))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            track_request_load,
        ))
        .layer(cors)
}
