//! Scenario-oriented process-boundary tests for Parlando's Prolific integration.

use std::{
    fs::File,
    future::Future,
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, bail, Context, Result};
use chrono::DateTime;
use clap::Parser;
use futures_util::{SinkExt, StreamExt};
use reqwest::{header, Client, Response, StatusCode};
use serde::Serialize;
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::time;
use tokio_tungstenite::{connect_async, tungstenite::Message};

const EXPERIMENT_ID: &str = "prolific-test";
const API_TOKEN: &str = "prolific-test-token";
const STUDY_ID: &str = "study1";

/// Public runner options intended for both local use and CI.
#[derive(Clone, Debug, Parser)]
#[command(about = "Run Parlando's Prolific integration scenario matrix")]
struct Args {
    /// Print the scenario catalogue without starting any processes.
    #[arg(long)]
    list: bool,
    /// Stop after the first failed scenario rather than recording later skips.
    #[arg(long)]
    fail_fast: bool,
    /// Preserve the temporary SQLite directory even after a successful run.
    #[arg(long)]
    keep: bool,
    /// Directory beneath which timestamped logs and reports are written.
    #[arg(long, default_value = "target/prolific-tests")]
    output: PathBuf,
    /// Explicit mock executable, primarily for packaged or release builds.
    #[arg(long)]
    mock_bin: Option<PathBuf>,
    /// Explicit test-server executable, primarily for packaged or release builds.
    #[arg(long)]
    server_bin: Option<PathBuf>,
}

/// Static entry shown before execution so users know the complete intended matrix.
struct ScenarioDefinition {
    id: &'static str,
    phase: &'static str,
    name: &'static str,
}

const SCENARIOS: &[ScenarioDefinition] = &[
    ScenarioDefinition {
        id: "ENV-01",
        phase: "Environment",
        name: "Standalone mock and Parlando processes become healthy",
    },
    ScenarioDefinition {
        id: "CFG-01",
        phase: "Game setup",
        name: "Configured API base and token require no global workspace binding",
    },
    ScenarioDefinition {
        id: "ACT-01",
        phase: "Activation",
        name: "Incorrect completion action and external URL block activation",
    },
    ScenarioDefinition {
        id: "ACT-02",
        phase: "Activation",
        name: "Documented study, project, URL, and completion paths activate",
    },
    ScenarioDefinition {
        id: "JWT-01",
        phase: "Admission",
        name: "Valid Secure external URL token admits a Prolific participant",
    },
    ScenarioDefinition {
        id: "JWT-02",
        phase: "Admission",
        name: "Tampered Secure external URL token is rejected",
    },
    ScenarioDefinition {
        id: "JWT-03",
        phase: "Admission",
        name: "Signed identity mismatch is rejected",
    },
    ScenarioDefinition {
        id: "MATCH-01",
        phase: "Waiting room",
        name: "Two Prolific participants form one active dyadic session",
    },
    ScenarioDefinition {
        id: "WAIT-01",
        phase: "Waiting room",
        name: "Active waiting-room leave records Game did not start handoff",
    },
    ScenarioDefinition {
        id: "WAIT-02",
        phase: "Waiting room",
        name: "Waiting deadline records the same Game did not start handoff",
    },
    ScenarioDefinition {
        id: "GAME-01",
        phase: "Active game",
        name: "Active leaver and good-faith partner receive distinct outcomes",
    },
    ScenarioDefinition {
        id: "FLOW-01",
        phase: "Completion",
        name: "A normal dyadic game completes for both Prolific participants",
    },
    ScenarioDefinition {
        id: "FLOW-02",
        phase: "Completion",
        name: "Terminal Prolific submission re-entry resumes the durable result",
    },
    ScenarioDefinition {
        id: "FLOW-03",
        phase: "Fallback",
        name: "Unsigned launch is verified through the submission API",
    },
    ScenarioDefinition {
        id: "CONSENT-01",
        phase: "Consent",
        name: "Required consent blocks Prolific matchmaking until accepted",
    },
    ScenarioDefinition {
        id: "MATCH-02",
        phase: "Matchmaking",
        name: "Prolific recruitment rejects direct intake and forms a Prolific-only dyad",
    },
    ScenarioDefinition {
        id: "FORM-01",
        phase: "Before start",
        name: "Assigned pre-start leaver and remaining partner receive distinct outcomes",
    },
    ScenarioDefinition {
        id: "FORM-02",
        phase: "Before start",
        name: "Assigned dyad which never connects reaches one coherent waiting deadline",
    },
    ScenarioDefinition {
        id: "CONN-01",
        phase: "Connection",
        name: "Brief active-game disconnect pauses and resumes the same session",
    },
    ScenarioDefinition {
        id: "CONN-02",
        phase: "Connection",
        name: "Single reconnect expiry distinguishes disconnected and waiting roles",
    },
    ScenarioDefinition {
        id: "CONN-03",
        phase: "Connection",
        name: "Two expired disconnects classify both participants as connection lost",
    },
    ScenarioDefinition {
        id: "IDLE-01",
        phase: "Inactivity",
        name: "No meaningful activity expires both participants",
    },
    ScenarioDefinition {
        id: "IDLE-02",
        phase: "Inactivity",
        name: "Heartbeats preserve transport but do not extend the idle deadline",
    },
    ScenarioDefinition {
        id: "IDLE-03",
        phase: "Inactivity",
        name: "Accepted game activity extends the shared idle deadline",
    },
    ScenarioDefinition {
        id: "LIFE-01",
        phase: "Lifetime",
        name: "Absolute lifetime approves both otherwise healthy participants",
    },
    ScenarioDefinition {
        id: "RACE-01",
        phase: "Race",
        name: "Simultaneous explicit leaves commit one actor and one partner result",
    },
    ScenarioDefinition {
        id: "RACE-02",
        phase: "Race",
        name: "Completion racing with leave commits one coherent terminal result",
    },
    ScenarioDefinition {
        id: "RACE-03",
        phase: "Replay",
        name: "Repeated terminal leave and admission preserve the first result",
    },
    ScenarioDefinition {
        id: "WAIT-03",
        phase: "Waiting room",
        name: "Brief waiting-room disconnect reconnects without extending matchmaking",
    },
    ScenarioDefinition {
        id: "WAIT-04",
        phase: "Waiting room",
        name: "Waiting-room reconnect expiry records connection lost",
    },
    ScenarioDefinition {
        id: "IDLE-04",
        phase: "Inactivity",
        name: "Rejected game input does not extend the idle deadline",
    },
    ScenarioDefinition {
        id: "CONSENT-02",
        phase: "Consent",
        name: "Explicitly declined required consent remains outside matchmaking",
    },
    ScenarioDefinition {
        id: "CONSENT-03",
        phase: "Consent",
        name: "Accepted required consent enables ordinary Prolific matchmaking",
    },
    ScenarioDefinition {
        id: "DASH-01",
        phase: "Dashboard",
        name: "Session details expose identities, deadlines, causes, and recipient results",
    },
    ScenarioDefinition {
        id: "SAFE-01",
        phase: "Provider safety",
        name: "Provider journal contains only authenticated read operations",
    },
];

/// One terminal status retained in both the human table and JSON report.
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum ScenarioStatus {
    Passed,
    Failed,
    Skipped,
}

/// Durable result for one named scenario.
#[derive(Debug, Serialize)]
struct ScenarioResult {
    id: &'static str,
    phase: &'static str,
    name: &'static str,
    status: ScenarioStatus,
    elapsed_ms: u128,
    detail: Option<String>,
}

/// Machine-readable summary written after children have been stopped.
#[derive(Debug, Serialize)]
struct RunReport {
    schema_version: u32,
    started_at_unix_seconds: u64,
    elapsed_ms: u128,
    mock_base_url: String,
    parlando_base_url: String,
    database_path: Option<String>,
    database_preserved: bool,
    success: bool,
    passed: usize,
    failed: usize,
    skipped: usize,
    scenarios: Vec<ScenarioResult>,
}

/// Child process which is always terminated before the runner returns.
struct ManagedChild {
    name: &'static str,
    child: Child,
    stdout_path: PathBuf,
    stderr_path: PathBuf,
}

impl ManagedChild {
    /// Returns early when a supposedly live child has exited.
    fn ensure_running(&mut self) -> Result<()> {
        if let Some(status) = self.child.try_wait()? {
            bail!(
                "{} exited with {status}; inspect {} and {}",
                self.name,
                self.stdout_path.display(),
                self.stderr_path.display()
            );
        }
        Ok(())
    }

    /// Stops the child and reaps it without treating test-directed termination as failure.
    fn stop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

impl Drop for ManagedChild {
    /// Prevents orphan mock or server processes after errors and interrupts.
    fn drop(&mut self) {
        self.stop();
    }
}

/// Authenticated administrator connection shared across experiment runtimes.
#[derive(Clone)]
struct AdminAuth {
    cookie: String,
    csrf: String,
}

/// Opaque public participant access and its retained research pseudonym.
#[derive(Clone)]
struct Participant {
    credential: String,
    research_id: String,
    experiment_id: String,
}

/// Mutable facts passed between dependent admission and lifecycle scenarios.
#[derive(Default)]
struct ScenarioContext {
    first_participant: Option<Participant>,
    paired_session: Option<String>,
    unmatched_session: Option<String>,
    timed_out_waiting_session: Option<String>,
    partner_left_session: Option<String>,
    completed_session: Option<String>,
    completed_research_id: Option<String>,
    forming_left_session: Option<String>,
    reconnect_session: Option<String>,
    both_disconnected_session: Option<String>,
    idle_session: Option<String>,
    lifetime_session: Option<String>,
}

type GameSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Runs the complete matrix and writes artifacts regardless of scenario success.
#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    print_catalogue();
    if args.list {
        return Ok(());
    }
    let started_at = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let started = Instant::now();
    let run_dir = args
        .output
        .join(format!("{started_at}-{}", std::process::id()));
    std::fs::create_dir_all(&run_dir)?;
    println!("\nArtifacts: {}", run_dir.display());

    let (mock_bin, server_bin) = companion_binaries(&args)?;
    let temporary = tempfile::Builder::new()
        .prefix("parlando-prolific-tests-")
        .tempdir()?;
    let database = temporary.path().join("prolific-test.sqlite");
    let mock_address = reserve_address()?;
    let server_address = reserve_address()?;
    let mock_base = format!("http://{mock_address}");
    let server_base = format!("http://{server_address}");
    let mut mock = spawn_logged(
        "prolific-mock",
        &mock_bin,
        &["--bind".into(), mock_address.to_string()],
        &run_dir,
    )?;
    let mut server = spawn_logged(
        "prolific-test-server",
        &server_bin,
        &[
            "--bind".into(),
            server_address.to_string(),
            "--database".into(),
            database.display().to_string(),
        ],
        &run_dir,
    )?;
    let client = Client::builder().timeout(Duration::from_secs(15)).build()?;
    let mut results = Vec::new();
    let mut context = ScenarioContext::default();

