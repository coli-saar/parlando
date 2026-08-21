//! Production-boundary stress harness for the Parlando runtime.
//!
//! The public process supervises two hidden child modes. The server child uses
//! [`parlando::Server`], while the provider child implements the external wire
//! protocols used by the production Speechmatics and ElevenLabs adapters.

use std::{
    collections::{BTreeMap, VecDeque},
    fs::File,
    io::Write,
    net::SocketAddr,
    path::{Path, PathBuf},
    process::{ExitStatus, Stdio},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use axum::{
    extract::{
        ws::{Message as AxumMessage, WebSocket, WebSocketUpgrade},
        Path as AxumPath, State,
    },
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::get,
    Router,
};
use base64::Engine as _;
use clap::{Parser, ValueEnum};
use futures_util::{SinkExt, StreamExt};
use parlando::{
    agent::{
        Agent, Context as AgentContext, Definition as AgentDefinition, Factory as AgentFactory,
        Identity as AgentIdentity, Response as AgentResponse,
    },
    bundled_runtime_limits,
    test_support::{
        run_dashboard, AudioFrame, DashboardHealth, DashboardMetric, DashboardPanel,
        DashboardSeries, DashboardSnapshot, DashboardTile,
    },
    ActionRejection, Game, GameInitializationContext, GameMetadata, PlayerRole, Server,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::{
    net::TcpListener,
    process::{Child, Command},
    signal,
    sync::{oneshot, watch},
    time,
};
use tokio_tungstenite::{connect_async, tungstenite::Message};

const HARNESS_REVISION: u32 = 5;
const PROFILE: &str = "resident-capacity-v1";
const PROVIDER_KEY: &str = "runtime-stress-provider-key";
const FRAME_INTERVAL: Duration = Duration::from_millis(20);
const SOCKET_TIMEOUT: Duration = Duration::from_secs(30);

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Public workload and artifact options. Hidden child modes are parsed before Clap.
#[derive(Clone, Debug, Parser)]
#[command(about = "Stress Parlando through its production process and network boundaries")]
struct Args {
    /// Concurrent sessions offered by the resident-capacity workload.
    #[arg(long, default_value_t = 100)]
    sessions: usize,
    /// Measured duration in seconds.
    #[arg(long, default_value_t = 60)]
    seconds: u64,
    /// Participant composition used for every session.
    #[arg(long, value_enum, default_value_t = Pairing::HumanAgent)]
    pairing: Pairing,
    /// Disable the terminal dashboard without changing the workload.
    #[arg(long)]
    headless: bool,
    /// Preserve the temporary SQLite directory after a successful run.
    #[arg(long)]
    keep: bool,
    /// Stable run-artifact root.
    #[arg(long, default_value = "target/runtime-stress")]
    output: PathBuf,
    /// Seed for deterministic provider and participant timing.
    #[arg(long, default_value_t = 1)]
    seed: u64,
}

/// Supported participant composition.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
enum Pairing {
    HumanAgent,
    HumanHuman,
}

/// Versioned fixture state with bounded history and representative padding.
#[derive(Clone, Debug, Deserialize, Serialize)]
struct StressState {
    counts: [u64; 2],
    last_sequence: [u64; 2],
    complete: bool,
    history: VecDeque<(String, u64)>,
    padding: String,
}

/// Monotonic fixture action used by humans and the compiled agent.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum StressAction {
    Mark { sequence: u64, finish: bool },
}

/// Role-specific state projection used to verify reconnect snapshots.
#[derive(Clone, Debug, Serialize)]
struct StressObservation {
    role: String,
    counts: [u64; 2],
    last_sequence: [u64; 2],
    complete: bool,
    padding: String,
}

/// Terminal fixture result.
#[derive(Clone, Debug, Serialize)]
struct StressCompletion {
    total_actions: u64,
}

/// Cheap deterministic game compiled only into the stress executable.
#[derive(Clone)]
struct StressGame;

struct StressGameFactory;

impl parlando::GameFactory for StressGameFactory {
    type Game = StressGame;

    /// Creates one stateless stress-session game.
    fn create(&self, _context: parlando::GameSessionContext) -> Result<StressGame> {
        Ok(StressGame)
    }
}

impl Game for StressGame {
    type Config = Value;
    type State = StressState;
    type Action = StressAction;
    type Observation = StressObservation;
    type Completion = StressCompletion;

    /// Creates one stateless stress-game value for an admitted session.
    /// Creates one fixed-shape state value per admitted session.
    fn initial_state(&self, _: GameInitializationContext<'_, Value>) -> Result<StressState> {
        Ok(StressState {
            counts: [0, 0],
            last_sequence: [0, 0],
            complete: false,
            history: VecDeque::new(),
            padding: "p".repeat(2048),
        })
    }

    /// Rejects stale, duplicate, and post-terminal actions through the normal path.
    fn apply_action(
        &self,
        state: &StressState,
        action: &StressAction,
        actor: PlayerRole,
    ) -> std::result::Result<StressState, ActionRejection> {
        if state.complete {
            return Err(ActionRejection::new("session_complete"));
        }
        let StressAction::Mark { sequence, finish } = action;
        let index = usize::from(actor == PlayerRole::B);
        if *sequence <= state.last_sequence[index] {
            return Err(ActionRejection::new("stale_or_duplicate_sequence"));
        }
        let mut next = state.clone();
        next.counts[index] += 1;
        next.last_sequence[index] = *sequence;
        next.complete = *finish;
        next.history.push_back((actor.as_str().into(), *sequence));
        while next.history.len() > 32 {
            next.history.pop_front();
        }
        Ok(next)
    }

    /// Returns the recipient role alongside all transition-verification fields.
    fn observation(&self, state: &StressState, player: PlayerRole) -> StressObservation {
        StressObservation {
            role: player.as_str().into(),
            counts: state.counts,
            last_sequence: state.last_sequence,
            complete: state.complete,
            padding: state.padding.clone(),
        }
    }

    /// Advertises representative terminal and nonterminal actions.
    fn available_actions(&self, _: &StressState, _: PlayerRole) -> Option<Vec<StressAction>> {
        Some(vec![
            StressAction::Mark {
                sequence: 1,
                finish: false,
            },
            StressAction::Mark {
                sequence: 1,
                finish: true,
            },
        ])
    }

    /// Produces a terminal summary only after completion.
    fn completion(&self, state: &StressState) -> Option<StressCompletion> {
        state.complete.then_some(StressCompletion {
            total_actions: state.counts.iter().sum(),
        })
    }
}

/// Deterministic session-local agent.
struct StressAgent {
    sequence: u64,
    pending: bool,
    reply_tag: String,
}

#[async_trait]
impl Agent<StressGame> for StressAgent {
    /// Schedules the initial agent turn.
    async fn start(&mut self, _: StressObservation) -> Result<()> {
        self.pending = true;
        self.reply_tag = "initial".into();
        Ok(())
    }
    /// Responds after role A changes game state.
    async fn observe_transition(
        &mut self,
        _actor: PlayerRole,
        _: StressAction,
        _: StressObservation,
    ) -> Result<()> {
        self.pending = false;
        Ok(())
    }
    /// Responds after a role A conversation message.
    async fn observe_message(&mut self, sender: PlayerRole, text: String) -> Result<()> {
        self.pending = sender == PlayerRole::A;
        if self.pending {
            self.reply_tag = text;
        }
        Ok(())
    }
    /// Emits one deterministic action and TTS-triggering message.
    async fn respond(
        &mut self,
        _: Option<Vec<StressAction>>,
    ) -> Result<Option<AgentResponse<StressAction>>> {
        if !self.pending {
            return Ok(None);
        }
        self.pending = false;
        self.sequence += 1;
        Ok(Some(AgentResponse::action_and_message(
            StressAction::Mark {
                sequence: self.sequence,
                finish: false,
            },
            format!("agent reply {}", self.reply_tag),
        )))
    }
}

/// Factory registered through the same public builder used by deployed games.
struct StressAgentFactory;

#[async_trait]
impl AgentFactory<StressGame> for StressAgentFactory {
    /// Describes the compiled fixture agent for experiment selection.
    fn definition(&self) -> AgentDefinition {
        AgentDefinition {
            id: "runtime-stress".into(),
            name: "Runtime stress agent".into(),
            description: "Deterministic fixture agent".into(),
            config_fields: vec![],
        }
    }
    /// Creates isolated state for one session.
    async fn create(&self, _: AgentContext) -> Result<Box<dyn Agent<StressGame> + Send>> {
        Ok(Box::new(StressAgent {
            sequence: 0,
            pending: false,
            reply_tag: String::new(),
        }))
    }
    /// Returns stable stored provenance.
    fn identity(&self, _: &Value) -> Result<AgentIdentity> {
        Ok(AgentIdentity {
            name: "runtime-stress-agent".into(),
            version: "1".into(),
        })
    }
}

/// Atomic provider readiness document consumed by the supervisor.
#[derive(Clone, Debug, Deserialize, Serialize)]
struct ProviderReady {
    speechmatics_url: String,
    elevenlabs_url: String,
    seed: u64,
    timing: ProviderTiming,
}

/// Deterministic external-service delay profile recorded in every report.
#[derive(Clone, Debug, Deserialize, Serialize)]
struct ProviderTiming {
    startup_ms: u64,
    partial_ms: u64,
    final_ms: u64,
    tts_first_byte_ms: u64,
    tts_chunk_ms: u64,
}

/// Administrator session retained for telemetry and lifecycle cleanup.
#[derive(Clone)]
struct AdminAuth {
    cookie: String,
    csrf: String,
    config_revision: i64,
}

/// Child-process termination information recorded in the report.
#[derive(Debug, Serialize)]
struct ChildReport {
    status: String,
    forced: bool,
    sampled_rss_bytes: Option<u64>,
    sampled_cpu_seconds: Option<f64>,
    sampled_open_fds: Option<usize>,
}

/// Thread-safe workload measurements shared by tasks and the dashboard.
#[derive(Default)]
struct Counters {
    staged_sessions: AtomicU64,
    started_sessions: AtomicU64,
    finished_sessions: AtomicU64,
    game_messages: AtomicU64,
    actions: AtomicU64,
    pcm_sent: AtomicU64,
    pcm_verified: AtomicU64,
    reconnects: AtomicU64,
    tts_audio: AtomicU64,
    failures: AtomicU64,
    elapsed_ms: AtomicU64,
    first_failure: Mutex<Option<String>>,
    room_failures: Mutex<Vec<bool>>,
    rates: Mutex<RateHistory>,
}

/// Bounded rolling throughput samples rendered by the dashboard.
#[derive(Default)]
struct RateHistory {
    messages: VecDeque<u64>,
    actions: VecDeque<u64>,
    pcm: VecDeque<u64>,
    tts: VecDeque<u64>,
}

/// Previous cumulative values used to derive interval throughput.
#[derive(Default)]
struct RateTotals {
    messages: u64,
    actions: u64,
    pcm: u64,
    tts: u64,
}

impl Counters {
    /// Records one correctness or transport failure with its session identity.
    fn fail(&self, session: usize, error: &anyhow::Error) {
        self.failures.fetch_add(1, Ordering::Relaxed);
        let mut first = self.first_failure.lock().expect("failure mutex poisoned");
        if first.is_none() {
            *first = Some(format!("session {session}: {error:#}"));
        }
        let mut rooms = self
            .room_failures
            .lock()
            .expect("room failure mutex poisoned");
        if let Some(failed) = rooms.get_mut(session) {
            *failed = true;
        }
    }

    /// Produces the durable counter projection.
    fn report(&self) -> WorkloadReport {
        WorkloadReport {
            staged_sessions: self.staged_sessions.load(Ordering::Relaxed),
            started_sessions: self.started_sessions.load(Ordering::Relaxed),
            finished_sessions: self.finished_sessions.load(Ordering::Relaxed),
            game_messages: self.game_messages.load(Ordering::Relaxed),
            actions: self.actions.load(Ordering::Relaxed),
            pcm_sent: self.pcm_sent.load(Ordering::Relaxed),
            pcm_verified: self.pcm_verified.load(Ordering::Relaxed),
            reconnects: self.reconnects.load(Ordering::Relaxed),
            tts_audio: self.tts_audio.load(Ordering::Relaxed),
            failures: self.failures.load(Ordering::Relaxed),
            first_failure: self
                .first_failure
                .lock()
                .expect("failure mutex poisoned")
                .clone(),
        }
    }

    /// Samples cumulative counters into bounded per-second throughput histories.
    fn sample_rates(&self, previous: &mut RateTotals, interval: Duration) {
        let current = RateTotals {
            messages: self.game_messages.load(Ordering::Relaxed),
            actions: self.actions.load(Ordering::Relaxed),
            pcm: self.pcm_sent.load(Ordering::Relaxed),
            tts: self.tts_audio.load(Ordering::Relaxed),
        };
        let seconds = interval.as_secs_f64().max(0.001);
        let rate =
            |now: u64, before: u64| ((now.saturating_sub(before)) as f64 / seconds).round() as u64;
        let mut history = self.rates.lock().expect("rate history mutex poisoned");
        push_bounded(
            &mut history.messages,
            rate(current.messages, previous.messages),
        );
        push_bounded(
            &mut history.actions,
            rate(current.actions, previous.actions),
        );
        push_bounded(&mut history.pcm, rate(current.pcm, previous.pcm));
        push_bounded(&mut history.tts, rate(current.tts, previous.tts));
        *previous = current;
    }
}

/// Appends one rolling one-second sample while retaining at most one minute.
fn push_bounded(history: &mut VecDeque<u64>, value: u64) {
    history.push_back(value);
    if history.len() > 60 {
        history.pop_front();
    }
}

/// Machine-readable workload counts and first invariant failure.
#[derive(Clone, Debug, Default, Serialize)]
struct WorkloadReport {
    staged_sessions: u64,
    started_sessions: u64,
    finished_sessions: u64,
    game_messages: u64,
    actions: u64,
    pcm_sent: u64,
    pcm_verified: u64,
    reconnects: u64,
    tts_audio: u64,
    failures: u64,
    first_failure: Option<String>,
}

/// SQLite row and file verification results.
#[derive(Clone, Debug, Default, Serialize)]
struct DatabaseReport {
    participants: i64,
    consents: i64,
    sessions: i64,
    memberships: i64,
    events: i64,
    transcripts: i64,
    actions: i64,
    terminal_events: i64,
    main_bytes: u64,
    wal_bytes: u64,
    shm_bytes: u64,
    checkpointed_bytes: u64,
}

/// Authoritative report skeleton; detailed operation counters are added as the workload evolves.
#[derive(Debug, Serialize)]
struct Report {
    schema_revision: u32,
    harness_revision: u32,
    profile: &'static str,
    seed: u64,
    sessions: usize,
    seconds: u64,
    pairing: Pairing,
    experiment_id: &'static str,
    config_revision: Option<i64>,
    provider: ProviderReady,
    phases_seconds: BTreeMap<&'static str, u64>,
    descriptor_limits: DescriptorReport,
    server: ChildReport,
    providers: ChildReport,
    workload: WorkloadReport,
    database: DatabaseReport,
    cleanup_ms: u64,
    success: bool,
    error: Option<String>,
}

/// Process descriptor preflight values.
#[derive(Debug, Serialize)]
struct DescriptorReport {
    original_soft: u64,
    hard: u64,
    adjusted_soft: u64,
    required: u64,
}

/// Dispatches hidden modes without exposing them in ordinary Clap help.
#[tokio::main]
async fn main() -> Result<()> {
    let raw = std::env::args().collect::<Vec<_>>();
    match raw.get(1).map(String::as_str) {
        Some("__server") => server_child(&raw[2..]).await,
        Some("__providers") => provider_child(&raw[2..]).await,
        _ => supervise(Args::parse()).await,
    }
}

/// Runs the public supervisor and owns all artifacts and child cleanup.
async fn supervise(args: Args) -> Result<()> {
    if args.sessions == 0 {
        bail!("--sessions must be positive");
    }
    if args.seconds < 10 {
        bail!("--seconds must be at least 10");
    }
    workload_preflight(args.sessions, args.pairing)?;
    let limits = descriptor_preflight(args.sessions, args.pairing)?;
    let run_id = format!(
        "{}-{}",
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
        std::process::id()
    );
    let run_dir = args.output.join(run_id);
    std::fs::create_dir_all(&run_dir)?;
    let temp = tempfile::Builder::new()
        .prefix("parlando-runtime-stress-")
        .tempdir()?;
    let database = temp.path().join("runtime-stress.sqlite");
    let readiness = run_dir.join("providers-ready.json");
    let executable = std::env::current_exe()?;

    let mut providers = spawn_logged(
        &executable,
        &[
            "__providers",
            &readiness.display().to_string(),
            &args.seed.to_string(),
        ],
        &run_dir,
        "providers",
    )?;
    let provider = wait_readiness(&readiness, &mut providers).await?;
    let address = reserve_address().await?;
    let mut server = spawn_logged(
        &executable,
        &[
            "__server",
            &address.to_string(),
            &database.display().to_string(),
        ],
        &run_dir,
        "server",
    )?;
    let base = format!("http://{address}");
    wait_health(&base, &mut server).await?;

    let setup_result =
        configure_experiment(&base, &provider, args.pairing, args.sessions, &run_dir).await;
    let admin = setup_result.as_ref().ok().cloned();
    let counters = Arc::new(Counters::default());
    counters
        .room_failures
        .lock()
        .expect("room failure mutex poisoned")
        .resize(args.sessions, false);
    let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let initial = workload_snapshot(&args, "staging", &counters, false);
    let (snapshots, receiver) = watch::channel(initial);
    let runtime_base = format!("{base}/e/runtime-stress");
    let ws_base = format!("ws://{address}/e/runtime-stress");
    let workload = if let Err(error) = setup_result {
        Err(error)
    } else {
        let task = tokio::spawn(run_workload(
            args.clone(),
            runtime_base,
            ws_base,
            counters.clone(),
            snapshots,
            cancelled.clone(),
        ));
        if args.headless {
            match task.await {
                Ok(result) => result,
                Err(error) => Err(anyhow!("workload task panicked: {error}")),
            }
        } else {
            supervise_dashboard(task, receiver, cancelled.clone()).await
        }
    };
    let cleanup_started = Instant::now();
    let cleanup_result = if let Some(admin) = &admin {
        match deactivate(&base, admin).await {
            Ok(()) => wait_runtime_drain(&base, admin).await,
            Err(error) => Err(error),
        }
    } else {
        Ok(())
    };
    let cleanup_ms = cleanup_started.elapsed().as_millis() as u64;
    let server_report = stop_child(&mut server).await;
    let provider_report = stop_child(&mut providers).await;
    let database_report = verify_database(&database, args.sessions, args.pairing).await;
    let error = workload
        .as_ref()
        .err()
        .map(|error| format!("{error:#}"))
        .or_else(|| {
            database_report
                .as_ref()
                .err()
                .map(|error| format!("{error:#}"))
        });
    let error = error.or_else(|| cleanup_result.err().map(|error| format!("{error:#}")));
    let report = Report {
        schema_revision: 1,
        harness_revision: HARNESS_REVISION,
        profile: PROFILE,
        seed: args.seed,
        sessions: args.sessions,
        seconds: args.seconds,
        pairing: args.pairing,
        experiment_id: "runtime-stress",
        config_revision: admin.as_ref().map(|auth| auth.config_revision),
        provider,
        phases_seconds: phase_durations(args.seconds),
        descriptor_limits: limits,
        server: server_report,
        providers: provider_report,
        workload: counters.report(),
        database: database_report.unwrap_or_default(),
        cleanup_ms,
        success: error.is_none(),
        error,
    };
    let report_path = run_dir.join("report.json");
    std::fs::write(&report_path, serde_json::to_vec_pretty(&report)?)?;
    print_report_summary(&report, &report_path);
    if args.keep || !report.success {
        let kept = temp.keep();
        eprintln!("Preserved database: {}", kept.display());
    }
    workload.map(|_| ())?;
    if let Some(error) = report.error {
        bail!(error);
    }
    Ok(())
}

/// Runs the dashboard as a supervised peer and cancels the workload if presentation fails.
async fn supervise_dashboard(
    mut workload: tokio::task::JoinHandle<Result<()>>,
    receiver: watch::Receiver<DashboardSnapshot>,
    cancelled: Arc<std::sync::atomic::AtomicBool>,
) -> Result<()> {
    let ui_cancelled = cancelled.clone();
    let (ui_done_sender, mut ui_done_receiver) =
        oneshot::channel::<std::result::Result<(), String>>();
    let ui = std::thread::spawn(move || {
        let mut terminal = ratatui::init();
        let mut receiver = receiver;
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_dashboard(
                &mut terminal,
                &mut receiver,
                ui_cancelled,
                Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                dashboard_snapshot,
            )
        }));
        ratatui::restore();
        let status = match outcome {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => Err(format!("dashboard error: {error:#}")),
            Err(_) => Err("dashboard panicked".to_string()),
        };
        let _ = ui_done_sender.send(status);
    });

    let result = tokio::select! {
        workload_result = &mut workload => {
            let workload_result = flatten_workload_result(workload_result, "workload");
            let dashboard_result = ui_done_receiver.await
                .map_err(|_| anyhow!("dashboard exited without reporting status"))
                .and_then(|status| status.map_err(anyhow::Error::msg));
            workload_result.and(dashboard_result)
        }
        dashboard_result = &mut ui_done_receiver => {
            cancelled.store(true, Ordering::Relaxed);
            let dashboard_result = dashboard_result
                .map_err(|_| anyhow!("dashboard exited without reporting status"))
                .and_then(|status| status.map_err(anyhow::Error::msg));
            let workload_result = flatten_workload_result(
                workload.await,
                "workload after dashboard exit",
            );
            match dashboard_result {
                Ok(()) => Err(anyhow!("dashboard exited before the workload completed")),
                Err(error) => Err(anyhow!(
                    "{error:#}; workload shutdown: {}",
                    workload_result
                        .as_ref()
                        .err()
                        .map(ToString::to_string)
                        .unwrap_or_else(|| "clean".to_string())
                )),
            }
        }
    };
    let ui_result = ui
        .join()
        .map_err(|_| anyhow!("dashboard thread escaped panic containment"));
    result.and(ui_result)
}

