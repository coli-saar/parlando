use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use anyhow::{bail, Context, Result};
use clap::{Parser, ValueEnum};
use parlando::test_support::{
    run_dashboard as run_shared_dashboard, AudioFrame, AudioOutbound, AudioSessionRegistry,
    DashboardHealth, DashboardMetric, DashboardPanel, DashboardSeries, DashboardSnapshot,
    DashboardTile, AUDIO_FRAME_BYTES, AUDIO_OUTBOUND_QUEUE_CAPACITY,
};
use ratatui::DefaultTerminal;
use tokio::sync::{mpsc, watch};

const FRAME_INTERVAL: Duration = Duration::from_millis(20);
const METRICS_TICK: Duration = Duration::from_millis(250);
const ROOM_TIMEOUT: Duration = Duration::from_secs(2);
const RATE_WINDOW_SECONDS: usize = 60;
const LATENCY_WINDOW_SAMPLES: usize = 20_000;
const TTS_BURST_FRAMES: u16 = 10;
const IMPAIRED_SLOW_SESSIONS_PERCENT: usize = 5;
const IMPAIRED_PAUSE_ROUNDS: u64 = 80;
const IMPAIRED_CYCLE_ROUNDS: u64 = 750;

/// Standalone audio-session stress dashboard arguments.
#[derive(Clone, Debug, Parser)]
#[command(about = "Stress Parlando's process-local audio sessions with verified PCM frames")]
struct StressArgs {
    /// Workload model: paced production cadence, maximum throughput, or deterministic impairment.
    #[arg(long, value_enum, default_value_t = StressMode::Realistic)]
    mode: StressMode,
    /// Number of isolated two-participant audio sessions.
    #[arg(long, default_value_t = 200, value_parser = parse_positive_usize)]
    sessions: usize,
    /// Wall-clock test duration in seconds.
    #[arg(long, default_value_t = 600, value_parser = parse_positive_u64)]
    seconds: u64,
    /// Percentage of sessions selected for each simulated agent-TTS burst.
    #[arg(long, default_value_t = 10, value_parser = parse_percent)]
    tts_session_percent: u8,
    /// Seconds between ten-frame (200 ms) agent-TTS bursts.
    #[arg(long, default_value_t = 5, value_parser = parse_positive_u64)]
    tts_interval_seconds: u64,
}

/// Parses a strictly positive session count for the CLI.
fn parse_positive_usize(value: &str) -> std::result::Result<usize, String> {
    value
        .parse::<usize>()
        .map_err(|error| error.to_string())
        .and_then(|parsed| {
            (parsed > 0)
                .then_some(parsed)
                .ok_or("must be at least 1".into())
        })
}

/// Parses a strictly positive duration or interval for the CLI.
fn parse_positive_u64(value: &str) -> std::result::Result<u64, String> {
    value
        .parse::<u64>()
        .map_err(|error| error.to_string())
        .and_then(|parsed| {
            (parsed > 0)
                .then_some(parsed)
                .ok_or("must be at least 1".into())
        })
}

/// Parses a percentage in the inclusive range from zero through one hundred.
fn parse_percent(value: &str) -> std::result::Result<u8, String> {
    value
        .parse::<u8>()
        .map_err(|error| error.to_string())
        .and_then(|parsed| {
            (parsed <= 100)
                .then_some(parsed)
                .ok_or("must be between 0 and 100".into())
        })
}

/// Workload families exposed by the stress dashboard CLI.
#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum StressMode {
    /// Full-duplex 20 ms participant cadence plus periodic TTS bursts.
    Realistic,
    /// Unpaced full-duplex traffic for maximum in-memory registry throughput.
    Saturation,
    /// Realistic cadence with jitter, deadline pressure, and slow consumers.
    Impaired,
}

impl StressMode {
    /// Returns the concise dashboard label for this workload.
    fn label(self) -> &'static str {
        match self {
            Self::Realistic => "realistic",
            Self::Saturation => "saturation",
            Self::Impaired => "impaired",
        }
    }

    /// Reports whether this workload follows the production 20 ms frame clock.
    fn is_paced(self) -> bool {
        !matches!(self, Self::Saturation)
    }
}

#[derive(Clone, Debug)]
struct StressConfig {
    mode: StressMode,
    session_count: usize,
    duration: Duration,
    tts_session_percent: u8,
    tts_interval: Duration,
}

