//! Shared game and agent fixtures for end-to-end headless runner contracts.

mod execution;
mod resume;
mod training;
mod validation;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, OnceLock};

use async_trait::async_trait;
use serde::Deserialize;
use tempfile::TempDir;

use super::planning::hash_value;
use super::spec::{ExperimentFile, EXPERIMENT_SCHEMA};
use super::*;
use crate::game::AgentDefinition;
use crate::{
    agents::{Agent, AgentContext, AgentIdentity, AgentResponse},
    component::{ComponentValue, GameFrame},
    game::ActionRejection,
};
use serde_json::{json, Value};
use std::time::Duration;
use uuid::Uuid;

/// Compiles the embedded guest once, then shares it across independent test sessions.
///
/// Rebuild from the repository root:
/// ```sh
/// fixture=rust-server-tests/stress-fixture
/// cargo build --locked --release --target wasm32-unknown-unknown \
///   --manifest-path "$fixture/Cargo.toml" --example headless
/// core="$fixture/target/wasm32-unknown-unknown/release/examples/headless.wasm"
/// wasm-tools component new "$core" -o "$core.component"
/// wasm-tools strip --all "$core.component" -o rust-server/src/agent_experiment/tests/headless.wasm
/// ```
fn tiny_game() -> LoadedGame {
    static GAME: OnceLock<LoadedGame> = OnceLock::new();
    GAME.get_or_init(|| LoadedGame::from_bytes(include_bytes!("tests/headless.wasm")).unwrap())
        .clone()
}

/// The counter and role exposed by the test guest.
#[derive(Deserialize)]
struct TinyObservation {
    count: u8,
    role: String,
}

/// Encodes one counter action using the same named fields as the guest schema.
fn increment(amount: u8) -> ComponentValue {
    ComponentValue::from_typed(&json!({"amount": amount})).unwrap()
}

/// Factory which creates role-aware scripted agents and records lifecycle calls.
struct TinyAgentFactory {
    events: Arc<Mutex<Vec<String>>>,
    fail_role: Option<PlayerRole>,
    increments: Vec<u8>,
}

/// Agent which plays a scripted sequence from role A and yields from role B.
struct TinyAgent {
    role: PlayerRole,
    increments: VecDeque<u8>,
    events: Arc<Mutex<Vec<String>>>,
}

/// Agent whose start callback never completes, used to verify callback deadlines.
struct HangingStartAgent;

#[async_trait]
impl Agent for HangingStartAgent {
    /// Deliberately remains pending until the runner's callback timeout cancels it.
    async fn start(&mut self, _initial_observation: ComponentValue) -> Result<()> {
        std::future::pending().await
    }

    /// Would yield immediately if execution incorrectly progressed past start.
    async fn respond(&mut self) -> Result<Option<AgentResponse>> {
        Ok(None)
    }
}

/// Creates timeout-test agents without coupling the ordinary scripted fixture to failures.
struct HangingStartFactory;

#[async_trait]
impl AgentFactory for HangingStartFactory {
    /// Exposes a settings-free timeout-test definition.
    fn definition(&self) -> AgentDefinition {
        AgentDefinition {
            id: "tiny.hanging".to_string(),
            name: "Hanging start agent".to_string(),
            description: "Never completes start.".to_string(),
            config_fields: Vec::new(),
        }
    }

    /// Creates one deliberately hanging instance.
    async fn create(&self, _context: AgentContext) -> Result<Box<dyn Agent + Send>> {
        Ok(Box::new(HangingStartAgent))
    }

    /// Returns stable timeout-test provenance.
    fn identity(&self, _settings: &Value) -> Result<AgentIdentity> {
        Ok(AgentIdentity {
            name: "HangingStart".to_string(),
            version: "1".to_string(),
        })
    }
}

#[async_trait]
impl Agent for TinyAgent {
    /// Records initial observation delivery.
    async fn start(&mut self, initial_observation: ComponentValue) -> Result<()> {
        anyhow::ensure!(
            initial_observation.decode::<TinyObservation>()?.role == self.role.as_str()
        );
        self.events
            .lock()
            .unwrap()
            .push(format!("start:{}", self.role.as_str()));
        Ok(())
    }

    /// Records accepted transition delivery.
    async fn observe_transition(
        &mut self,
        _actor: PlayerRole,
        _action: ComponentValue,
        observation: ComponentValue,
    ) -> Result<()> {
        anyhow::ensure!(observation.decode::<TinyObservation>()?.role == self.role.as_str());
        self.events
            .lock()
            .unwrap()
            .push(format!("observe:{}", self.role.as_str()));
        Ok(())
    }

