//! Full-stack loopback stress tool for Parlando's participant runtime.
//!
//! The tool deliberately uses public HTTP and WebSocket endpoints with a
//! temporary SQLite database. It is an optional developer/release tool, not an
//! ordinary integration test.

use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use clap::{Parser, ValueEnum};
use futures_util::{SinkExt, StreamExt};
use parlando::{
    agent::{
        Agent, Context as AgentContext, Definition as AgentDefinition, Factory as AgentFactory,
        Identity as AgentIdentity, Response as AgentResponse,
    },
    test_support::{
        build_router, run_dashboard, AgentsConfig, AgentsMode, AudioChunk, AudioFrame,
        ConsentItemConfig, DashboardHealth, DashboardMetric, DashboardPanel, DashboardSeries,
        DashboardSnapshot, DashboardTile, DatabaseConfig, DirectConfig, ExperimentConfig,
        ExperimentIdentityConfig, FinalTranscriptUtterance, HumanVsAgentConfig, ServeOptions,
        SpeechmaticsConfig, StreamingTtsProvider, TranscriptionConfig, TranscriptionEvent,
        TranscriptionInput, TranscriptionProvider, TranscriptionSessionContext,
        TranscriptionSessionHandle, TtsConfig, VoiceConfig,
    },
    ActionRejection, Game, GameInitializationContext, PlayerRole,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::{
    net::TcpListener,
    sync::{mpsc, watch},
    time,
};
use tokio_tungstenite::{connect_async, tungstenite::Message};

const FRAME_INTERVAL: Duration = Duration::from_millis(20);
const METRIC_INTERVAL: Duration = Duration::from_millis(250);
/// Allows an admitted room to finish server-side initialization under high concurrent load.
const GAME_SETUP_TIMEOUT: Duration = Duration::from_secs(30);
/// Bounds live audio relay validation without hiding an overloaded audio path.
const AUDIO_FRAME_TIMEOUT: Duration = Duration::from_secs(2);
/// Incremented whenever the runtime-stress harness behavior changes.
const HARNESS_REVISION: u32 = 4;

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Command-line configuration for a real loopback runtime stress run.
#[derive(Clone, Debug, Parser)]
#[command(about = "Stress Parlando's live game and audio WebSocket runtime")]
struct Args {
    /// Named duration and scale preset.
    #[arg(long, value_enum, default_value_t = Preset::Acceptance)]
    preset: Preset,
    /// Pairing workload to execute.
    #[arg(long, value_enum, default_value_t = Pairing::HumanAgent)]
    pairing: Pairing,
    /// Concurrent game sessions after ramp-up.
    #[arg(long)]
    sessions: Option<usize>,
    /// Total wall-clock duration, including ramp and drain.
    #[arg(long)]
    seconds: Option<u64>,
    /// Disable the shared interactive Ratatui dashboard.
    #[arg(long)]
    no_tui: bool,
    /// Preserve the temporary SQLite directory after a healthy run.
    #[arg(long)]
    keep_database: bool,
    /// Machine-readable report path.
    #[arg(long, default_value = "runtime-stress-report.json")]
    report: PathBuf,
}

/// Named stress scales with the common ramp, steady, churn, and drain phases.
#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum Preset {
    /// One-minute, twenty-session developer run.
    Smoke,
    /// Ten-minute, two-hundred-session release run.
    Acceptance,
    /// Thirty-minute stability run.
    Soak,
}

/// Live pairing mode exercised by one stress server.
#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum Pairing {
    /// Two browser-like human participants per room.
    HumanHuman,
    /// One browser-like human plus one session-local agent per room.
    HumanAgent,
}

/// One behaviorally distinct interval in every stress run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    Ramp,
    Steady,
    Churn,
    Drain,
}

impl Phase {
    /// Maps elapsed time to the fixed 1:4:3:2 phase proportions.
    fn at(elapsed: Duration, duration: Duration) -> Self {
        let ratio = elapsed.as_secs_f64() / duration.as_secs_f64().max(1.0);
        match ratio {
            value if value < 0.1 => Self::Ramp,
            value if value < 0.5 => Self::Steady,
            value if value < 0.8 => Self::Churn,
            _ => Self::Drain,
        }
    }

    /// Returns the stable report and dashboard label.
    fn as_str(self) -> &'static str {
        match self {
            Self::Ramp => "ramp",
            Self::Steady => "steady",
            Self::Churn => "churn",
            Self::Drain => "drain",
        }
    }
}

/// Resolved immutable workload configuration recorded in reports.
#[derive(Clone, Debug, Serialize)]
struct RunConfig {
    harness_revision: u32,
    sessions: usize,
    seconds: u64,
    pairing: String,
    report: PathBuf,
}

impl Args {
    /// Resolves preset defaults while retaining explicit scale overrides.
    fn resolve(&self) -> RunConfig {
        let (sessions, seconds) = match self.preset {
            Preset::Smoke => (20, 60),
            Preset::Acceptance => (200, 600),
            Preset::Soak => (200, 1_800),
        };
        RunConfig {
            harness_revision: HARNESS_REVISION,
            sessions: self.sessions.unwrap_or(sessions),
            seconds: self.seconds.unwrap_or(seconds),
            pairing: match self.pairing {
                Pairing::HumanHuman => "human-human",
                Pairing::HumanAgent => "human-agent",
            }
            .to_string(),
            report: self.report.clone(),
        }
    }
}

/// A tiny deterministic game that keeps transitions cheap while exercising storage.
#[derive(Clone)]
struct StressGame;

/// Authoritative state for the stress fixture game.
#[derive(Clone, Debug, Deserialize, Serialize)]
struct StressState {
    actions: u64,
    complete: bool,
}

/// Public action accepted by the stress fixture game.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum StressAction {
    /// Advances the session and optionally completes it.
    Mark { sequence: u64, finish: bool },
}

/// Role-safe observation returned by the fixture game.
#[derive(Clone, Debug, Serialize)]
struct StressObservation {
    role: String,
    actions: u64,
    complete: bool,
}

/// Terminal result returned when the fixture game completes.
#[derive(Clone, Debug, Serialize)]
struct StressCompletion {
    actions: u64,
}

impl Game for StressGame {
    type Config = Value;
    type State = StressState;
    type Action = StressAction;
    type Observation = StressObservation;
    type Completion = StressCompletion;

    /// Creates an empty fixture state for each admitted session.
    fn initial_state(
        &self,
        _context: GameInitializationContext<'_, Self::Config>,
    ) -> Result<Self::State> {
        Ok(StressState {
            actions: 0,
            complete: false,
        })
    }