impl From<StressArgs> for StressConfig {
    /// Converts validated CLI arguments into runtime durations and counts.
    fn from(args: StressArgs) -> Self {
        Self {
            mode: args.mode,
            session_count: args.sessions,
            duration: Duration::from_secs(args.seconds),
            tts_session_percent: args.tts_session_percent,
            tts_interval: Duration::from_secs(args.tts_interval_seconds),
        }
    }
}

#[derive(Clone, Debug)]
struct StressSnapshot {
    mode: StressMode,
    elapsed: Duration,
    duration: Duration,
    session_count: usize,
    rounds: u64,
    verified_frames: u64,
    participant_frames: u64,
    tts_frames: u64,
    expected_rate: u64,
    current_rate: u64,
    average_rate: u64,
    peak_rate: u64,
    p50_us: u64,
    p95_us: u64,
    p99_us: u64,
    max_us: u64,
    session_pressure: Vec<u64>,
    rate_history: Vec<u64>,
    payload_errors: u64,
    timeouts: u64,
    unexpected_messages: u64,
    dropped_frames: u64,
    deadline_misses: u64,
    max_schedule_lag_us: u64,
    milestones: Vec<String>,
    finished: bool,
    cancelled: bool,
    failure: Option<String>,
}

impl StressSnapshot {
    /// Creates an empty dashboard state for the selected workload.
    fn new(config: &StressConfig) -> Self {
        let expected_rate = if config.mode.is_paced() {
            config.session_count as u64 * 2 * 50
        } else {
            0
        };
        Self {
            mode: config.mode,
            elapsed: Duration::ZERO,
            duration: config.duration,
            session_count: config.session_count,
            rounds: 0,
            verified_frames: 0,
            participant_frames: 0,
            tts_frames: 0,
            expected_rate,
            current_rate: 0,
            average_rate: 0,
            peak_rate: 0,
            p50_us: 0,
            p95_us: 0,
            p99_us: 0,
            max_us: 0,
            session_pressure: vec![0; config.session_count],
            rate_history: vec![],
            payload_errors: 0,
            timeouts: 0,
            unexpected_messages: 0,
            dropped_frames: 0,
            deadline_misses: 0,
            max_schedule_lag_us: 0,
            milestones: vec![format!(
                "Configured {} mode: {} sessions for {}",
                config.mode.label(),
                config.session_count,
                format_clock(config.duration)
            )],
            finished: false,
            cancelled: false,
            failure: None,
        }
    }

    /// Reports failures that always invalidate the workload.
    fn fatal_error_count(&self) -> u64 {
        self.payload_errors + self.timeouts + self.unexpected_messages
    }
}

struct StressMetrics {
    snapshot: StressSnapshot,
    recent_latencies_us: VecDeque<u64>,
    rate_buckets: VecDeque<u64>,
    bucket_started: Instant,
    bucket_frames: u64,
    next_milestone: u64,
}

impl StressMetrics {
    /// Creates rolling latency, throughput, and milestone accumulators.
    fn new(config: &StressConfig, now: Instant) -> Self {
        Self {
            snapshot: StressSnapshot::new(config),
            recent_latencies_us: VecDeque::with_capacity(LATENCY_WINDOW_SAMPLES),
            rate_buckets: VecDeque::with_capacity(RATE_WINDOW_SECONDS),
            bucket_started: now,
            bucket_frames: 0,
            next_milestone: 1_000_000,
        }
    }

    /// Records one verified frame and its in-memory queue latency.
    fn record_frame(&mut self, session_index: usize, latency: Duration, source: FrameSource) {
        let latency_us = latency.as_micros().min(u128::from(u64::MAX)) as u64;
        self.snapshot.verified_frames += 1;
        self.bucket_frames += 1;
        match source {
            FrameSource::Agent => self.snapshot.tts_frames += 1,
            FrameSource::A | FrameSource::B => self.snapshot.participant_frames += 1,
        }
        self.snapshot.session_pressure[session_index] = latency_us;
        self.snapshot.max_us = self.snapshot.max_us.max(latency_us);
        self.recent_latencies_us.push_back(latency_us);
        if self.recent_latencies_us.len() > LATENCY_WINDOW_SAMPLES {
            self.recent_latencies_us.pop_front();
        }
        if self.snapshot.verified_frames >= self.next_milestone {
            self.push_milestone(format!(
                "{} verified frames without cross-session leakage",
                format_count(self.snapshot.verified_frames)
            ));
            self.next_milestone = self.next_milestone.saturating_add(1_000_000);
        }
    }

