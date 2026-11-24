//! Serves the administrator shell and its embedded browser assets. The router exposes each
//! asset with its media type. The browser loads experiment data through separate
//! authenticated API endpoints.

use crate::app::{
    Arc, HeaderValue, Html, IntoResponse, Response, ServerContext, State, CACHE_CONTROL,
    CONTENT_TYPE,
};

/// Serves the shared parlando-mark.svg brand asset, including on the sign-in page.
pub(in crate::app) async fn parlando_mark() -> Response {
    admin_dashboard_asset(
        include_str!("../../../brand/parlando-mark.svg"),
        "image/svg+xml",
    )
}

/// Serves the shared parlando-wordmark.svg brand asset, including on the sign-in page.
pub(in crate::app) async fn parlando_wordmark() -> Response {
    admin_dashboard_asset(
        include_str!("../../../brand/parlando-wordmark.svg"),
        "image/svg+xml",
    )
}

/// Serves the shared palette.css brand asset, including on the sign-in page.
pub(in crate::app) async fn parlando_palette() -> Response {
    admin_dashboard_asset(
        include_str!("../../../brand/palette.css"),
        "text/css; charset=utf-8",
    )
}

/// Serves the database-backed experiment/session dashboard.
pub(in crate::app) async fn admin_experiments_page() -> Html<&'static str> {
    Html(ADMIN_EXPERIMENT_HTML)
}

/// Serves the original administration UI with this game's authenticated route prefix.
pub(in crate::app) async fn hosted_admin_experiments_page(
    State(state): State<Arc<ServerContext>>,
) -> Response {
    let prefix = format!("/{}", state.installation.game_descriptor.id);
    let html = ADMIN_EXPERIMENT_HTML
        .replace("<body>", &format!(r#"<body data-admin-prefix="{prefix}">"#))
        .replace("/admin/assets/", &format!("{prefix}/admin/assets/"));
    let mut response = Html(html).into_response();
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// Serves the embedded dashboard stylesheet as a same-origin protected asset.
pub(in crate::app) async fn admin_dashboard_css() -> Response {
    admin_dashboard_asset(ADMIN_EXPERIMENT_CSS, "text/css; charset=utf-8")
}

/// Serves the embedded dashboard module as a same-origin protected asset.
pub(in crate::app) async fn admin_dashboard_javascript() -> Response {
    admin_dashboard_asset(
        ADMIN_EXPERIMENT_JAVASCRIPT,
        "text/javascript; charset=utf-8",
    )
}

/// Serves the dashboard's mutable state module as a same-origin protected asset.
pub(in crate::app) async fn admin_dashboard_state_javascript() -> Response {
    admin_dashboard_asset(
        ADMIN_EXPERIMENT_STATE_JAVASCRIPT,
        "text/javascript; charset=utf-8",
    )
}

/// Serves pure dashboard formatting helpers as a same-origin protected asset.
pub(in crate::app) async fn admin_dashboard_format_javascript() -> Response {
    admin_dashboard_asset(
        ADMIN_EXPERIMENT_FORMAT_JAVASCRIPT,
        "text/javascript; charset=utf-8",
    )
}

/// Serves administrator request helpers as a same-origin protected asset.
pub(in crate::app) async fn admin_dashboard_api_javascript() -> Response {
    admin_dashboard_asset(
        ADMIN_EXPERIMENT_API_JAVASCRIPT,
        "text/javascript; charset=utf-8",
    )
}

/// Serves the configuration controls shared by both administrator dashboards.
pub(in crate::app) async fn admin_communication_config_javascript() -> Response {
    admin_dashboard_asset(
        include_str!("../communication_config.mjs"),
        "text/javascript; charset=utf-8",
    )
}

/// Serves the country-specific imprint generator and its editable identity controls.
pub(in crate::app) async fn admin_imprint_config_javascript() -> Response {
    admin_dashboard_asset(
        include_str!("../imprint_config.mjs"),
        "text/javascript; charset=utf-8",
    )
}

/// Prevents a restarted dashboard from combining newly embedded markup with stale assets.
fn admin_dashboard_asset(body: &'static str, content_type: &'static str) -> Response {
    let mut response = body.into_response();
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

const ADMIN_EXPERIMENT_HTML: &str = include_str!("../admin_dashboard.html");
const ADMIN_EXPERIMENT_CSS: &str = include_str!("../admin_dashboard.css");
const ADMIN_EXPERIMENT_JAVASCRIPT: &str = include_str!("../admin_dashboard.js");
const ADMIN_EXPERIMENT_STATE_JAVASCRIPT: &str = include_str!("../admin_dashboard_state.js");
const ADMIN_EXPERIMENT_FORMAT_JAVASCRIPT: &str = include_str!("../admin_dashboard_format.mjs");
const ADMIN_EXPERIMENT_API_JAVASCRIPT: &str = include_str!("../admin_dashboard_api.mjs");
