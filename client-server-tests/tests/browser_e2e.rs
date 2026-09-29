use std::{collections::BTreeSet, fs, path::Path, sync::Arc, time::Instant};

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use parlando::{
    agent::{
        Context as AgentContext, Definition as AgentDefinition, Factory as AgentFactory,
        Identity as AgentIdentity,
    },
    test_support::{AgentsConfig, AgentsMode, ExperimentConfig, HumanVsAgentConfig, ServeOptions},
};
use parlando_client_server_contract_tests::{
    contract_config, enable_browser_fixture, enable_voice, run_browser_driver, spawn_server,
    ContractGame,
};
use serde::Deserialize;
use serde_json::json;
use serde_json::Value;

/// One configurable real-browser state-machine scenario.
struct Scenario {
    name: &'static str,
    configure: fn(&mut ExperimentConfig),
    options: fn() -> ServeOptions<ContractGame>,
}

/// Agent factory whose asynchronous construction failure exercises infrastructure termination.
struct FailingAgentFactory;

#[async_trait]
impl AgentFactory<ContractGame> for FailingAgentFactory {
    /// Identifies the test-only factory selected by the scenario configuration.
    fn definition(&self) -> AgentDefinition {
        AgentDefinition {
            id: "browser.failure".to_string(),
            name: "Browser failure fixture".to_string(),
            description: "Fails construction for end-to-end lifecycle coverage.".to_string(),
            config_fields: Vec::new(),
        }
    }

    /// Fails after the human has received a waiting assignment.
    async fn create(
        &self,
        _context: AgentContext,
    ) -> Result<Box<dyn parlando::agent::Agent<ContractGame> + Send>> {
        anyhow::bail!("intentional browser end-to-end agent construction failure")
    }

    /// Supplies stable provenance before the deliberate construction failure.
    fn identity(&self, _settings: &Value) -> Result<AgentIdentity> {
        Ok(AgentIdentity {
            name: "Browser failure fixture".to_string(),
            version: "1".to_string(),
        })
    }
}

/// Structured result returned by the browser process even when an assertion fails.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScenarioResult {
    scenario: String,
    status: String,
    elapsed_ms: u64,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    transitions: Vec<String>,
    #[serde(default)]
    outcomes: Vec<String>,
    #[serde(default)]
    end_causes: Vec<String>,
}

/// Retains every result so the matrix reports later failures after an earlier one.
#[derive(Default)]
struct MatrixReport {
    results: Vec<ScenarioResult>,
    transitions: BTreeSet<String>,
    outcomes: BTreeSet<String>,
    end_causes: BTreeSet<String>,
}

impl MatrixReport {
    /// Records observed coverage only from scenarios whose assertions completed successfully.
    fn record(&mut self, result: ScenarioResult) {
        if result.status == "passed" {
            self.transitions.extend(result.transitions.iter().cloned());
            self.outcomes.extend(result.outcomes.iter().cloned());
            self.end_causes.extend(result.end_causes.iter().cloned());
        }
        self.results.push(result);
    }

    /// Produces the durable human-readable report used locally and in release CI artifacts.
    fn markdown(&self) -> String {
        let mut text = String::from("# Parlando browser end-to-end report\n\n");
        text.push_str("| Scenario | Status | Time | Detail |\n|---|---:|---:|---|\n");
        for result in &self.results {
            let detail = result
                .error
                .as_deref()
                .unwrap_or("")
                .lines()
                .next()
                .unwrap_or("")
                .replace('|', "\\|");
            text.push_str(&format!(
                "| {} | {} | {} ms | {} |\n",
                result.scenario, result.status, result.elapsed_ms, detail
            ));
        }
        text.push_str("\n## Observed state-machine coverage\n\n");
        text.push_str(&format!("- Transitions: {}\n", joined(&self.transitions)));
        text.push_str(&format!(
            "- Participant outcomes: {}\n",
            joined(&self.outcomes)
        ));
        text.push_str(&format!(
            "- Session end causes: {}\n",
            joined(&self.end_causes)
        ));
        text
    }
}