/// Converts a Tokio workload join result into the harness error type without escaping cleanup.
fn flatten_workload_result(
    result: std::result::Result<Result<()>, tokio::task::JoinError>,
    context: &str,
) -> Result<()> {
    match result {
        Ok(result) => result,
        Err(error) => Err(anyhow!("{context} task panicked: {error}")),
    }
}

/// Rejects workloads that cannot pass deterministic bundled server admission ceilings.
fn workload_preflight(sessions: usize, pairing: Pairing) -> Result<()> {
    let participants_per_session = match pairing {
        Pairing::HumanAgent => 1,
        Pairing::HumanHuman => 2,
    };
    let required = sessions.saturating_mul(participants_per_session);
    let limit = &bundled_runtime_limits().participant_creation;
    if required <= limit.max_attempts {
        return Ok(());
    }
    let maximum_sessions = limit.max_attempts / participants_per_session;
    let pairing = match pairing {
        Pairing::HumanAgent => "human-agent",
        Pairing::HumanHuman => "human-human",
    };
    bail!(
        "--sessions {sessions} with --pairing {pairing} requires {required} direct participant \
         creations during staging, but bundled server limit \
         participant_creation.max_attempts is {} per {} seconds. Use --sessions \
         {maximum_sessions} or lower, or raise participant_creation.max_attempts in \
         rust-server/config/runtime-limits.json and rebuild",
        limit.max_attempts,
        limit.window_seconds
    )
}