    /// Applies one monotonic fixture action while preserving terminal exclusion.
    fn apply_action(
        &self,
        state: &Self::State,
        action: &Self::Action,
        _actor: PlayerRole,
    ) -> std::result::Result<Self::State, ActionRejection> {
        if state.complete {
            return Err(ActionRejection::new("session_complete"));
        }
        let StressAction::Mark { finish, .. } = action;
        Ok(StressState {
            actions: state.actions + 1,
            complete: *finish,
        })
    }

    /// Exposes only the shared counter and recipient role to a participant.
    fn observation(&self, state: &Self::State, player: PlayerRole) -> Self::Observation {
        StressObservation {
            role: player.as_str().to_string(),
            actions: state.actions,
            complete: state.complete,
        }
    }

    /// Publishes the legal nonterminal and terminal fixture actions.
    fn available_actions(
        &self,
        _state: &Self::State,
        _player: PlayerRole,
    ) -> Option<Vec<Self::Action>> {
        Some(vec![
            StressAction::Mark {
                sequence: 0,
                finish: false,
            },
            StressAction::Mark {
                sequence: 0,
                finish: true,
            },
        ])
    }

    /// Converts the fixture terminal flag into a compact completion record.
    fn completion(&self, state: &Self::State) -> Option<Self::Completion> {
        state.complete.then_some(StressCompletion {
            actions: state.actions,
        })
    }
}

/// One participant credential returned by the public participant endpoint.
struct Participant {
    credential: String,
}

/// Owns the ephemeral server task and the temporary SQLite directory.
struct TestServer {
    base_url: String,
    ws_base_url: String,
    database_dir: TempDir,
    task: tokio::task::JoinHandle<()>,
}

/// Bounded counters collected without retaining participant content.
#[derive(Default)]
struct Counters {
    sessions_started: AtomicU64,
    sessions_finished: AtomicU64,
    game_messages: AtomicU64,
    heartbeats: AtomicU64,
    actions: AtomicU64,
    audio_frames: AtomicU64,
    audio_verified: AtomicU64,
    reconnects: AtomicU64,
    transcripts: AtomicU64,
    tts_chunks: AtomicU64,
    startup_latency_ms_total: AtomicU64,
    startup_latency_ms_max: AtomicU64,
    audio_deadline_misses: AtomicU64,
    max_audio_lag_us: AtomicU64,
    failures: AtomicU64,
    first_failure: Mutex<Option<String>>,
}

impl Counters {
    /// Records one failed virtual session and preserves the first diagnostic verbatim.
    fn record_failure(&self, session: usize, error: &anyhow::Error) {
        self.failures.fetch_add(1, Ordering::Relaxed);
        let mut first = self
            .first_failure
            .lock()
            .expect("stress failure diagnostic mutex poisoned");
        if first.is_none() {
            *first = Some(format!("session {session}: {error:#}"));
        }
    }

    /// Returns the first virtual-session failure for live and durable reporting.
    fn first_failure(&self) -> Option<String> {
        self.first_failure
            .lock()
            .expect("stress failure diagnostic mutex poisoned")
            .clone()
    }
}

/// Dashboard state sent periodically from the workload to the shared TUI.
#[derive(Clone, Debug)]
struct Snapshot {
    phase: Phase,
    elapsed: Duration,
    duration: Duration,
    target_sessions: usize,
    active_sessions: usize,
    sessions_started: u64,
    sessions_finished: u64,
    game_messages: u64,
    heartbeats: u64,
    actions: u64,
    audio_frames: u64,
    audio_verified: u64,
    reconnects: u64,
    transcripts: u64,
    tts_chunks: u64,
    average_startup_ms: u64,
    maximum_startup_ms: u64,
    audio_deadline_misses: u64,
    max_audio_lag_us: u64,
    failures: u64,
    rate_history: Vec<u64>,
    events: Vec<String>,
    finished: bool,
    cancelled: bool,
    failure: Option<String>,
}

/// Machine-readable result and SQLite-volume summary.
#[derive(Serialize)]
struct StressReport<'a> {
    config: &'a RunConfig,
    success: bool,
    error: Option<String>,
    counters: ReportCounters,
    database: DatabaseReport,
    capacity: CapacityReport,
}

/// Stable counter projection used by the JSON report.
#[derive(Serialize)]
struct ReportCounters {
    sessions_started: u64,
    sessions_finished: u64,
    game_messages: u64,
    heartbeats: u64,
    actions: u64,
    audio_frames: u64,
    audio_verified: u64,
    reconnects: u64,
    transcripts: u64,
    tts_chunks: u64,
    average_startup_ms: u64,
    maximum_startup_ms: u64,
    audio_deadline_misses: u64,
    max_audio_lag_us: u64,
    failures: u64,
}

/// File and row counts needed to estimate storage per hosted game.
#[derive(Serialize)]
struct DatabaseReport {
    pre_checkpoint_sqlite_bytes: u64,
    pre_checkpoint_wal_bytes: u64,
    pre_checkpoint_shm_bytes: u64,
    checkpointed_sqlite_bytes: u64,
    participants: i64,
    sessions: i64,
    session_events: i64,
    transcript_events: i64,
    checkpointed_bytes_per_started_session: f64,
}

/// Conservative interpretation of one finite target run.
#[derive(Serialize)]
struct CapacityReport {
    healthy_concurrent_games_lower_bound: u64,
    target_human_websockets: usize,
    offered_audio_frames_per_second: usize,
    interpretation: &'static str,
}

/// Deterministic local agent that answers human activity through the real runtime.
struct StressAgent {
    respond_next: bool,
    sequence: u64,
}

#[async_trait]
impl Agent<StressGame> for StressAgent {
    /// Allows one initial response after the session starts.
    async fn start(&mut self, _observation: StressObservation) -> Result<()> {
        self.respond_next = true;
        Ok(())
    }

    /// Schedules a response only when the human produced the accepted action.
    async fn observe_transition(
        &mut self,
        actor: PlayerRole,
        _action: StressAction,
        _observation: StressObservation,
    ) -> Result<()> {
        self.respond_next = actor == PlayerRole::A;
        Ok(())
    }

    /// Schedules a response only to the human's message.
    async fn observe_message(&mut self, sender: PlayerRole, _text: String) -> Result<()> {
        self.respond_next = sender == PlayerRole::A;
        Ok(())
    }