    let environment_ok = run_scenario(&mut results, &SCENARIOS[0], async {
        wait_health(&client, &mock_base, "/__mock/health", &mut mock).await?;
        wait_health(&client, &server_base, "/health", &mut server).await?;
        Ok("two independent child processes are accepting HTTP requests".to_string())
    })
    .await;
    if !environment_ok {
        skip_remaining(&mut results, 1, "environment startup failed");
        return finish_run(
            args,
            temporary,
            started_at,
            started,
            mock_base,
            server_base,
            database,
            results,
            mock,
            server,
        );
    }

    let admin = match setup_admin(&client, &server_base).await {
        Ok(admin) => admin,
        Err(error) => {
            record_failure(&mut results, &SCENARIOS[1], Duration::ZERO, error);
            skip_remaining(&mut results, 2, "administrator setup failed");
            return finish_run(
                args,
                temporary,
                started_at,
                started,
                mock_base,
                server_base,
                database,
                results,
                mock,
                server,
            );
        }
    };

    let settings_ok = run_scenario(&mut results, &SCENARIOS[1], async {
        configure_game_settings(&client, &server_base, &mock_base, &admin).await?;
        let settings =
            admin_json(&client, &server_base, &admin, "/api/admin/game/settings").await?;
        if settings["prolific_api_base_url"] != mock_base {
            bail!("Prolific API base URL was not retained");
        }
        Ok("API endpoint and protected token were stored without a workspace id".to_string())
    })
    .await;

    if !settings_ok && args.fail_fast {
        skip_remaining(&mut results, 2, "stopped after CFG-01 failure");
        return finish_run(
            args,
            temporary,
            started_at,
            started,
            mock_base,
            server_base,
            database,
            results,
            mock,
            server,
        );
    }

    run_scenario(&mut results, &SCENARIOS[2], async {
        configure_mock_study(
            &client,
            &mock_base,
            &server_base,
            "invalid-action",
            Some(("participation_ended_early", "AUTOMATICALLY_APPROVE")),
        )
        .await?;
        create_experiment(&client, &server_base, &admin, "invalid-action").await?;
        let response = activate_experiment(&client, &server_base, &admin, "invalid-action").await?;
        let status = response.status();
        let body = response.text().await?;
        if status != StatusCode::CONFLICT || !body.contains("REQUEST_RETURN") {
            bail!("expected completion-action conflict, received {status}: {body}");
        }
        configure_mock_study(
            &client,
            &mock_base,
            &server_base,
            "another-experiment",
            None,
        )
        .await?;
        create_experiment(&client, &server_base, &admin, "invalid-url").await?;
        let response = activate_experiment(&client, &server_base, &admin, "invalid-url").await?;
        let status = response.status();
        let body = response.text().await?;
        if status != StatusCode::CONFLICT || !body.contains("must exactly match") {
            bail!("expected external-URL conflict, received {status}: {body}");
        }
        Ok("activation rejected both a wrong completion action and another experiment's URL"
            .to_string())
    })
    .await;

    let activation_ok = run_scenario(&mut results, &SCENARIOS[3], async {
        configure_mock_study(&client, &mock_base, &server_base, EXPERIMENT_ID, None).await?;
        create_experiment(&client, &server_base, &admin, EXPERIMENT_ID).await?;
        let response = activate_experiment(&client, &server_base, &admin, EXPERIMENT_ID).await?;
        expect_status(response, StatusCode::OK, "activate valid experiment").await?;
        let journal = mock_requests(&client, &mock_base).await?;
        assert_journal_call(&journal, "/api/v1/studies/study1/")?;
        assert_journal_call(&journal, "/api/v1/projects/project1/")?;
        Ok("study ownership, URL, timing, and six completion paths passed preflight".to_string())
    })
    .await;

    if !activation_ok {
        skip_remaining(&mut results, 4, "valid experiment did not activate");
        return finish_run(
            args,
            temporary,
            started_at,
            started,
            mock_base,
            server_base,
            database,
            results,
            mock,
            server,
        );
    }

    run_scenario(&mut results, &SCENARIOS[4], async {
        let participant = create_signed_participant(
            &client,
            &mock_base,
            &server_base,
            "participantA",
            "submissionA",
        )
        .await?;
        context.first_participant = Some(participant.clone());
        Ok(format!(
            "admitted participant as {}",
            participant.research_id
        ))
    })
    .await;

    run_scenario(&mut results, &SCENARIOS[5], async {
        let audience = external_study_url(&server_base, EXPERIMENT_ID);
        let mut token = sign_token(
            &client,
            &mock_base,
            "participantTampered",
            "submissionTampered",
            &audience,
        )
        .await?;
        let last = token.pop().context("mock returned an empty token")?;
        token.push(if last == 'a' { 'b' } else { 'a' });
        let response = participant_request(
            &client,
            &server_base,
            "participantTampered",
            "submissionTampered",
            &token,
        )
        .await?;
        expect_status(response, StatusCode::BAD_REQUEST, "reject tampered token").await?;
        Ok("signature verification rejected a one-character mutation".to_string())
    })
    .await;

    run_scenario(&mut results, &SCENARIOS[6], async {
        let audience = external_study_url(&server_base, EXPERIMENT_ID);
        let token = sign_token(
            &client,
            &mock_base,
            "signedParticipant",
            "submissionMismatch",
            &audience,
        )
        .await?;
        let response = participant_request(
            &client,
            &server_base,
            "differentParticipant",
            "submissionMismatch",
            &token,
        )
        .await?;
        expect_status(
            response,
            StatusCode::BAD_REQUEST,
            "reject identity mismatch",
        )
        .await?;
        Ok("signed payload could not be rebound to different URL parameters".to_string())
    })
    .await;

    let pairing_ok = run_scenario(&mut results, &SCENARIOS[7], async {
        let first = context
            .first_participant
            .clone()
            .context("JWT-01 did not retain its participant")?;
        let second = create_signed_participant(
            &client,
            &mock_base,
            &server_base,
            "participantB",
            "submissionB",
        )
        .await?;
        let waiting = create_session(&client, &server_base, &first).await?;
        if waiting["participant_state"]["state"] != "waiting" {
            bail!("first participant did not enter waiting state: {waiting}");
        }
        let joined = create_session(&client, &server_base, &second).await?;
        let session = joined["participant_state"]["public_session_id"]
            .as_str()
            .context("joined state omitted public session id")?
            .to_string();
        if waiting["participant_state"]["public_session_id"] != session {
            bail!("participants were assigned to different sessions");
        }
        let (mut first_socket, mut second_socket) =
            open_game_pair(&client, &server_base, &session, &first, &second).await?;
        read_socket_state(&mut first_socket, "active").await?;
        read_socket_state(&mut second_socket, "active").await?;
        let first_state = participant_state(&client, &server_base, &first).await?;
        if first_state["participant_state"]["state"] != "active" {
            bail!("first participant was not promoted to active: {first_state}");
        }
        let _ = first_socket.close(None).await;
        let _ = second_socket.close(None).await;
        context.paired_session = Some(session.clone());
        Ok(format!(
            "both Prolific participants are active in {session}"
        ))
    })
    .await;

    if !pairing_ok && args.fail_fast {
        skip_remaining(&mut results, 8, "stopped after MATCH-01 failure");
        return finish_run(
            args,
            temporary,
            started_at,
            started,
            mock_base,
            server_base,
            database,
            results,
            mock,
            server,
        );
    }

    run_scenario(&mut results, &SCENARIOS[8], async {
        let participant = create_signed_participant(
            &client,
            &mock_base,
            &server_base,
            "participantWaiting",
            "submissionWaiting",
        )
        .await?;
        let waiting = create_session(&client, &server_base, &participant).await?;
        let session = waiting["participant_state"]["public_session_id"]
            .as_str()
            .context("waiting state omitted public session id")?
            .to_string();
        let ended = leave_session(&client, &server_base, &participant, &session).await?;
        assert_outcome(&ended, "left_waiting_room", "UNMATCHEDCODE")?;
        context.unmatched_session = Some(session.clone());
        Ok(format!(
            "{session} ended with the Game did not start REQUEST_RETURN code"
        ))
    })
    .await;

    run_scenario(&mut results, &SCENARIOS[9], async {
        let participant = create_signed_participant(
            &client,
            &mock_base,
            &server_base,
            "participantWaitingTimeout",
            "submissionWaitingTimeout",
        )
        .await?;
        let waiting = create_session(&client, &server_base, &participant).await?;
        let session = waiting["participant_state"]["public_session_id"]
            .as_str()
            .context("waiting state omitted public session id")?
            .to_string();
        let ended =
            wait_for_outcome(&client, &server_base, &participant, Duration::from_secs(6)).await?;
        assert_outcome(&ended, "partner_unavailable", "UNMATCHEDCODE")?;
        context.timed_out_waiting_session = Some(session.clone());
        Ok(format!(
            "{session} expired with the same Game did not start REQUEST_RETURN code"
        ))
    })
    .await;

    run_scenario(&mut results, &SCENARIOS[10], async {
        let leaver = create_signed_participant(
            &client,
            &mock_base,
            &server_base,
            "participantLeaver",
            "submissionLeaver",
        )
        .await?;
        let partner = create_signed_participant(
            &client,
            &mock_base,
            &server_base,
            "participantPartner",
            "submissionPartner",
        )
        .await?;
        let waiting = create_session(&client, &server_base, &leaver).await?;
        let session = waiting["participant_state"]["public_session_id"]
            .as_str()
            .context("waiting state omitted public session id")?
            .to_string();
        let joined = create_session(&client, &server_base, &partner).await?;
        if joined["participant_state"]["public_session_id"] != session {
            bail!("partner did not join the leaver's session");
        }
        let (mut leaver_socket, mut partner_socket) =
            open_game_pair(&client, &server_base, &session, &leaver, &partner).await?;
        read_socket_state(&mut leaver_socket, "active").await?;
        read_socket_state(&mut partner_socket, "active").await?;
        let leaver_end = leave_session(&client, &server_base, &leaver, &session).await?;
        assert_outcome(&leaver_end, "left_game", "TIMEOUTCODE")?;
        let partner_end = participant_state(&client, &server_base, &partner).await?;
        assert_outcome(&partner_end, "partner_left", "PARTNERCODE")?;
        let _ = leaver_socket.close(None).await;
        let _ = partner_socket.close(None).await;
        context.partner_left_session = Some(session.clone());
        Ok(format!(
            "{session} preserved actor and good-faith partner outcomes"
        ))
    })
    .await;

