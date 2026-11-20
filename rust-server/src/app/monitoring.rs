//! Collects installation-wide operational samples.
//!
//! The background sampler aggregates occupancy, transport health, and telemetry from
//! hosted experiment revisions. The administrator load endpoint combines recent samples
//! with per-room liveness. Request middleware supplies timing and request counts.

use super::{
    build_session_liveness, json, speechmatics_readiness_required, AppError, AppState, Arc,
    CapacitySample, ConnectionLivenessSnapshot, ConnectionSample, Duration, Game, Json, LoadSample,
    Next, Occupancy, Request, Response, Result, ServerContext, SessionLiveness, State, Value, Weak,
};

/// Classifies one current game transport from its most recent heartbeat or message.
pub(super) fn game_connection_health(
    snapshot: &ConnectionLivenessSnapshot,
    now_ms: i64,
) -> &'static str {
    let observed_at = snapshot
        .last_heartbeat_at_ms
        .unwrap_or(snapshot.last_message_at_ms);
    match now_ms.saturating_sub(observed_at) {
        0..=5_000 => "live",
        5_001..=15_000 => "delayed",
        _ => "stale",
    }
}

/// Takes a short-lock snapshot of current runtime capacity and operational counters.
async fn build_load_sample<A: Game>(state: &Arc<AppState<A>>) -> LoadSample {
    let now = chrono::Utc::now();
    let now_ms = now.timestamp_millis();
    let mut usage = Occupancy::default();
    {
        let memory = state.memory.read().await;
        usage.include(
            &memory,
            speechmatics_readiness_required(&state.context.config),
        );
    }
    let game_snapshots = state.game_connections.snapshots().await;
    let game_live = game_snapshots
        .values()
        .filter(|snapshot| game_connection_health(snapshot, now_ms) == "live")
        .count();
    let game_delayed = game_snapshots
        .values()
        .filter(|snapshot| game_connection_health(snapshot, now_ms) == "delayed")
        .count();
    let game_stale = game_snapshots.len() - game_live - game_delayed;
    let audio_connections = state.audio_connections.count().await;
    let pending_rejections = state
        .rejection_windows
        .read()
        .await
        .values()
        .map(|window| window.occurrences)
        .sum();
    let storage = match state.context.installation.store.storage_capacity().await {
        Ok(storage) => storage,
        Err(error) => {
            tracing::warn!(%error, "could not sample storage capacity for load telemetry");
            None
        }
    };
    let accepting = !state.retired.load(std::sync::atomic::Ordering::Acquire);
    LoadSample {
        sampled_at: now.to_rfc3339(),
        sampled_at_ms: now_ms,
        counters: state.context.installation.telemetry.counters(),
        capacity: CapacitySample {
            active_reserved_sessions: usage.active,
            active_session_limit: if accepting {
                state.context.config.capacity.max_active_sessions
            } else {
                0
            },
            waiting_sessions: usage.waiting,
            waiting_session_limit: if accepting {
                state.context.config.capacity.max_waiting_sessions
            } else {
                0
            },
            completed_retained_sessions: usage.completed,
            unattached_participants: usage.unattached,
            unattached_participant_limit: if accepting {
                state.context.config.capacity.max_unattached_participants
            } else {
                0
            },
            transcription_streams_reserved: usage.transcription,
            transcription_stream_limit: if accepting {
                state.context.config.capacity.max_transcription_streams
            } else {
                0
            },
            active_agents: state.started_agents.read().await.len(),
            storage_reserve_bytes: state
                .context
                .config
                .capacity
                .storage_reserve_megabytes
                .saturating_mul(1024 * 1024),
        },
        connections: ConnectionSample {
            game_connections: game_snapshots.len(),
            game_live,
            game_delayed,
            game_stale,
            audio_connections,
        },
        pending_rejections,
        storage,
    }
}

/// Returns strong references to every currently hosted experiment runtime.
async fn hosted_runtime_states<A: Game>(state: &ServerContext<A>) -> Vec<Arc<AppState<A>>> {
    let mut runtimes = state
        .installation
        .runtime_registry
        .read()
        .await
        .values()
        .flatten()
        .filter_map(Weak::upgrade)
        .collect::<Vec<_>>();
    runtimes.sort_by(|left, right| left.experiment_id.cmp(&right.experiment_id));
    runtimes
}