    /// Emits one combined legal action and spoken message per scheduled turn.
    async fn respond(
        &mut self,
        _available_actions: Option<Vec<StressAction>>,
    ) -> Result<Option<AgentResponse<StressAction>>> {
        if !self.respond_next {
            return Ok(None);
        }
        self.respond_next = false;
        self.sequence += 1;
        Ok(Some(AgentResponse::action_and_message(
            StressAction::Mark {
                sequence: self.sequence,
                finish: false,
            },
            format!("agent-stress:{}", self.sequence),
        )))
    }
}

/// Factory that gives each room an isolated deterministic agent.
struct StressAgentFactory;

#[async_trait]
impl AgentFactory<StressGame> for StressAgentFactory {
    /// Describes the local synthetic agent in stored provenance.
    fn definition(&self) -> AgentDefinition {
        AgentDefinition {
            id: "runtime-stress".into(),
            name: "Runtime stress agent".into(),
            description: "Deterministic local load-test agent".into(),
            config_fields: vec![],
        }
    }

    /// Creates a fresh agent for one role-B participant.
    async fn create(&self, _context: AgentContext) -> Result<Box<dyn Agent<StressGame> + Send>> {
        Ok(Box::new(StressAgent {
            respond_next: false,
            sequence: 0,
        }))
    }

    /// Records a stable identity without external model credentials.
    fn identity(&self, _settings: &Value) -> Result<AgentIdentity> {
        Ok(AgentIdentity {
            name: "runtime-stress-agent".into(),
            version: "1".into(),
        })
    }
}

/// Credential-free ASR peer that emits deterministic final utterances.
struct LocalTranscription {
    counters: Arc<Counters>,
}