    /// Records a deliberately induced bounded-queue drop in impaired mode.
    fn record_drop(&mut self, session_index: usize) {
        self.snapshot.dropped_frames += 1;
        self.snapshot.session_pressure[session_index] = AUDIO_OUTBOUND_QUEUE_CAPACITY as u64;
    }

    /// Rolls one-second verified-frame buckets into the 60-second sparkline.
    fn update_rate(&mut self, now: Instant) {
        let elapsed = now.duration_since(self.bucket_started);
        if elapsed < Duration::from_secs(1) {
            return;
        }
        let rate = (self.bucket_frames as f64 / elapsed.as_secs_f64()).round() as u64;
        self.snapshot.current_rate = rate;
        if rate > self.snapshot.peak_rate {
            self.snapshot.peak_rate = rate;
            self.push_milestone(format!(
                "New verified-frame peak: {} frames/s",
                format_count(rate)
            ));
        }
        self.rate_buckets.push_back(rate);
        while self.rate_buckets.len() > RATE_WINDOW_SECONDS {
            self.rate_buckets.pop_front();
        }
        self.snapshot.rate_history = self.rate_buckets.iter().copied().collect();
        self.bucket_started = now;
        self.bucket_frames = 0;
    }

    /// Recomputes elapsed-rate and rolling latency-percentile summaries.
    fn refresh_summary(&mut self, started_at: Instant, now: Instant) {
        self.snapshot.elapsed = now.duration_since(started_at).min(self.snapshot.duration);
        self.snapshot.average_rate = if self.snapshot.elapsed.is_zero() {
            0
        } else {
            (self.snapshot.verified_frames as f64 / self.snapshot.elapsed.as_secs_f64()).round()
                as u64
        };
        let mut sorted = self.recent_latencies_us.iter().copied().collect::<Vec<_>>();
        sorted.sort_unstable();
        self.snapshot.p50_us = percentile(&sorted, 0.50);
        self.snapshot.p95_us = percentile(&sorted, 0.95);
        self.snapshot.p99_us = percentile(&sorted, 0.99);
    }