    run_scenario(&mut results, &SCENARIOS[11], async {
        let first = create_signed_participant(
            &client,
            &mock_base,
            &server_base,
            "participantCompletedA",
            "submissionCompletedA",
        )
        .await?;
        let second = create_signed_participant(
            &client,
            &mock_base,
            &server_base,
            "participantCompletedB",
            "submissionCompletedB",
        )
        .await?;
        let waiting = create_session(&client, &server_base, &first).await?;
        let session = waiting["participant_state"]["public_session_id"]
            .as_str()
            .context("completion workflow omitted public session id")?
            .to_string();
        create_session(&client, &server_base, &second).await?;
        let (mut first_socket, mut second_socket) =
            open_game_pair(&client, &server_base, &session, &first, &second).await?;
        read_socket_state(&mut first_socket, "active").await?;
        read_socket_state(&mut second_socket, "active").await?;
        first_socket
            .send(Message::Text(json!({"type": "ready"}).to_string()))
            .await?;
        second_socket
            .send(Message::Text(json!({"type": "ready"}).to_string()))
            .await?;
        first_socket
            .send(Message::Text(
                json!({"type": "action", "action": {"finish": true}}).to_string(),
            ))
            .await?;
        let first_end =
            wait_for_outcome(&client, &server_base, &first, Duration::from_secs(5)).await?;
        let second_end =
            wait_for_outcome(&client, &server_base, &second, Duration::from_secs(5)).await?;
        assert_outcome(&first_end, "completed", "DONECODE")?;
        assert_outcome(&second_end, "completed", "DONECODE")?;
        if first_end["participant_state"]["result"]["completion"]["actions"] != 1
            || second_end["participant_state"]["result"]["completion"]["actions"] != 1
        {
            bail!("normal completion did not retain the game-owned result");
        }
        let _ = first_socket.close(None).await;
        let _ = second_socket.close(None).await;
        context.completed_session = Some(session.clone());
        context.completed_research_id = Some(first.research_id.clone());
        Ok(format!(
            "{session} completed once and returned DONECODE to both participants"
        ))
    })
    .await;

    run_scenario(&mut results, &SCENARIOS[12], async {
        let audience = external_study_url(&server_base, EXPERIMENT_ID);
        let token = sign_token(
            &client,
            &mock_base,
            "participantCompletedA",
            "submissionCompletedA",
            &audience,
        )
        .await?;
        let response = participant_request(
            &client,
            &server_base,
            "participantCompletedA",
            "submissionCompletedA",
            &token,
        )
        .await?;
        let response = expect_status(
            response,
            StatusCode::OK,
            "resume terminal Prolific admission",
        )
        .await?;
        let body = response.json::<Value>().await?;
        if body["participant_id"]
            != context
                .completed_research_id
                .as_deref()
                .context("FLOW-01 did not retain its research id")?
        {
            bail!("terminal re-entry created a different research participant");
        }
        let resumed = Participant {
            credential: body["participant_credential"]
                .as_str()
                .context("resumed admission omitted credential")?
                .to_string(),
            research_id: body["participant_id"]
                .as_str()
                .context("resumed admission omitted research id")?
                .to_string(),
            experiment_id: EXPERIMENT_ID.to_string(),
        };
        let terminal = participant_state(&client, &server_base, &resumed).await?;
        assert_outcome(&terminal, "completed", "DONECODE")?;
        Ok("re-entry retained both the research id and completed handoff".to_string())
    })
    .await;

    run_scenario(&mut results, &SCENARIOS[13], async {
        let fallback_id = "unsigned-fallback";
        let response = client
            .post(format!("{mock_base}/__mock/configure"))
            .json(&json!({
                "external_study_url": external_study_url(&server_base, fallback_id),
                "secure": false,
                "action_overrides": {}
            }))
            .send()
            .await?;
        expect_status(response, StatusCode::OK, "configure unsigned mock study").await?;
        create_experiment(&client, &server_base, &admin, fallback_id).await?;
        let response = activate_experiment(&client, &server_base, &admin, fallback_id).await?;
        expect_status(
            response,
            StatusCode::OK,
            "activate unsigned fallback experiment",
        )
        .await?;
        add_mock_submission(
            &client,
            &mock_base,
            "participantUnsigned",
            "submissionUnsigned",
        )
        .await?;
        let response = participant_request_for(
            &client,
            &server_base,
            fallback_id,
            "participantUnsigned",
            "submissionUnsigned",
            None,
        )
        .await?;
        let response = expect_status(
            response,
            StatusCode::OK,
            "admit API-verified unsigned participant",
        )
        .await?;
        let body = response.json::<Value>().await?;
        let participant = Participant {
            credential: body["participant_credential"]
                .as_str()
                .context("unsigned admission omitted credential")?
                .to_string(),
            research_id: body["participant_id"]
                .as_str()
                .context("unsigned admission omitted research id")?
                .to_string(),
            experiment_id: fallback_id.to_string(),
        };
        let waiting = create_session(&client, &server_base, &participant).await?;
        let session = waiting["participant_state"]["public_session_id"]
            .as_str()
            .context("unsigned workflow omitted public session id")?;
        let terminal = leave_session(&client, &server_base, &participant, session).await?;
        assert_outcome(&terminal, "left_waiting_room", "UNMATCHEDCODE")?;
        let journal = mock_requests(&client, &mock_base).await?;
        assert_journal_call(&journal, "/api/v1/submissions/submissionUnsigned/")?;
        Ok("submission API admission continued through the ordinary waiting workflow".to_string())
    })
    .await;

    run_scenario(&mut results, &SCENARIOS[14], async {
        let experiment_id = "consent-required";
        prepare_experiment(
            &client,
            &mock_base,
            &server_base,
            &admin,
            experiment_id,
            fixture_config(
                2,
                2,
                60,
                120,
                false,
                json!([{"id":"study","title":"Study consent","body":"I agree.","required":true}]),
            ),
        )
        .await?;
        let participant = create_signed_participant_for(
            &client,
            &mock_base,
            &server_base,
            experiment_id,
            "participantConsent",
            "submissionConsent",
        )
        .await?;
        let response = create_session_response(&client, &server_base, &participant).await?;
        expect_status(response, StatusCode::FORBIDDEN, "block missing consent").await?;
        Ok("missing required consent remained outside matchmaking".to_string())
    })
    .await;

    run_scenario(&mut results, &SCENARIOS[15], async {
        let experiment_id = "source-isolation";
        prepare_experiment(
            &client,
            &mock_base,
            &server_base,
            &admin,
            experiment_id,
            fixture_config(2, 2, 60, 120, true, json!([])),
        )
        .await?;
        let direct_response = client
            .post(format!("{server_base}/e/{experiment_id}/api/participants"))
            .json(&json!({}))
            .send()
            .await?;
        expect_status(
            direct_response,
            StatusCode::BAD_REQUEST,
            "reject direct intake in Prolific experiment",
        )
        .await?;
        let prolific = create_signed_participant_for(
            &client,
            &mock_base,
            &server_base,
            experiment_id,
            "participantSource",
            "submissionSource",
        )
        .await?;
        let partner = create_signed_participant_for(
            &client,
            &mock_base,
            &server_base,
            experiment_id,
            "participantSourcePartner",
            "submissionSourcePartner",
        )
        .await?;
        let prolific_waiting = create_session(&client, &server_base, &prolific).await?;
        let prolific_session = prolific_waiting["participant_state"]["public_session_id"]
            .as_str()
            .context("Prolific waiting state omitted session")?;
        let joined = create_session(&client, &server_base, &partner).await?;
        if joined["participant_state"]["public_session_id"] != prolific_session {
            bail!("two Prolific participants did not form one dyad");
        }
        let prolific_end =
            leave_session(&client, &server_base, &prolific, prolific_session).await?;
        let partner_end = participant_state(&client, &server_base, &partner).await?;
        assert_outcome(&prolific_end, "left_waiting_room", "UNMATCHEDCODE")?;
        assert_outcome(&partner_end, "partner_left", "PARTNERCODE")?;
        Ok(
            "direct intake was rejected and both assigned roles retained Prolific handoffs"
                .to_string(),
        )
    })
    .await;

    run_scenario(&mut results, &SCENARIOS[16], async {
        let first = create_signed_participant(
            &client,
            &mock_base,
            &server_base,
            "participantFormLeaveA",
            "submissionFormLeaveA",
        )
        .await?;
        let second = create_signed_participant(
            &client,
            &mock_base,
            &server_base,
            "participantFormLeaveB",
            "submissionFormLeaveB",
        )
        .await?;
        let waiting = create_session(&client, &server_base, &first).await?;
        let session = waiting["participant_state"]["public_session_id"]
            .as_str()
            .context("pre-start fixture omitted session")?
            .to_string();
        create_session(&client, &server_base, &second).await?;
        let second_end = leave_session(&client, &server_base, &second, &session).await?;
        let first_end = participant_state(&client, &server_base, &first).await?;
        assert_outcome(&second_end, "left_waiting_room", "UNMATCHEDCODE")?;
        assert_outcome(&first_end, "partner_left", "PARTNERCODE")?;
        context.forming_left_session = Some(session.clone());
        Ok(format!(
            "{session} retained pre-start actor and partner outcomes"
        ))
    })
    .await;

    run_scenario(&mut results, &SCENARIOS[17], async {
        let first = create_signed_participant(
            &client,
            &mock_base,
            &server_base,
            "participantFormDeadlineA",
            "submissionFormDeadlineA",
        )
        .await?;
        let second = create_signed_participant(
            &client,
            &mock_base,
            &server_base,
            "participantFormDeadlineB",
            "submissionFormDeadlineB",
        )
        .await?;
        let waiting = create_session(&client, &server_base, &first).await?;
        let session = waiting["participant_state"]["public_session_id"]
            .as_str()
            .context("pre-start deadline omitted session")?
            .to_string();
        create_session(&client, &server_base, &second).await?;
        let first_end =
            wait_for_outcome(&client, &server_base, &first, Duration::from_secs(6)).await?;
        let second_end = participant_state(&client, &server_base, &second).await?;
        assert_outcome(&first_end, "partner_unavailable", "UNMATCHEDCODE")?;
        assert_outcome(&second_end, "partner_unavailable", "UNMATCHEDCODE")?;
        Ok(format!("{session} produced one shared pre-start deadline"))
    })
    .await;

