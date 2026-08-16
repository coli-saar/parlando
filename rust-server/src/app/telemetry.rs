use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicI64, AtomicU64, Ordering},
        Arc,
    },
    time::Instant,
};

use axum::http::StatusCode;
use serde::Serialize;
use tokio::sync::RwLock;

/// Maximum five-second samples retained for the administrator load history.
const LOAD_HISTORY_CAPACITY: usize = 720;

/// Bounded operational counters shared by every runtime in one game process.
///
/// The telemetry deliberately excludes participant content and bearer credentials.
/// It is reset on process restart and never enters research exports.
pub(crate) struct RuntimeTelemetry {
    started_at: Instant,
    requests_in_flight: AtomicU64,
    requests_total: AtomicU64,
    request_client_errors_total: AtomicU64,
    request_server_errors_total: AtomicU64,
    request_duration_ms_total: AtomicU64,
    request_duration_ms_max: AtomicU64,
    game_messages_total: AtomicU64,
    heartbeats_total: AtomicU64,
    actions_accepted_total: AtomicU64,
    actions_rejected_total: AtomicU64,
    chat_messages_accepted_total: AtomicU64,
    chat_messages_rejected_total: AtomicU64,
    transport_messages_rejected_total: AtomicU64,
    audio_frames_total: AtomicU64,
    audio_frames_dropped_total: AtomicU64,
    asr_backpressure_total: AtomicU64,
    reconnections_total: AtomicU64,
    tts_in_flight: AtomicU64,
    tts_messages_total: AtomicU64,
    tts_failures_total: AtomicU64,
    history: RwLock<VecDeque<LoadSample>>,
}

impl Default for RuntimeTelemetry {
    /// Creates an empty operational telemetry registry for a fresh runtime.
    fn default() -> Self {
        Self {
            started_at: Instant::now(),
            requests_in_flight: AtomicU64::new(0),
            requests_total: AtomicU64::new(0),
            request_client_errors_total: AtomicU64::new(0),
            request_server_errors_total: AtomicU64::new(0),
            request_duration_ms_total: AtomicU64::new(0),
            request_duration_ms_max: AtomicU64::new(0),
            game_messages_total: AtomicU64::new(0),
            heartbeats_total: AtomicU64::new(0),
            actions_accepted_total: AtomicU64::new(0),
            actions_rejected_total: AtomicU64::new(0),
            chat_messages_accepted_total: AtomicU64::new(0),
            chat_messages_rejected_total: AtomicU64::new(0),
            transport_messages_rejected_total: AtomicU64::new(0),
            audio_frames_total: AtomicU64::new(0),
            audio_frames_dropped_total: AtomicU64::new(0),
            asr_backpressure_total: AtomicU64::new(0),
            reconnections_total: AtomicU64::new(0),
            tts_in_flight: AtomicU64::new(0),
            tts_messages_total: AtomicU64::new(0),
            tts_failures_total: AtomicU64::new(0),
            history: RwLock::new(VecDeque::with_capacity(LOAD_HISTORY_CAPACITY)),
        }
    }
}

impl RuntimeTelemetry {
    /// Starts tracking one HTTP request and returns its completion guard.
    pub(crate) fn begin_request(self: &Arc<Self>) -> RequestGuard {
        self.requests_in_flight.fetch_add(1, Ordering::Relaxed);
        RequestGuard {
            telemetry: self.clone(),
            started_at: Instant::now(),
        }
    }

    /// Records one inbound game-channel text message before semantic parsing.
    pub(crate) fn record_game_message(&self) {
        self.game_messages_total.fetch_add(1, Ordering::Relaxed);
    }

    /// Records one transport-only browser heartbeat.
    pub(crate) fn record_heartbeat(&self) {
        self.heartbeats_total.fetch_add(1, Ordering::Relaxed);
    }

    /// Records one action committed to durable storage.
    pub(crate) fn record_action_accepted(&self) {
        self.actions_accepted_total.fetch_add(1, Ordering::Relaxed);
    }

    /// Records one rejected action occurrence before durable coalescing.
    pub(crate) fn record_action_rejected(&self) {
        self.actions_rejected_total.fetch_add(1, Ordering::Relaxed);
    }