    /// Appends a bounded newest-first event to the dashboard.
    fn push_milestone(&mut self, message: String) {
        self.snapshot.milestones.insert(
            0,
            format!("{}  {message}", format_clock(self.snapshot.elapsed)),
        );
        self.snapshot.milestones.truncate(6);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FrameSource {
    A,
    B,
    Agent,
}

impl FrameSource {
    /// Encodes the source identity inside the deterministic PCM payload.
    fn marker(self) -> u8 {
        match self {
            Self::A => 0xA1,
            Self::B => 0xB2,
            Self::Agent => 0xCC,
        }
    }
}

#[derive(Debug)]
struct ExpectedFrame {
    bytes: Vec<u8>,
    sent_at: Instant,
    source: FrameSource,
}

#[derive(Debug)]
struct RoomHarness {
    public_session_id: String,
    receiver_a: mpsc::Receiver<AudioOutbound>,
    receiver_b: mpsc::Receiver<AudioOutbound>,
    expected_a: VecDeque<ExpectedFrame>,
    expected_b: VecDeque<ExpectedFrame>,
    tts_remaining: u16,
}

/// Runs the terminal dashboard and exits nonzero after a fatal invariant failure.
#[tokio::main]
async fn main() -> Result<()> {
    let config = StressConfig::from(StressArgs::parse());
    let (snapshot_tx, snapshot_rx) = watch::channel(StressSnapshot::new(&config));
    let cancelled = Arc::new(AtomicBool::new(false));
    let stress_task = tokio::spawn(run_stress(config, snapshot_tx, Arc::clone(&cancelled)));
    let mut terminal = ratatui::init();
    let ui_result = run_audio_dashboard(&mut terminal, snapshot_rx, Arc::clone(&cancelled));
    ratatui::restore();
    cancelled.store(true, Ordering::Relaxed);
    let stress_result = stress_task.await.context("audio stress task panicked")?;
    let final_snapshot = ui_result?;
    print_final_summary(&final_snapshot);
    stress_result
}

/// Creates isolated full-duplex sessions and executes the selected workload model.
async fn run_stress(
    config: StressConfig,
    snapshots: watch::Sender<StressSnapshot>,
    cancelled: Arc<AtomicBool>,
) -> Result<()> {
    let registry = AudioSessionRegistry::default();
    let now = Instant::now();
    let mut metrics = StressMetrics::new(&config, now);
    let mut sessions = Vec::with_capacity(config.session_count);
    for session_index in 0..config.session_count {
        let public_session_id = format!("stress-session-{session_index}");
        let (_, receiver_a) = registry.connect(&public_session_id, "A").await;
        let (_, receiver_b) = registry.connect(&public_session_id, "B").await;
        sessions.push(RoomHarness {
            public_session_id,
            receiver_a,
            receiver_b,
            expected_a: VecDeque::with_capacity(AUDIO_OUTBOUND_QUEUE_CAPACITY),
            expected_b: VecDeque::with_capacity(AUDIO_OUTBOUND_QUEUE_CAPACITY),
            tts_remaining: 0,
        });
    }

    let started_at = Instant::now();
    let mut next_metrics_at = started_at;
    let mut next_tts_at = started_at + config.tts_interval;
    let mut tts_burst_index = 0u64;
    while started_at.elapsed() < config.duration && !cancelled.load(Ordering::Relaxed) {
        if config.mode.is_paced() {
            pace_round(&config, started_at, metrics.snapshot.rounds, &mut metrics).await;
        }
        let now = Instant::now();
        if config.tts_session_percent > 0 && now >= next_tts_at {
            start_tts_burst(&config, &mut sessions, tts_burst_index, &mut metrics);
            tts_burst_index += 1;
            next_tts_at += config.tts_interval;
        }

        let drops_before_round = metrics.snapshot.dropped_frames;
        send_round(&registry, &mut sessions, &mut metrics).await;
        if config.mode != StressMode::Impaired
            && metrics.snapshot.dropped_frames > drops_before_round
        {
            return finish_with_failure(
                &snapshots,
                &mut metrics,
                started_at,
                "bounded audio queue dropped a frame outside impaired mode".to_string(),
            );
        }
        receive_round(&config, &mut sessions, &snapshots, &mut metrics, started_at).await?;
        metrics.snapshot.rounds += 1;
        let now = Instant::now();
        metrics.update_rate(now);
        if now >= next_metrics_at {
            metrics.refresh_summary(started_at, now);
            let _ = snapshots.send(metrics.snapshot.clone());
            next_metrics_at = now + METRICS_TICK;
        }
    }

    drain_all_rooms(&config, &mut sessions, &snapshots, &mut metrics, started_at).await?;
    metrics.snapshot.cancelled = cancelled.load(Ordering::Relaxed);
    metrics.snapshot.finished = true;
    metrics.refresh_summary(started_at, Instant::now());
    metrics.push_milestone(if metrics.snapshot.cancelled {
        "Stopped by user".to_string()
    } else {
        "Completed configured duration with all fatal invariants healthy".to_string()
    });
    let _ = snapshots.send(metrics.snapshot.clone());
    Ok(())
}

/// Sleeps to an absolute 20 ms deadline and records scheduler lateness.
async fn pace_round(
    config: &StressConfig,
    started_at: Instant,
    round: u64,
    metrics: &mut StressMetrics,
) {
    let base = started_at + FRAME_INTERVAL.mul_f64(round as f64);
    let deadline = if config.mode == StressMode::Impaired {
        let jitter_ms = deterministic_jitter_ms(round);
        if jitter_ms >= 0 {
            base + Duration::from_millis(jitter_ms as u64)
        } else {
            base - Duration::from_millis((-jitter_ms) as u64)
        }
    } else {
        base
    };
    tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)).await;
    let lag = Instant::now().saturating_duration_since(deadline);
    let lag_us = lag.as_micros().min(u128::from(u64::MAX)) as u64;
    metrics.snapshot.max_schedule_lag_us = metrics.snapshot.max_schedule_lag_us.max(lag_us);
    if lag > Duration::from_millis(2) {
        metrics.snapshot.deadline_misses += 1;
    }
}

/// Selects deterministic sessions for a ten-frame agent-TTS burst.
fn start_tts_burst(
    config: &StressConfig,
    sessions: &mut [RoomHarness],
    burst_index: u64,
    metrics: &mut StressMetrics,
) {
    let mut selected = 0usize;
    for (session_index, session) in sessions.iter_mut().enumerate() {
        let selector = (session_index as u64 * 37 + burst_index * 17) % 100;
        if selector < u64::from(config.tts_session_percent) {
            session.tts_remaining = TTS_BURST_FRAMES;
            selected += 1;
        }
    }
    metrics.push_milestone(format!(
        "Started 200 ms agent-TTS burst in {selected} sessions"
    ));
}

