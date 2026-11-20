//! Enforces administrator and participant access at the HTTP boundary.
//!
//! Routing installs these middleware checks before protected operations. Participant
//! credentials resolve to a principal passed to handlers. Administrator operations apply
//! the configured network policy and verify the administrator session, including origin
//! and CSRF checks where required.

use super::{
    validate_admin_request_origin, AppError, AppState, Arc, ConnectInfo, Extension, Game,
    HeaderMap, IntoResponse, IpAddr, Method, Next, ParticipantPrincipal, Redirect, Request,
    Response, Result, ServerContext, SocketAddr, State, StatusCode, AUTHORIZATION, COOKIE,
};

/// Authenticates participant API requests and attaches the resolved principal.
pub(super) async fn require_participant_auth<A: Game>(
    State(state): State<Arc<AppState<A>>>,
    mut request: Request,
    next: Next,
) -> Result<Response, AppError> {
    let credential = bearer_token(request.headers())
        .ok_or_else(|| AppError::new(StatusCode::UNAUTHORIZED, "Authentication required"))?;
    let principal = state
        .participant_auth
        .authenticate(credential)
        .await
        .ok_or_else(|| AppError::new(StatusCode::UNAUTHORIZED, "Invalid participant credential"))?;
    request.extensions_mut().insert(principal);
    Ok(next.run(request).await)
}

/// Restricts all administrator surfaces to configured direct-peer CIDR ranges.
pub(super) async fn require_admin_network<A: Game>(
    State(state): State<Arc<ServerContext<A>>>,
    request: Request,
    next: Next,
) -> Result<Response, AppError> {
    let ranges = state
        .installation
        .game_settings
        .read()
        .await
        .admin_allowed_ip_ranges
        .clone();
    if ranges.is_empty() {
        return Ok(next.run(request).await);
    }
    let client_ip = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|peer| peer.0.ip());
    let allowed = admin_network_allows(&ranges, client_ip);
    if !allowed {
        tracing::warn!(?client_ip, "administrator network policy rejected request");
        return Err(AppError::new(
            StatusCode::FORBIDDEN,
            "Administrator access is not available from this network",
        ));
    }
    Ok(next.run(request).await)
}

/// Applies a validated CIDR list to one direct network peer.
pub(super) fn admin_network_allows(ranges: &[String], client_ip: Option<IpAddr>) -> bool {
    ranges.is_empty()
        || client_ip.is_some_and(|client_ip| {
            ranges.iter().any(|range| {
                range
                    .parse::<ipnet::IpNet>()
                    .is_ok_and(|network| network.contains(&client_ip))
            })
        })
}

/// Authenticates every administrator route and enforces role and CSRF on mutations.
pub(super) async fn require_admin_auth<A: Game>(
    State(state): State<Arc<ServerContext<A>>>,
    mut request: Request,
    next: Next,
) -> Result<Response, AppError> {
    let browser_page =
        request.method() == Method::GET && request.uri().path().starts_with("/admin/");
    let Some(token) = cookie_value(request.headers(), "parlando_admin") else {
        if browser_page {
            return Ok(Redirect::temporary("/admin/login").into_response());
        }
        return Err(AppError::new(
            StatusCode::UNAUTHORIZED,
            "Authentication required",
        ));
    };
    if !state.installation.admin_auth.is_configured().await {
        if browser_page {
            return Ok(Redirect::temporary("/admin/login").into_response());
        }
        return Err(AppError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "Administrator authentication is not configured",
        ));
    }
    let Some(session) = state.installation.admin_auth.authenticate(token).await? else {
        if browser_page {
            return Ok(Redirect::temporary("/admin/login").into_response());
        }
        return Err(AppError::new(
            StatusCode::UNAUTHORIZED,
            "Invalid administrator session",
        ));
    };
    if request.method() != Method::GET {
        validate_admin_request_origin(&state.config, request.headers())?;
        let csrf = request
            .headers()
            .get("x-csrf-token")
            .and_then(|value| value.to_str().ok());
        if csrf != Some(session.csrf_token.as_str()) {
            return Err(AppError::forbidden("Invalid CSRF token"));
        }
        if request.uri().path() != "/api/admin/logout" && !session.role.may_mutate_experiments() {
            return Err(AppError::forbidden("Administrator role required"));
        }
    }
    request.extensions_mut().insert(session);
    Ok(next.run(request).await)
}

/// Returns the bearer value from an RFC 6750 Authorization header.
pub(super) fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .filter(|token| !token.is_empty())
}

/// Returns one named cookie value without logging the surrounding cookie header.
pub(super) fn cookie_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get(COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .find_map(|part| {
            let (candidate, value) = part.trim().split_once('=')?;
            (candidate == name).then_some(value)
        })
}

/// Returns the participant identity established by bearer authentication.
pub(super) fn authenticated_participant_id(
    principal: Option<Extension<ParticipantPrincipal>>,
) -> Result<String, AppError> {
    if let Some(Extension(principal)) = principal {
        return Ok(principal.participant_session_id);
    }
    Err(AppError::new(
        StatusCode::UNAUTHORIZED,
        "Authentication required",
    ))
}
