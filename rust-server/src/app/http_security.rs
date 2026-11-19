//! Origin validation, URL generation, and HTTP security headers.

use super::{
    anyhow, AllowOrigin, AppError, CorsLayer, ExperimentConfig, HeaderMap, HeaderName, HeaderValue,
    Method, Next, Request, Response, Result, StatusCode, AUTHORIZATION, CACHE_CONTROL,
    CONTENT_TYPE, HOST, ORIGIN,
};

/// Builds an exact-origin CORS policy from the public URL and configured additions.
pub(super) fn configured_cors(config: &ExperimentConfig) -> Result<CorsLayer> {
    let origins = allowed_origin_values(config)?;
    Ok(CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_methods([Method::GET, Method::POST])
        .allow_headers([
            AUTHORIZATION,
            CONTENT_TYPE,
            HeaderName::from_static("x-csrf-token"),
        ]))
}

/// Returns canonical configured origins, always including the public base origin.
fn allowed_origin_values(config: &ExperimentConfig) -> Result<Vec<HeaderValue>> {
    let mut origins = config.server.allowed_origins.clone();
    origins.push(canonical_origin(&config.server.public_base_url)?);
    origins.sort();
    origins.dedup();
    origins
        .into_iter()
        .map(|origin| {
            origin
                .parse::<HeaderValue>()
                .map_err(|error| anyhow!("invalid allowed origin {origin:?}: {error}"))
        })
        .collect()
}

/// Extracts a scheme-and-authority origin from an absolute HTTP(S) URL.
fn canonical_origin(url: &str) -> Result<String> {
    let uri = url
        .parse::<http::Uri>()
        .map_err(|error| anyhow!("invalid public URL {url:?}: {error}"))?;
    let scheme = uri
        .scheme_str()
        .filter(|scheme| matches!(*scheme, "http" | "https"))
        .ok_or_else(|| anyhow!("public URL must use http or https"))?;
    let authority = uri
        .authority()
        .ok_or_else(|| anyhow!("public URL must contain an authority"))?;
    Ok(format!("{scheme}://{authority}"))
}

/// Enforces the configured browser origin policy on WebSocket upgrades.
pub(super) fn validate_websocket_origin(
    config: &ExperimentConfig,
    headers: &HeaderMap,
) -> Result<(), AppError> {
    let origin = headers.get(ORIGIN).and_then(|value| value.to_str().ok());
    if origin.is_some_and(|origin| origin_matches_request_host(origin, headers)) {
        return Ok(());
    }
    let public_origin = canonical_origin(&config.server.public_base_url)
        .map_err(|error| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    if origin.is_none() && is_loopback_origin(&public_origin) {
        return Ok(());
    }
    let Some(origin) = origin else {
        return Err(AppError::forbidden("WebSocket Origin header is required"));
    };
    let mut allowed = config.server.allowed_origins.clone();
    allowed.push(public_origin);
    if allowed.iter().any(|candidate| candidate == origin) {
        Ok(())
    } else {
        Err(AppError::forbidden("WebSocket origin is not allowed"))
    }
}

/// Rejects cross-site administrator mutations using Origin and Fetch Metadata signals.
pub(super) fn validate_admin_request_origin(
    config: &ExperimentConfig,
    headers: &HeaderMap,
) -> Result<(), AppError> {
    if headers
        .get("sec-fetch-site")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == "cross-site")
    {
        return Err(AppError::forbidden(
            "Cross-site administrator request rejected",
        ));
    }
    let Some(origin) = headers.get(ORIGIN).and_then(|value| value.to_str().ok()) else {
        let public_origin = canonical_origin(&config.server.public_base_url)
            .map_err(|error| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
        if is_loopback_origin(&public_origin) {
            return Ok(());
        }
        return Err(AppError::forbidden(
            "Administrator Origin header is required",
        ));
    };
    if origin_matches_request_host(origin, headers) {
        return Ok(());
    }
    let mut allowed = config.server.allowed_origins.clone();
    allowed.push(
        canonical_origin(&config.server.public_base_url)
            .map_err(|error| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?,
    );
    if allowed.iter().any(|candidate| candidate == origin) {
        Ok(())
    } else {
        Err(AppError::forbidden("Administrator origin is not allowed"))
    }
}

/// Returns whether an Origin represents the same authority as the HTTP Host header.
fn origin_matches_request_host(origin: &str, headers: &HeaderMap) -> bool {
    let Some(host) = headers.get(HOST).and_then(|value| value.to_str().ok()) else {
        return false;
    };
    origin
        .parse::<http::Uri>()
        .ok()
        .and_then(|uri| uri.authority().map(ToString::to_string))
        .is_some_and(|authority| authority.eq_ignore_ascii_case(host))
}

/// Returns whether a canonical HTTP origin names a local loopback host.
fn is_loopback_origin(origin: &str) -> bool {
    origin.starts_with("http://localhost")
        || origin.starts_with("http://127.0.0.1")
        || origin.starts_with("http://[::1]")
}

/// Builds an origin-independent WebSocket path under the experiment route prefix.
pub(super) fn websocket_path(config: &ExperimentConfig, suffix: &str) -> String {
    let route_prefix = config
        .server
        .public_base_url
        .parse::<http::Uri>()
        .ok()
        .map(|uri| uri.path().trim_end_matches('/').to_string())
        .unwrap_or_default();
    format!("{route_prefix}/{}", suffix.trim_start_matches('/'))
}

/// Adds browser hardening headers to every normal HTTP response.
#[derive(Clone)]
pub(super) struct CspNonce(pub(super) String);

/// Adds browser hardening headers and a request-specific script nonce.
pub(super) async fn security_headers(mut request: Request, next: Next) -> Response {
    let no_store =
        request.uri().path().starts_with("/api/") || request.uri().path().starts_with("/admin");
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    request.extensions_mut().insert(CspNonce(nonce.clone()));
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    if no_store {
        headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    headers.insert(
        HeaderName::from_static("content-security-policy"),
        HeaderValue::from_str(&format!(
            "default-src 'self'; connect-src 'self' ws: wss:; img-src 'self' data:; style-src 'self' 'unsafe-inline'; script-src 'self' 'nonce-{nonce}'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'"
        ))
        .expect("generated CSP nonce is valid header text"),
    );
    headers.insert(
        HeaderName::from_static("strict-transport-security"),
        HeaderValue::from_static("max-age=31536000; includeSubDomains"),
    );
    headers.insert(
        HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        HeaderName::from_static("x-frame-options"),
        HeaderValue::from_static("DENY"),
    );
    headers.insert(
        HeaderName::from_static("referrer-policy"),
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        HeaderName::from_static("permissions-policy"),
        HeaderValue::from_static("camera=(), geolocation=(), payment=(), usb=()"),
    );
    response
}