/// Sends one participant frame in each direction and active agent frames to both peers.
async fn send_round(
    registry: &AudioSessionRegistry,
    sessions: &mut [RoomHarness],
    metrics: &mut StressMetrics,
) {
    let round = metrics.snapshot.rounds;
    for (session_index, session) in sessions.iter_mut().enumerate() {
        let now = Instant::now();
        let from_a = stress_frame(session_index, round, FrameSource::A);
        let from_b = stress_frame(session_index, round, FrameSource::B);
        registry
            .relay_partner(&session.public_session_id, "A", from_a.clone())
            .await;
        registry
            .relay_partner(&session.public_session_id, "B", from_b.clone())
            .await;
        enqueue_expected(
            &mut session.expected_b,
            from_a,
            now,
            FrameSource::A,
            session_index,
            metrics,
        );
        enqueue_expected(
            &mut session.expected_a,
            from_b,
            now,
            FrameSource::B,
            session_index,
            metrics,
        );
        if session.tts_remaining > 0 {
            let agent = stress_frame(session_index, round, FrameSource::Agent);
            registry
                .publish_agent(&session.public_session_id, agent.clone())
                .await;
            enqueue_expected(
                &mut session.expected_a,
                agent.clone(),
                now,
                FrameSource::Agent,
                session_index,
                metrics,
            );
            enqueue_expected(
                &mut session.expected_b,
                agent,
                now,
                FrameSource::Agent,
                session_index,
                metrics,
            );
            session.tts_remaining -= 1;
        }
        metrics.snapshot.session_pressure[session_index] =
            session.expected_a.len().max(session.expected_b.len()) as u64;
    }
}

/// Mirrors the registry's bounded-queue drop behavior in the expected-frame model.
fn enqueue_expected(
    queue: &mut VecDeque<ExpectedFrame>,
    bytes: Vec<u8>,
    sent_at: Instant,
    source: FrameSource,
    session_index: usize,
    metrics: &mut StressMetrics,
) {
    if queue.len() >= AUDIO_OUTBOUND_QUEUE_CAPACITY {
        metrics.record_drop(session_index);
        return;
    }
    queue.push_back(ExpectedFrame {
        bytes,
        sent_at,
        source,
    });
}

/// Drains every non-paused session and verifies complete frame identity and ordering.
async fn receive_round(
    config: &StressConfig,
    sessions: &mut [RoomHarness],
    snapshots: &watch::Sender<StressSnapshot>,
    metrics: &mut StressMetrics,
    started_at: Instant,
) -> Result<()> {
    let round = metrics.snapshot.rounds;
    for (session_index, session) in sessions.iter_mut().enumerate() {
        if config.mode == StressMode::Impaired && slow_session_consumer_paused(session_index, round)
        {
            metrics.snapshot.session_pressure[session_index] =
                session.expected_a.len().max(session.expected_b.len()) as u64;
            continue;
        }
        drain_room(
            config.mode,
            session_index,
            session,
            snapshots,
            metrics,
            started_at,
        )
        .await?;
    }
    Ok(())
}

/// Drains all remaining expected frames before a successful final snapshot.
async fn drain_all_rooms(
    config: &StressConfig,
    sessions: &mut [RoomHarness],
    snapshots: &watch::Sender<StressSnapshot>,
    metrics: &mut StressMetrics,
    started_at: Instant,
) -> Result<()> {
    for (session_index, session) in sessions.iter_mut().enumerate() {
        drain_room(
            config.mode,
            session_index,
            session,
            snapshots,
            metrics,
            started_at,
        )
        .await?;
    }
    Ok(())
}

/// Verifies both participant queues for one full-duplex session.
async fn drain_room(
    mode: StressMode,
    session_index: usize,
    session: &mut RoomHarness,
    snapshots: &watch::Sender<StressSnapshot>,
    metrics: &mut StressMetrics,
    started_at: Instant,
) -> Result<()> {
    while let Some(expected) = session.expected_a.pop_front() {
        verify_next(
            &session.public_session_id,
            &mut session.receiver_a,
            expected,
            session_index,
            snapshots,
            metrics,
            started_at,
        )
        .await?;
    }
    while let Some(expected) = session.expected_b.pop_front() {
        verify_next(
            &session.public_session_id,
            &mut session.receiver_b,
            expected,
            session_index,
            snapshots,
            metrics,
            started_at,
        )
        .await?;
    }
    if mode == StressMode::Impaired {
        metrics.snapshot.session_pressure[session_index] = 0;
    }
    Ok(())
}