#[async_trait]
impl TranscriptionProvider for LocalTranscription {
    /// Starts a bounded provider session with realistic asynchronous delivery.
    async fn start_session(
        &self,
        context: TranscriptionSessionContext,
    ) -> Result<TranscriptionSessionHandle> {
        let (input, mut inputs) = mpsc::channel(250);
        let (events, event_receiver) = mpsc::channel(64);
        let counters = self.counters.clone();
        tokio::spawn(async move {
            if events.send(TranscriptionEvent::Ready).await.is_err() {
                return;
            }
            let mut frames = 0u64;
            while let Some(input) = inputs.recv().await {
                match input {
                    TranscriptionInput::Audio(_) => {
                        frames += 1;
                        if frames % 100 == 0 {
                            time::sleep(Duration::from_millis(35)).await;
                            let utterance = FinalTranscriptUtterance {
                                start_time_ms: (frames as i64 - 100) * 20,
                                end_time_ms: frames as i64 * 20,
                                text: format!("synthetic {} utterance", context.role),
                                result_ids: vec![format!("{}-{frames}", context.role)],
                            };
                            if events
                                .send(TranscriptionEvent::FinalUtterance(utterance))
                                .await
                                .is_err()
                            {
                                break;
                            }
                            counters.transcripts.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    TranscriptionInput::Finish => break,
                }
            }
        });
        Ok(TranscriptionSessionHandle {
            input,
            events: event_receiver,
        })
    }
}

/// Credential-free TTS peer that produces canonical 24 kHz mono PCM.
struct LocalTts {
    counters: Arc<Counters>,
}

#[async_trait]
impl StreamingTtsProvider for LocalTts {
    /// Synthesizes five 20 ms frames after a small provider-like delay.
    async fn synthesize(&self, text: &str, _message_id: &str) -> Result<Vec<AudioChunk>> {
        time::sleep(Duration::from_millis(40)).await;
        self.counters.tts_chunks.fetch_add(1, Ordering::Relaxed);
        Ok(vec![AudioChunk {
            data: vec![text.len() as u8; 4_800],
            sample_rate: 24_000,
            channels: 1,
        }])
    }
}

/// Starts the loopback server, workload, and optional shared TUI.
#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let run = args.resolve();
    anyhow::ensure!(run.sessions > 0, "--sessions must be positive");
    anyhow::ensure!(run.seconds >= 10, "--seconds must be at least ten");
    let counters = Arc::new(Counters::default());
    let final_counters = counters.clone();
    let server = spawn_server(args.pairing, counters.clone()).await?;
    let cancelled = Arc::new(AtomicBool::new(false));
    let initial = empty_snapshot(&run);
    let (snapshots, receiver) = watch::channel(initial);
    let workload = tokio::spawn(run_workload(
        run.clone(),
        args.pairing,
        server.base_url.clone(),
        server.ws_base_url.clone(),
        counters,
        snapshots,
        cancelled.clone(),
    ));
    let ui_result = if args.no_tui {
        Ok(())
    } else {
        let ui_cancelled = cancelled.clone();
        let ui_thread = std::thread::spawn(move || {
            let mut terminal = ratatui::init();
            let mut receiver = receiver;
            let result = run_dashboard(
                &mut terminal,
                &mut receiver,
                ui_cancelled,
                Arc::new(AtomicUsize::new(0)),
                dashboard_snapshot,
            );
            ratatui::restore();
            result
        });
        tokio::task::spawn_blocking(move || ui_thread.join())
            .await
            .context("runtime stress dashboard join task panicked")?
            .map_err(|_| anyhow!("runtime stress dashboard thread panicked"))?
    };
    if !args.no_tui {
        cancelled.store(true, Ordering::Relaxed);
    }
    let result = workload.await.context("runtime stress task panicked")?;
    server.task.abort();
    let database = server.database_dir.path().join("runtime-stress.sqlite");
    let report_result = write_report(&run, &database, &final_counters, result.as_ref().err()).await;
    eprintln!("Runtime stress database: {}", database.display());
    if args.keep_database || result.is_err() || report_result.is_err() {
        let kept = server.database_dir.keep();
        eprintln!("Preserved temporary database directory: {}", kept.display());
    }
    ui_result?;
    report_result?;
    eprintln!(
        "Runtime stress r{}: {} sessions started, {} finished, {} game messages, {} actions, {} PCM frames verified, {} failures.",
        HARNESS_REVISION,
        final_counters.sessions_started.load(Ordering::Relaxed),
        final_counters.sessions_finished.load(Ordering::Relaxed),
        final_counters.game_messages.load(Ordering::Relaxed),
        final_counters.actions.load(Ordering::Relaxed),
        final_counters.audio_verified.load(Ordering::Relaxed),
        final_counters.failures.load(Ordering::Relaxed),
    );
    result
}

/// Creates a production router backed by a real temporary SQLite database.
async fn spawn_server(pairing: Pairing, counters: Arc<Counters>) -> Result<TestServer> {
    let database_dir = TempDir::new()?;
    let database = database_dir.path().join("runtime-stress.sqlite");
    let mut config = ExperimentConfig {
        experiment: ExperimentIdentityConfig {
            id: Some("runtime-stress".to_string()),
        },
        database: DatabaseConfig {
            url: format!("sqlite:///{}", database.display()),
        },
        direct: DirectConfig {
            consents: vec![ConsentItemConfig {
                id: "stress".to_string(),
                title: "Stress consent".to_string(),
                body: "Synthetic stress participant".to_string(),
                required: true,
            }],
            ..DirectConfig::default()
        },
        voice: VoiceConfig {
            enabled: true,
            ..VoiceConfig::default()
        },
        speechmatics: SpeechmaticsConfig::default(),
        transcription: TranscriptionConfig {
            enabled: true,
            ..TranscriptionConfig::default()
        },
        tts: TtsConfig {
            enabled: pairing == Pairing::HumanAgent,
            api_key: "local-stress-key".into(),
            voice_id: "local-stress-voice".into(),
            ..TtsConfig::default()
        },
        agents: AgentsConfig {
            mode: match pairing {
                Pairing::HumanHuman => AgentsMode::HumanVsHuman,
                Pairing::HumanAgent => AgentsMode::HumanVsAgent,
            },
            human_vs_agent: (pairing == Pairing::HumanAgent).then_some(HumanVsAgentConfig {
                factory: Some("runtime-stress".into()),
                ..HumanVsAgentConfig::default()
            }),
            ..AgentsConfig::default()
        },
        ..ExperimentConfig::default()
    };
    config.capacity.max_active_sessions = 2_000;
    config.capacity.max_waiting_sessions = 2_000;
    config.capacity.max_unattached_participants = 4_000;
    // The local provider intentionally exercises browser-style ASR admission for every
    // synthetic human. Keep this stress-only reservation above the largest supported
    // workload so the requested session count, rather than the production default of
    // 32 streams, determines the test scale.
    config.capacity.max_transcription_streams = 4_000;
    let options = ServeOptions {
        agent_factory: (pairing == Pairing::HumanAgent)
            .then(|| Arc::new(StressAgentFactory) as Arc<dyn AgentFactory<StressGame>>),
        agent_definitions: (pairing == Pairing::HumanAgent)
            .then(|| StressAgentFactory.definition())
            .into_iter()
            .collect(),
        tts_provider: (pairing == Pairing::HumanAgent).then(|| {
            Arc::new(LocalTts {
                counters: counters.clone(),
            }) as Arc<dyn StreamingTtsProvider>
        }),
        transcription_provider: Some(Arc::new(LocalTranscription { counters })),
        ..ServeOptions::default()
    };
    let router = build_router(StressGame, config, options).await?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let task = tokio::spawn(async move {
        axum::serve(listener, router)
            .await
            .expect("stress server failed");
    });
    let server = TestServer {
        base_url: format!("http://{address}"),
        ws_base_url: format!("ws://{address}"),
        database_dir,
        task,
    };
    activate(&server.base_url).await?;
    Ok(server)
}

/// Performs the authenticated setup and activation needed for direct intake.
async fn activate(base_url: &str) -> Result<()> {
    let client = reqwest::Client::new();
    let response = client
        .post(format!("{base_url}/api/admin/setup"))
        .json(&json!({"username":"stress-admin","password":"stress-password"}))
        .send()
        .await?
        .error_for_status()?;
    let cookie = response
        .headers()
        .get(reqwest::header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .ok_or_else(|| anyhow!("missing setup cookie"))?
        .to_string();
    let csrf = response.json::<Value>().await?["csrf_token"]
        .as_str()
        .ok_or_else(|| anyhow!("missing setup csrf token"))?
        .to_string();
    client
        .post(format!("{base_url}/api/admin/experiment/status"))
        .header(reqwest::header::COOKIE, cookie)
        .header("x-csrf-token", csrf)
        .json(&json!({"status":"active"}))
        .send()
        .await?
        .error_for_status()?;
    Ok(())
}

/// Runs pairs of virtual humans through real game and audio WebSockets.
async fn run_workload(
    run: RunConfig,
    pairing: Pairing,
    base_url: String,
    ws_base_url: String,
    counters: Arc<Counters>,
    snapshots: watch::Sender<Snapshot>,
    cancelled: Arc<AtomicBool>,
) -> Result<()> {
    let started = Instant::now();
    let duration = Duration::from_secs(run.seconds);
    let deadline = started + duration;
    let ramp_duration = duration.mul_f64(0.1);
    let client = reqwest::Client::new();
    let mut tasks = Vec::new();
    let (phase_sender, phase_receiver) = watch::channel(Phase::Ramp);
    let mut history = VecDeque::new();
    let mut last_frames = 0;
    let mut next_metrics = Instant::now();
    let mut admitted = 0usize;
    let mut previous_phase = Phase::Ramp;
    let mut events = VecDeque::from(["entered ramp phase".to_string()]);
    while Instant::now() < deadline && !cancelled.load(Ordering::Relaxed) {
        let elapsed = started.elapsed();
        let phase = Phase::at(elapsed, duration);
        if phase != previous_phase {
            previous_phase = phase;
            let _ = phase_sender.send(phase);
            events.push_front(format!("entered {} phase", phase.as_str()));
            events.truncate(8);
        }
        let should_be_admitted = if phase == Phase::Ramp {
            ((elapsed.as_secs_f64() / ramp_duration.as_secs_f64().max(0.001)) * run.sessions as f64)
                .ceil() as usize
        } else {
            run.sessions
        }
        .min(run.sessions);
        while admitted < should_be_admitted {
            let task_client = client.clone();
            let task_base = base_url.clone();
            let task_ws = ws_base_url.clone();
            let task_counters = counters.clone();
            let failure_counters = counters.clone();
            let task_phase = phase_receiver.clone();
            let task_cancelled = cancelled.clone();
            let session = admitted;
            tasks.push(tokio::spawn(async move {
                let result = run_session(
                    session,
                    pairing,
                    task_client,
                    task_base,
                    task_ws,
                    deadline,
                    task_phase,
                    task_cancelled,
                    task_counters,
                )
                .await;
                if let Err(error) = &result {
                    failure_counters.record_failure(session, error);
                }
                result
            }));
            admitted += 1;
        }
        if Instant::now() >= next_metrics {
            let frames = counters.audio_frames.load(Ordering::Relaxed);
            history.push_back(frames.saturating_sub(last_frames) * 4);
            last_frames = frames;
            while history.len() > 60 {
                history.pop_front();
            }
            let _ = snapshots.send(snapshot_from(
                &run,
                phase,
                elapsed,
                counters.sessions_started.load(Ordering::Relaxed) as usize
                    - counters.sessions_finished.load(Ordering::Relaxed) as usize,
                &counters,
                history.iter().copied().collect(),
                events.iter().cloned().collect(),
                false,
                cancelled.load(Ordering::Relaxed),
                None,
            ));
            next_metrics += METRIC_INTERVAL;
        }
        time::sleep(Duration::from_millis(20)).await;
    }
    for task in tasks {
        match task.await.context("virtual session task panicked")? {
            Ok(()) => {}
            Err(error) => {
                let _ = snapshots.send(snapshot_from(
                    &run,
                    Phase::Drain,
                    started.elapsed(),
                    0,
                    &counters,
                    history.iter().copied().collect(),
                    events.iter().cloned().collect(),
                    true,
                    cancelled.load(Ordering::Relaxed),
                    Some(error.to_string()),
                ));
                return Err(error);
            }
        }
    }
    let _ = snapshots.send(snapshot_from(
        &run,
        Phase::Drain,
        started.elapsed(),
        0,
        &counters,
        history.iter().copied().collect(),
        events.iter().cloned().collect(),
        true,
        cancelled.load(Ordering::Relaxed),
        None,
    ));
    Ok(())
}

/// Executes one paired session with messages, actions, and bidirectional PCM relay.
async fn run_session(
    session: usize,
    pairing: Pairing,
    client: reqwest::Client,
    base_url: String,
    ws_base_url: String,
    deadline: Instant,
    mut phase: watch::Receiver<Phase>,
    cancelled: Arc<AtomicBool>,
    counters: Arc<Counters>,
) -> Result<()> {
    let setup_started = Instant::now();
    let participant_a = create_participant(&client, &base_url)
        .await
        .context("creating participant A")?;
    consent(&client, &base_url, &participant_a)
        .await
        .context("recording participant A consent")?;
    let room_a = create_room(&client, &base_url, &participant_a)
        .await
        .context("admitting participant A to a room")?;
    let room_id = room_a["room_id"]
        .as_str()
        .ok_or_else(|| anyhow!("missing room id"))?
        .to_string();
    let participant_b = if pairing == Pairing::HumanHuman {
        let participant = create_participant(&client, &base_url).await?;
        consent(&client, &base_url, &participant).await?;
        let _ = create_room(&client, &base_url, &participant).await?;
        Some(participant)
    } else {
        None
    };
    let mut game_a = game_socket(&client, &base_url, &ws_base_url, &room_id, &participant_a)
        .await
        .context("opening participant A game socket")?;
    let mut game_b = match &participant_b {
        Some(participant) => {
            Some(game_socket(&client, &base_url, &ws_base_url, &room_id, participant).await?)
        }
        None => None,
    };
    let mut audio_a = audio_socket(&client, &base_url, &ws_base_url, &room_id, &participant_a)
        .await
        .context("opening participant A audio socket")?;
    let mut audio_b = match &participant_b {
        Some(participant) => {
            Some(audio_socket(&client, &base_url, &ws_base_url, &room_id, participant).await?)
        }
        None => None,
    };
    read_type(&mut game_a, "session_started")
        .await
        .context("waiting for participant A session_started")?;
    send_json(&mut game_a, json!({"type":"ready"})).await?;
    if let Some(game) = &mut game_b {
        read_type(game, "session_started").await?;
        send_json(game, json!({"type":"ready"})).await?;
    }
    let startup_ms = setup_started.elapsed().as_millis() as u64;
    counters
        .startup_latency_ms_total
        .fetch_add(startup_ms, Ordering::Relaxed);
    counters
        .startup_latency_ms_max
        .fetch_max(startup_ms, Ordering::Relaxed);
    counters.sessions_started.fetch_add(1, Ordering::Relaxed);
    let mut sequence = 0u32;
    let message_interval = Duration::from_secs(8 + session as u64 % 23);
    let action_interval = Duration::from_secs(5 + (session as u64 * 7) % 16);
    let mut next_message = Instant::now() + message_interval;
    let mut next_action = Instant::now() + action_interval;
    let mut next_heartbeat = Instant::now() + Duration::from_secs(10);
    let mut reconnected = false;
    let mut next_frame = Instant::now();
    while Instant::now() < deadline {
        if cancelled.load(Ordering::Relaxed) {
            break;
        }
        let current_phase = *phase.borrow_and_update();
        if current_phase == Phase::Drain {
            send_json(
                &mut game_a,
                json!({"type":"action","action":{"type":"mark","sequence":sequence,"finish":true}}),
            )
            .await?;
            counters.actions.fetch_add(1, Ordering::Relaxed);
            break;
        }
        if current_phase == Phase::Churn && session % 4 == 0 && !reconnected {
            let _ = game_a.close(None).await;
            let _ = audio_a.close(None).await;
            game_a =
                game_socket(&client, &base_url, &ws_base_url, &room_id, &participant_a).await?;
            audio_a =
                audio_socket(&client, &base_url, &ws_base_url, &room_id, &participant_a).await?;
            read_type(&mut game_a, "session_started").await?;
            send_json(&mut game_a, json!({"type":"ready"})).await?;
            counters.reconnects.fetch_add(2, Ordering::Relaxed);
            reconnected = true;
        }
        let frame_a = AudioFrame {
            sequence,
            timestamp_ms: u64::from(sequence) * 20,
            pcm: pcm(session, 0, sequence),
        }
        .encode();
        audio_a.send(Message::Binary(frame_a.clone())).await?;
        counters.audio_frames.fetch_add(1, Ordering::Relaxed);
        if let Some(audio) = &mut audio_b {
            anyhow::ensure!(
                read_binary(audio).await? == frame_a,
                "audio payload mismatch from role A"
            );
            counters.audio_verified.fetch_add(1, Ordering::Relaxed);
            let frame_b = AudioFrame {
                sequence,
                timestamp_ms: u64::from(sequence) * 20,
                pcm: pcm(session, 1, sequence),
            }
            .encode();
            audio.send(Message::Binary(frame_b.clone())).await?;
            counters.audio_frames.fetch_add(1, Ordering::Relaxed);
            anyhow::ensure!(
                read_binary(&mut audio_a).await? == frame_b,
                "audio payload mismatch from role B"
            );
            counters.audio_verified.fetch_add(1, Ordering::Relaxed);
        }
        if Instant::now() >= next_message {
            send_json(
                &mut game_a,
                json!({"type":"message","text":format!("stress:{session}:{sequence}")}),
            )
            .await?;
            counters.game_messages.fetch_add(1, Ordering::Relaxed);
            if pairing == Pairing::HumanAgent {
                let _ = time::timeout(Duration::from_secs(2), read_binary(&mut audio_a))
                    .await
                    .context("waiting for agent TTS deadline")??;
                counters.audio_verified.fetch_add(1, Ordering::Relaxed);
            }
            next_message += message_interval;
        }
        if Instant::now() >= next_action {
            let actor = game_b.as_mut().unwrap_or(&mut game_a);
            send_json(actor, json!({"type":"action","action":{"type":"mark","sequence":sequence,"finish":false}})).await?;
            counters.actions.fetch_add(1, Ordering::Relaxed);
            next_action += action_interval;
        }
        if Instant::now() >= next_heartbeat {
            send_json(&mut game_a, json!({"type":"heartbeat"})).await?;
            counters.heartbeats.fetch_add(1, Ordering::Relaxed);
            if let Some(game) = &mut game_b {
                send_json(game, json!({"type":"heartbeat"})).await?;
                counters.heartbeats.fetch_add(1, Ordering::Relaxed);
            }
            next_heartbeat += Duration::from_secs(10);
        }
        sequence = sequence.wrapping_add(1);
        next_frame += FRAME_INTERVAL;
        let now = Instant::now();
        if now > next_frame {
            let lag = now.duration_since(next_frame).as_micros() as u64;
            counters
                .audio_deadline_misses
                .fetch_add(1, Ordering::Relaxed);
            counters.max_audio_lag_us.fetch_max(lag, Ordering::Relaxed);
            next_frame = now;
        } else {
            time::sleep_until(next_frame.into()).await;
        }
    }
    let _ = game_a.close(None).await;
    if let Some(game) = &mut game_b {
        let _ = game.close(None).await;
    }
    let _ = audio_a.close(None).await;
    if let Some(audio) = &mut audio_b {
        let _ = audio.close(None).await;
    }
    counters.sessions_finished.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

/// Creates one synthetic participant through the public direct-intake endpoint.
async fn create_participant(client: &reqwest::Client, base_url: &str) -> Result<Participant> {
    let response = client
        .post(format!("{base_url}/api/participants"))
        .json(&json!({}))
        .send()
        .await?;
    let value = response_json(response, "participant creation").await?;
    Ok(Participant {
        credential: value["participant_credential"]
            .as_str()
            .ok_or_else(|| anyhow!("missing participant credential"))?
            .to_string(),
    })
}

/// Records the required synthetic study consent for one participant.
async fn consent(
    client: &reqwest::Client,
    base_url: &str,
    participant: &Participant,
) -> Result<()> {
    client
        .post(format!("{base_url}/api/consent"))
        .bearer_auth(&participant.credential)
        .json(&json!({"decisions":{"stress":true}}))
        .send()
        .await?
        .error_for_status()?;
    Ok(())
}

/// Creates or pairs one participant through normal direct room admission.
async fn create_room(
    client: &reqwest::Client,
    base_url: &str,
    participant: &Participant,
) -> Result<Value> {
    let response = client
        .post(format!("{base_url}/api/rooms"))
        .bearer_auth(&participant.credential)
        .json(&json!({}))
        .send()
        .await?;
    response_json(response, "room admission").await
}

/// Mints a one-use game ticket and opens the public game socket.
async fn game_socket(
    client: &reqwest::Client,
    base: &str,
    ws: &str,
    room: &str,
    participant: &Participant,
) -> Result<Socket> {
    let response = client
        .post(format!("{base}/api/rooms/{room}/game-session"))
        .bearer_auth(&participant.credential)
        .json(&json!({}))
        .send()
        .await?;
    let plan = response_json(response, "game ticket creation").await?;
    let token = plan["token"]
        .as_str()
        .ok_or_else(|| anyhow!("missing game ticket"))?;
    Ok(connect_async(format!("{ws}/ws/game/{room}?token={token}"))
        .await?
        .0)
}

/// Mints a one-use audio ticket and opens the public audio socket.
async fn audio_socket(
    client: &reqwest::Client,
    base: &str,
    ws: &str,
    room: &str,
    participant: &Participant,
) -> Result<Socket> {
    let response = client
        .post(format!("{base}/api/rooms/{room}/audio-session"))
        .bearer_auth(&participant.credential)
        .json(&json!({}))
        .send()
        .await?;
    let plan = response_json(response, "audio ticket creation").await?;
    let token = plan["token"]
        .as_str()
        .ok_or_else(|| anyhow!("missing audio ticket"))?;
    Ok(connect_async(format!("{ws}/ws/audio/{room}?token={token}"))
        .await?
        .0)
}

/// Decodes one JSON response while retaining unsuccessful response bodies for diagnostics.
async fn response_json(response: reqwest::Response, operation: &str) -> Result<Value> {
    let status = response.status();
    let body = response.text().await?;
    anyhow::ensure!(status.is_success(), "{operation} returned {status}: {body}");
    serde_json::from_str(&body).with_context(|| format!("decoding {operation} response"))
}

/// Sends one JSON protocol message through a game WebSocket.
async fn send_json(socket: &mut Socket, value: Value) -> Result<()> {
    socket.send(Message::Text(value.to_string())).await?;
    Ok(())
}

/// Reads through nonmatching JSON messages until the required game message arrives.
async fn read_type(socket: &mut Socket, expected: &str) -> Result<Value> {
    for _ in 0..32 {
        let next = time::timeout(GAME_SETUP_TIMEOUT, socket.next()).await?;
        let message = next.ok_or_else(|| anyhow!("game socket closed"))??;
        if let Message::Text(text) = message {
            let value: Value = serde_json::from_str(&text)?;
            if value["type"] == expected {
                return Ok(value);
            }
            if value["type"] == "error" {
                return Err(anyhow!("game socket error: {value}"));
            }
        }
    }
    Err(anyhow!("timed out waiting for {expected}"))
}

/// Reads one relayed binary audio frame with a bounded deadline.
async fn read_binary(socket: &mut Socket) -> Result<Vec<u8>> {
    loop {
        let next = time::timeout(AUDIO_FRAME_TIMEOUT, socket.next()).await?;
        let message = next.ok_or_else(|| anyhow!("audio socket closed"))??;
        if let Message::Binary(bytes) = message {
            return Ok(bytes);
        }
    }
}

/// Writes row counts and physical SQLite files into the durable JSON result.
async fn write_report(
    run: &RunConfig,
    database: &std::path::Path,
    counters: &Counters,
    error: Option<&anyhow::Error>,
) -> Result<()> {
    let url = format!("sqlite:///{}", database.display());
    let pool = sqlx::SqlitePool::connect(&url).await?;
    let count = |table: &'static str| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>(&format!("select count(*) from {table}"))
                .fetch_one(&pool)
                .await
        }
    };
    let participants = count("participants").await?;
    let sessions = count("sessions").await?;
    let session_events = count("session_events").await?;
    let transcript_events = sqlx::query_scalar::<_, i64>(
        "select count(*) from session_events where event_type = 'conversation_message' and json_extract(payload_json, '$.origin') = 'voice_transcript'",
    )
    .fetch_one(&pool)
    .await?;
    let file_size = |path: PathBuf| std::fs::metadata(path).map(|item| item.len()).unwrap_or(0);
    let sqlite_bytes = file_size(database.to_path_buf());
    let wal_bytes = file_size(PathBuf::from(format!("{}-wal", database.display())));
    let shm_bytes = file_size(PathBuf::from(format!("{}-shm", database.display())));
    sqlx::query("pragma wal_checkpoint(truncate)")
        .execute(&pool)
        .await?;
    pool.close().await;
    let checkpointed_sqlite_bytes = file_size(database.to_path_buf());
    let started = counters.sessions_started.load(Ordering::Relaxed);
    let validation_failure = if error.is_none() && sessions as u64 != started {
        Some("SQLite session count did not match admitted sessions".to_string())
    } else if error.is_none() && session_events == 0 {
        Some("SQLite contained no session events".to_string())
    } else if error.is_none() && transcript_events == 0 {
        Some("SQLite contained no final transcript events".to_string())
    } else {
        None
    };
    let report = StressReport {
        config: run,
        success: error.is_none()
            && validation_failure.is_none()
            && counters.failures.load(Ordering::Relaxed) == 0,
        error: error.map(ToString::to_string).or(validation_failure.clone()),
        counters: ReportCounters {
            sessions_started: started,
            sessions_finished: counters.sessions_finished.load(Ordering::Relaxed),
            game_messages: counters.game_messages.load(Ordering::Relaxed),
            heartbeats: counters.heartbeats.load(Ordering::Relaxed),
            actions: counters.actions.load(Ordering::Relaxed),
            audio_frames: counters.audio_frames.load(Ordering::Relaxed),
            audio_verified: counters.audio_verified.load(Ordering::Relaxed),
            reconnects: counters.reconnects.load(Ordering::Relaxed),
            transcripts: counters.transcripts.load(Ordering::Relaxed),
            tts_chunks: counters.tts_chunks.load(Ordering::Relaxed),
            average_startup_ms: if started == 0 {
                0
            } else {
                counters.startup_latency_ms_total.load(Ordering::Relaxed) / started
            },
            maximum_startup_ms: counters.startup_latency_ms_max.load(Ordering::Relaxed),
            audio_deadline_misses: counters.audio_deadline_misses.load(Ordering::Relaxed),
            max_audio_lag_us: counters.max_audio_lag_us.load(Ordering::Relaxed),
            failures: counters.failures.load(Ordering::Relaxed),
        },
        database: DatabaseReport {
            pre_checkpoint_sqlite_bytes: sqlite_bytes,
            pre_checkpoint_wal_bytes: wal_bytes,
            pre_checkpoint_shm_bytes: shm_bytes,
            checkpointed_sqlite_bytes,
            participants,
            sessions,
            session_events,
            transcript_events,
            checkpointed_bytes_per_started_session: if started == 0 {
                0.0
            } else {
                checkpointed_sqlite_bytes as f64 / started as f64
            },
        },
        capacity: CapacityReport {
            healthy_concurrent_games_lower_bound: if error.is_none()
                && validation_failure.is_none()
                && counters.failures.load(Ordering::Relaxed) == 0
            {
                started
            } else {
                0
            },
            target_human_websockets: run.sessions
                * if run.pairing == "human-human" { 4 } else { 2 },
            offered_audio_frames_per_second: run.sessions
                * if run.pairing == "human-human" { 100 } else { 50 },
            interpretation: "A healthy run establishes a workload-specific lower bound, not a maximum; repeat on the deployment host at increasing targets and retain headroom.",
        },
    };
    tokio::fs::write(&run.report, serde_json::to_vec_pretty(&report)?).await?;
    eprintln!("Runtime stress report: {}", run.report.display());
    if let Some(failure) = validation_failure {
        return Err(anyhow!(failure));
    }
    Ok(())
}

/// Builds deterministic PCM content whose first bytes identify its source frame.
fn pcm(session: usize, role: u8, sequence: u32) -> Vec<u8> {
    let mut pcm = vec![0; 960];
    pcm[..8].copy_from_slice(&(session as u64).to_be_bytes());
    pcm[8] = role;
    pcm[9..13].copy_from_slice(&sequence.to_be_bytes());
    pcm
}

/// Creates an empty snapshot before the workload has admitted its first session.
fn empty_snapshot(run: &RunConfig) -> Snapshot {
    snapshot_from(
        run,
        Phase::Ramp,
        Duration::ZERO,
        0,
        &Counters::default(),
        vec![],
        vec!["preparing ramp phase".into()],
        false,
        false,
        None,
    )
}

/// Converts atomic counters into a bounded UI snapshot.
fn snapshot_from(
    run: &RunConfig,
    phase: Phase,
    elapsed: Duration,
    active: usize,
    counters: &Counters,
    rate_history: Vec<u64>,
    events: Vec<String>,
    finished: bool,
    cancelled: bool,
    failure: Option<String>,
) -> Snapshot {
    Snapshot {
        phase,
        elapsed,
        duration: Duration::from_secs(run.seconds),
        target_sessions: run.sessions,
        active_sessions: active,
        sessions_started: counters.sessions_started.load(Ordering::Relaxed),
        sessions_finished: counters.sessions_finished.load(Ordering::Relaxed),
        game_messages: counters.game_messages.load(Ordering::Relaxed),
        heartbeats: counters.heartbeats.load(Ordering::Relaxed),
        actions: counters.actions.load(Ordering::Relaxed),
        audio_frames: counters.audio_frames.load(Ordering::Relaxed),
        audio_verified: counters.audio_verified.load(Ordering::Relaxed),
        reconnects: counters.reconnects.load(Ordering::Relaxed),
        transcripts: counters.transcripts.load(Ordering::Relaxed),
        tts_chunks: counters.tts_chunks.load(Ordering::Relaxed),
        average_startup_ms: {
            let started = counters.sessions_started.load(Ordering::Relaxed);
            if started == 0 {
                0
            } else {
                counters.startup_latency_ms_total.load(Ordering::Relaxed) / started
            }
        },
        maximum_startup_ms: counters.startup_latency_ms_max.load(Ordering::Relaxed),
        audio_deadline_misses: counters.audio_deadline_misses.load(Ordering::Relaxed),
        max_audio_lag_us: counters.max_audio_lag_us.load(Ordering::Relaxed),
        failures: counters.failures.load(Ordering::Relaxed),
        rate_history,
        events,
        finished,
        cancelled,
        failure: failure.or_else(|| counters.first_failure()),
    }
}

/// Adapts runtime measurements to the shared dashboard without exposing content.
fn dashboard_snapshot(snapshot: &Snapshot) -> DashboardSnapshot {
    let metric = |label: &str, value: String, health| DashboardMetric {
        label: label.into(),
        value,
        health,
    };
    let (status, status_health) = match (&snapshot.failure, snapshot.failures) {
        (Some(error), _) => (error.clone(), DashboardHealth::Error),
        (None, 0) => ("healthy".to_string(), DashboardHealth::Good),
        (None, failures) => (
            format!("degraded — {failures} task failures"),
            DashboardHealth::Error,
        ),
    };
    DashboardSnapshot {
        title: format!("Parlando runtime stress r{HARNESS_REVISION}"),
        mode: "game + audio + ASR/TTS".into(),
        phase: Some(snapshot.phase.as_str().into()),
        elapsed: snapshot.elapsed,
        duration: snapshot.duration,
        finished: snapshot.finished,
        cancelled: snapshot.cancelled,
        failure: snapshot.failure.clone(),
        panels: [
            DashboardPanel {
                title: " Workload ".into(),
                metrics: vec![
                    metric(
                        "Target / active",
                        format!(
                            "{} / {}",
                            snapshot.target_sessions, snapshot.active_sessions
                        ),
                        DashboardHealth::Neutral,
                    ),
                    metric(
                        "Started / finished",
                        format!(
                            "{} / {}",
                            snapshot.sessions_started, snapshot.sessions_finished
                        ),
                        DashboardHealth::Good,
                    ),
                    metric(
                        "Game messages",
                        snapshot.game_messages.to_string(),
                        DashboardHealth::Good,
                    ),
                    metric(
                        "Heartbeats total",
                        snapshot.heartbeats.to_string(),
                        DashboardHealth::Good,
                    ),
                    metric(
                        "Actions",
                        snapshot.actions.to_string(),
                        DashboardHealth::Good,
                    ),
                    metric(
                        "Reconnects",
                        snapshot.reconnects.to_string(),
                        DashboardHealth::Neutral,
                    ),
                ],
            },
            DashboardPanel {
                title: " Cadence & audio ".into(),
                metrics: vec![
                    metric(
                        "PCM sent / verified",
                        format!("{} / {}", snapshot.audio_frames, snapshot.audio_verified),
                        DashboardHealth::Good,
                    ),
                    metric(
                        "PCM frames/s total",
                        snapshot
                            .rate_history
                            .last()
                            .copied()
                            .unwrap_or(0)
                            .to_string(),
                        DashboardHealth::Neutral,
                    ),
                    metric(
                        "Startup avg / max",
                        format!(
                            "{} / {} ms",
                            snapshot.average_startup_ms, snapshot.maximum_startup_ms
                        ),
                        DashboardHealth::Neutral,
                    ),
                    metric(
                        "Misses / max lag",
                        format!(
                            "{} / {} us",
                            snapshot.audio_deadline_misses, snapshot.max_audio_lag_us
                        ),
                        if snapshot.audio_deadline_misses == 0 {
                            DashboardHealth::Good
                        } else {
                            DashboardHealth::Warning
                        },
                    ),
                    metric(
                        "Final transcripts",
                        snapshot.transcripts.to_string(),
                        DashboardHealth::Good,
                    ),
                    metric(
                        "TTS syntheses",
                        snapshot.tts_chunks.to_string(),
                        DashboardHealth::Good,
                    ),
                ],
            },
            DashboardPanel {
                title: " Verification ".into(),
                metrics: vec![
                    metric(
                        "Failures",
                        snapshot.failures.to_string(),
                        if snapshot.failures == 0 {
                            DashboardHealth::Good
                        } else {
                            DashboardHealth::Error
                        },
                    ),
                    metric("Status", status, status_health),
                ],
            },
        ],
        series: vec![DashboardSeries {
            title: " Audio frames/s — last 60 samples ".into(),
            values: snapshot.rate_history.clone(),
        }],
        tiles: (0..snapshot.target_sessions)
            .map(|index| DashboardTile {
                health: if snapshot.failure.is_some() {
                    DashboardHealth::Error
                } else if index >= snapshot.active_sessions {
                    DashboardHealth::Neutral
                } else {
                    DashboardHealth::Good
                },
            })
            .collect(),
        tile_legend: "green healthy · red failed".into(),
        events: snapshot.events.clone(),
        footer: "combined game and audio traffic".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::{Args, Pairing, Phase, Preset};
    use std::{path::PathBuf, time::Duration};

    /// Verifies the default acceptance run preserves the requested 1:4:3:2 phases.
    #[test]
    fn phases_follow_acceptance_proportions() {
        let duration = Duration::from_secs(600);
        assert_eq!(Phase::at(Duration::from_secs(59), duration), Phase::Ramp);
        assert_eq!(Phase::at(Duration::from_secs(60), duration), Phase::Steady);
        assert_eq!(Phase::at(Duration::from_secs(299), duration), Phase::Steady);
        assert_eq!(Phase::at(Duration::from_secs(300), duration), Phase::Churn);
        assert_eq!(Phase::at(Duration::from_secs(479), duration), Phase::Churn);
        assert_eq!(Phase::at(Duration::from_secs(480), duration), Phase::Drain);
    }

    /// Verifies explicit scale overrides are what the machine report records.
    #[test]
    fn preset_resolution_retains_overrides() {
        let args = Args {
            preset: Preset::Smoke,
            pairing: Pairing::HumanHuman,
            sessions: Some(7),
            seconds: Some(20),
            no_tui: true,
            keep_database: false,
            report: PathBuf::from("report.json"),
        };
        let run = args.resolve();
        assert_eq!(run.sessions, 7);
        assert_eq!(run.seconds, 20);
        assert_eq!(run.pairing, "human-human");
    }
}