    run_scenario(&mut results, &SCENARIOS[18], async {
        let (first, second, session, mut first_socket, mut second_socket) = create_active_pair(
            &client,
            &mock_base,
            &server_base,
            EXPERIMENT_ID,
            "ReconnectBrief",
        )
        .await?;
        first_socket.close(None).await?;
        read_socket_state(&mut second_socket, "paused").await?;
        let mut replacement = open_game_socket(&client, &server_base, &session, &first).await?;
        read_socket_state(&mut second_socket, "active").await?;
        replacement
            .send(Message::Text(
                json!({"type":"action","action":{"finish":true}}).to_string(),
            ))
            .await?;
        let first_end =
            wait_for_outcome(&client, &server_base, &first, Duration::from_secs(5)).await?;
        let second_end = participant_state(&client, &server_base, &second).await?;
        assert_outcome(&first_end, "completed", "DONECODE")?;
        assert_outcome(&second_end, "completed", "DONECODE")?;
        let _ = replacement.close(None).await;
        let _ = second_socket.close(None).await;
        Ok(format!(
            "{session} resumed and completed after a transient disconnect"
        ))
    })
    .await;

    run_scenario(&mut results, &SCENARIOS[19], async {
        let (first, second, session, mut first_socket, mut second_socket) = create_active_pair(
            &client,
            &mock_base,
            &server_base,
            EXPERIMENT_ID,
            "ReconnectSingle",
        )
        .await?;
        first_socket.close(None).await?;
        read_socket_state(&mut second_socket, "paused").await?;
        let first_end =
            wait_for_outcome(&client, &server_base, &first, Duration::from_secs(6)).await?;
        let second_end = participant_state(&client, &server_base, &second).await?;
        assert_outcome(&first_end, "connection_lost", "TIMEOUTCODE")?;
        assert_outcome(&second_end, "partner_left", "PARTNERCODE")?;
        let _ = second_socket.close(None).await;
        context.reconnect_session = Some(session.clone());
        Ok(format!(
            "{session} distinguished disconnected and waiting participants"
        ))
    })
    .await;

    run_scenario(&mut results, &SCENARIOS[20], async {
        let (first, second, session, mut first_socket, mut second_socket) = create_active_pair(
            &client,
            &mock_base,
            &server_base,
            EXPERIMENT_ID,
            "ReconnectBoth",
        )
        .await?;
        first_socket.close(None).await?;
        second_socket.close(None).await?;
        let first_end =
            wait_for_outcome(&client, &server_base, &first, Duration::from_secs(6)).await?;
        let second_end = participant_state(&client, &server_base, &second).await?;
        context.both_disconnected_session = Some(session.clone());
        assert_outcome(&first_end, "connection_lost", "TIMEOUTCODE")?;
        assert_outcome(&second_end, "connection_lost", "TIMEOUTCODE")?;
        Ok(format!(
            "{session} classified both expired disconnects without a false stayer"
        ))
    })
    .await;

    run_scenario(&mut results, &SCENARIOS[21], async {
        let experiment_id = "idle-none";
        prepare_experiment(
            &client,
            &mock_base,
            &server_base,
            &admin,
            experiment_id,
            fixture_config(10, 10, 2, 120, false, json!([])),
        )
        .await?;
        let (first, second, session, mut first_socket, mut second_socket) =
            create_active_pair(&client, &mock_base, &server_base, experiment_id, "IdleNone")
                .await?;
        let first_end =
            wait_for_outcome(&client, &server_base, &first, Duration::from_secs(6)).await?;
        let second_end = participant_state(&client, &server_base, &second).await?;
        assert_outcome(&first_end, "idle_limit_reached", "TIMEOUTCODE")?;
        assert_outcome(&second_end, "idle_limit_reached", "TIMEOUTCODE")?;
        let _ = first_socket.close(None).await;
        let _ = second_socket.close(None).await;
        context.idle_session = Some(session.clone());
        Ok(format!(
            "{session} expired both roles after shared inactivity"
        ))
    })
    .await;

    run_scenario(&mut results, &SCENARIOS[22], async {
        let experiment_id = "idle-heartbeat";
        prepare_experiment(
            &client,
            &mock_base,
            &server_base,
            &admin,
            experiment_id,
            fixture_config(10, 10, 2, 120, false, json!([])),
        )
        .await?;
        let (first, second, session, mut first_socket, mut second_socket) = create_active_pair(
            &client,
            &mock_base,
            &server_base,
            experiment_id,
            "IdleHeartbeat",
        )
        .await?;
        for _ in 0..8 {
            first_socket
                .send(Message::Text(json!({"type":"heartbeat"}).to_string()))
                .await?;
            time::sleep(Duration::from_millis(350)).await;
        }
        let first_end =
            wait_for_outcome(&client, &server_base, &first, Duration::from_secs(4)).await?;
        let second_end = participant_state(&client, &server_base, &second).await?;
        assert_outcome(&first_end, "idle_limit_reached", "TIMEOUTCODE")?;
        assert_outcome(&second_end, "idle_limit_reached", "TIMEOUTCODE")?;
        let _ = first_socket.close(None).await;
        let _ = second_socket.close(None).await;
        Ok(format!(
            "{session} ignored heartbeats for research-activity timing"
        ))
    })
    .await;

    run_scenario(&mut results, &SCENARIOS[23], async {
        let experiment_id = "idle-activity";
        prepare_experiment(
            &client,
            &mock_base,
            &server_base,
            &admin,
            experiment_id,
            fixture_config(10, 10, 2, 120, false, json!([])),
        )
        .await?;
        let (first, second, session, mut first_socket, mut second_socket) = create_active_pair(
            &client,
            &mock_base,
            &server_base,
            experiment_id,
            "IdleActivity",
        )
        .await?;
        time::sleep(Duration::from_millis(1_200)).await;
        first_socket
            .send(Message::Text(
                json!({"type":"action","action":{"finish":false}}).to_string(),
            ))
            .await?;
        time::sleep(Duration::from_millis(1_100)).await;
        let still_active = participant_state(&client, &server_base, &first).await?;
        if still_active["participant_state"]["state"] != "active" {
            bail!("accepted activity did not extend the idle deadline: {still_active}");
        }
        let first_end =
            wait_for_outcome(&client, &server_base, &first, Duration::from_secs(5)).await?;
        let second_end = participant_state(&client, &server_base, &second).await?;
        assert_outcome(&first_end, "idle_limit_reached", "TIMEOUTCODE")?;
        assert_outcome(&second_end, "idle_limit_reached", "TIMEOUTCODE")?;
        let _ = first_socket.close(None).await;
        let _ = second_socket.close(None).await;
        Ok(format!(
            "{session} extended once, then expired from the new idle deadline"
        ))
    })
    .await;

    run_scenario(&mut results, &SCENARIOS[24], async {
        let experiment_id = "absolute-lifetime";
        prepare_experiment(
            &client,
            &mock_base,
            &server_base,
            &admin,
            experiment_id,
            fixture_config(10, 10, 3, 3, false, json!([])),
        )
        .await?;
        let (first, second, session, mut first_socket, mut second_socket) =
            create_active_pair(&client, &mock_base, &server_base, experiment_id, "Lifetime")
                .await?;
        let first_end =
            wait_for_outcome(&client, &server_base, &first, Duration::from_secs(7)).await?;
        let second_end = participant_state(&client, &server_base, &second).await?;
        assert_outcome(&first_end, "lifetime_limit_reached", "TECHNICALCODE")?;
        assert_outcome(&second_end, "lifetime_limit_reached", "TECHNICALCODE")?;
        let _ = first_socket.close(None).await;
        let _ = second_socket.close(None).await;
        context.lifetime_session = Some(session.clone());
        Ok(format!(
            "{session} applied the absolute bound symmetrically"
        ))
    })
    .await;

    run_scenario(&mut results, &SCENARIOS[25], async {
        let (first, second, session, mut first_socket, mut second_socket) = create_active_pair(
            &client,
            &mock_base,
            &server_base,
            EXPERIMENT_ID,
            "RaceLeaves",
        )
        .await?;
        let (first_response, second_response) = tokio::join!(
            leave_session(&client, &server_base, &first, &session),
            leave_session(&client, &server_base, &second, &session)
        );
        let outcomes = [
            first_response?["participant_state"]["result"]["outcome"]
                .as_str()
                .context("first leave omitted outcome")?
                .to_string(),
            second_response?["participant_state"]["result"]["outcome"]
                .as_str()
                .context("second leave omitted outcome")?
                .to_string(),
        ];
        if !outcomes.contains(&"left_game".to_string())
            || !outcomes.contains(&"partner_left".to_string())
        {
            bail!("simultaneous leaves produced incoherent results: {outcomes:?}");
        }
        let _ = first_socket.close(None).await;
        let _ = second_socket.close(None).await;
        Ok(format!("{session} committed exactly one departure actor"))
    })
    .await;

    run_scenario(&mut results, &SCENARIOS[26], async {
        for iteration in 0..4 {
            let suffix = format!("RaceComplete{iteration}");
            let (first, second, session, mut first_socket, mut second_socket) =
                create_active_pair(&client, &mock_base, &server_base, EXPERIMENT_ID, &suffix)
                    .await?;
            let finish = first_socket.send(Message::Text(
                json!({"type":"action","action":{"finish":true}}).to_string(),
            ));
            let leave = leave_session(&client, &server_base, &second, &session);
            let (finish_result, leave_result) = tokio::join!(finish, leave);
            finish_result?;
            leave_result?;
            let first_end =
                wait_for_outcome(&client, &server_base, &first, Duration::from_secs(5)).await?;
            let second_end = participant_state(&client, &server_base, &second).await?;
            let pair = (
                first_end["participant_state"]["result"]["outcome"]
                    .as_str()
                    .context("race first outcome missing")?,
                second_end["participant_state"]["result"]["outcome"]
                    .as_str()
                    .context("race second outcome missing")?,
            );
            if pair != ("completed", "completed") && pair != ("partner_left", "left_game") {
                bail!("completion/leave race produced mixed terminal results: {pair:?}");
            }
            let _ = first_socket.close(None).await;
            let _ = second_socket.close(None).await;
        }
        Ok("four completion/leave races each committed one coherent terminal family".to_string())
    })
    .await;