/// Receives and verifies one expected binary frame from a session peer queue.
async fn verify_next(
    public_session_id: &str,
    receiver: &mut mpsc::Receiver<AudioOutbound>,
    expected: ExpectedFrame,
    session_index: usize,
    snapshots: &watch::Sender<StressSnapshot>,
    metrics: &mut StressMetrics,
    started_at: Instant,
) -> Result<()> {
    let outbound = match tokio::time::timeout(ROOM_TIMEOUT, receiver.recv()).await {
        Ok(Some(message)) => message,
        Ok(None) => {
            metrics.snapshot.timeouts += 1;
            return finish_with_failure(
                snapshots,
                metrics,
                started_at,
                format!("{public_session_id} queue closed unexpectedly"),
            );
        }
        Err(_) => {
            metrics.snapshot.timeouts += 1;
            return finish_with_failure(
                snapshots,
                metrics,
                started_at,
                format!("{public_session_id} exceeded receive timeout"),
            );
        }
    };
    let AudioOutbound::Binary(bytes) = outbound else {
        metrics.snapshot.unexpected_messages += 1;
        return finish_with_failure(
            snapshots,
            metrics,
            started_at,
            format!("{public_session_id} received unexpected control message"),
        );
    };
    if bytes != expected.bytes {
        metrics.snapshot.payload_errors += 1;
        return finish_with_failure(
            snapshots,
            metrics,
            started_at,
            format!("{public_session_id} received wrong session, source, sequence, or payload"),
        );
    }
    metrics.record_frame(session_index, expected.sent_at.elapsed(), expected.source);
    Ok(())
}

/// Records a fatal failure snapshot and returns a nonzero workload result.
fn finish_with_failure(
    snapshots: &watch::Sender<StressSnapshot>,
    metrics: &mut StressMetrics,
    started_at: Instant,
    failure: String,
) -> Result<()> {
    metrics.snapshot.failure = Some(failure.clone());
    metrics.snapshot.finished = true;
    metrics.refresh_summary(started_at, Instant::now());
    metrics.push_milestone(format!("FAILED: {failure}"));
    let _ = snapshots.send(metrics.snapshot.clone());
    bail!(failure)
}

/// Builds one canonical session-, round-, and source-specific PCM frame.
fn stress_frame(session_index: usize, round: u64, source: FrameSource) -> Vec<u8> {
    let mut pcm = vec![source.marker(); AUDIO_FRAME_BYTES];
    pcm[..8].copy_from_slice(&(session_index as u64).to_be_bytes());
    pcm[8..16].copy_from_slice(&round.to_be_bytes());
    pcm[16] = source.marker();
    AudioFrame {
        sequence: round as u32,
        timestamp_ms: round.saturating_mul(20),
        pcm,
    }
    .encode()
}

/// Returns deterministic ±2 ms scheduler jitter for impaired mode.
fn deterministic_jitter_ms(round: u64) -> i64 {
    ((round.wrapping_mul(17).wrapping_add(3) % 5) as i64) - 2
}

/// Selects five percent of sessions for an 80-frame consumer pause every 15 seconds.
fn slow_session_consumer_paused(session_index: usize, round: u64) -> bool {
    session_index % 100 < IMPAIRED_SLOW_SESSIONS_PERCENT
        && round % IMPAIRED_CYCLE_ROUNDS < IMPAIRED_PAUSE_ROUNDS
}

/// Drives terminal redraws and keyboard handling until completion or cancellation.
fn run_audio_dashboard(
    terminal: &mut DefaultTerminal,
    mut snapshots: watch::Receiver<StressSnapshot>,
    cancelled: Arc<AtomicBool>,
) -> Result<StressSnapshot> {
    run_shared_dashboard(
        terminal,
        &mut snapshots,
        cancelled,
        Arc::new(AtomicUsize::new(0)),
        audio_dashboard_snapshot,
    )?;
    Ok(snapshots.borrow().clone())
}