    /// Produces the next scripted increment from role A or yields.
    async fn respond(&mut self) -> Result<Option<AgentResponse>> {
        Ok(self
            .increments
            .pop_front()
            .map(|amount| AgentResponse::action(increment(amount))))
    }

    /// Records normal completion delivery.
    async fn finish(&mut self, _completion: ComponentValue) -> Result<()> {
        self.events
            .lock()
            .unwrap()
            .push(format!("finish:{}", self.role.as_str()));
        Ok(())
    }

    /// Records unconditional cleanup.
    async fn shutdown(&mut self) -> Result<()> {
        self.events
            .lock()
            .unwrap()
            .push(format!("shutdown:{}", self.role.as_str()));
        Ok(())
    }
}

#[async_trait]
impl AgentFactory for TinyAgentFactory {
    /// Exposes one settings-free compiled agent definition.
    fn definition(&self) -> AgentDefinition {
        AgentDefinition {
            id: "tiny.agent".to_string(),
            name: "Tiny agent".to_string(),
            description: "Completes the tiny test game from role A.".to_string(),
            config_fields: Vec::new(),
        }
    }

    /// Creates one role-bound scripted agent and records construction.
    async fn create(&self, context: AgentContext) -> Result<Box<dyn Agent + Send>> {
        self.events
            .lock()
            .unwrap()
            .push(format!("create:{}", context.role.as_str()));
        if self.fail_role == Some(context.role) {
            bail!(
                "deliberate factory failure for role {}",
                context.role.as_str()
            );
        }
        Ok(Box::new(TinyAgent {
            role: context.role,
            increments: if context.role == PlayerRole::A {
                self.increments.clone().into()
            } else {
                VecDeque::new()
            },
            events: self.events.clone(),
        }))
    }

    /// Returns stable provenance for every test agent instance.
    fn identity(&self, _settings: &Value) -> Result<AgentIdentity> {
        Ok(AgentIdentity {
            name: "TinyAgent".to_string(),
            version: "1".to_string(),
        })
    }
}

/// Renders the smallest complete experiment using the requested output directory.
fn experiment_yaml(output: &Path) -> String {
    format!(
        r#"schema: parlando-agent-experiment/v1
game:
  id: tiny
agents:
  policy:
    factory: tiny.agent
sessions:
  scenarios:
    - name: one-step
      config: {{ target: 1 }}
      seeds: [7]
  seats:
    player_a: policy
    player_b: policy
schedule:
  kind: alternate_every_response
  first: player_a
limits:
  callback_timeout_seconds: 1
  shutdown_timeout_seconds: 1
  session_timeout_seconds: 5
  decisions: 4
  accepted_actions: 2
  messages: 2
  rejected_actions: 2
execution:
  concurrency: 2
output:
  directory: '{}'
  trace: decisions
"#,
        output.display()
    )
}

/// Builds the test runner with a fresh lifecycle event sink.
fn test_runner(events: Arc<Mutex<Vec<String>>>) -> ExperimentRunner {
    test_runner_with_increments(events, vec![1])
}

/// Builds a runner whose role-A agent proposes the supplied increments.
fn test_runner_with_increments(
    events: Arc<Mutex<Vec<String>>>,
    increments: Vec<u8>,
) -> ExperimentRunner {
    ExperimentRunner::new(
        GameMetadata {
            id: "tiny".to_string(),
            name: "Tiny".to_string(),
            version: semver::Version::new(1, 0, 0),
            build_manifest: json!({"test": true}),
        },
        tiny_game(),
    )
    .unwrap()
    .agent(TinyAgentFactory {
        events,
        fail_role: None,
        increments,
    })
    .unwrap()
}

/// Builds a runner whose second agent constructor fails after role A exists.
fn failing_test_runner(events: Arc<Mutex<Vec<String>>>) -> ExperimentRunner {
    ExperimentRunner::new(
        GameMetadata {
            id: "tiny".to_string(),
            name: "Tiny".to_string(),
            version: semver::Version::new(1, 0, 0),
            build_manifest: json!({"test": true}),
        },
        tiny_game(),
    )
    .unwrap()
    .agent(TinyAgentFactory {
        events,
        fail_role: Some(PlayerRole::B),
        increments: vec![1],
    })
    .unwrap()
}
