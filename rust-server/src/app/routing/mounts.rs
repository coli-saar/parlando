//! Mount retirement and request dispatch across active and draining runtime generations.
//!
//! The host routes new intake to the current mount. Existing assignments can continue on
//! a draining revision, selected using participant or session identity. Selection
//! identifies the target service. Its authentication middleware and single-use ticket
//! checks still apply.

use super::*;
use tower::ServiceExt;

/// A mounted experiment has one intake owner and may have previous owners draining existing rooms.
pub(in crate::app) struct ExperimentMounts<A: Game> {
    current: Option<Arc<BuiltRouters<A>>>,
    draining: HashMap<u64, Arc<BuiltRouters<A>>>,
    identity: MountIdentity,
}

impl<A: Game> ExperimentMounts<A> {
    /// Closes admission on the previous owner while retaining its room and participant services.
    pub(in crate::app) fn retire(&mut self) {
        if let Some(owner) = self.current.take() {
            owner
                .state
                .retired
                .store(true, std::sync::atomic::Ordering::Release);
            self.draining.insert(owner.state.mount_id, owner);
        }
    }

    /// Returns owners of existing rooms for repeat Prolific admission.
    pub(in crate::app) fn draining_states(&self) -> Vec<Arc<AppState<A>>> {
        self.draining
            .values()
            .map(|owner| owner.state.clone())
            .collect()
    }

    /// Releases the old router after its last live room and registration are gone.
    pub(in crate::app) fn finish_draining(&mut self, generation: u64) {
        self.draining.remove(&generation);
    }
}

/// Public namespace selecting one of an experiment runtime's two route surfaces.
#[derive(Clone, Copy)]
enum RuntimeSurface {
    Participant,
    Admin,
}

impl<A: Game> GameHost<A> {
    /// Returns the admission owner for the current configuration, mounting it once if necessary.
    async fn experiment_router(&self, experiment_id: &str) -> Result<Arc<BuiltRouters<A>>> {
        if let Some(router) = self
            .routers
            .read()
            .await
            .get(experiment_id)
            .and_then(|entry| entry.current.clone())
        {
            return Ok(router);
        }
        let _guard = lock_runtime_router_build(&self.dashboard, experiment_id).await;
        let mut identity = {
            let entries = self.routers.read().await;
            match entries.get(experiment_id) {
                Some(entry) => {
                    if let Some(router) = &entry.current {
                        return Ok(router.clone());
                    }
                    entry.identity.clone()
                }
                None => MountIdentity::default(),
            }
        };
        let config = hydrated_experiment_config(&self.dashboard, experiment_id)
            .await
            .map_err(|error| anyhow!(error.message))?;
        let options = (self.options_factory)(&config)?;
        identity.generation += 1;
        let mut built = build_experiment_router(
            self.dashboard.installation.clone(),
            config,
            options,
            true,
            identity.clone(),
        )
        .await?;
        built.runtime_admin = built
            .runtime_admin
            .layer(Extension(AdminExperimentScope(experiment_id.to_string())));
        let router = Arc::new(built);
        let mut entries = self.routers.write().await;
        let entry = entries
            .entry(experiment_id.to_string())
            .or_insert_with(|| ExperimentMounts {
                current: None,
                draining: HashMap::new(),
                identity: identity.clone(),
            });
        entry.identity = identity;
        entry.current = Some(router.clone());
        Ok(router)
    }

    /// Keeps assigned rooms on their original owner. Reopened direct intake can adopt a
    /// still-unassigned registration under the current purpose and consent configuration.
    async fn participant_owner(
        old: &Arc<BuiltRouters<A>>,
        current: Option<&Arc<BuiltRouters<A>>>,
        participant: &str,
    ) -> Arc<BuiltRouters<A>> {
        let Some(current) = current else {
            return old.clone();
        };
        {
            let memory = old.state.memory.read().await;
            if memory
                .sessions
                .values()
                .any(|room| room.participants.contains_key(participant))
            {
                return old.clone();
            }
        }
        let Ok(lifecycle) = require_open_experiment(&current.state).await else {
            return old.clone();
        };
        let _admission = current.state.session_admission.reserve().await;
        let mut old_memory = old.state.memory.write().await;
        let Some(registration) = old_memory.participants.remove(participant) else {
            return current.clone();
        };
        drop(old_memory);
        let config = &current.state.context.config;
        if registration.source == "direct"
            && config.direct.enabled
            && !config.recruitment.prolific.enabled
        {
            current
                .state
                .memory
                .write()
                .await
                .create_participant_with_id(
                    registration.id,
                    registration.participant_id,
                    registration.research_id,
                    registration.source,
                    lifecycle.data_purpose().to_owned(),
                );
            old.state
                .participant_auth
                .transfer_registration(participant, old.state.mount_id, current.state.mount_id)
                .await;
        } else {
            // Repeat the configured recruitment flow before admitting a newly eligible seat.
            old.state
                .participant_auth
                .revoke_participant_session(participant, old.state.mount_id)
                .await;
        }
        current.clone()
    }

