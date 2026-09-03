//! Shared live-server fixture for Parlando's cross-language contract tests.
//!
//! This crate is deliberately separate from the publishable Rust runtime. Its tests may require
//! both Rust and Node, while ordinary `cargo test` for `parlando` remains self-contained.

use std::{path::Path, process::Stdio, time::Duration};

use anyhow::{anyhow, bail, Result};
use axum::Router;
use parlando::{
    test_support::{
        build_router, AgentsConfig, AgentsMode, ConsentItemConfig, DatabaseConfig, DirectConfig,
        ExperimentConfig, ExperimentIdentityConfig, ServeOptions, VoiceConfig,
    },
    ActionRejection, Game, GameFactory, GameInitializationContext, GameSessionContext, PlayerRole,
};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::{io::AsyncWriteExt, net::TcpListener};

/// Complete authoritative state for the contract fixture game.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ContractState {
    actions: usize,
    done: bool,
}

/// Actions chosen to expose accepted, rejected, and completing transitions.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ContractAction {
    /// Advances the shared counter and optionally completes the game.
    Mark { finish: bool },
    /// Exercises a stable game-level rejection without changing state.
    Reject,
}

/// Role-specific public projection returned after every accepted action.
#[derive(Clone, Debug, Serialize)]
pub struct ContractObservation {
    role: String,
    actions: usize,
    done: bool,
}

/// Shared completion value delivered to both participant roles.
#[derive(Clone, Debug, Serialize)]
pub struct ContractCompletion {
    done: bool,
    actions: usize,
}

/// Stateless game implementation used by every contract scenario.
#[derive(Clone, Copy)]
pub struct ContractGame;

/// Creates one independent contract game for each admitted session.
#[derive(Clone, Copy)]
pub struct ContractGameFactory;

impl GameFactory for ContractGameFactory {
    type Game = ContractGame;

    /// Creates the fixture without external resources or nondeterministic setup.
    fn create(&self, _context: GameSessionContext) -> Result<Self::Game> {
        Ok(ContractGame)
    }
}

impl Game for ContractGame {
    type Config = Value;
    type State = ContractState;
    type Action = ContractAction;
    type Observation = ContractObservation;
    type Completion = ContractCompletion;

    /// Starts every scenario from the same incomplete zero-action state.
    fn initial_state(
        &self,
        _context: GameInitializationContext<'_, Self::Config>,
    ) -> Result<Self::State> {
        Ok(ContractState {
            actions: 0,
            done: false,
        })
    }

    /// Applies one accepted marker or returns the fixture's deliberate rejection.
    fn apply_action(
        &self,
        state: &Self::State,
        action: &Self::Action,
        _actor: PlayerRole,
    ) -> std::result::Result<Self::State, ActionRejection> {
        match action {
            ContractAction::Reject => Err(ActionRejection::new("fixture_rejected")),
            ContractAction::Mark { finish } if state.done => {
                Err(ActionRejection::new("game_complete"))
            }
            ContractAction::Mark { finish } => Ok(ContractState {
                actions: state.actions + 1,
                done: *finish,
            }),
        }
    }

    /// Projects the shared counter together with only the recipient's role.
    fn observation(&self, state: &Self::State, player: PlayerRole) -> Self::Observation {
        ContractObservation {
            role: player.as_str().to_string(),
            actions: state.actions,
            done: state.done,
        }
    }

    /// Enumerates all meaningful fixture actions for client affordance checks.
    fn available_actions(
        &self,
        _state: &Self::State,
        _player: PlayerRole,
    ) -> Option<Vec<Self::Action>> {
        Some(vec![
            ContractAction::Mark { finish: false },
            ContractAction::Mark { finish: true },
            ContractAction::Reject,
        ])
    }

    /// Completes precisely after an accepted marker requests completion.
    fn completion(&self, state: &Self::State) -> Option<Self::Completion> {
        state.done.then_some(ContractCompletion {
            done: true,
            actions: state.actions,
        })
    }
}