    run_scenario(&mut results, &SCENARIOS[27], async {
        let participant = create_signed_participant(
            &client,
            &mock_base,
            &server_base,
            "participantReplayLeave",
            "submissionReplayLeave",
        )
        .await?;
        let waiting = create_session(&client, &server_base, &participant).await?;
        let session = waiting["participant_state"]["public_session_id"]
            .as_str()
            .context("replay fixture omitted session")?
            .to_string();
        let first = leave_session(&client, &server_base, &participant, &session).await?;
        let second = leave_session(&client, &server_base, &participant, &session).await?;
        assert_outcome(&first, "left_waiting_room", "UNMATCHEDCODE")?;
        assert_outcome(&second, "left_waiting_room", "UNMATCHEDCODE")?;
        let audience = external_study_url(&server_base, EXPERIMENT_ID);
        let token = sign_token(
            &client,
            &mock_base,
            "participantReplayLeave",
            "submissionReplayLeave",
            &audience,
        )
        .await?;
        let response = participant_request(
            &client,
            &server_base,
            "participantReplayLeave",
            "submissionReplayLeave",
            &token,
        )
        .await?;
        let body = expect_json(response, StatusCode::OK, "re-admit terminal leaver").await?;
        let resumed = Participant {
            credential: body["participant_credential"]
                .as_str()
                .context("terminal replay omitted credential")?
                .to_string(),
            research_id: body["participant_id"]
                .as_str()
                .context("terminal replay omitted research id")?
                .to_string(),
            experiment_id: EXPERIMENT_ID.to_string(),
        };
        let replayed = participant_state(&client, &server_base, &resumed).await?;
        assert_outcome(&replayed, "left_waiting_room", "UNMATCHEDCODE")?;
        Ok(format!(
            "{session} returned the same terminal result three times"
        ))
    })
    .await;

    run_scenario(&mut results, &SCENARIOS[28], async {
        let first = create_signed_participant(
            &client,
            &mock_base,
            &server_base,
            "participantWaitReconnectA",
            "submissionWaitReconnectA",
        )
        .await?;
        let second = create_signed_participant(
            &client,
            &mock_base,
            &server_base,
            "participantWaitReconnectB",
            "submissionWaitReconnectB",
        )
        .await?;
        let waiting = create_session(&client, &server_base, &first).await?;
        let session = waiting["participant_state"]["public_session_id"]
            .as_str()
            .context("waiting reconnect fixture omitted session")?
            .to_string();
        let mut first_socket = open_game_socket(&client, &server_base, &session, &first).await?;
        read_socket_state(&mut first_socket, "waiting").await?;
        first_socket.close(None).await?;
        let mut replacement = open_game_socket(&client, &server_base, &session, &first).await?;
        read_socket_state(&mut replacement, "waiting").await?;
        let joined = create_session(&client, &server_base, &second).await?;
        if joined["participant_state"]["public_session_id"] != session {
            bail!("partner did not join the reconnected waiting participant");
        }
        let mut second_socket = open_game_socket(&client, &server_base, &session, &second).await?;
        read_socket_state(&mut replacement, "active").await?;
        read_socket_state(&mut second_socket, "active").await?;
        replacement
            .send(Message::Text(
                json!({"type":"action","action":{"finish":true}}).to_string(),
            ))
            .await?;
        let first_end =
            wait_for_outcome(&client, &server_base, &first, Duration::from_secs(5)).await?;
        let second_end = participant_state(&client, &server_base, &second).await?;
        assert_outcome(&first_end, "completed", "DONECODE")?;
        assert_outcome(&second_end, "completed", "DONECODE")?;
        let _ = replacement.close(None).await;
        let _ = second_socket.close(None).await;
        Ok(format!(
            "{session} retained assignment across a waiting-room reconnect"
        ))
    })
    .await;

    run_scenario(&mut results, &SCENARIOS[29], async {
        let experiment_id = "waiting-reconnect-expiry";
        prepare_experiment(
            &client,
            &mock_base,
            &server_base,
            &admin,
            experiment_id,
            fixture_config(10, 2, 60, 120, false, json!([])),
        )
        .await?;
        let participant = create_signed_participant_for(
            &client,
            &mock_base,
            &server_base,
            experiment_id,
            "participantWaitLost",
            "submissionWaitLost",
        )
        .await?;
        let waiting = create_session(&client, &server_base, &participant).await?;
        let session = waiting["participant_state"]["public_session_id"]
            .as_str()
            .context("waiting connection-loss fixture omitted session")?
            .to_string();
        let mut socket = open_game_socket(&client, &server_base, &session, &participant).await?;
        read_socket_state(&mut socket, "waiting").await?;
        socket.close(None).await?;
        let terminal =
            wait_for_outcome(&client, &server_base, &participant, Duration::from_secs(6)).await?;
        assert_outcome(&terminal, "connection_lost", "UNMATCHEDCODE")?;
        Ok(format!(
            "{session} classified a dropped waiter as Game did not start"
        ))
    })
    .await;

    run_scenario(&mut results, &SCENARIOS[30], async {
        let experiment_id = "idle-rejected";
        prepare_experiment(
            &client,
            &mock_base,
            &server_base,
            &admin,
            experiment_id,
            fixture_config(10, 10, 2, 120, false, json!([])),
        )
        .await?;
        let (first, second, session, mut first_socket, mut second_socket) = create_active_pair(
            &client,
            &mock_base,
            &server_base,
            experiment_id,
            "IdleRejected",
        )
        .await?;
        for _ in 0..8 {
            first_socket
                .send(Message::Text(
                    json!({"type":"action","action":{"finish":false,"invalid":true}}).to_string(),
                ))
                .await?;
            time::sleep(Duration::from_millis(350)).await;
        }
        let first_end =
            wait_for_outcome(&client, &server_base, &first, Duration::from_secs(4)).await?;
        let second_end = participant_state(&client, &server_base, &second).await?;
        assert_outcome(&first_end, "idle_limit_reached", "TIMEOUTCODE")?;
        assert_outcome(&second_end, "idle_limit_reached", "TIMEOUTCODE")?;
        let _ = first_socket.close(None).await;
        let _ = second_socket.close(None).await;
        Ok(format!(
            "{session} ignored rejected actions for idle timing"
        ))
    })
    .await;

    run_scenario(&mut results, &SCENARIOS[31], async {
        let experiment_id = "consent-declined";
        prepare_experiment(
            &client,
            &mock_base,
            &server_base,
            &admin,
            experiment_id,
            fixture_config(
                2,
                2,
                60,
                120,
                false,
                json!([{"id":"study","title":"Study consent","body":"I agree.","required":true}]),
            ),
        )
        .await?;
        let participant = create_signed_participant_for(
            &client,
            &mock_base,
            &server_base,
            experiment_id,
            "participantConsentDeclined",
            "submissionConsentDeclined",
        )
        .await?;
        record_consent(&client, &server_base, &participant, json!({"study": false})).await?;
        let response = create_session_response(&client, &server_base, &participant).await?;
        expect_status(response, StatusCode::FORBIDDEN, "block declined consent").await?;
        Ok("explicit refusal remained a pre-session decision".to_string())
    })
    .await;

    run_scenario(&mut results, &SCENARIOS[32], async {
        let experiment_id = "consent-accepted";
        prepare_experiment(
            &client,
            &mock_base,
            &server_base,
            &admin,
            experiment_id,
            fixture_config(
                2,
                2,
                60,
                120,
                false,
                json!([{"id":"study","title":"Study consent","body":"I agree.","required":true}]),
            ),
        )
        .await?;
        let participant = create_signed_participant_for(
            &client,
            &mock_base,
            &server_base,
            experiment_id,
            "participantConsentAccepted",
            "submissionConsentAccepted",
        )
        .await?;
        record_consent(&client, &server_base, &participant, json!({"study": true})).await?;
        let waiting = create_session(&client, &server_base, &participant).await?;
        let session = waiting["participant_state"]["public_session_id"]
            .as_str()
            .context("accepted-consent fixture omitted waiting session")?;
        let terminal = leave_session(&client, &server_base, &participant, session).await?;
        assert_outcome(&terminal, "left_waiting_room", "UNMATCHEDCODE")?;
        Ok("accepted consent enabled the ordinary waiting-room workflow".to_string())
    })
    .await;

    run_scenario(&mut results, &SCENARIOS[33], async {
        let sessions = admin_json(&client, &server_base, &admin, &format!("/api/admin/runtime/{EXPERIMENT_ID}/sessions")).await?;
        let rows = sessions["sessions"].as_array().context("dashboard response omitted sessions")?;
        for public_id in [
            &context.unmatched_session,
            &context.timed_out_waiting_session,
            &context.partner_left_session,
            &context.completed_session,
            &context.forming_left_session,
            &context.reconnect_session,
            &context.both_disconnected_session,
        ] {
            let public_id = public_id.as_deref().context("terminal scenario did not retain its session")?;
            let row = rows.iter().find(|row| row["public_session_id"] == public_id)
                .with_context(|| format!("dashboard omitted session {public_id}"))?;
            let session_id = row["session_id"].as_i64().context("dashboard row omitted numeric session id")?;
            let detail = admin_json(&client, &server_base, &admin, &format!("/api/admin/runtime/{EXPERIMENT_ID}/sessions/{session_id}")).await?;
            let participants = detail["participants"].as_array().context("session detail omitted participants")?;
            if participants.is_empty() || participants.iter().any(|participant| participant["identity_provider"] != "prolific") {
                bail!("session {public_id} did not expose exclusively Prolific participants: {participants:?}");
            }
            if participants.iter().any(|participant| participant["prolific_session_id"].as_str().is_none()) {
                bail!("session {public_id} omitted Prolific submission identifiers");
            }
            assert_dashboard_workflow(row, &detail, public_id)?;
        }
        for (experiment_id, retained_session) in [
            ("idle-none", &context.idle_session),
            ("absolute-lifetime", &context.lifetime_session),
        ] {
            let public_id = retained_session
                .as_deref()
                .with_context(|| format!("{experiment_id} scenario did not retain its session"))?;
            let sessions = admin_json(
                &client,
                &server_base,
                &admin,
                &format!("/api/admin/runtime/{experiment_id}/sessions"),
            )
            .await?;
            let row = sessions["sessions"]
                .as_array()
                .context("custom dashboard response omitted sessions")?
                .iter()
                .find(|row| row["public_session_id"] == public_id)
                .with_context(|| format!("dashboard omitted session {public_id}"))?;
            let session_id = row["session_id"]
                .as_i64()
                .context("custom dashboard row omitted numeric session id")?;
            let detail = admin_json(
                &client,
                &server_base,
                &admin,
                &format!("/api/admin/runtime/{experiment_id}/sessions/{session_id}"),
            )
            .await?;
            assert_dashboard_workflow(row, &detail, public_id)?;
        }
        Ok("dashboard rows and details retained outcomes, identities, and submission ids".to_string())
    })
    .await;