/// Starts the production public server API in the hidden server process.
async fn server_child(arguments: &[String]) -> Result<()> {
    let address: SocketAddr = arguments
        .first()
        .context("missing server address")?
        .parse()?;
    let database = PathBuf::from(arguments.get(1).context("missing database path")?);
    let metadata = GameMetadata {
        id: "runtime-stress".into(),
        name: "Runtime Stress Fixture".into(),
        version: semver::Version::new(1, 0, 0),
        build_manifest: json!({"harness_revision": HARNESS_REVISION}),
    };
    Server::new(StressGameFactory, metadata)?
        .database_url(format!("sqlite:///{}", database.display()))
        .agent(StressAgentFactory)?
        .serve(address)
        .await
}

/// Runs both external provider-protocol peers on one ephemeral listener.
async fn provider_child(arguments: &[String]) -> Result<()> {
    let readiness = PathBuf::from(arguments.first().context("missing readiness path")?);
    let seed: u64 = arguments.get(1).context("missing provider seed")?.parse()?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let timing = ProviderTiming {
        startup_ms: 8 + seed % 5,
        partial_ms: 12 + seed % 7,
        final_ms: 24 + seed % 11,
        tts_first_byte_ms: 18 + seed % 9,
        tts_chunk_ms: 6 + seed % 4,
    };
    let app = Router::new()
        .route("/speechmatics", get(speechmatics_upgrade))
        .route(
            "/v1/text-to-speech/:voice/stream-input",
            get(elevenlabs_upgrade),
        )
        .with_state(timing.clone());
    let ready = ProviderReady {
        speechmatics_url: format!("ws://{address}/speechmatics"),
        elevenlabs_url: format!("ws://{address}"),
        seed,
        timing,
    };
    atomic_json(&readiness, &ready)?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

/// Validates authorization before upgrading a Speechmatics connection.
async fn speechmatics_upgrade(
    State(timing): State<ProviderTiming>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> impl IntoResponse {
    if headers.get("authorization").and_then(|v| v.to_str().ok())
        != Some(&format!("Bearer {PROVIDER_KEY}"))
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    upgrade
        .on_upgrade(move |socket| speechmatics_socket(socket, timing))
        .into_response()
}

/// Implements the production client's Speechmatics startup, audio, and shutdown contract.
async fn speechmatics_socket(mut socket: WebSocket, timing: ProviderTiming) {
    let Some(Ok(AxumMessage::Text(start))) = socket.recv().await else {
        return;
    };
    let Ok(start): Result<Value, _> = serde_json::from_str(&start) else {
        return;
    };
    if validate_speechmatics_start(&start).is_err() {
        return;
    }
    time::sleep(Duration::from_millis(timing.startup_ms)).await;
    let _ = socket
        .send(AxumMessage::Text(
            json!({"message":"RecognitionStarted","id":"stress"}).to_string(),
        ))
        .await;
    let mut sequence = 0u64;
    while let Some(Ok(message)) = socket.recv().await {
        match message {
            AxumMessage::Binary(_) => {
                sequence += 1;
                if sequence % 100 == 0 {
                    let partial = json!({"message":"AddPartialTranscript","metadata":{"transcript":"stre","start_time":0.0,"end_time":sequence as f64 * 0.02}});
                    time::sleep(Duration::from_millis(timing.partial_ms)).await;
                    let _ = socket.send(AxumMessage::Text(partial.to_string())).await;
                    time::sleep(Duration::from_millis(timing.final_ms)).await;
                    let result = json!({"type":"word","start_time":(sequence-100) as f64 * 0.02,
                        "end_time":sequence as f64 * 0.02,"alternatives":[{"content":"stress","confidence":1.0}]});
                    let payload = json!({"message":"AddTranscript","metadata":{"transcript":"stress","start_time":0.0,"end_time":sequence as f64 * 0.02},"results":[result]});
                    let _ = socket.send(AxumMessage::Text(payload.to_string())).await;
                    let _ = socket
                        .send(AxumMessage::Text(
                            json!({"message":"EndOfUtterance"}).to_string(),
                        ))
                        .await;
                }
            }
            AxumMessage::Text(text)
                if serde_json::from_str::<Value>(&text)
                    .ok()
                    .is_some_and(|value| {
                        value["message"] == "EndOfStream" && value["last_seq_no"] == sequence
                    }) =>
            {
                let _ = socket
                    .send(AxumMessage::Text(
                        json!({"message":"EndOfTranscript"}).to_string(),
                    ))
                    .await;
                break;
            }
            _ => {}
        }
    }
}

/// Upgrades the production ElevenLabs streaming route.
async fn elevenlabs_upgrade(
    State(timing): State<ProviderTiming>,
    AxumPath(_voice): AxumPath<String>,
    upgrade: WebSocketUpgrade,
) -> impl IntoResponse {
    upgrade.on_upgrade(move |socket| elevenlabs_socket(socket, timing))
}

/// Validates the initial key and returns deterministic 24 kHz mono PCM.
async fn elevenlabs_socket(mut socket: WebSocket, timing: ProviderTiming) {
    let Some(Ok(AxumMessage::Text(initial))) = socket.recv().await else {
        return;
    };
    let Ok(value): Result<Value, _> = serde_json::from_str(&initial) else {
        return;
    };
    if validate_elevenlabs_initial(&value).is_err() {
        return;
    }
    let mut text = String::new();
    let mut triggered = false;
    while let Some(Ok(AxumMessage::Text(payload))) = socket.recv().await {
        let Ok(value): Result<Value, _> = serde_json::from_str(&payload) else {
            return;
        };
        if value["text"] == "" {
            break;
        }
        if let Some(part) = value["text"].as_str() {
            text.push_str(part);
        }
        triggered |= value["try_trigger_generation"].as_bool().unwrap_or(false);
    }
    if text.trim().is_empty() || !triggered {
        return;
    }
    let fingerprint = text.bytes().fold(0u8, u8::wrapping_add);
    eprintln!("elevenlabs synthesis text={text:?} fingerprint={fingerprint}");
    time::sleep(Duration::from_millis(timing.tts_first_byte_ms)).await;
    let audio = base64::engine::general_purpose::STANDARD.encode(vec![fingerprint; 4_800]);
    let _ = socket
        .send(AxumMessage::Text(json!({"audio":audio}).to_string()))
        .await;
    time::sleep(Duration::from_millis(timing.tts_chunk_ms)).await;
    let _ = socket
        .send(AxumMessage::Text(json!({"isFinal":true}).to_string()))
        .await;
}

/// Validates every production Speechmatics startup field owned by the profile.
fn validate_speechmatics_start(value: &Value) -> Result<()> {
    if value["message"] != "StartRecognition"
        || value["audio_format"]["type"] != "raw"
        || value["audio_format"]["encoding"] != "pcm_s16le"
        || value["audio_format"]["sample_rate"] != 24_000
        || value["transcription_config"]["language"].as_str().is_none()
        || value["transcription_config"]["enable_partials"] != true
        || value["transcription_config"]["max_delay"]
            .as_f64()
            .is_none()
    {
        bail!("invalid Speechmatics StartRecognition contract");
    }
    Ok(())
}

/// Validates the production ElevenLabs authentication and voice-settings message.
fn validate_elevenlabs_initial(value: &Value) -> Result<()> {
    if value["text"] != " "
        || value["xi_api_key"] != PROVIDER_KEY
        || value["voice_settings"]["stability"] != 0.5
        || value["voice_settings"]["similarity_boost"] != 0.75
    {
        bail!("invalid ElevenLabs initial message contract");
    }
    Ok(())
}

/// Configures, reads back, and activates one immutable experiment over admin HTTP.
async fn configure_experiment(
    base: &str,
    provider: &ProviderReady,
    pairing: Pairing,
    sessions: usize,
    run_dir: &Path,
) -> Result<AdminAuth> {
    let client = reqwest::Client::new();
    let response = client.post(format!("{base}/api/admin/setup"))
        .json(&json!({"username":"stress-admin","password":"runtime-stress-password","password_confirmation":"runtime-stress-password"})).send().await?.error_for_status()?;
    let cookie = response
        .headers()
        .get(reqwest::header::SET_COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .context("setup response omitted session cookie")?
        .to_string();
    let csrf = response.json::<Value>().await?["csrf_token"]
        .as_str()
        .context("setup response omitted CSRF token")?
        .to_string();
    let settings = client
        .get(format!("{base}/api/admin/game/settings"))
        .header(reqwest::header::COOKIE, &cookie)
        .send()
        .await?;
    let settings = response_value(settings, "read game settings").await?;
    let provider_secrets = client
        .post(format!("{base}/api/admin/game/settings"))
        .header(reqwest::header::COOKIE, &cookie)
        .header("x-csrf-token", &csrf)
        .json(&json!({
            "expected_revision": settings["revision"],
            "institution": settings["institution"],
            "admin_allowed_ip_ranges": settings["admin_allowed_ip_ranges"],
            "speechmatics_realtime_url": provider.speechmatics_url,
            "tts_base_url": provider.elevenlabs_url,
            "secret_updates": {"speechmatics.api_key":PROVIDER_KEY,"tts.api_key":PROVIDER_KEY}
        }))
        .send()
        .await?;
    ensure_response(provider_secrets, "store provider credentials").await?;
    let agents = if pairing == Pairing::HumanAgent {
        json!({"mode":"human_vs_agent","human_vs_agent":{"factory":"runtime-stress"}})
    } else {
        json!({"mode":"human_vs_human"})
    };
    let config = json!({
        "direct":{"consents":[{"id":"stress","title":"Stress consent","body":"Synthetic participant","required":true}]},
        "voice":{"enabled":true},
        "transcription":{"enabled":true},
        "speechmatics":{"realtime_url":provider.speechmatics_url},
        "tts":{"enabled":pairing == Pairing::HumanAgent,"base_url":provider.elevenlabs_url,"voice_id":"stress","model":"stress","output_format":"pcm_24000"},
        "agents":agents,
        "capacity":{"max_active_sessions":sessions + 16,"max_waiting_sessions":sessions + 16,
            "max_unattached_participants":sessions * 2 + 32,"max_transcription_streams":sessions * 2 + 32}
    });
    let created = client.post(format!("{base}/api/admin/experiments")).header(reqwest::header::COOKIE, &cookie)
        .header("x-csrf-token", &csrf).json(&json!({"experiment_id":"runtime-stress","config":config,"notes":"generated by cargo stress"}))
        .send().await?;
    ensure_response(created, "create experiment").await?;
    let effective = client
        .get(format!(
            "{base}/api/admin/experiments/runtime-stress/config"
        ))
        .header(reqwest::header::COOKIE, &cookie)
        .send()
        .await?;
    let effective = response_value(effective, "read effective experiment").await?;
    if effective["experiment"]["config"]["speechmatics"]["realtime_url"]
        != provider.speechmatics_url
    {
        bail!("Speechmatics URL changed during normalization");
    }
    if effective["experiment"]["config"]["tts"]["base_url"] != provider.elevenlabs_url {
        bail!("ElevenLabs URL changed during normalization");
    }
    std::fs::write(
        run_dir.join("effective-experiment.json"),
        serde_json::to_vec_pretty(&effective)?,
    )?;
    let active = client
        .post(format!(
            "{base}/api/admin/runtime/runtime-stress/experiment/status"
        ))
        .header(reqwest::header::COOKIE, &cookie)
        .header("x-csrf-token", &csrf)
        .json(&json!({"status":"active"}))
        .send()
        .await?;
    ensure_response(active, "activate experiment").await?;
    Ok(AdminAuth {
        cookie,
        csrf,
        config_revision: effective["experiment"]["config_revision"]
            .as_i64()
            .unwrap_or(1),
    })
}

/// One staged virtual room and its public participant credentials.
struct StagedSession {
    room: String,
    a: Participant,
    b: Option<Participant>,
}

/// One public participant credential.
struct Participant {
    credential: String,
}

/// Stages intake before measurement, then drives ramp, steady, churn, and drain.
async fn run_workload(
    args: Args,
    base: String,
    ws: String,
    counters: Arc<Counters>,
    snapshots: watch::Sender<DashboardSnapshot>,
    cancelled: Arc<std::sync::atomic::AtomicBool>,
) -> Result<()> {
    let result = run_workload_inner(
        &args,
        base,
        ws,
        counters.clone(),
        snapshots.clone(),
        cancelled,
    )
    .await;
    let phase = if result.is_ok() { "complete" } else { "failed" };
    let _ = snapshots.send(workload_snapshot(&args, phase, &counters, true));
    result
}

/// Performs workload setup and traffic while the outer function guarantees terminal publication.
async fn run_workload_inner(
    args: &Args,
    base: String,
    ws: String,
    counters: Arc<Counters>,
    snapshots: watch::Sender<DashboardSnapshot>,
    cancelled: Arc<std::sync::atomic::AtomicBool>,
) -> Result<()> {
    let client = reqwest::Client::new();
    let mut staged = Vec::with_capacity(args.sessions);
    for session in 0..args.sessions {
        if cancelled.load(Ordering::Relaxed) {
            bail!("stress run cancelled during staging");
        }
        let a = create_participant(&client, &base)
            .await
            .with_context(|| format!("staging session {session} participant A"))?;
        consent(&client, &base, &a).await?;
        let room_a = create_room(&client, &base, &a).await?;
        let room = room_a["public_session_id"]
            .as_str()
            .context("session admission omitted public_session_id")?
            .to_string();
        let b = if args.pairing == Pairing::HumanHuman {
            let b = create_participant(&client, &base).await?;
            consent(&client, &base, &b).await?;
            let room_b = create_room(&client, &base, &b).await?;
            if room_b["public_session_id"] != room {
                bail!("human pair admitted to different rooms");
            }
            Some(b)
        } else {
            None
        };
        staged.push(StagedSession { room, a, b });
        counters.staged_sessions.fetch_add(1, Ordering::Relaxed);
        let _ = snapshots.send(workload_snapshot(&args, "staging", &counters, false));
        time::sleep(Duration::from_millis(15)).await;
    }
    let started = Instant::now();
    let mut last_rate_sample = started;
    let mut previous_rates = RateTotals::default();
    let deadline = started + Duration::from_secs(args.seconds);
    let ramp = Duration::from_secs(args.seconds).mul_f64(0.1);
    let mut tasks = Vec::with_capacity(staged.len());
    for (index, session) in staged.into_iter().enumerate() {
        let start_at = started + ramp.mul_f64(index as f64 / args.sessions as f64);
        let task_base = base.clone();
        let task_ws = ws.clone();
        let task_client = client.clone();
        let task_counters = counters.clone();
        let task_cancelled = cancelled.clone();
        let pairing = args.pairing;
        tasks.push(tokio::spawn(async move {
            time::sleep_until(start_at.into()).await;
            let result = drive_session(
                index,
                pairing,
                task_client,
                task_base,
                task_ws,
                session,
                started,
                deadline,
                task_counters.clone(),
                task_cancelled,
            )
            .await;
            if let Err(error) = &result {
                task_counters.fail(index, error);
            }
            result
        }));
    }
    while Instant::now() < deadline && !cancelled.load(Ordering::Relaxed) {
        let now = Instant::now();
        let phase = phase_name(started.elapsed(), Duration::from_secs(args.seconds));
        counters
            .elapsed_ms
            .store(started.elapsed().as_millis() as u64, Ordering::Relaxed);
        let rate_interval = now.duration_since(last_rate_sample);
        if rate_interval >= Duration::from_secs(1) {
            counters.sample_rates(&mut previous_rates, rate_interval);
            last_rate_sample = now;
        }
        let _ = snapshots.send(workload_snapshot(&args, phase, &counters, false));
        time::sleep(Duration::from_millis(250)).await;
    }
    for task in tasks {
        task.await.context("virtual participant task panicked")??;
    }
    if cancelled.load(Ordering::Relaxed) {
        bail!("stress run cancelled by user");
    }
    if counters.failures.load(Ordering::Relaxed) != 0 {
        bail!(counters
            .report()
            .first_failure
            .unwrap_or_else(|| "virtual participant failure".into()));
    }
    if counters.started_sessions.load(Ordering::Relaxed) != args.sessions as u64 {
        bail!("not all staged sessions started");
    }
    Ok(())
}

/// Drives public game/audio sockets for one staged session and verifies relay/TTS traffic.
async fn drive_session(
    index: usize,
    pairing: Pairing,
    client: reqwest::Client,
    base: String,
    ws: String,
    staged: StagedSession,
    measured_start: Instant,
    deadline: Instant,
    counters: Arc<Counters>,
    cancelled: Arc<std::sync::atomic::AtomicBool>,
) -> Result<()> {
    let mut game_a = game_socket(&client, &base, &ws, &staged.room, &staged.a).await?;
    let mut audio_a = audio_socket(&client, &base, &ws, &staged.room, &staged.a).await?;
    let mut game_b = if let Some(b) = &staged.b {
        Some(game_socket(&client, &base, &ws, &staged.room, b).await?)
    } else {
        None
    };
    let mut audio_b = if let Some(b) = &staged.b {
        Some(audio_socket(&client, &base, &ws, &staged.room, b).await?)
    } else {
        None
    };
    read_type(&mut game_a, "session_started").await?;
    send_json(&mut game_a, json!({"type":"ready"})).await?;
    if let Some(game) = &mut game_b {
        read_type(game, "session_started").await?;
        send_json(game, json!({"type":"ready"})).await?;
    }
    if pairing == Pairing::HumanAgent {
        read_tts_fingerprint(&mut audio_a, "agent reply initial").await?;
        counters.tts_audio.fetch_add(1, Ordering::Relaxed);
    }
    counters.started_sessions.fetch_add(1, Ordering::Relaxed);
    let mut sequence = 1u32;
    let mut reconnected = false;
    let mut next_message = Instant::now() + Duration::from_secs(2);
    let mut next_action = Instant::now() + Duration::from_secs(3);
    let mut next_frame = Instant::now();
    while Instant::now() < deadline && !cancelled.load(Ordering::Relaxed) {
        let phase = phase_name(
            measured_start.elapsed(),
            deadline.duration_since(measured_start),
        );
        if phase == "drain" {
            finish_session(&mut game_a, &staged.room, sequence, &counters).await?;
            break;
        }
        if phase == "churn" && index % 4 == 0 && !reconnected {
            let _ = game_a.close(None).await;
            let _ = audio_a.close(None).await;
            game_a = game_socket(&client, &base, &ws, &staged.room, &staged.a).await?;
            audio_a = audio_socket(&client, &base, &ws, &staged.room, &staged.a).await?;
            read_type(&mut game_a, "session_started").await?;
            send_json(&mut game_a, json!({"type":"ready"})).await?;
            counters.reconnects.fetch_add(2, Ordering::Relaxed);
            reconnected = true;
        }
        let encoded = AudioFrame {
            sequence,
            timestamp_ms: u64::from(sequence) * 20,
            pcm: pcm(index, 0, sequence),
        }
        .encode();
        audio_a.send(Message::Binary(encoded.clone())).await?;
        counters.pcm_sent.fetch_add(1, Ordering::Relaxed);
        if let Some(peer) = &mut audio_b {
            if read_binary(peer).await? != encoded {
                bail!("room {} role A PCM was corrupted or misrouted", staged.room);
            }
            counters.pcm_verified.fetch_add(1, Ordering::Relaxed);
            let reply = AudioFrame {
                sequence,
                timestamp_ms: u64::from(sequence) * 20,
                pcm: pcm(index, 1, sequence),
            }
            .encode();
            peer.send(Message::Binary(reply.clone())).await?;
            counters.pcm_sent.fetch_add(1, Ordering::Relaxed);
            if read_binary(&mut audio_a).await? != reply {
                bail!("room {} role B PCM was corrupted or misrouted", staged.room);
            }
            counters.pcm_verified.fetch_add(1, Ordering::Relaxed);
        }
        if Instant::now() >= next_message {
            let text = format!("stress:{index}:{sequence}");
            send_json(&mut game_a, json!({"type":"message","text":text})).await?;
            counters.game_messages.fetch_add(1, Ordering::Relaxed);
            next_message += Duration::from_secs(4);
            if pairing == Pairing::HumanAgent {
                read_tts_fingerprint(&mut audio_a, &format!("agent reply {text}")).await?;
                counters.tts_audio.fetch_add(1, Ordering::Relaxed);
            }
        }
        if Instant::now() >= next_action {
            send_json(&mut game_a, json!({"type":"action","action":{"type":"mark","sequence":sequence,"finish":false}})).await?;
            counters.actions.fetch_add(1, Ordering::Relaxed);
            next_action += Duration::from_secs(5);
        }
        sequence = sequence.wrapping_add(1);
        next_frame += FRAME_INTERVAL;
        if next_frame > Instant::now() {
            time::sleep_until(next_frame.into()).await;
        } else {
            next_frame = Instant::now();
        }
    }
    let _ = game_a.close(None).await;
    let _ = audio_a.close(None).await;
    if let Some(socket) = &mut game_b {
        let _ = socket.close(None).await;
    }
    if let Some(socket) = &mut audio_b {
        let _ = socket.close(None).await;
    }
    counters.finished_sessions.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

/// Keeps both participants connected until the server durably accepts session completion.
async fn finish_session(
    game: &mut Socket,
    room: &str,
    sequence: u32,
    counters: &Counters,
) -> Result<()> {
    let deadline = time::Instant::now() + Duration::from_secs(10);
    let mut last_rejection = None;
    loop {
        send_json(
            game,
            json!({"type":"action","action":{"type":"mark","sequence":sequence,"finish":true}}),
        )
        .await?;
        counters.actions.fetch_add(1, Ordering::Relaxed);
        let outcome = read_finish_outcome(game, deadline).await.with_context(|| {
            format!(
                "room {room} did not complete within 10s; last readiness rejection: {}",
                last_rejection.as_deref().unwrap_or("none")
            )
        })?;
        match outcome {
            FinishOutcome::Completed => return Ok(()),
            FinishOutcome::Retryable(code) => {
                last_rejection = Some(code);
                if time::Instant::now() >= deadline {
                    break;
                }
                time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
    bail!(
        "room {room} did not become ready for completion within 10s; last rejection: {}",
        last_rejection.unwrap_or_else(|| "none".to_string())
    )
}

/// One server response relevant to a requested terminal game transition.
enum FinishOutcome {
    /// The server committed and broadcast the terminal transition.
    Completed,
    /// The server reports a temporary room-readiness condition that may clear.
    Retryable(String),
}

/// Reads through unrelated broadcasts until completion or an actionable rejection arrives.
async fn read_finish_outcome(game: &mut Socket, deadline: time::Instant) -> Result<FinishOutcome> {
    loop {
        let remaining = deadline.saturating_duration_since(time::Instant::now());
        if remaining.is_zero() {
            bail!("timed out waiting for session completion");
        }
        let message = time::timeout(remaining, game.next())
            .await
            .context("timed out waiting for session completion")?
            .context("game socket closed before session completion")??;
        let Message::Text(text) = message else {
            continue;
        };
        let value: Value = serde_json::from_str(&text)?;
        match value["type"].as_str() {
            Some("completed") => return Ok(FinishOutcome::Completed),
            Some("action_rejected") => {
                let code = value["code"]
                    .as_str()
                    .unwrap_or("unknown_action_rejection")
                    .to_string();
                if matches!(
                    code.as_str(),
                    "transcription_not_ready" | "players_not_ready"
                ) {
                    return Ok(FinishOutcome::Retryable(code));
                }
                bail!("session completion action was rejected: {code}");
            }
            Some("error") => bail!("game socket error while completing session: {value}"),
            _ => {}
        }
    }
}

/// Creates a participant through public resident intake.
async fn create_participant(client: &reqwest::Client, base: &str) -> Result<Participant> {
    let value = response_value(
        client
            .post(format!("{base}/api/participants"))
            .json(&json!({}))
            .send()
            .await?,
        "participant creation",
    )
    .await?;
    Ok(Participant {
        credential: value["participant_credential"]
            .as_str()
            .context("participant credential missing")?
            .into(),
    })
}

/// Records the fixture's required consent through the participant endpoint.
async fn consent(client: &reqwest::Client, base: &str, participant: &Participant) -> Result<()> {
    ensure_response(
        client
            .post(format!("{base}/api/consent"))
            .bearer_auth(&participant.credential)
            .json(&json!({"decisions":{"stress":true}}))
            .send()
            .await?,
        "consent",
    )
    .await
}

/// Admits one participant through the normal room endpoint.
async fn create_room(
    client: &reqwest::Client,
    base: &str,
    participant: &Participant,
) -> Result<Value> {
    response_value(
        client
            .post(format!("{base}/api/sessions"))
            .bearer_auth(&participant.credential)
            .json(&json!({}))
            .send()
            .await?,
        "room admission",
    )
    .await
}

/// Opens a public game WebSocket from a one-use ticket.
async fn game_socket(
    client: &reqwest::Client,
    base: &str,
    ws: &str,
    room: &str,
    participant: &Participant,
) -> Result<Socket> {
    ticket_socket(client, base, ws, room, participant, "game").await
}

/// Opens a public audio WebSocket from a one-use ticket.
async fn audio_socket(
    client: &reqwest::Client,
    base: &str,
    ws: &str,
    room: &str,
    participant: &Participant,
) -> Result<Socket> {
    ticket_socket(client, base, ws, room, participant, "audio").await
}

/// Mints and consumes one production WebSocket ticket.
async fn ticket_socket(
    client: &reqwest::Client,
    base: &str,
    ws: &str,
    room: &str,
    participant: &Participant,
    kind: &str,
) -> Result<Socket> {
    let value = response_value(
        client
            .post(format!("{base}/api/sessions/{room}/{kind}-session"))
            .bearer_auth(&participant.credential)
            .json(&json!({}))
            .send()
            .await?,
        &format!("{kind} ticket"),
    )
    .await?;
    let token = value["token"]
        .as_str()
        .context("socket ticket omitted token")?;
    Ok(
        connect_async(format!("{ws}/ws/{kind}/{room}?token={token}"))
            .await?
            .0,
    )
}

/// Reads the next matching game protocol message.
async fn read_type(socket: &mut Socket, expected: &str) -> Result<Value> {
    for _ in 0..64 {
        let message = time::timeout(SOCKET_TIMEOUT, socket.next())
            .await
            .context("game socket timeout")?
            .context("game socket closed")??;
        if let Message::Text(text) = message {
            let value: Value = serde_json::from_str(&text)?;
            if value["type"] == "error" {
                bail!("game socket error: {value}");
            }
            if value["type"] == expected {
                return Ok(value);
            }
        }
    }
    bail!("did not receive {expected}")
}

/// Sends one game protocol JSON message.
async fn send_json(socket: &mut Socket, value: Value) -> Result<()> {
    socket.send(Message::Text(value.to_string())).await?;
    Ok(())
}

/// Reads one binary audio message within the correctness deadline.
async fn read_binary(socket: &mut Socket) -> Result<Vec<u8>> {
    loop {
        let message = time::timeout(Duration::from_secs(3), socket.next())
            .await
            .context("audio deadline")?
            .context("audio socket closed")??;
        if let Message::Binary(bytes) = message {
            return Ok(bytes);
        }
    }
}

/// Builds canonical deterministic PCM with a source fingerprint.
fn pcm(session: usize, role: u8, sequence: u32) -> Vec<u8> {
    let mut pcm = vec![0u8; 960];
    pcm[..8].copy_from_slice(&(session as u64).to_be_bytes());
    pcm[8] = role;
    pcm[9..13].copy_from_slice(&sequence.to_be_bytes());
    pcm[13] = 1;
    pcm
}

/// Verifies that published TTS PCM belongs to the exact requested agent message.
fn verify_tts_fingerprint(encoded: &[u8], text: &str) -> Result<()> {
    let frame = AudioFrame::decode(encoded)?;
    let expected = text.bytes().fold(0u8, u8::wrapping_add);
    if frame.pcm.iter().any(|byte| *byte != expected) {
        bail!("agent TTS payload fingerprint was wrong-room, reordered, or corrupt");
    }
    Ok(())
}

/// Consumes and verifies every frame in the fixture's five-frame TTS chunk.
async fn read_tts_fingerprint(socket: &mut Socket, text: &str) -> Result<()> {
    let expected = text.bytes().fold(0u8, u8::wrapping_add);
    let transcript_text = "agent reply stress";
    let transcript = transcript_text.bytes().fold(0u8, u8::wrapping_add);
    for _ in 0..8 {
        let first = read_binary(socket).await?;
        let fingerprint = AudioFrame::decode(&first)?.pcm[0];
        let matched_text = if fingerprint == expected {
            text
        } else if fingerprint == transcript {
            transcript_text
        } else {
            bail!("agent TTS payload fingerprint was wrong-room, reordered, or corrupt");
        };
        verify_tts_fingerprint(&first, matched_text)?;
        for _ in 1..5 {
            verify_tts_fingerprint(&read_binary(socket).await?, matched_text)?;
        }
        if fingerprint == expected {
            return Ok(());
        }
    }
    bail!("expected agent TTS message was not published")
}

/// Maps measured elapsed time to the fixed phase ratios.
fn phase_name(elapsed: Duration, total: Duration) -> &'static str {
    let ratio = elapsed.as_secs_f64() / total.as_secs_f64().max(0.001);
    if ratio < 0.1 {
        "ramp"
    } else if ratio < 0.5 {
        "steady"
    } else if ratio < 0.8 {
        "churn"
    } else {
        "drain"
    }
}

/// Attempts the normal inactive lifecycle transition during cleanup.
async fn deactivate(base: &str, admin: &AdminAuth) -> Result<()> {
    let response = reqwest::Client::new()
        .post(format!(
            "{base}/api/admin/runtime/runtime-stress/experiment/status"
        ))
        .header(reqwest::header::COOKIE, &admin.cookie)
        .header("x-csrf-token", &admin.csrf)
        .json(&json!({"status":"inactive"}))
        .send()
        .await?;
    ensure_response(response, "deactivate experiment").await
}

/// Polls production load telemetry until transports, ASR streams, and agents drain.
async fn wait_runtime_drain(base: &str, admin: &AdminAuth) -> Result<()> {
    let client = reqwest::Client::new();
    let deadline = time::Instant::now() + Duration::from_secs(15);
    loop {
        let response = client
            .get(format!("{base}/api/admin/load"))
            .header(reqwest::header::COOKIE, &admin.cookie)
            .send()
            .await?;
        let value = response_value(response, "read cleanup load telemetry").await?;
        let current = &value["current"];
        let connections = current["connections"]["game_connections"]
            .as_u64()
            .unwrap_or(u64::MAX)
            + current["connections"]["audio_connections"]
                .as_u64()
                .unwrap_or(u64::MAX);
        let agents = current["capacity"]["active_agents"]
            .as_u64()
            .unwrap_or(u64::MAX);
        let transcription = current["capacity"]["transcription_streams_reserved"]
            .as_u64()
            .unwrap_or(u64::MAX);
        if connections == 0 && agents == 0 && transcription == 0 {
            return Ok(());
        }
        if time::Instant::now() >= deadline {
            bail!("runtime resources did not drain before cleanup deadline: connections={connections}, agents={agents}, transcription_streams={transcription}");
        }
        time::sleep(Duration::from_millis(100)).await;
    }
}

/// Produces the same bounded measurement projection used by TUI and headless modes.
fn workload_snapshot(
    args: &Args,
    phase: &str,
    counters: &Counters,
    finished: bool,
) -> DashboardSnapshot {
    let report = counters.report();
    let rates = counters.rates.lock().expect("rate history mutex poisoned");
    let latest = |history: &VecDeque<u64>| history.back().copied().unwrap_or(0);
    let phase_position = match phase {
        "ramp" => "1/4",
        "steady" => "2/4",
        "churn" => "3/4",
        "drain" => "4/4",
        _ => "—",
    };
    let metric = |label: &str, value: String, health| DashboardMetric {
        label: label.into(),
        value,
        health,
    };
    DashboardSnapshot {
        title: format!("Parlando runtime stress r{HARNESS_REVISION}"),
        mode: PROFILE.into(),
        phase: Some(phase.into()),
        elapsed: Duration::from_millis(counters.elapsed_ms.load(Ordering::Relaxed)),
        duration: Duration::from_secs(args.seconds),
        finished,
        cancelled: false,
        failure: report.first_failure.clone(),
        panels: [
            DashboardPanel {
                title: " Workload ".into(),
                metrics: vec![
                    metric(
                        "Phase",
                        format!("{phase_position} {phase}"),
                        DashboardHealth::Neutral,
                    ),
                    metric(
                        "Target / staged",
                        format!("{} / {}", args.sessions, report.staged_sessions),
                        DashboardHealth::Neutral,
                    ),
                    metric(
                        "Started / finished",
                        format!("{} / {}", report.started_sessions, report.finished_sessions),
                        DashboardHealth::Good,
                    ),
                    metric(
                        "Messages/s / actions/s",
                        format!("{} / {}", latest(&rates.messages), latest(&rates.actions)),
                        DashboardHealth::Good,
                    ),
                    metric(
                        "Reconnects",
                        report.reconnects.to_string(),
                        DashboardHealth::Neutral,
                    ),
                ],
            },
            DashboardPanel {
                title: " Audio & correctness ".into(),
                metrics: vec![
                    metric(
                        "PCM sent / verified",
                        format!("{} / {}", report.pcm_sent, report.pcm_verified),
                        DashboardHealth::Good,
                    ),
                    metric(
                        "Agent TTS",
                        format!("{} total · {}/s", report.tts_audio, latest(&rates.tts)),
                        DashboardHealth::Good,
                    ),
                    metric(
                        "Failures",
                        report.failures.to_string(),
                        if report.failures == 0 {
                            DashboardHealth::Good
                        } else {
                            DashboardHealth::Error
                        },
                    ),
                ],
            },
            DashboardPanel {
                title: " Process & storage ".into(),
                metrics: vec![
                    metric("Profile", PROFILE.into(), DashboardHealth::Neutral),
                    metric(
                        "Pairing",
                        format!("{:?}", args.pairing),
                        DashboardHealth::Neutral,
                    ),
                    metric("Seed", args.seed.to_string(), DashboardHealth::Neutral),
                    metric(
                        "PCM throughput",
                        format!("{} frames/s", latest(&rates.pcm)),
                        DashboardHealth::Good,
                    ),
                ],
            },
        ],
        series: vec![
            DashboardSeries {
                title: " Actions / second ".into(),
                values: rates.actions.iter().copied().collect(),
            },
            DashboardSeries {
                title: " Messages / second ".into(),
                values: rates.messages.iter().copied().collect(),
            },
            DashboardSeries {
                title: " PCM frames / second ".into(),
                values: rates.pcm.iter().copied().collect(),
            },
            DashboardSeries {
                title: " TTS publications / second ".into(),
                values: rates.tts.iter().copied().collect(),
            },
        ],
        tiles: counters
            .room_failures
            .lock()
            .expect("room failure mutex poisoned")
            .iter()
            .map(|failed| DashboardTile {
                health: if *failed {
                    DashboardHealth::Error
                } else {
                    DashboardHealth::Good
                },
            })
            .collect(),
        tile_legend: "green healthy; red failed invariant".into(),
        events: report.first_failure.into_iter().collect(),
        footer: "phases: ramp → steady → churn → drain; JSON report is authoritative".into(),
    }
}

/// Returns dashboard snapshots unchanged so presentation cannot alter measurements.
fn dashboard_snapshot(snapshot: &DashboardSnapshot) -> DashboardSnapshot {
    snapshot.clone()
}

/// Prints decision-oriented findings while retaining JSON as the authoritative artifact.
fn print_report_summary(report: &Report, path: &Path) {
    let outcome = if report.success { "PASS" } else { "FAIL" };
    let seconds = report.seconds.max(1) as f64;
    let rate = |total: u64| total as f64 / seconds;
    let completion = if report.sessions == 0 {
        0.0
    } else {
        report.workload.finished_sessions as f64 * 100.0 / report.sessions as f64
    };
    let pairing = match report.pairing {
        Pairing::HumanAgent => "human-agent",
        Pairing::HumanHuman => "human-human",
    };
    let session_noun = if report.sessions == 1 {
        "session"
    } else {
        "sessions"
    };
    let run_directory = path.parent().unwrap_or_else(|| Path::new("."));
    eprintln!();
    eprintln!(
        "Parlando runtime stress: {outcome} — {pairing}, {} {session_noun} for {}s",
        report.sessions, report.seconds
    );
    if report.success {
        eprintln!(
            "  Capacity: sustained all {} fixture sessions; this is a tested lower bound, not a maximum.",
            report.sessions
        );
    } else {
        eprintln!(
            "  Capacity: this run did not establish {}-session capacity ({completion:.1}% finished).",
            report.sessions
        );
    }
    eprintln!(
        "  Completion: {} staged, {} started, {} finished; {} invariant/transport failures.",
        report.workload.staged_sessions,
        report.workload.started_sessions,
        report.workload.finished_sessions,
        report.workload.failures
    );
    eprintln!(
        "  Average load: {:.1} PCM frames/s, {:.1} messages/s, {:.1} offered actions/s, {:.1} TTS publications/s.",
        rate(report.workload.pcm_sent),
        rate(report.workload.game_messages),
        rate(report.workload.actions),
        rate(report.workload.tts_audio)
    );
    match report.pairing {
        Pairing::HumanHuman => eprintln!(
            "  Audio: {} of {} full-duplex relay frames verified ({:.1}%).",
            report.workload.pcm_verified,
            report.workload.pcm_sent,
            if report.workload.pcm_sent == 0 { 0.0 } else {
                report.workload.pcm_verified as f64 * 100.0 / report.workload.pcm_sent as f64
            }
        ),
        Pairing::HumanAgent => eprintln!(
            "  Audio: human PCM exercised ASR; {} final transcripts and {} fingerprint-verified TTS publications were durable.",
            report.database.transcripts, report.workload.tts_audio
        ),
    }
    let bytes_per_session = if report.database.sessions <= 0 {
        0
    } else {
        report.database.checkpointed_bytes / report.database.sessions as u64
    };
    eprintln!(
        "  Durability: {} events, {} accepted actions, {}/{} terminal events; {:.1} KiB SQLite/session.",
        report.database.events,
        report.database.actions,
        report.database.terminal_events,
        report.database.sessions,
        bytes_per_session as f64 / 1024.0
    );
    if let Some(rss) = report.server.sampled_rss_bytes {
        eprintln!(
            "  Server snapshot: {:.1} MiB RSS, {} open FDs, {:.3}s CPU; cleanup took {} ms.",
            rss as f64 / (1024.0 * 1024.0),
            report
                .server
                .sampled_open_fds
                .map_or_else(|| "n/a".into(), |value| value.to_string()),
            report.server.sampled_cpu_seconds.unwrap_or_default(),
            report.cleanup_ms
        );
    } else {
        eprintln!(
            "  Cleanup: {} ms; server {}; providers {}.",
            report.cleanup_ms, report.server.status, report.providers.status
        );
    }
    if let Some(error) = &report.error {
        eprintln!("  First actionable failure: {error}");
    }
    eprintln!("  Artifacts: {}", run_directory.display());
    eprintln!("  JSON report: {}", path.display());
    if report.success {
        let increased =
            (report.sessions.saturating_mul(5).saturating_add(3) / 4).max(report.sessions + 1);
        eprintln!("  Next: repeat this exact run three times to establish consistency:");
        eprintln!("        cargo stress --sessions {} --seconds {} --pairing {pairing} --seed {} --headless", report.sessions, report.seconds, report.seed);
        eprintln!("        Then probe higher: cargo stress --sessions {increased} --seconds {} --pairing {pairing} --seed {} --headless", report.seconds.max(120), report.seed);
    } else {
        eprintln!(
            "  Next: inspect {}/server.stderr.log and retry the same seed headlessly:",
            run_directory.display()
        );
        eprintln!("        cargo stress --sessions {} --seconds {} --pairing {pairing} --seed {} --headless --keep", report.sessions, report.seconds, report.seed);
    }
}

/// Opens the stopped SQLite database, checkpoints WAL, and verifies durable invariants.
async fn verify_database(
    database: &Path,
    expected_sessions: usize,
    pairing: Pairing,
) -> Result<DatabaseReport> {
    let bytes = |path: &Path| std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let wal = PathBuf::from(format!("{}-wal", database.display()));
    let shm = PathBuf::from(format!("{}-shm", database.display()));
    let main_bytes = bytes(database);
    let wal_bytes = bytes(&wal);
    let shm_bytes = bytes(&shm);
    let pool = sqlx::SqlitePool::connect(&format!("sqlite:///{}", database.display())).await?;
    let count = |table: &'static str| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>(&format!("select count(*) from {table}"))
                .fetch_one(&pool)
                .await
        }
    };
    let participants = count("participants").await?;
    let consents = count("consent_declarations").await?;
    let sessions = count("sessions").await?;
    let memberships = count("session_participants").await?;
    let events = count("session_events").await?;
    let event_count = |kind: &'static str| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>("select count(*) from session_events where event_type = ?")
                .bind(kind)
                .fetch_one(&pool)
                .await
        }
    };
    let actions = event_count("game_action_accepted").await?;
    let transcripts = sqlx::query_scalar::<_, i64>("select count(*) from session_events where event_type = 'conversation_message' and json_extract(payload_json, '$.origin') = 'voice_transcript'").fetch_one(&pool).await?;
    let terminal_events = sqlx::query_scalar::<_, i64>("select count(*) from session_events where event_type in ('session_completed','session_abandoned','session_expired')").fetch_one(&pool).await?;
    if sessions != expected_sessions as i64 {
        bail!("SQLite sessions {sessions} did not match target {expected_sessions}");
    }
    let expected_participants = if pairing == Pairing::HumanHuman {
        expected_sessions as i64 * 2
    } else {
        expected_sessions as i64 + 1
    };
    let expected_consents =
        expected_sessions as i64 * if pairing == Pairing::HumanHuman { 2 } else { 1 };
    if participants != expected_participants || consents != expected_consents {
        bail!("SQLite participant/consent counts are inconsistent");
    }
    let expected_memberships = expected_sessions as i64 * 2;
    if memberships < expected_memberships || events == 0 || actions == 0 {
        bail!("SQLite is missing memberships, events, or actions");
    }
    if transcripts == 0 {
        bail!("SQLite contains no final Speechmatics transcripts");
    }
    if terminal_events != expected_sessions as i64 {
        bail!(
            "SQLite terminal events {terminal_events} did not match sessions {expected_sessions}"
        );
    }
    sqlx::query("pragma wal_checkpoint(truncate)")
        .execute(&pool)
        .await?;
    pool.close().await;
    Ok(DatabaseReport {
        participants,
        consents,
        sessions,
        memberships,
        events,
        transcripts,
        actions,
        terminal_events,
        main_bytes,
        wal_bytes,
        shm_bytes,
        checkpointed_bytes: bytes(database),
    })
}

/// Computes and raises the inherited descriptor soft limit before spawning children.
fn descriptor_preflight(sessions: usize, pairing: Pairing) -> Result<DescriptorReport> {
    let required = 128
        + sessions as u64
            * if pairing == Pairing::HumanHuman {
                10
            } else {
                8
            };
    #[cfg(unix)]
    unsafe {
        let mut limit = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        if libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let original = limit.rlim_cur as u64;
        let hard = limit.rlim_max as u64;
        if hard < required {
            bail!("descriptor hard limit {hard} is below required {required}");
        }
        limit.rlim_cur = required.max(original) as libc::rlim_t;
        if libc::setrlimit(libc::RLIMIT_NOFILE, &limit) != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        return Ok(DescriptorReport {
            original_soft: original,
            hard,
            adjusted_soft: limit.rlim_cur as u64,
            required,
        });
    }
    #[cfg(not(unix))]
    {
        Ok(DescriptorReport {
            original_soft: 0,
            hard: 0,
            adjusted_soft: 0,
            required,
        })
    }
}

/// Spawns one child with stable stdout/stderr artifacts and an owned Unix process group.
fn spawn_logged(
    executable: &Path,
    arguments: &[&str],
    run_dir: &Path,
    name: &str,
) -> Result<Child> {
    let stdout = File::create(run_dir.join(format!("{name}.stdout.log")))?;
    let stderr = File::create(run_dir.join(format!("{name}.stderr.log")))?;
    let mut command = Command::new(executable);
    command
        .args(arguments)
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    #[cfg(unix)]
    unsafe {
        command.pre_exec(|| {
            if libc::setpgid(0, 0) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    command
        .spawn()
        .with_context(|| format!("spawning {name} child"))
}

/// Waits for an atomic provider readiness file or diagnoses early exit.
async fn wait_readiness(path: &Path, child: &mut Child) -> Result<ProviderReady> {
    let deadline = time::Instant::now() + Duration::from_secs(15);
    loop {
        if let Ok(bytes) = std::fs::read(path) {
            return Ok(serde_json::from_slice(&bytes)?);
        }
        if let Some(status) = child.try_wait()? {
            bail!("provider child exited during startup: {status}");
        }
        if time::Instant::now() >= deadline {
            bail!("provider readiness deadline expired");
        }
        time::sleep(Duration::from_millis(25)).await;
    }
}

/// Polls the production health route until SQLite is writable.
async fn wait_health(base: &str, child: &mut Child) -> Result<()> {
    let deadline = time::Instant::now() + Duration::from_secs(20);
    loop {
        if reqwest::get(format!("{base}/health"))
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false)
        {
            return Ok(());
        }
        if let Some(status) = child.try_wait()? {
            bail!("server child exited during startup: {status}");
        }
        if time::Instant::now() >= deadline {
            bail!("server health deadline expired");
        }
        time::sleep(Duration::from_millis(50)).await;
    }
}

/// Reserves an ephemeral address before the server child starts.
async fn reserve_address() -> Result<SocketAddr> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    drop(listener);
    Ok(address)
}

/// Stops a child, escalating after a bounded graceful interval.
async fn stop_child(child: &mut Child) -> ChildReport {
    let resources = child.id().and_then(sample_process);
    if let Ok(Some(status)) = child.try_wait() {
        return child_report(status, false, resources);
    }
    #[cfg(unix)]
    if let Some(id) = child.id() {
        unsafe {
            libc::kill(-(id as i32), libc::SIGTERM);
        }
    }
    if let Ok(Ok(status)) = time::timeout(Duration::from_secs(5), child.wait()).await {
        return child_report(status, false, resources);
    }
    let _ = child.kill().await;
    let status = child.wait().await.ok();
    ChildReport {
        status: status
            .map(|s| s.to_string())
            .unwrap_or_else(|| "unknown".into()),
        forced: true,
        sampled_rss_bytes: resources.as_ref().and_then(|r| r.0),
        sampled_cpu_seconds: resources.as_ref().and_then(|r| r.1),
        sampled_open_fds: resources.and_then(|r| r.2),
    }
}

/// Converts an exit status into its durable representation.
fn child_report(
    status: ExitStatus,
    forced: bool,
    resources: Option<(Option<u64>, Option<f64>, Option<usize>)>,
) -> ChildReport {
    ChildReport {
        status: status.to_string(),
        forced,
        sampled_rss_bytes: resources.as_ref().and_then(|r| r.0),
        sampled_cpu_seconds: resources.as_ref().and_then(|r| r.1),
        sampled_open_fds: resources.and_then(|r| r.2),
    }
}

/// Samples one child process without changing its state.
fn sample_process(pid: u32) -> Option<(Option<u64>, Option<f64>, Option<usize>)> {
    #[cfg(target_os = "macos")]
    {
        return sample_macos_process(pid);
    }
    #[cfg(not(target_os = "macos"))]
    {
        let output = std::process::Command::new("ps")
            .args(["-o", "rss=", "-o", "%cpu=", "-p", &pid.to_string()])
            .output()
            .ok()?;
        let text = String::from_utf8(output.stdout).ok()?;
        let mut fields = text.split_whitespace();
        let rss = fields
            .next()
            .and_then(|value| value.parse::<u64>().ok())
            .map(|kib| kib * 1024);
        let cpu = fields.next().and_then(|value| value.parse::<f64>().ok());
        let fds = std::fs::read_dir(format!("/proc/{pid}/fd"))
            .ok()
            .map(|items| items.count())
            .or_else(|| {
                std::process::Command::new("lsof")
                    .args(["-a", "-p", &pid.to_string(), "-Fn"])
                    .output()
                    .ok()
                    .map(|output| {
                        String::from_utf8_lossy(&output.stdout)
                            .lines()
                            .filter(|line| line.starts_with('f'))
                            .count()
                    })
            });
        Some((rss, cpu, fds))
    }
}

/// Reads resident memory, CPU time, and descriptor count using macOS libproc.
#[cfg(target_os = "macos")]
fn sample_macos_process(pid: u32) -> Option<(Option<u64>, Option<f64>, Option<usize>)> {
    #[repr(C)]
    #[derive(Default)]
    struct TaskInfo {
        virtual_size: u64,
        resident_size: u64,
        total_user: u64,
        total_system: u64,
        threads_user: u64,
        threads_system: u64,
        policy: i32,
        faults: i32,
        pageins: i32,
        cow_faults: i32,
        messages_sent: i32,
        messages_received: i32,
        syscalls_mach: i32,
        syscalls_unix: i32,
        csw: i32,
        threadnum: i32,
        numrunning: i32,
        priority: i32,
    }
    unsafe extern "C" {
        fn proc_pidinfo(
            pid: i32,
            flavor: i32,
            arg: u64,
            buffer: *mut libc::c_void,
            buffersize: i32,
        ) -> i32;
    }
    let mut info = TaskInfo::default();
    let size = std::mem::size_of::<TaskInfo>() as i32;
    let read = unsafe { proc_pidinfo(pid as i32, 4, 0, (&mut info as *mut TaskInfo).cast(), size) };
    if read != size {
        return None;
    }
    let cpu_seconds = (info.total_user + info.total_system) as f64 / 1_000_000_000.0;
    let fd_bytes = unsafe { proc_pidinfo(pid as i32, 1, 0, std::ptr::null_mut(), 0) };
    let fd_count = (fd_bytes > 0).then_some(fd_bytes as usize / 8);
    Some((Some(info.resident_size), Some(cpu_seconds), fd_count))
}

/// Writes JSON by rename so partial readiness is never observable.
fn atomic_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let temporary = path.with_extension("tmp");
    let mut file = File::create(&temporary)?;
    file.write_all(&serde_json::to_vec(value)?)?;
    file.sync_all()?;
    std::fs::rename(temporary, path)?;
    Ok(())
}

/// Returns the fixed 1:4:3:2 phase split, preserving the requested total.
fn phase_durations(total: u64) -> BTreeMap<&'static str, u64> {
    let ramp = total / 10;
    let steady = total * 4 / 10;
    let churn = total * 3 / 10;
    BTreeMap::from([
        ("ramp", ramp),
        ("steady", steady),
        ("churn", churn),
        ("drain", total - ramp - steady - churn),
    ])
}

/// Decodes a JSON response while preserving an unsuccessful body.
async fn response_value(response: reqwest::Response, operation: &str) -> Result<Value> {
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        bail!("{operation} returned {status}: {body}");
    }
    serde_json::from_str(&body).with_context(|| format!("decoding {operation}"))
}