/// Joins one sorted coverage vocabulary without hiding an empty observation set.
fn joined(values: &BTreeSet<String>) -> String {
    if values.is_empty() {
        "none".to_string()
    } else {
        values.iter().cloned().collect::<Vec<_>>().join(", ")
    }
}

/// Leaves ordinary lifecycle limits in place for scenarios that terminate themselves.
fn ordinary(_config: &mut ExperimentConfig) {}

/// Makes the unmatched-participant deadline short enough for an end-to-end test.
fn short_wait(config: &mut ExperimentConfig) {
    config.session.waiting_session_timeout_seconds = 4;
}

/// Leaves enough reconnect time for both browser reloads in the paused-state scenario.
fn paused_refresh_window(config: &mut ExperimentConfig) {
    config.session.reconnect_grace_seconds = 6;
}

/// Permits a real browser to reconnect while keeping the test short.
fn reconnect_window(config: &mut ExperimentConfig) {
    config.session.reconnect_grace_seconds = 3;
}

/// Makes reconnect expiry observable without a long test delay.
fn short_reconnect(config: &mut ExperimentConfig) {
    config.session.reconnect_grace_seconds = 1;
}

/// Makes the inactivity deadline short while retaining a later absolute lifetime.
fn short_idle(config: &mut ExperimentConfig) {
    config.session.session_idle_timeout_seconds = 2;
    config.session.session_max_lifetime_seconds = 10;
}

/// Makes the absolute lifetime short while accepted actions keep resetting inactivity.
fn short_lifetime(config: &mut ExperimentConfig) {
    config.session.session_idle_timeout_seconds = 2;
    config.session.session_max_lifetime_seconds = 3;
}

/// Enables real browser audio with a short, observable post-completion period.
fn short_farewell(config: &mut ExperimentConfig) {
    enable_voice(config);
    config.voice.post_completion_seconds = 4;
}

/// Selects a human-versus-agent session whose agent construction deliberately fails.
fn failing_agent(config: &mut ExperimentConfig) {
    config.agents = AgentsConfig {
        mode: AgentsMode::HumanVsAgent,
        human_vs_agent: Some(HumanVsAgentConfig {
            factory: Some("browser.failure".to_string()),
            ..HumanVsAgentConfig::default()
        }),
    };
}

/// Returns ordinary server dependencies for human-human browser scenarios.
fn ordinary_options() -> ServeOptions<ContractGame> {
    ServeOptions::default()
}

/// Installs the test-only failing agent factory for technical-failure coverage.
fn failing_agent_options() -> ServeOptions<ContractGame> {
    ServeOptions {
        agent_factory: Some(Arc::new(FailingAgentFactory)),
        ..ServeOptions::default()
    }
}

/// Returns required vocabulary entries absent from the observed successful scenarios.
fn missing(required: &[&str], observed: &BTreeSet<String>) -> Vec<String> {
    required
        .iter()
        .filter(|value| !observed.contains(**value))
        .map(|value| (*value).to_string())
        .collect()
}