    /// Records one accepted human-authored typed message.
    pub(crate) fn record_chat_accepted(&self) {
        self.chat_messages_accepted_total
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Records one rejected human-authored typed message.
    pub(crate) fn record_chat_rejected(&self) {
        self.chat_messages_rejected_total
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Records one game-channel message dropped by the generous overload shaper.
    pub(crate) fn record_transport_rejected(&self) {
        self.transport_messages_rejected_total
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Records one validated audio frame admitted to relay and transcription.
    pub(crate) fn record_audio_frame(&self) {
        self.audio_frames_total.fetch_add(1, Ordering::Relaxed);
    }

    /// Records one invalid or ahead-of-real-time audio frame.
    pub(crate) fn record_audio_frame_dropped(&self) {
        self.audio_frames_dropped_total
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Records one frame that could not enter the bounded ASR input queue.
    pub(crate) fn record_asr_backpressure(&self) {
        self.asr_backpressure_total.fetch_add(1, Ordering::Relaxed);
    }

    /// Records one game or audio transport replacing an older owner for its role.
    pub(crate) fn record_reconnection(&self) {
        self.reconnections_total.fetch_add(1, Ordering::Relaxed);
    }

    /// Marks one trusted-agent speech synthesis request as started.
    pub(crate) fn begin_tts(&self) {
        self.tts_in_flight.fetch_add(1, Ordering::Relaxed);
        self.tts_messages_total.fetch_add(1, Ordering::Relaxed);
    }

    /// Marks one speech synthesis request as finished and optionally failed.
    pub(crate) fn finish_tts(&self, failed: bool) {
        self.tts_in_flight.fetch_sub(1, Ordering::Relaxed);
        if failed {
            self.tts_failures_total.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Returns the current cumulative counters and instantaneous request/provider gauges.
    pub(crate) fn counters(&self) -> TelemetryCounters {
        TelemetryCounters {
            uptime_seconds: self.started_at.elapsed().as_secs(),
            requests_in_flight: self.requests_in_flight.load(Ordering::Relaxed),
            requests_total: self.requests_total.load(Ordering::Relaxed),
            request_client_errors_total: self.request_client_errors_total.load(Ordering::Relaxed),
            request_server_errors_total: self.request_server_errors_total.load(Ordering::Relaxed),
            request_duration_ms_total: self.request_duration_ms_total.load(Ordering::Relaxed),
            request_duration_ms_max: self.request_duration_ms_max.load(Ordering::Relaxed),
            game_messages_total: self.game_messages_total.load(Ordering::Relaxed),
            heartbeats_total: self.heartbeats_total.load(Ordering::Relaxed),
            actions_accepted_total: self.actions_accepted_total.load(Ordering::Relaxed),
            actions_rejected_total: self.actions_rejected_total.load(Ordering::Relaxed),
            chat_messages_accepted_total: self.chat_messages_accepted_total.load(Ordering::Relaxed),
            chat_messages_rejected_total: self.chat_messages_rejected_total.load(Ordering::Relaxed),
            transport_messages_rejected_total: self
                .transport_messages_rejected_total
                .load(Ordering::Relaxed),
            audio_frames_total: self.audio_frames_total.load(Ordering::Relaxed),
            audio_frames_dropped_total: self.audio_frames_dropped_total.load(Ordering::Relaxed),
            asr_backpressure_total: self.asr_backpressure_total.load(Ordering::Relaxed),
            reconnections_total: self.reconnections_total.load(Ordering::Relaxed),
            tts_in_flight: self.tts_in_flight.load(Ordering::Relaxed),
            tts_messages_total: self.tts_messages_total.load(Ordering::Relaxed),
            tts_failures_total: self.tts_failures_total.load(Ordering::Relaxed),
        }
    }

    /// Appends one bounded five-second load sample for later dashboard readers.
    pub(crate) async fn push_sample(&self, sample: LoadSample) {
        let mut history = self.history.write().await;
        if history.len() == LOAD_HISTORY_CAPACITY {
            history.pop_front();
        }
        history.push_back(sample);
    }

    /// Returns the retained chronological load history.
    pub(crate) async fn history(&self) -> Vec<LoadSample> {
        self.history.read().await.iter().cloned().collect()
    }
}

/// RAII request tracker that always releases the in-flight gauge.
pub(crate) struct RequestGuard {
    telemetry: Arc<RuntimeTelemetry>,
    started_at: Instant,
}

impl RequestGuard {
    /// Records the completed response class and elapsed processing time.
    pub(crate) fn finish(self, status: StatusCode) {
        let elapsed_ms = self.started_at.elapsed().as_millis().min(u64::MAX as u128) as u64;
        self.telemetry
            .requests_total
            .fetch_add(1, Ordering::Relaxed);
        self.telemetry
            .request_duration_ms_total
            .fetch_add(elapsed_ms, Ordering::Relaxed);
        update_max(&self.telemetry.request_duration_ms_max, elapsed_ms);
        if status.is_client_error() {
            self.telemetry
                .request_client_errors_total
                .fetch_add(1, Ordering::Relaxed);
        }
        if status.is_server_error() {
            self.telemetry
                .request_server_errors_total
                .fetch_add(1, Ordering::Relaxed);
        }
    }
}

impl Drop for RequestGuard {
    /// Releases the in-flight gauge even if a request future is cancelled.
    fn drop(&mut self) {
        self.telemetry
            .requests_in_flight
            .fetch_sub(1, Ordering::Relaxed);
    }
}

/// Cumulative operational counters returned with every load sample.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct TelemetryCounters {
    pub uptime_seconds: u64,
    pub requests_in_flight: u64,
    pub requests_total: u64,
    pub request_client_errors_total: u64,
    pub request_server_errors_total: u64,
    pub request_duration_ms_total: u64,
    pub request_duration_ms_max: u64,
    pub game_messages_total: u64,
    pub heartbeats_total: u64,
    pub actions_accepted_total: u64,
    pub actions_rejected_total: u64,
    pub chat_messages_accepted_total: u64,
    pub chat_messages_rejected_total: u64,
    pub transport_messages_rejected_total: u64,
    pub audio_frames_total: u64,
    pub audio_frames_dropped_total: u64,
    pub asr_backpressure_total: u64,
    pub reconnections_total: u64,
    pub tts_in_flight: u64,
    pub tts_messages_total: u64,
    pub tts_failures_total: u64,
}

/// One bounded operational sample rendered on the administrator load page.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct LoadSample {
    pub sampled_at: String,
    pub sampled_at_ms: i64,
    pub counters: TelemetryCounters,
    pub capacity: CapacitySample,
    pub connections: ConnectionSample,
    pub pending_rejections: u64,
    pub storage: Option<crate::storage::StorageCapacity>,
}

/// Current use and configured ceilings for admitted research resources.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct CapacitySample {
    pub active_reserved_sessions: usize,
    pub active_session_limit: usize,
    pub waiting_sessions: usize,
    pub waiting_session_limit: usize,
    pub completed_retained_sessions: usize,
    pub unattached_participants: usize,
    pub unattached_participant_limit: usize,
    pub transcription_streams_reserved: usize,
    pub transcription_stream_limit: usize,
    pub active_agents: usize,
    pub storage_reserve_bytes: u64,
}

/// Instantaneous transport counts and heartbeat health bands.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct ConnectionSample {
    pub game_connections: usize,
    pub game_live: usize,
    pub game_delayed: usize,
    pub game_stale: usize,
    pub audio_connections: usize,
}

/// Process-local timestamps for one current role-owned transport.
pub(crate) struct ConnectionLiveness {
    connected_at_ms: i64,
    last_message_at_ms: AtomicI64,
    last_heartbeat_at_ms: AtomicI64,
    last_payload_at_ms: AtomicI64,
    messages_total: AtomicU64,
}

impl ConnectionLiveness {
    /// Creates timestamps for a newly accepted game or audio transport.
    pub(crate) fn new() -> Self {
        let now = chrono::Utc::now().timestamp_millis();
        Self {
            connected_at_ms: now,
            last_message_at_ms: AtomicI64::new(now),
            last_heartbeat_at_ms: AtomicI64::new(0),
            last_payload_at_ms: AtomicI64::new(0),
            messages_total: AtomicU64::new(0),
        }
    }

    /// Marks any inbound transport message and optionally a substantive payload.
    pub(crate) fn touch_message(&self, payload: bool) {
        let now = chrono::Utc::now().timestamp_millis();
        self.last_message_at_ms.store(now, Ordering::Relaxed);
        if payload {
            self.last_payload_at_ms.store(now, Ordering::Relaxed);
        }
        self.messages_total.fetch_add(1, Ordering::Relaxed);
    }

    /// Marks a game heartbeat independently from meaningful participant activity.
    pub(crate) fn touch_heartbeat(&self) {
        let now = chrono::Utc::now().timestamp_millis();
        self.last_message_at_ms.store(now, Ordering::Relaxed);
        self.last_heartbeat_at_ms.store(now, Ordering::Relaxed);
        self.messages_total.fetch_add(1, Ordering::Relaxed);
    }

    /// Returns the transport timestamps without acquiring the connection registry lock.
    pub(crate) fn snapshot(&self) -> ConnectionLivenessSnapshot {
        ConnectionLivenessSnapshot {
            connected_at_ms: self.connected_at_ms,
            last_message_at_ms: self.last_message_at_ms.load(Ordering::Relaxed),
            last_heartbeat_at_ms: nonzero(self.last_heartbeat_at_ms.load(Ordering::Relaxed)),
            last_payload_at_ms: nonzero(self.last_payload_at_ms.load(Ordering::Relaxed)),
            messages_total: self.messages_total.load(Ordering::Relaxed),
        }
    }
}

/// Serializable timestamp view for one game or audio connection.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct ConnectionLivenessSnapshot {
    pub connected_at_ms: i64,
    pub last_message_at_ms: i64,
    pub last_heartbeat_at_ms: Option<i64>,
    pub last_payload_at_ms: Option<i64>,
    pub messages_total: u64,
}

/// Operational liveness for one participant role in a runtime session.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct ParticipantLiveness {
    pub role: String,
    pub source: String,
    pub game_health: String,
    pub game: Option<ConnectionLivenessSnapshot>,
    pub audio: Option<ConnectionLivenessSnapshot>,
    pub audio_ready: bool,
}