    run_scenario(&mut results, &SCENARIOS[34], async {
        let journal = mock_requests(&client, &mock_base).await?;
        let calls = journal["requests"]
            .as_array()
            .context("mock journal omitted requests")?;
        if calls.is_empty() {
            bail!("mock journal is empty");
        }
        for call in calls {
            if call["method"] != "GET" {
                bail!("unexpected provider mutation in journal: {call}");
            }
            let path = call["path"].as_str().unwrap_or_default();
            if path.starts_with("/api/") && call["authorized"] != true {
                bail!("provider API request was not authenticated: {call}");
            }
        }
        Ok(format!(
            "verified {} provider calls; no payment or mutation request exists",
            calls.len()
        ))
    })
    .await;

    finish_run(
        args,
        temporary,
        started_at,
        started,
        mock_base,
        server_base,
        database,
        results,
        mock,
        server,
    )
}

/// Prints the complete scenario matrix before any potentially failing setup work.
fn print_catalogue() {
    println!("Parlando × Prolific integration scenario matrix");
    println!("{:<9}  {:<14}  Scenario", "ID", "Phase");
    println!("{:-<9}  {:-<14}  {:-<58}", "", "", "");
    for scenario in SCENARIOS {
        println!(
            "{:<9}  {:<14}  {}",
            scenario.id, scenario.phase, scenario.name
        );
    }
}

/// Finds companion programs and builds missing development binaries on demand.
fn companion_binaries(args: &Args) -> Result<(PathBuf, PathBuf)> {
    let current = std::env::current_exe()?;
    let directory = current
        .parent()
        .context("runner executable has no parent directory")?;
    let mut mock = args
        .mock_bin
        .clone()
        .unwrap_or_else(|| directory.join("prolific-mock"));
    let mut server = args
        .server_bin
        .clone()
        .unwrap_or_else(|| directory.join("prolific-test-server"));
    if args.mock_bin.is_none() && args.server_bin.is_none() && mock.is_file() && server.is_file() {
        println!("\n[SETUP] Using companion binaries beside the runner.");
        return Ok((mock, server));
    }
    if args.mock_bin.is_some() || args.server_bin.is_some() {
        if mock.is_file() && server.is_file() {
            return Ok((mock, server));
        }
        bail!("explicit companion binary does not exist");
    }
    println!("\n[SETUP] Building missing companion binaries...");
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let mut command = Command::new("cargo");
    command
        .arg("build")
        .arg("--manifest-path")
        .arg(manifest)
        .arg("--bin")
        .arg("prolific-mock")
        .arg("--bin")
        .arg("prolific-test-server");
    if directory.file_name().is_some_and(|name| name == "release") {
        command.arg("--release");
    }
    let status = command
        .status()
        .context("could not invoke cargo for companion binaries")?;
    if !status.success() {
        bail!("building Prolific test companion binaries failed");
    }
    mock = directory.join("prolific-mock");
    server = directory.join("prolific-test-server");
    if !mock.is_file() || !server.is_file() {
        bail!("cargo completed but companion binaries were not found beside the runner");
    }
    Ok((mock, server))
}

/// Reserves one currently free loopback TCP address for a child process.
fn reserve_address() -> Result<SocketAddr> {
    let listener = TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))?;
    Ok(listener.local_addr()?)
}

/// Starts one child with durable stdout and stderr logs.
fn spawn_logged(
    name: &'static str,
    executable: &Path,
    arguments: &[String],
    run_dir: &Path,
) -> Result<ManagedChild> {
    let stdout_path = run_dir.join(format!("{name}.stdout.log"));
    let stderr_path = run_dir.join(format!("{name}.stderr.log"));
    let stdout = File::create(&stdout_path)?;
    let stderr = File::create(&stderr_path)?;
    let child = Command::new(executable)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
        .with_context(|| format!("could not start {name} at {}", executable.display()))?;
    Ok(ManagedChild {
        name,
        child,
        stdout_path,
        stderr_path,
    })
}

/// Polls one health endpoint while also detecting early child exit.
async fn wait_health(
    client: &Client,
    base: &str,
    path: &str,
    child: &mut ManagedChild,
) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        child.ensure_running()?;
        if let Ok(response) = client.get(format!("{base}{path}")).send().await {
            if response.status().is_success() {
                return Ok(());
            }
        }
        if Instant::now() >= deadline {
            bail!("{} did not become healthy within 20 seconds", child.name);
        }
        time::sleep(Duration::from_millis(100)).await;
    }
}

/// Runs one scenario, prints immediate progress, and records its durable result.
async fn run_scenario<F>(
    results: &mut Vec<ScenarioResult>,
    definition: &'static ScenarioDefinition,
    future: F,
) -> bool
where
    F: Future<Output = Result<String>>,
{
    print!("\n[RUN ] {} {} ... ", definition.id, definition.name);
    use std::io::Write as _;
    let _ = std::io::stdout().flush();
    let started = Instant::now();
    match future.await {
        Ok(detail) => {
            let elapsed = started.elapsed();
            println!("PASS ({} ms)\n       {detail}", elapsed.as_millis());
            results.push(ScenarioResult {
                id: definition.id,
                phase: definition.phase,
                name: definition.name,
                status: ScenarioStatus::Passed,
                elapsed_ms: elapsed.as_millis(),
                detail: Some(detail),
            });
            true
        }
        Err(error) => {
            let elapsed = started.elapsed();
            println!("FAIL ({} ms)\n       {error:#}", elapsed.as_millis());
            results.push(ScenarioResult {
                id: definition.id,
                phase: definition.phase,
                name: definition.name,
                status: ScenarioStatus::Failed,
                elapsed_ms: elapsed.as_millis(),
                detail: Some(format!("{error:#}")),
            });
            false
        }
    }
}

/// Records one setup failure which occurred before its scenario future could run.
fn record_failure(
    results: &mut Vec<ScenarioResult>,
    definition: &'static ScenarioDefinition,
    elapsed: Duration,
    error: anyhow::Error,
) {
    println!(
        "\n[FAIL] {} {}\n       {error:#}",
        definition.id, definition.name
    );
    results.push(ScenarioResult {
        id: definition.id,
        phase: definition.phase,
        name: definition.name,
        status: ScenarioStatus::Failed,
        elapsed_ms: elapsed.as_millis(),
        detail: Some(format!("{error:#}")),
    });
}

/// Marks all unattempted scenarios explicitly rather than hiding missing coverage.
fn skip_remaining(results: &mut Vec<ScenarioResult>, start: usize, reason: &str) {
    for definition in &SCENARIOS[start..] {
        println!(
            "\n[SKIP] {} {}\n       {reason}",
            definition.id, definition.name
        );
        results.push(ScenarioResult {
            id: definition.id,
            phase: definition.phase,
            name: definition.name,
            status: ScenarioStatus::Skipped,
            elapsed_ms: 0,
            detail: Some(reason.to_string()),
        });
    }
}

/// Creates the first administrator account and retains its cookie and CSRF token.
async fn setup_admin(client: &Client, server_base: &str) -> Result<AdminAuth> {
    let response = client
        .post(format!("{server_base}/api/admin/setup"))
        .json(&json!({
            "username": "prolific-test-admin",
            "password": "prolific-test-password",
            "password_confirmation": "prolific-test-password"
        }))
        .send()
        .await?;
    let response = expect_status(response, StatusCode::OK, "create test administrator").await?;
    let cookie = response
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .context("administrator setup omitted session cookie")?
        .to_string();
    let body = response.json::<Value>().await?;
    let csrf = body["csrf_token"]
        .as_str()
        .context("administrator setup omitted CSRF token")?
        .to_string();
    Ok(AdminAuth { cookie, csrf })
}

/// Stores the mock origin and protected token through the real game-settings endpoint.
async fn configure_game_settings(
    client: &Client,
    server_base: &str,
    mock_base: &str,
    admin: &AdminAuth,
) -> Result<()> {
    let settings = admin_json(client, server_base, admin, "/api/admin/game/settings").await?;
    let response = client
        .post(format!("{server_base}/api/admin/game/settings"))
        .header(header::COOKIE, &admin.cookie)
        .header("x-csrf-token", &admin.csrf)
        .json(&json!({
            "expected_revision": settings["revision"],
            "institution": "Parlando integration tests",
            "admin_allowed_ip_ranges": [],
            "speechmatics_realtime_url": settings["speechmatics_realtime_url"],
            "tts_base_url": settings["tts_base_url"],
            "prolific_api_base_url": mock_base,
            "secret_updates": {"prolific.api_token": API_TOKEN},
            "secret_deletions": []
        }))
        .send()
        .await?;
    expect_status(response, StatusCode::OK, "store Prolific game settings").await?;
    Ok(())
}

/// Replaces the mock study for one activation scenario.
async fn configure_mock_study(
    client: &Client,
    mock_base: &str,
    server_base: &str,
    experiment_id: &str,
    action_override: Option<(&str, &str)>,
) -> Result<()> {
    let mut overrides = serde_json::Map::new();
    if let Some((path, action)) = action_override {
        overrides.insert(path.to_string(), Value::String(action.to_string()));
    }
    let response = client
        .post(format!("{mock_base}/__mock/configure"))
        .json(&json!({
            "external_study_url": external_study_url(server_base, experiment_id),
            "secure": true,
            "action_overrides": overrides
        }))
        .send()
        .await?;
    expect_status(response, StatusCode::OK, "configure mock study").await?;
    Ok(())
}

/// Returns the exact provider URL used for activation and JWT audience validation.
fn external_study_url(server_base: &str, experiment_id: &str) -> String {
    format!(
        "{server_base}/e/{experiment_id}/?PROLIFIC_PID={{{{%PROLIFIC_PID%}}}}&STUDY_ID={{{{%STUDY_ID%}}}}&SESSION_ID={{{{%SESSION_ID%}}}}"
    )
}

/// Builds one complete fixture configuration with scenario-specific lifecycle limits.
fn fixture_config(
    waiting_seconds: i64,
    reconnect_seconds: i64,
    idle_seconds: i64,
    lifetime_seconds: i64,
    direct_enabled: bool,
    consents: Value,
) -> Value {
    json!({
        "direct": {"enabled": direct_enabled, "consents": consents},
        "recruitment": {"prolific": {
            "enabled": true,
            "study_id": STUDY_ID,
            "completion_paths": {
                "completed": "DONECODE",
                "partner_left": "PARTNERCODE",
                "game_did_not_start": "UNMATCHEDCODE",
                "participation_ended_early": "TIMEOUTCODE",
                "technical_failure": "TECHNICALCODE",
                "no_consent": "NOCONSENTCODE"
            }
        }},
        "session": {
            "waiting_session_timeout_seconds": waiting_seconds,
            "reconnect_grace_seconds": reconnect_seconds,
            "session_idle_timeout_seconds": idle_seconds,
            "session_max_lifetime_seconds": lifetime_seconds
        },
        "agents": {"mode": "human_vs_human"},
        "voice": {"enabled": false},
        "transcription": {"enabled": false},
        "game": {}
    })
}