/// Runs the real-browser lifecycle matrix to completion and reports all failures together.
#[tokio::test]
#[ignore = "requires a locally installed Playwright browser; run through make test-browser-e2e"]
async fn real_browsers_exercise_the_participant_state_machine() -> Result<()> {
    let scenarios = [
        Scenario {
            name: "happy-path",
            configure: ordinary,
            options: ordinary_options,
        },
        Scenario {
            name: "waiting-leave",
            configure: ordinary,
            options: ordinary_options,
        },
        Scenario {
            name: "waiting-timeout",
            configure: short_wait,
            options: ordinary_options,
        },
        Scenario {
            name: "waiting-refresh",
            configure: short_wait,
            options: ordinary_options,
        },
        Scenario {
            name: "reconnect",
            configure: reconnect_window,
            options: ordinary_options,
        },
        Scenario {
            name: "paused-refresh",
            configure: paused_refresh_window,
            options: ordinary_options,
        },
        Scenario {
            name: "duplicate-tab",
            configure: reconnect_window,
            options: ordinary_options,
        },
        Scenario {
            name: "reconnect-expires",
            configure: short_reconnect,
            options: ordinary_options,
        },
        Scenario {
            name: "active-leave",
            configure: ordinary,
            options: ordinary_options,
        },
        Scenario {
            name: "idle-timeout",
            configure: short_idle,
            options: ordinary_options,
        },
        Scenario {
            name: "lifetime-timeout",
            configure: short_lifetime,
            options: ordinary_options,
        },
        Scenario {
            name: "technical-failure",
            configure: failing_agent,
            options: failing_agent_options,
        },
        Scenario {
            name: "farewell-voice",
            configure: short_farewell,
            options: ordinary_options,
        },
    ];
    let started = Instant::now();
    let mut report = MatrixReport::default();

    for scenario in scenarios {
        let mut config = contract_config();
        (scenario.configure)(&mut config);
        enable_browser_fixture(&mut config)?;
        let server = spawn_server(config, (scenario.options)()).await?;
        let result = match run_browser_driver(&server, json!({"scenario": scenario.name})).await {
            Ok(value) => serde_json::from_value(value)?,
            Err(error) => ScenarioResult {
                scenario: scenario.name.to_string(),
                status: "failed".to_string(),
                elapsed_ms: 0,
                error: Some(format!("{error:#}")),
                transitions: Vec::new(),
                outcomes: Vec::new(),
                end_causes: Vec::new(),
            },
        };
        println!(
            "[{}] {:<20} {} ms{}",
            result.status.to_uppercase(),
            result.scenario,
            result.elapsed_ms,
            result
                .error
                .as_deref()
                .map(|error| format!("\n{error}"))
                .unwrap_or_default()
        );
        report.record(result);
    }

    let markdown = format!(
        "{}\n## Run summary\n\n- Total duration: {} ms\n- Passed: {}\n- Failed: {}\n",
        report.markdown(),
        started.elapsed().as_millis(),
        report
            .results
            .iter()
            .filter(|result| result.status == "passed")
            .count(),
        report
            .results
            .iter()
            .filter(|result| result.status != "passed")
            .count(),
    );
    let report_directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/browser-e2e");
    fs::create_dir_all(&report_directory)?;
    let report_path = report_directory.join("report.md");
    fs::write(&report_path, &markdown)?;
    println!("\n{markdown}\nReport: {}", report_path.display());

    let failures = report
        .results
        .iter()
        .filter(|result| result.status != "passed")
        .map(|result| result.scenario.clone())
        .collect::<Vec<_>>();
    let missing_transitions = missing(
        &[
            "registered->waiting",
            "waiting->active",
            "waiting->ended",
            "active->paused",
            "active->ended",
            "paused->active",
            "paused->ended",
        ],
        &report.transitions,
    );
    let missing_outcomes = missing(
        &[
            "completed",
            "left_waiting_room",
            "left_game",
            "connection_lost",
            "partner_left",
            "partner_unavailable",
            "idle_limit_reached",
            "technical_failure",
            "lifetime_limit_reached",
        ],
        &report.outcomes,
    );
    let missing_end_causes = missing(
        &[
            "game_completed",
            "participant_left",
            "partner_unavailable",
            "reconnect_timed_out",
            "idle_timed_out",
            "lifetime_timed_out",
            "technical_failure",
        ],
        &report.end_causes,
    );
    if !failures.is_empty()
        || !missing_transitions.is_empty()
        || !missing_outcomes.is_empty()
        || !missing_end_causes.is_empty()
    {
        return Err(anyhow!(
            "browser matrix incomplete: {} failed [{}]; missing transitions [{}], outcomes [{}], end causes [{}] (see {})",
            failures.len(),
            failures.join(", "),
            missing_transitions.join(", "),
            missing_outcomes.join(", "),
            missing_end_causes.join(", "),
            report_path.display()
        ));
    }
    Ok(())
}