/// Converts audio-workload measurements into the common stress-dashboard model.
fn audio_dashboard_snapshot(snapshot: &StressSnapshot) -> DashboardSnapshot {
    let error = |count| {
        if count == 0 {
            DashboardHealth::Good
        } else {
            DashboardHealth::Error
        }
    };
    let warning = |count| {
        if count == 0 {
            DashboardHealth::Good
        } else {
            DashboardHealth::Warning
        }
    };
    let metric = |label: &str, value: String, health| DashboardMetric {
        label: label.to_string(),
        value,
        health,
    };
    DashboardSnapshot {
        title: "Parlando audio stress".to_string(),
        mode: snapshot.mode.label().to_string(),
        phase: None,
        elapsed: snapshot.elapsed,
        duration: snapshot.duration,
        finished: snapshot.finished,
        cancelled: snapshot.cancelled,
        failure: snapshot.failure.clone(),
        panels: [
            DashboardPanel {
                title: " Workload ".to_string(),
                metrics: vec![
                    metric(
                        "Rooms / streams",
                        format!(
                            "{} / {}",
                            snapshot.session_count,
                            snapshot.session_count * 2
                        ),
                        DashboardHealth::Neutral,
                    ),
                    metric(
                        "Participant frames",
                        format_count(snapshot.participant_frames),
                        DashboardHealth::Good,
                    ),
                    metric(
                        "Agent TTS frames",
                        format_count(snapshot.tts_frames),
                        DashboardHealth::Good,
                    ),
                    metric(
                        "Verified frames/s",
                        format_count(snapshot.current_rate),
                        DashboardHealth::Neutral,
                    ),
                    metric(
                        "Expected base/s",
                        if snapshot.expected_rate == 0 {
                            "unpaced".into()
                        } else {
                            format_count(snapshot.expected_rate)
                        },
                        DashboardHealth::Neutral,
                    ),
                ],
            },
            DashboardPanel {
                title: " Cadence & in-memory latency ".to_string(),
                metrics: vec![
                    metric(
                        "p50 queue latency",
                        format_latency(snapshot.p50_us),
                        DashboardHealth::Good,
                    ),
                    metric(
                        "p95 queue latency",
                        format_latency(snapshot.p95_us),
                        DashboardHealth::Warning,
                    ),
                    metric(
                        "p99 queue latency",
                        format_latency(snapshot.p99_us),
                        DashboardHealth::Warning,
                    ),
                    metric(
                        "Maximum latency",
                        format_latency(snapshot.max_us),
                        DashboardHealth::Warning,
                    ),
                    metric(
                        "Deadline misses",
                        format_count(snapshot.deadline_misses),
                        warning(snapshot.deadline_misses),
                    ),
                    metric(
                        "Max schedule lag",
                        format_latency(snapshot.max_schedule_lag_us),
                        DashboardHealth::Warning,
                    ),
                ],
            },
            DashboardPanel {
                title: " Verification ".to_string(),
                metrics: vec![
                    metric(
                        "Wrong session/payload",
                        format_count(snapshot.payload_errors),
                        error(snapshot.payload_errors),
                    ),
                    metric(
                        "Timeouts",
                        format_count(snapshot.timeouts),
                        error(snapshot.timeouts),
                    ),
                    metric(
                        "Unexpected control",
                        format_count(snapshot.unexpected_messages),
                        error(snapshot.unexpected_messages),
                    ),
                    metric(
                        "Queue drops",
                        format_count(snapshot.dropped_frames),
                        warning(snapshot.dropped_frames),
                    ),
                    metric(
                        "Fatal failures",
                        format_count(snapshot.fatal_error_count()),
                        error(snapshot.fatal_error_count()),
                    ),
                ],
            },
        ],
        series: vec![DashboardSeries {
            title: format!(
                " Verified frame throughput — last {}s ",
                snapshot.rate_history.len()
            ),
            values: snapshot.rate_history.clone(),
        }],
        tiles: snapshot
            .session_pressure
            .iter()
            .map(|value| DashboardTile {
                health: if snapshot.mode == StressMode::Impaired
                    && *value >= AUDIO_OUTBOUND_QUEUE_CAPACITY as u64
                {
                    DashboardHealth::Warning
                } else if *value > snapshot.p99_us {
                    DashboardHealth::Error
                } else if *value > snapshot.p50_us {
                    DashboardHealth::Warning
                } else {
                    DashboardHealth::Good
                },
            })
            .collect(),
        tile_legend: if snapshot.mode == StressMode::Impaired {
            "queue pressure: green low · yellow building · red full".to_string()
        } else {
            "latency: green ≤ p50 · yellow ≤ p95 · red > p99".to_string()
        },
        events: snapshot.milestones.clone(),
        footer: "realistic = full duplex at 20 ms/frame".to_string(),
    }
}

