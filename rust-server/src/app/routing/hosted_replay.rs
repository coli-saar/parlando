//! Authorizes one replay on the participant origin without exposing an administrator cookie.
//! The authenticated dashboard issues a short-lived capability for one session. The
//! participant renderer redeems it once using the same Fetch contract as a standalone replay.

use super::*;
use uuid::Uuid;

const GRANT_LIFETIME: Duration = Duration::from_secs(60);
const MAX_PENDING_GRANTS: usize = 128;

/// The only authority transferred to a package frontend, with a fixed experiment and session.
struct ReplayGrant {
    experiment_id: String,
    session_id: i64,
    expires_at: Instant,
}

/// Holds live replay grants for one published game revision.
#[derive(Default)]
pub(super) struct ReplayGrants(Mutex<HashMap<Uuid, ReplayGrant>>);

impl ReplayGrants {
    /// Returns the exact experiment and numeric session from a grant issuance path.
    pub(super) fn issue_target(path: &str) -> Option<(&str, i64)> {
        let rest = path.strip_prefix("/api/admin/runtime/")?;
        let (experiment_id, tail) = rest.split_once("/sessions/")?;
        let session_id = tail.strip_suffix("/replay-link")?.parse().ok()?;
        if experiment_id.is_empty()
            || experiment_id.contains('/')
            || matches!(experiment_id, "." | "..")
            || !experiment_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
        {
            return None;
        }
        Some((experiment_id, session_id))
    }

    /// Parses only this game's capability URL, with no information about other grants.
    pub(super) fn credential(game_id: &str, path: &str) -> Option<Uuid> {
        let (game, token) = path.strip_prefix("/api/admin/replays/")?.split_once('/')?;
        (game == game_id).then_some(())?;
        Uuid::parse_str(token).ok()
    }

    /// Mints a bounded, non-cacheable, one-use link after the host validates the request.
    pub(super) async fn issue(
        &self,
        context: &ServerContext,
        game_id: &str,
        experiment_id: &str,
        session_id: i64,
    ) -> Result<Response, AppError> {
        if context
            .installation
            .store
            .session_communication(experiment_id, session_id)
            .await?
            .is_none()
        {
            return Err(AppError::not_found("Session not found."));
        }
        let now = Instant::now();
        let mut grants = self.0.lock().await;
        grants.retain(|_, grant| now < grant.expires_at);
        if grants.len() >= MAX_PENDING_GRANTS {
            return Err(AppError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "Too many pending replay links.",
            ));
        }
        let token = Uuid::new_v4();
        grants.insert(
            token,
            ReplayGrant {
                experiment_id: experiment_id.to_string(),
                session_id,
                expires_at: now + GRANT_LIFETIME,
            },
        );
        let participant =
            experiment_public_base_url(&context.config.server.public_base_url, experiment_id);
        Ok(no_store(Json(json!({
            "url": format!("{participant}/?parlandoReplay=/api/admin/replays/{game_id}/{token}")
        }))))
    }

    /// Consumes a grant before projecting exactly its recorded session.
    pub(super) async fn redeem(&self, context: &ServerContext, token: Uuid) -> Response {
        let grant = self.0.lock().await.remove(&token);
        let Some(grant) = grant.filter(|grant| Instant::now() < grant.expires_at) else {
            return no_store(AppError::not_found(
                "Replay link expired or has already been used.",
            ));
        };
        no_store(experiment_session_replay(context, &grant.experiment_id, grant.session_id).await)
    }
}

/// Forbids caching a replay response or a URL that grants access to one.
fn no_store(result: impl IntoResponse) -> Response {
    let mut response = result.into_response();
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
    response
}