/// Creates one immutable Prolific-enabled experiment through the dashboard API.
async fn create_experiment(
    client: &Client,
    server_base: &str,
    admin: &AdminAuth,
    experiment_id: &str,
) -> Result<()> {
    create_experiment_with_config(
        client,
        server_base,
        admin,
        experiment_id,
        fixture_config(2, 2, 60, 120, false, json!([])),
    )
    .await
}

/// Creates one immutable experiment from an explicit workflow-test configuration.
async fn create_experiment_with_config(
    client: &Client,
    server_base: &str,
    admin: &AdminAuth,
    experiment_id: &str,
    config: Value,
) -> Result<()> {
    let response = client
        .post(format!("{server_base}/api/admin/experiments"))
        .header(header::COOKIE, &admin.cookie)
        .header("x-csrf-token", &admin.csrf)
        .json(&json!({
            "experiment_id": experiment_id,
            "config": config,
            "notes": "generated by prolific-test-runner"
        }))
        .send()
        .await?;
    expect_status(response, StatusCode::OK, "create experiment").await?;
    Ok(())
}

/// Configures the mock study and activates one scenario-specific experiment.
async fn prepare_experiment(
    client: &Client,
    mock_base: &str,
    server_base: &str,
    admin: &AdminAuth,
    experiment_id: &str,
    config: Value,
) -> Result<()> {
    configure_mock_study(client, mock_base, server_base, experiment_id, None).await?;
    create_experiment_with_config(client, server_base, admin, experiment_id, config).await?;
    let response = activate_experiment(client, server_base, admin, experiment_id).await?;
    expect_status(response, StatusCode::OK, "activate workflow experiment").await?;
    Ok(())
}

/// Requests activation through the experiment-specific runtime boundary.
async fn activate_experiment(
    client: &Client,
    server_base: &str,
    admin: &AdminAuth,
    experiment_id: &str,
) -> Result<Response> {
    Ok(client
        .post(format!(
            "{server_base}/api/admin/runtime/{experiment_id}/experiment/status"
        ))
        .header(header::COOKIE, &admin.cookie)
        .header("x-csrf-token", &admin.csrf)
        .json(&json!({"status": "active"}))
        .send()
        .await?)
}

/// Creates the mock submission and signed token before calling public Parlando intake.
async fn create_signed_participant(
    client: &Client,
    mock_base: &str,
    server_base: &str,
    participant_id: &str,
    session_id: &str,
) -> Result<Participant> {
    create_signed_participant_for(
        client,
        mock_base,
        server_base,
        EXPERIMENT_ID,
        participant_id,
        session_id,
    )
    .await
}

/// Creates one signed Prolific participant for a selected experiment runtime.
async fn create_signed_participant_for(
    client: &Client,
    mock_base: &str,
    server_base: &str,
    experiment_id: &str,
    participant_id: &str,
    session_id: &str,
) -> Result<Participant> {
    add_mock_submission(client, mock_base, participant_id, session_id).await?;
    let audience = external_study_url(server_base, experiment_id);
    let token = sign_token(client, mock_base, participant_id, session_id, &audience).await?;
    let response = participant_request_for(
        client,
        server_base,
        experiment_id,
        participant_id,
        session_id,
        Some(&token),
    )
    .await?;
    let response = expect_status(
        response,
        StatusCode::OK,
        "admit signed Prolific participant",
    )
    .await?;
    let body = response.json::<Value>().await?;
    Ok(Participant {
        credential: body["participant_credential"]
            .as_str()
            .context("participant response omitted credential")?
            .to_string(),
        research_id: body["participant_id"]
            .as_str()
            .context("participant response omitted research id")?
            .to_string(),
        experiment_id: experiment_id.to_string(),
    })
}

/// Creates one active provider submission independently of the selected launch method.
async fn add_mock_submission(
    client: &Client,
    mock_base: &str,
    participant_id: &str,
    session_id: &str,
) -> Result<()> {
    let response = client
        .post(format!("{mock_base}/__mock/submissions"))
        .json(&json!({
            "id": session_id,
            "participant": participant_id,
            "status": "ACTIVE"
        }))
        .send()
        .await?;
    expect_status(response, StatusCode::OK, "create mock submission").await?;
    Ok(())
}

/// Obtains a realistic RS256 Secure external URL token from the independent mock.
async fn sign_token(
    client: &Client,
    mock_base: &str,
    participant_id: &str,
    session_id: &str,
    audience: &str,
) -> Result<String> {
    let response = client
        .post(format!("{mock_base}/__mock/sign"))
        .json(&json!({
            "participant_id": participant_id,
            "session_id": session_id,
            "audience": audience
        }))
        .send()
        .await?;
    let response = expect_status(response, StatusCode::OK, "sign mock launch").await?;
    let body = response.json::<Value>().await?;
    body["token"]
        .as_str()
        .context("mock sign response omitted token")
        .map(str::to_string)
}

/// Calls the real participant-creation endpoint with Prolific launch values.
async fn participant_request(
    client: &Client,
    server_base: &str,
    participant_id: &str,
    session_id: &str,
    token: &str,
) -> Result<Response> {
    participant_request_for(
        client,
        server_base,
        EXPERIMENT_ID,
        participant_id,
        session_id,
        Some(token),
    )
    .await
}

/// Calls one experiment's public intake with either signed or API-verified launch values.
async fn participant_request_for(
    client: &Client,
    server_base: &str,
    experiment_id: &str,
    participant_id: &str,
    session_id: &str,
    token: Option<&str>,
) -> Result<Response> {
    Ok(client
        .post(format!("{server_base}/e/{experiment_id}/api/participants"))
        .json(&json!({"prolific": {
            "participant_id": participant_id,
            "study_id": STUDY_ID,
            "session_id": session_id,
            "prolific_token": token
        }}))
        .send()
        .await?)
}

/// Admits one authenticated participant to matchmaking.
async fn create_session(
    client: &Client,
    server_base: &str,
    participant: &Participant,
) -> Result<Value> {
    let response = create_session_response(client, server_base, participant).await?;
    expect_json(response, StatusCode::OK, "create session").await
}

/// Returns the raw matchmaking response for scenarios which expect admission rejection.
async fn create_session_response(
    client: &Client,
    server_base: &str,
    participant: &Participant,
) -> Result<Response> {
    Ok(client
        .post(format!(
            "{server_base}/e/{}/api/sessions",
            participant.experiment_id
        ))
        .bearer_auth(&participant.credential)
        .json(&json!({}))
        .send()
        .await?)
}

/// Records consent decisions through the authenticated participant endpoint.
async fn record_consent(
    client: &Client,
    server_base: &str,
    participant: &Participant,
    decisions: Value,
) -> Result<()> {
    let response = client
        .post(format!(
            "{server_base}/e/{}/api/consent",
            participant.experiment_id
        ))
        .bearer_auth(&participant.credential)
        .json(&json!({"decisions": decisions}))
        .send()
        .await?;
    expect_status(response, StatusCode::OK, "record consent decision").await?;
    Ok(())
}

/// Reads one authenticated participant's authoritative lifecycle state.
async fn participant_state(
    client: &Client,
    server_base: &str,
    participant: &Participant,
) -> Result<Value> {
    let response = client
        .get(format!(
            "{server_base}/e/{}/api/participant-state",
            participant.experiment_id
        ))
        .bearer_auth(&participant.credential)
        .send()
        .await?;
    expect_json(response, StatusCode::OK, "read participant state").await
}

/// Polls a participant until a server-owned lifecycle deadline produces terminal state.
async fn wait_for_outcome(
    client: &Client,
    server_base: &str,
    participant: &Participant,
    timeout: Duration,
) -> Result<Value> {
    let deadline = Instant::now() + timeout;
    loop {
        let state = participant_state(client, server_base, participant).await?;
        if state["participant_state"]["state"] == "ended" {
            return Ok(state);
        }
        if Instant::now() >= deadline {
            bail!("participant did not reach terminal state within {timeout:?}: {state}");
        }
        time::sleep(Duration::from_millis(100)).await;
    }
}

/// Records an explicit leave through the normal participant endpoint.
async fn leave_session(
    client: &Client,
    server_base: &str,
    participant: &Participant,
    public_session_id: &str,
) -> Result<Value> {
    let response = client
        .post(format!(
            "{server_base}/e/{}/api/sessions/{public_session_id}/leave",
            participant.experiment_id
        ))
        .bearer_auth(&participant.credential)
        .json(&json!({}))
        .send()
        .await?;
    expect_json(response, StatusCode::OK, "leave session").await
}

/// Opens both real game WebSockets so a fully assigned dyad enters the active phase.
async fn open_game_pair(
    client: &Client,
    server_base: &str,
    public_session_id: &str,
    first: &Participant,
    second: &Participant,
) -> Result<(GameSocket, GameSocket)> {
    let first_socket = open_game_socket(client, server_base, public_session_id, first).await?;
    let second_socket = open_game_socket(client, server_base, public_session_id, second).await?;
    Ok((first_socket, second_socket))
}

/// Creates and starts one Prolific dyad for a selected experiment.
async fn create_active_pair(
    client: &Client,
    mock_base: &str,
    server_base: &str,
    experiment_id: &str,
    suffix: &str,
) -> Result<(Participant, Participant, String, GameSocket, GameSocket)> {
    let first = create_signed_participant_for(
        client,
        mock_base,
        server_base,
        experiment_id,
        &format!("participant{suffix}A"),
        &format!("submission{suffix}A"),
    )
    .await?;
    let second = create_signed_participant_for(
        client,
        mock_base,
        server_base,
        experiment_id,
        &format!("participant{suffix}B"),
        &format!("submission{suffix}B"),
    )
    .await?;
    let waiting = create_session(client, server_base, &first).await?;
    let session = waiting["participant_state"]["public_session_id"]
        .as_str()
        .context("active-pair fixture omitted session id")?
        .to_string();
    let joined = create_session(client, server_base, &second).await?;
    if joined["participant_state"]["public_session_id"] != session {
        bail!("active-pair participants were not matched");
    }
    let (mut first_socket, mut second_socket) =
        open_game_pair(client, server_base, &session, &first, &second).await?;
    read_socket_state(&mut first_socket, "active").await?;
    read_socket_state(&mut second_socket, "active").await?;
    Ok((first, second, session, first_socket, second_socket))
}