/// Runtime-only session status used by dashboard liveness views.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct SessionLiveness {
    /// Experiment runtime which owns this session identifier.
    pub experiment_id: String,
    pub session_id: i64,
    pub room_id: String,
    pub status: String,
    pub health: String,
    pub meaningful_activity_at: String,
    pub lifecycle_deadline_at: Option<String>,
    pub deadline_reason: Option<String>,
    pub participants: Vec<ParticipantLiveness>,
}

/// Updates an atomic maximum without imposing a lock on request completion.
fn update_max(target: &AtomicU64, candidate: u64) {
    let mut current = target.load(Ordering::Relaxed);
    while candidate > current {
        match target.compare_exchange_weak(current, candidate, Ordering::Relaxed, Ordering::Relaxed)
        {
            Ok(_) => return,
            Err(observed) => current = observed,
        }
    }
}

/// Converts the zero sentinel used by atomics into an absent timestamp.
fn nonzero(value: i64) -> Option<i64> {
    (value > 0).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Creates the smallest complete load sample for history-retention tests.
    fn sample(index: i64, telemetry: &RuntimeTelemetry) -> LoadSample {
        LoadSample {
            sampled_at: index.to_string(),
            sampled_at_ms: index,
            counters: telemetry.counters(),
            capacity: CapacitySample {
                active_reserved_sessions: 0,
                active_session_limit: 1,
                waiting_sessions: 0,
                waiting_session_limit: 1,
                completed_retained_sessions: 0,
                unattached_participants: 0,
                unattached_participant_limit: 1,
                transcription_streams_reserved: 0,
                transcription_stream_limit: 1,
                active_agents: 0,
                storage_reserve_bytes: 0,
            },
            connections: ConnectionSample {
                game_connections: 0,
                game_live: 0,
                game_delayed: 0,
                game_stale: 0,
                audio_connections: 0,
            },
            pending_rejections: 0,
            storage: None,
        }
    }

    /// Verifies every semantic counter and TTS gauge is reflected in one snapshot.
    #[test]
    fn telemetry_records_all_runtime_events() {
        let telemetry = RuntimeTelemetry::default();
        telemetry.record_game_message();
        telemetry.record_heartbeat();
        telemetry.record_action_accepted();
        telemetry.record_action_rejected();
        telemetry.record_chat_accepted();
        telemetry.record_chat_rejected();
        telemetry.record_transport_rejected();
        telemetry.record_audio_frame();
        telemetry.record_audio_frame_dropped();
        telemetry.record_asr_backpressure();
        telemetry.record_reconnection();
        telemetry.begin_tts();

        let active = telemetry.counters();
        assert_eq!(active.game_messages_total, 1);
        assert_eq!(active.heartbeats_total, 1);
        assert_eq!(active.actions_accepted_total, 1);
        assert_eq!(active.actions_rejected_total, 1);
        assert_eq!(active.chat_messages_accepted_total, 1);
        assert_eq!(active.chat_messages_rejected_total, 1);
        assert_eq!(active.transport_messages_rejected_total, 1);
        assert_eq!(active.audio_frames_total, 1);
        assert_eq!(active.audio_frames_dropped_total, 1);
        assert_eq!(active.asr_backpressure_total, 1);
        assert_eq!(active.reconnections_total, 1);
        assert_eq!(active.tts_in_flight, 1);
        assert_eq!(active.tts_messages_total, 1);

        telemetry.finish_tts(true);
        let finished = telemetry.counters();
        assert_eq!(finished.tts_in_flight, 0);
        assert_eq!(finished.tts_failures_total, 1);
    }

    /// Confirms request cancellation releases the gauge and completion classifies status codes.
    #[test]
    fn request_guard_is_cancellation_safe_and_classifies_responses() {
        let telemetry = Arc::new(RuntimeTelemetry::default());
        let cancelled = telemetry.begin_request();
        assert_eq!(telemetry.counters().requests_in_flight, 1);
        drop(cancelled);
        assert_eq!(telemetry.counters().requests_in_flight, 0);
        assert_eq!(telemetry.counters().requests_total, 0);

        telemetry.begin_request().finish(StatusCode::BAD_REQUEST);
        telemetry
            .begin_request()
            .finish(StatusCode::INTERNAL_SERVER_ERROR);
        telemetry.begin_request().finish(StatusCode::NO_CONTENT);
        let counters = telemetry.counters();
        assert_eq!(counters.requests_in_flight, 0);
        assert_eq!(counters.requests_total, 3);
        assert_eq!(counters.request_client_errors_total, 1);
        assert_eq!(counters.request_server_errors_total, 1);
    }

    /// Confirms history remains chronological and discards only the oldest overflow sample.
    #[tokio::test]
    async fn load_history_is_bounded() {
        let telemetry = RuntimeTelemetry::default();
        for index in 0..=LOAD_HISTORY_CAPACITY as i64 {
            telemetry.push_sample(sample(index, &telemetry)).await;
        }
        let history = telemetry.history().await;
        assert_eq!(history.len(), LOAD_HISTORY_CAPACITY);
        assert_eq!(history.first().unwrap().sampled_at_ms, 1);
        assert_eq!(
            history.last().unwrap().sampled_at_ms,
            LOAD_HISTORY_CAPACITY as i64
        );
    }

    /// Verifies heartbeats and payloads update distinct liveness dimensions.
    #[test]
    fn connection_liveness_separates_heartbeat_and_payload_activity() {
        let liveness = ConnectionLiveness::new();
        let initial = liveness.snapshot();
        assert_eq!(initial.messages_total, 0);
        assert_eq!(initial.last_heartbeat_at_ms, None);
        assert_eq!(initial.last_payload_at_ms, None);

        liveness.touch_heartbeat();
        let heartbeat = liveness.snapshot();
        assert_eq!(heartbeat.messages_total, 1);
        assert!(heartbeat.last_heartbeat_at_ms.is_some());
        assert_eq!(heartbeat.last_payload_at_ms, None);

        liveness.touch_message(true);
        let payload = liveness.snapshot();
        assert_eq!(payload.messages_total, 2);
        assert!(payload.last_payload_at_ms.is_some());
        assert!(payload.last_message_at_ms >= payload.connected_at_ms);
    }

    /// Covers the atomic maximum and zero-sentinel helpers at their boundaries.
    #[test]
    fn telemetry_atomic_helpers_preserve_maximum_and_absence() {
        let maximum = AtomicU64::new(7);
        update_max(&maximum, 3);
        assert_eq!(maximum.load(Ordering::Relaxed), 7);
        update_max(&maximum, 11);
        assert_eq!(maximum.load(Ordering::Relaxed), 11);
        assert_eq!(nonzero(-1), None);
        assert_eq!(nonzero(0), None);
        assert_eq!(nonzero(1), Some(1));
    }
}