/// Aggregates current capacity and transport use across every hosted experiment runtime.
async fn build_game_load_sample<A: Game>(state: &ServerContext<A>) -> LoadSample {
    let runtimes = hosted_runtime_states(state).await;
    let mut runtimes = runtimes.into_iter();
    let Some(first) = runtimes.next() else {
        let now = chrono::Utc::now();
        return LoadSample {
            sampled_at: now.to_rfc3339(),
            sampled_at_ms: now.timestamp_millis(),
            counters: state.installation.telemetry.counters(),
            capacity: CapacitySample {
                active_session_limit: state.config.capacity.max_active_sessions,
                waiting_session_limit: state.config.capacity.max_waiting_sessions,
                unattached_participant_limit: state.config.capacity.max_unattached_participants,
                transcription_stream_limit: state.config.capacity.max_transcription_streams,
                storage_reserve_bytes: state
                    .config
                    .capacity
                    .storage_reserve_megabytes
                    .saturating_mul(1024 * 1024),
                ..Default::default()
            },
            connections: ConnectionSample::default(),
            pending_rejections: 0,
            storage: state
                .installation
                .store
                .storage_capacity()
                .await
                .unwrap_or_default(),
        };
    };
    let mut aggregate = build_load_sample(&first).await;
    for runtime in runtimes {
        let sample = build_load_sample(&runtime).await;
        aggregate.capacity.active_reserved_sessions += sample.capacity.active_reserved_sessions;
        aggregate.capacity.active_session_limit += sample.capacity.active_session_limit;
        aggregate.capacity.waiting_sessions += sample.capacity.waiting_sessions;
        aggregate.capacity.waiting_session_limit += sample.capacity.waiting_session_limit;
        aggregate.capacity.completed_retained_sessions +=
            sample.capacity.completed_retained_sessions;
        aggregate.capacity.unattached_participants += sample.capacity.unattached_participants;
        aggregate.capacity.unattached_participant_limit +=
            sample.capacity.unattached_participant_limit;
        aggregate.capacity.transcription_streams_reserved +=
            sample.capacity.transcription_streams_reserved;
        aggregate.capacity.transcription_stream_limit += sample.capacity.transcription_stream_limit;
        aggregate.capacity.active_agents += sample.capacity.active_agents;
        aggregate.capacity.storage_reserve_bytes = aggregate
            .capacity
            .storage_reserve_bytes
            .max(sample.capacity.storage_reserve_bytes);
        aggregate.connections.game_connections += sample.connections.game_connections;
        aggregate.connections.game_live += sample.connections.game_live;
        aggregate.connections.game_delayed += sample.connections.game_delayed;
        aggregate.connections.game_stale += sample.connections.game_stale;
        aggregate.connections.audio_connections += sample.connections.audio_connections;
        aggregate.pending_rejections += sample.pending_rejections;
    }
    aggregate.counters = state.installation.telemetry.counters();
    aggregate
}

/// Returns liveness for every in-memory session across the hosted game process.
async fn build_game_session_liveness<A: Game>(state: &ServerContext<A>) -> Vec<SessionLiveness> {
    let mut sessions = Vec::new();
    for runtime in hosted_runtime_states(state).await {
        sessions.extend(build_session_liveness(&runtime).await);
    }
    sessions.sort_by(|left, right| {
        left.experiment_id
            .cmp(&right.experiment_id)
            .then(left.session_id.cmp(&right.session_id))
    });
    sessions
}

/// Samples bounded game-process operational history every five seconds.
pub(super) fn spawn_load_sampler<A: Game>(state: Arc<ServerContext<A>>) {
    let context = Arc::downgrade(&state);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(5));
        loop {
            interval.tick().await;
            let Some(state) = context.upgrade() else {
                break;
            };
            let sample = build_game_load_sample(&state).await;
            state.installation.telemetry.push_sample(sample).await;
        }
    });
}

/// Returns game-wide current load, one-hour history, and runtime session liveness.
pub(super) async fn admin_load<A: Game>(
    State(state): State<Arc<ServerContext<A>>>,
) -> Result<Json<Value>, AppError> {
    let current = build_game_load_sample(&state).await;
    let history = state.installation.telemetry.history().await;
    let sessions = build_game_session_liveness(&state).await;
    Ok(Json(json!({
        "current": current,
        "history": history,
        "sessions": sessions,
        "sampling": { "interval_seconds": 5, "retention_seconds": 3600 },
        "liveness_thresholds_ms": { "live": 5000, "delayed": 15000, "socket_timeout": 90000 },
    })))
}

/// Measures HTTP concurrency, response classes, and latency for the load dashboard.
pub(super) async fn track_request_load<A: Game>(
    State(state): State<Arc<ServerContext<A>>>,
    request: Request,
    next: Next,
) -> Response {
    let guard = state.installation.telemetry.begin_request();
    let response = next.run(request).await;
    guard.finish(response.status());
    response
}