/// Checks an HTTP response while preserving its diagnostic body.
async fn ensure_response(response: reqwest::Response, operation: &str) -> Result<()> {
    response_value(response, operation).await.map(|_| ())
}

/// Resolves Ctrl-C and Unix termination into the provider's graceful shutdown future.
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut terminate = signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler");
        tokio::select! { _ = signal::ctrl_c() => {}, _ = terminate.recv() => {} }
    }
    #[cfg(not(unix))]
    {
        let _ = signal::ctrl_c().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Confirms the public parser rejects hidden or implementation-detail knobs.
    #[test]
    fn public_cli_is_deliberately_small() {
        assert!(Args::try_parse_from(["runtime-stress", "--sessions", "4", "--headless"]).is_ok());
        assert!(Args::try_parse_from(["runtime-stress", "--database", "x"]).is_err());
        assert!(Args::try_parse_from(["runtime-stress", "__server"]).is_err());
    }

    /// Prevents an impossible participant burst before the supervisor creates any artifacts.
    #[test]
    fn workload_preflight_uses_bundled_participant_creation_limit() {
        assert!(workload_preflight(150, Pairing::HumanHuman).is_ok());
        let error = workload_preflight(200, Pairing::HumanHuman)
            .expect_err("400 direct participants must exceed the bundled ceiling");
        let message = error.to_string();
        assert!(message.contains("requires 400 direct participant creations"));
        assert!(message.contains("Use --sessions 150 or lower"));
        assert!(message.contains("rust-server/config/runtime-limits.json"));
        assert!(workload_preflight(300, Pairing::HumanAgent).is_ok());
    }

    /// Confirms phase rounding retains the requested measured duration.
    #[test]
    fn phase_split_preserves_total() {
        assert_eq!(phase_durations(61).values().sum::<u64>(), 61);
        assert_eq!(
            phase_durations(60),
            BTreeMap::from([("churn", 18), ("drain", 12), ("ramp", 6), ("steady", 24)])
        );
    }

    /// Locks the Speechmatics startup contract exercised by the external peer.
    #[test]
    fn speechmatics_contract_validation() {
        let valid = json!({"message":"StartRecognition","audio_format":{"type":"raw","encoding":"pcm_s16le","sample_rate":24000},
            "transcription_config":{"language":"en","enable_partials":true,"max_delay":0.7}});
        assert!(validate_speechmatics_start(&valid).is_ok());
        let mut invalid = valid;
        invalid["audio_format"]["sample_rate"] = json!(16000);
        assert!(validate_speechmatics_start(&invalid).is_err());
    }

    /// Locks the ElevenLabs initial authentication and voice-settings contract.
    #[test]
    fn elevenlabs_contract_validation() {
        let valid = json!({"text":" ","xi_api_key":PROVIDER_KEY,
            "voice_settings":{"stability":0.5,"similarity_boost":0.75}});
        assert!(validate_elevenlabs_initial(&valid).is_ok());
        let mut invalid = valid;
        invalid["xi_api_key"] = json!("wrong");
        assert!(validate_elevenlabs_initial(&invalid).is_err());
    }

    /// Confirms presentation receives the same authoritative counter projection.
    #[test]
    fn headless_and_tui_share_snapshot_data() {
        let args =
            Args::try_parse_from(["runtime-stress", "--sessions", "2", "--seconds", "10"]).unwrap();
        let counters = Counters::default();
        counters.staged_sessions.store(2, Ordering::Relaxed);
        counters.game_messages.store(7, Ordering::Relaxed);
        counters.actions.store(3, Ordering::Relaxed);
        counters.sample_rates(&mut RateTotals::default(), Duration::from_secs(1));
        counters.room_failures.lock().unwrap().resize(2, false);
        let snapshot = workload_snapshot(&args, "steady", &counters, false);
        let rendered = dashboard_snapshot(&snapshot);
        assert_eq!(
            snapshot.panels[0].metrics[0].value,
            rendered.panels[0].metrics[0].value
        );
        assert_eq!(snapshot.phase, rendered.phase);
        assert_eq!(snapshot.panels[0].metrics[0].value, "2/4 steady");
        assert_eq!(snapshot.tiles.len(), 2);
        assert_eq!(snapshot.series.len(), 4);
        assert_eq!(snapshot.series[0].values, vec![3]);
        assert_eq!(snapshot.series[1].values, vec![7]);
    }

    /// Confirms early workload failure always publishes a terminal dashboard snapshot.
    #[tokio::test]
    async fn failed_workload_finishes_dashboard() {
        let args = Args::try_parse_from([
            "runtime-stress",
            "--sessions",
            "1",
            "--seconds",
            "10",
            "--headless",
        ])
        .unwrap();
        let counters = Arc::new(Counters::default());
        counters.room_failures.lock().unwrap().resize(1, false);
        let (sender, receiver) =
            watch::channel(workload_snapshot(&args, "staging", &counters, false));
        let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let result = run_workload(
            args,
            "http://unused".into(),
            "ws://unused".into(),
            counters,
            sender,
            cancelled,
        )
        .await;
        assert!(result.is_err());
        assert!(receiver.borrow().finished);
        assert_eq!(receiver.borrow().phase.as_deref(), Some("failed"));
    }
}