/// Mints and consumes one production one-use game-socket ticket.
async fn open_game_socket(
    client: &Client,
    server_base: &str,
    public_session_id: &str,
    participant: &Participant,
) -> Result<GameSocket> {
    let response = client
        .post(format!(
            "{server_base}/e/{}/api/sessions/{public_session_id}/game-session",
            participant.experiment_id
        ))
        .bearer_auth(&participant.credential)
        .json(&json!({}))
        .send()
        .await?;
    let ticket = expect_json(response, StatusCode::OK, "mint game WebSocket ticket").await?;
    let token = ticket["token"]
        .as_str()
        .context("game WebSocket plan omitted token")?;
    let ws_base = server_base
        .strip_prefix("http://")
        .map(|base| format!("ws://{base}"))
        .or_else(|| {
            server_base
                .strip_prefix("https://")
                .map(|base| format!("wss://{base}"))
        })
        .context("server base URL is not HTTP or HTTPS")?;
    let (socket, _) = connect_async(format!(
        "{ws_base}/e/{}/ws/game/{public_session_id}?token={token}",
        participant.experiment_id
    ))
    .await?;
    Ok(socket)
}

/// Reads game protocol messages until one authoritative state reaches the expected phase.
async fn read_socket_state(socket: &mut GameSocket, expected: &str) -> Result<Value> {
    let deadline = time::Instant::now() + Duration::from_secs(10);
    loop {
        let message = time::timeout_at(deadline, socket.next())
            .await
            .context("timed out waiting for game participant state")?
            .context("game WebSocket closed before participant state")??;
        if let Message::Text(text) = message {
            let value: Value = serde_json::from_str(&text)?;
            if value["type"] == "error" {
                bail!("game WebSocket reported an error: {value}");
            }
            if value["type"] == "participant_state"
                && value["participant_state"]["state"] == expected
            {
                return Ok(value["participant_state"].clone());
            }
        }
    }
}

/// Verifies one terminal outcome and its exact provider handoff code.
fn assert_outcome(value: &Value, outcome: &str, code: &str) -> Result<()> {
    let result = &value["participant_state"]["result"];
    if value["participant_state"]["state"] != "ended"
        || result["outcome"] != outcome
        || result["handoff"]["provider"] != "prolific"
        || result["handoff"]["code"] != code
    {
        bail!("expected ended/{outcome}/{code}, received {value}");
    }
    let expected_url = format!("https://app.prolific.com/submissions/complete?cc={code}");
    if result["handoff"]["url"] != expected_url {
        bail!("completion URL did not preserve the configured code");
    }
    Ok(())
}

/// Checks dashboard clocks, shared causes, and participant-specific results for each workflow.
fn assert_dashboard_workflow(row: &Value, detail: &Value, public_session_id: &str) -> Result<()> {
    let waiting_started = DateTime::parse_from_rfc3339(
        row["waiting_started_at"]
            .as_str()
            .context("dashboard row omitted waiting start")?,
    )?;
    let ended = DateTime::parse_from_rfc3339(
        row["ended_at"]
            .as_str()
            .context("dashboard row omitted end time")?,
    )?;
    if ended < waiting_started {
        bail!("session {public_session_id} ended before its recorded waiting start");
    }
    let participants = detail["participants"]
        .as_array()
        .context("session detail omitted participants")?;
    let outcomes = participants
        .iter()
        .filter_map(|participant| {
            participant["participant_state"]["result"]["outcome"]
                .as_str()
                .map(str::to_string)
        })
        .collect::<Vec<_>>();
    let cause = row["session_end"]["cause"]["type"]
        .as_str()
        .context("dashboard row omitted terminal cause")?;
    match cause {
        "partner_unavailable" => {
            if outcomes != ["partner_unavailable"]
                || (ended - waiting_started).num_milliseconds() < 2_000
            {
                bail!("deadline-unmatched session has inconsistent timing or outcome");
            }
        }
        "participant_left" if outcomes.len() == 1 => {
            if outcomes != ["left_waiting_room"] {
                bail!("voluntary waiting departure has the wrong participant outcome");
            }
        }
        "participant_left" => {
            if outcomes.len() != 2
                || !outcomes.iter().any(|outcome| outcome == "partner_left")
                || !outcomes
                    .iter()
                    .any(|outcome| outcome == "left_game" || outcome == "left_waiting_room")
            {
                bail!("departure did not preserve leaver and partner outcomes");
            }
        }
        "reconnect_timed_out" => {
            if outcomes.is_empty()
                || outcomes
                    .iter()
                    .any(|outcome| outcome != "connection_lost" && outcome != "partner_left")
                || !outcomes.iter().any(|outcome| outcome == "connection_lost")
            {
                bail!("reconnect expiry has inconsistent participant outcomes");
            }
        }
        "idle_timed_out" => {
            if outcomes.len() != 2
                || outcomes
                    .iter()
                    .any(|outcome| outcome != "idle_limit_reached")
            {
                bail!("idle expiry did not preserve two idle-limit outcomes");
            }
        }
        "lifetime_timed_out" => {
            if outcomes.len() != 2
                || outcomes
                    .iter()
                    .any(|outcome| outcome != "lifetime_limit_reached")
            {
                bail!("lifetime expiry did not preserve two lifetime-limit outcomes");
            }
        }
        "game_completed" => {
            if outcomes.len() != 2 || outcomes.iter().any(|outcome| outcome != "completed") {
                bail!("completed session did not preserve two completed outcomes");
            }
            if row["completion"]["actions"] != 1 {
                bail!("completed dashboard row omitted the game result");
            }
        }
        other => bail!("unexpected dashboard workflow cause {other}"),
    }
    Ok(())
}

/// Performs one authenticated administrator GET and decodes its JSON response.
async fn admin_json(
    client: &Client,
    server_base: &str,
    admin: &AdminAuth,
    path: &str,
) -> Result<Value> {
    let response = client
        .get(format!("{server_base}{path}"))
        .header(header::COOKIE, &admin.cookie)
        .send()
        .await?;
    expect_json(response, StatusCode::OK, "administrator read").await
}

/// Retrieves the mock's redacted provider request evidence.
async fn mock_requests(client: &Client, mock_base: &str) -> Result<Value> {
    let response = client
        .get(format!("{mock_base}/__mock/requests"))
        .send()
        .await?;
    expect_json(response, StatusCode::OK, "read mock journal").await
}

/// Confirms one exact provider path appeared with valid authentication.
fn assert_journal_call(journal: &Value, path: &str) -> Result<()> {
    let found = journal["requests"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|call| call["path"] == path && call["authorized"] == true);
    if !found {
        bail!("provider journal omitted authenticated call to {path}");
    }
    Ok(())
}

/// Validates one response status while retaining its body in diagnostics.
async fn expect_status(
    response: Response,
    expected: StatusCode,
    operation: &str,
) -> Result<Response> {
    if response.status() == expected {
        return Ok(response);
    }
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    Err(anyhow!("{operation} returned {status}: {body}"))
}

/// Validates and decodes one JSON response.
async fn expect_json(response: Response, expected: StatusCode, operation: &str) -> Result<Value> {
    expect_status(response, expected, operation)
        .await?
        .json::<Value>()
        .await
        .with_context(|| format!("{operation} returned invalid JSON"))
}

/// Stops children, prints the final matrix, writes JSON, and returns CI status.
#[allow(clippy::too_many_arguments)]
fn finish_run(
    args: Args,
    temporary: TempDir,
    started_at: u64,
    started: Instant,
    mock_base: String,
    server_base: String,
    database: PathBuf,
    mut results: Vec<ScenarioResult>,
    mut mock: ManagedChild,
    mut server: ManagedChild,
) -> Result<()> {
    if results.len() < SCENARIOS.len() {
        let first_unattempted = results.len();
        skip_remaining(
            &mut results,
            first_unattempted,
            "runner ended before scenario was attempted",
        );
    }
    server.stop();
    mock.stop();
    let passed = results
        .iter()
        .filter(|result| matches!(result.status, ScenarioStatus::Passed))
        .count();
    let failed = results
        .iter()
        .filter(|result| matches!(result.status, ScenarioStatus::Failed))
        .count();
    let skipped = results
        .iter()
        .filter(|result| matches!(result.status, ScenarioStatus::Skipped))
        .count();
    let success = failed == 0 && skipped == 0;
    let database_preserved = args.keep || !success;
    let report = RunReport {
        schema_version: 1,
        started_at_unix_seconds: started_at,
        elapsed_ms: started.elapsed().as_millis(),
        mock_base_url: mock_base,
        parlando_base_url: server_base,
        database_path: database_preserved.then(|| database.display().to_string()),
        database_preserved,
        success,
        passed,
        failed,
        skipped,
        scenarios: results,
    };
    let run_dir = server
        .stdout_path
        .parent()
        .context("server log has no artifact directory")?;
    let report_path = run_dir.join("report.json");
    std::fs::write(&report_path, serde_json::to_vec_pretty(&report)?)?;
    print_summary(&report, &report_path);
    if database_preserved {
        let kept = temporary.keep();
        println!("Database directory preserved: {}", kept.display());
    }
    if !success {
        bail!("Prolific scenario matrix failed: {failed} failed, {skipped} skipped");
    }
    Ok(())
}

/// Renders a compact final matrix with statuses, durations, and actionable details.
fn print_summary(report: &RunReport, report_path: &Path) {
    println!("\nFinal scenario matrix");
    println!("{:<9}  {:<6}  {:>8}  Scenario", "ID", "Status", "Time");
    println!("{:-<9}  {:-<6}  {:-<8}  {:-<58}", "", "", "", "");
    for result in &report.scenarios {
        let status = match result.status {
            ScenarioStatus::Passed => "PASS",
            ScenarioStatus::Failed => "FAIL",
            ScenarioStatus::Skipped => "SKIP",
        };
        println!(
            "{:<9}  {:<6}  {:>6}ms  {}",
            result.id, status, result.elapsed_ms, result.name
        );
        if !matches!(result.status, ScenarioStatus::Passed) {
            if let Some(detail) = &result.detail {
                println!("           └─ {detail}");
            }
        }
    }
    println!(
        "\nResult: {} passed, {} failed, {} skipped in {:.2}s",
        report.passed,
        report.failed,
        report.skipped,
        report.elapsed_ms as f64 / 1000.0
    );
    println!("JSON report: {}", report_path.display());
    println!(
        "Child logs: {}",
        report_path.parent().unwrap_or(Path::new(".")).display()
    );
}