/// Prints a durable summary after the alternate terminal screen is restored.
fn print_final_summary(snapshot: &StressSnapshot) {
    let outcome = if snapshot.failure.is_some() {
        "FAILED"
    } else if snapshot.cancelled {
        "STOPPED"
    } else {
        "PASSED"
    };
    eprintln!(
        "Audio stress {outcome} ({}): {} sessions, {} verified frames in {}; average {} frames/s, p95 {}, drops {}, deadline misses {}, fatal failures {}.",
        snapshot.mode.label(), snapshot.session_count, format_count(snapshot.verified_frames),
        format_clock(snapshot.elapsed), format_count(snapshot.average_rate), format_latency(snapshot.p95_us),
        format_count(snapshot.dropped_frames), format_count(snapshot.deadline_misses), snapshot.fatal_error_count()
    );
    if let Some(failure) = &snapshot.failure {
        eprintln!("Failure: {failure}");
    }
}

/// Returns one nearest-rank percentile from sorted samples.
fn percentile(sorted: &[u64], fraction: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    sorted[((sorted.len() - 1) as f64 * fraction).round() as usize]
}

/// Formats seconds as hours, minutes, and seconds.
fn format_clock(duration: Duration) -> String {
    let seconds = duration.as_secs();
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3_600,
        seconds % 3_600 / 60,
        seconds % 60
    )
}

/// Formats a large counter in three-digit groups.
fn format_count(value: u64) -> String {
    let digits = value.to_string();
    let mut formatted = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, character) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            formatted.push(',');
        }
        formatted.push(character);
    }
    formatted
}

/// Formats microseconds as microseconds or milliseconds.
fn format_latency(microseconds: u64) -> String {
    if microseconds < 1_000 {
        format!("{microseconds} µs")
    } else {
        format!("{:.2} ms", microseconds as f64 / 1_000.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies modes and workload sizing are explicit validated CLI arguments.
    #[test]
    fn cli_parses_modes_and_rejects_invalid_workload_sizes() {
        let args = StressArgs::try_parse_from([
            "audio-stress-tui",
            "--mode",
            "impaired",
            "--sessions",
            "42",
            "--seconds",
            "30",
            "--tts-session-percent",
            "25",
            "--tts-interval-seconds",
            "3",
        ])
        .expect("valid stress arguments should parse");
        assert_eq!(args.mode, StressMode::Impaired);
        assert_eq!(args.sessions, 42);
        assert_eq!(args.seconds, 30);
        assert_eq!(args.tts_session_percent, 25);
        assert_eq!(args.tts_interval_seconds, 3);

        assert!(StressArgs::try_parse_from(["audio-stress-tui", "--sessions", "0"]).is_err());
        assert!(
            StressArgs::try_parse_from(["audio-stress-tui", "--tts-session-percent", "101"])
                .is_err()
        );
    }

    /// Verifies session, round, and source identities remain distinguishable.
    #[test]
    fn stress_frames_are_canonical_and_distinguishable() {
        let a = stress_frame(7, 11, FrameSource::A);
        assert_ne!(a, stress_frame(7, 11, FrameSource::B));
        assert_ne!(a, stress_frame(8, 11, FrameSource::A));
        assert_ne!(a, stress_frame(7, 12, FrameSource::A));
        assert!(AudioFrame::decode(&a).is_ok());
    }

    /// Verifies deterministic impairment and core dashboard helpers.
    #[test]
    fn impairment_and_dashboard_helpers_are_deterministic() {
        assert_eq!(
            (0..5).map(deterministic_jitter_ms).collect::<Vec<_>>(),
            vec![1, -2, 0, 2, -1]
        );
        assert!(slow_session_consumer_paused(0, 10));
        assert!(!slow_session_consumer_paused(7, 10));
        assert!(!slow_session_consumer_paused(0, 100));
        assert_eq!(percentile(&[10, 20, 30, 40, 50], 0.95), 50);
        assert_eq!(format_count(12_345_678), "12,345,678");
    }
}