/// Running live router backed by an isolated temporary SQLite database.
pub struct TestServer {
    /// HTTP origin used by request clients and the Node driver.
    pub base_url: String,
    _temp: TempDir,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for TestServer {
    /// Stops the fixture listener before deleting its temporary database.
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Returns a complete direct human-human experiment configuration for one scenario.
pub fn contract_config() -> ExperimentConfig {
    ExperimentConfig {
        experiment: ExperimentIdentityConfig {
            id: Some("client-server-contract".to_string()),
        },
        database: DatabaseConfig {
            url: "sqlite:///:memory:".to_string(),
        },
        direct: DirectConfig {
            enabled: true,
            participant_information_version: "contract-v1".to_string(),
            participant_information_url: "https://example.test/participant-information".to_string(),
            consents: vec![ConsentItemConfig {
                id: "study".to_string(),
                title: "Contract consent".to_string(),
                body: "I agree to the deterministic contract fixture.".to_string(),
                required: true,
            }],
        },
        agents: AgentsConfig {
            mode: AgentsMode::HumanVsHuman,
            ..AgentsConfig::default()
        },
        ..ExperimentConfig::default()
    }
}

/// Starts an active production router with the supplied fixture configuration and providers.
pub async fn spawn_server(
    mut config: ExperimentConfig,
    options: ServeOptions<ContractGame>,
) -> Result<TestServer> {
    let server = spawn_unactivated_server(&mut config, options).await?;
    activate_experiment(&server.base_url).await?;
    Ok(server)
}

/// Starts an inactive production router so closed-intake behavior can be tested directly.
pub async fn spawn_inactive_server(
    mut config: ExperimentConfig,
    options: ServeOptions<ContractGame>,
) -> Result<TestServer> {
    spawn_unactivated_server(&mut config, options).await
}

/// Builds one isolated router without changing its initial inactive lifecycle.
async fn spawn_unactivated_server(
    config: &mut ExperimentConfig,
    options: ServeOptions<ContractGame>,
) -> Result<TestServer> {
    let temp = TempDir::new()?;
    config.database.url = format!(
        "sqlite:///{}",
        temp.path().join("client-server-contract.sqlite").display()
    );
    let router = build_router(ContractGameFactory, config.clone(), options).await?;
    spawn_router(router, temp).await
}

/// Starts one Axum router on an ephemeral loopback port.
async fn spawn_router(router: Router, temp: TempDir) -> Result<TestServer> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let task = tokio::spawn(async move {
        axum::serve(listener, router)
            .await
            .expect("contract fixture server failed");
    });
    Ok(TestServer {
        base_url: format!("http://{address}"),
        _temp: temp,
        task,
    })
}

/// Creates the fixture administrator and opens participant intake.
async fn activate_experiment(base_url: &str) -> Result<()> {
    let client = Client::new();
    let response = client
        .post(format!("{base_url}/api/admin/setup"))
        .json(&json!({
            "username": "contract-administrator",
            "password": "contract-test-password"
        }))
        .send()
        .await?
        .error_for_status()?;
    let cookie = response
        .headers()
        .get(reqwest::header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .ok_or_else(|| anyhow!("administrator setup omitted its cookie"))?
        .to_string();
    let body: Value = response.json().await?;
    let csrf = body["csrf_token"]
        .as_str()
        .ok_or_else(|| anyhow!("administrator setup omitted its CSRF token"))?;
    client
        .post(format!("{base_url}/api/admin/experiment/status"))
        .header(reqwest::header::COOKIE, cookie)
        .header("x-csrf-token", csrf)
        .json(&json!({"status": "active"}))
        .send()
        .await?
        .error_for_status()?;
    Ok(())
}

/// Runs one Node contract driver against the live server and reports its full diagnostics.
///
/// The fixture is delivered through standard input so participant credentials created inside the
/// JavaScript process never need to cross the process boundary or appear in its environment.
pub async fn run_node_driver(server: &TestServer, driver: &str, fixture: Value) -> Result<Value> {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or_else(|| anyhow!("contract crate has no repository parent"))?;
    let mut child = tokio::process::Command::new("node")
        .arg(repository.join("client-server-tests/node").join(driver))
        .current_dir(repository)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let input = json!({
        "origin": server.base_url,
        "fixture": fixture,
    });
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| anyhow!("Node contract driver has no standard input"))?;
    stdin.write_all(input.to_string().as_bytes()).await?;
    stdin.shutdown().await?;
    drop(stdin);
    let output = tokio::time::timeout(Duration::from_secs(30), child.wait_with_output())
        .await
        .map_err(|_| anyhow!("Node contract driver {driver} timed out"))??;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        bail!("Node contract driver {driver} failed:\n{stderr}\n{stdout}");
    }
    let line = stdout
        .lines()
        .last()
        .ok_or_else(|| anyhow!("Node contract driver {driver} produced no result"))?;
    serde_json::from_str(line)
        .map_err(|error| anyhow!("Node contract driver {driver} returned invalid JSON: {error}"))
}

/// Enables the production audio route without selecting an external provider.
pub fn enable_voice(config: &mut ExperimentConfig) {
    config.voice = VoiceConfig {
        enabled: true,
        ..VoiceConfig::default()
    };
}