    /// Finds a previous mount by a credential's issued owner or by the existing session key.
    /// This only selects the route. Its normal authentication and ticket consumption still apply.
    async fn cached_owner(
        &self,
        experiment: &str,
        surface: RuntimeSurface,
        suffix: &str,
        headers: &HeaderMap,
    ) -> Option<Arc<BuiltRouters<A>>> {
        let (current, participants, draining) = {
            let entries = self.routers.read().await;
            let entry = entries.get(experiment)?;
            if entry.draining.is_empty() {
                return entry.current.clone();
            }
            (
                entry.current.clone(),
                entry.identity.participants.clone(),
                entry.draining.clone(),
            )
        };
        if matches!(surface, RuntimeSurface::Participant)
            && (matches!(
                suffix,
                "/api/participant-state" | "/api/consent" | "/api/sessions"
            ) || suffix.starts_with("/api/sessions/"))
        {
            if let Some(credential) = bearer_token(headers) {
                if let Some(principal) = participants.authenticate(credential).await {
                    if let Some(owner) = principal.mount_id.and_then(|id| draining.get(&id)) {
                        return Some(
                            Self::participant_owner(
                                owner,
                                current.as_ref(),
                                &principal.participant_session_id,
                            )
                            .await,
                        );
                    }
                }
            }
            return current;
        }
        let public_id = matches!(surface, RuntimeSurface::Participant)
            .then(|| {
                suffix
                    .strip_prefix("/ws/game/")
                    .or_else(|| suffix.strip_prefix("/ws/audio/"))
            })
            .flatten();
        let durable_id = match surface {
            RuntimeSurface::Admin => suffix.strip_prefix("/sessions/"),
            RuntimeSurface::Participant => suffix.strip_prefix("/api/admin/sessions/"),
        }
        .and_then(|tail| tail.split('/').next()?.parse::<i64>().ok());
        if public_id.is_none() && durable_id.is_none() {
            return current;
        }
        for owner in draining.into_values() {
            let memory = owner.state.memory.read().await;
            let owns = match public_id {
                Some(id) => memory.sessions.contains_key(id),
                None => memory
                    .sessions
                    .values()
                    .any(|room| Some(room.session_id) == durable_id),
            };
            drop(memory);
            if owns {
                return Some(owner);
            }
        }
        current
    }
}

/// Resolves one experiment namespace from an unmatched host URI without rewriting it.
pub(super) async fn dispatch_experiment_service<A: Game>(
    host: Arc<GameHost<A>>,
    request: Request,
) -> Response {
    let path = request.uri().path();
    let (surface, relative_path) = if let Some(path) = path.strip_prefix("/e/") {
        (RuntimeSurface::Participant, path)
    } else if let Some(path) = path.strip_prefix("/api/admin/runtime/") {
        (RuntimeSurface::Admin, path)
    } else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let experiment_id = relative_path.split('/').next().filter(|id| !id.is_empty());
    let Some(experiment_id) = experiment_id else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if matches!(surface, RuntimeSurface::Participant) && relative_path == experiment_id {
        return Redirect::temporary(&format!("/e/{experiment_id}/")).into_response();
    }
    let suffix = &relative_path[experiment_id.len()..];
    let owner = match host
        .cached_owner(experiment_id, surface, suffix, request.headers())
        .await
    {
        Some(owner) => owner,
        None => match host.experiment_router(experiment_id).await {
            Ok(owner) => host
                .cached_owner(experiment_id, surface, suffix, request.headers())
                .await
                .unwrap_or(owner),
            Err(error) => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(json!({"error":error.to_string()})),
                )
                    .into_response()
            }
        },
    };
    let router = match surface {
        RuntimeSurface::Participant => owner.primary.clone(),
        RuntimeSurface::Admin => owner.runtime_admin.clone(),
    };
    router
        .oneshot(request)
        .await
        .unwrap_or_else(|error| match error {})
}
