//! Shared game and agent fixtures for end-to-end headless runner contracts.

mod execution;
mod resume;
mod training;
mod validation;

use std::collections::VecDeque;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};

use async_trait::async_trait;
use serde::Deserialize;
use tempfile::TempDir;

use super::planning::hash_value;
use super::spec::{ExperimentFile, EXPERIMENT_SCHEMA};
use super::*;
use crate::game::AgentDefinition;
use crate::{
    agents::{Agent, AgentContext, AgentIdentity, AgentResponse},
    game::{ActionRejection, GameInitializationContext, GameSessionContext},
};
use serde_json::{json, Value};
use std::time::Duration;
use uuid::Uuid;

/// Minimal deterministic game used to exercise the complete headless lifecycle.
struct TinyGame;

/// Reusable constructor for a fresh tiny game per planned session.
#[derive(Default)]
struct TinyGameFactory {
    creations: Arc<AtomicUsize>,
}

/// Strict game configuration used by the YAML fixture.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct TinyConfig {
    target: u8,
}

/// Authoritative state which completes when its counter reaches the target.
#[derive(Deserialize, Serialize)]
struct TinyState {
    count: u8,
    target: u8,
}

/// One typed increment action.
#[derive(Clone, Deserialize, Serialize)]
struct TinyAction {
    amount: u8,
}

/// Role-safe observation for the tiny game.
#[derive(Deserialize, Serialize)]
struct TinyObservation {
    count: u8,
    role: PlayerRole,
}

/// Shared terminal result for the tiny game.
#[derive(Clone, Serialize)]
struct TinyCompletion {
    count: u8,
}

impl Game for TinyGame {
    type Config = TinyConfig;
    type State = TinyState;
    type Action = TinyAction;
    type Observation = TinyObservation;
    type Completion = TinyCompletion;

    /// Creates the configured zero state.
    fn initial_state(
        &self,
        context: GameInitializationContext<'_, Self::Config>,
    ) -> Result<Self::State> {
        Ok(TinyState {
            count: 0,
            target: context.config.target,
        })
    }

    /// Detects callers that bypass the fixture's step implementation.
    fn apply_action(
        &self,
        _state: &Self::State,
        _action: &Self::Action,
        _actor: PlayerRole,
    ) -> std::result::Result<Self::State, ActionRejection> {
        panic!("headless actions must use Game::step")
    }

    /// Applies a positive increment, rejects zero, or simulates a guest fault.
    fn step(
        &self,
        state: &Self::State,
        action: &Self::Action,
        _actor: PlayerRole,
    ) -> Result<std::result::Result<Self::State, ActionRejection>> {
        if action.amount == u8::MAX {
            bail!("guest step failed");
        }
        if action.amount == 0 {
            return Ok(Err(ActionRejection::new("zero_increment")));
        }
        Ok(Ok(TinyState {
            count: state.count.saturating_add(action.amount),
            target: state.target,
        }))
    }

    /// Reveals the public count and the viewer's own role.
    fn observation(&self, state: &Self::State, role: PlayerRole) -> Self::Observation {
        TinyObservation {
            count: state.count,
            role,
        }
    }

    /// Provides one fixture affordance encoding the current count and viewer's role.
    fn available_actions(&self, state: &Self::State, role: PlayerRole) -> Option<Vec<TinyAction>> {
        let offset = if role == PlayerRole::A { 1 } else { 2 };
        Some(vec![TinyAction {
            amount: state.count.saturating_add(offset),
        }])
    }

    /// Returns completion once the configured target is reached.
    fn completion(&self, state: &Self::State) -> Option<Self::Completion> {
        (state.count >= state.target).then_some(TinyCompletion { count: state.count })
    }
}

impl GameFactory for TinyGameFactory {
    type Game = TinyGame;

    /// Creates one stateless tiny game value.
    fn create(&self, _context: GameSessionContext) -> Result<Self::Game> {
        self.creations.fetch_add(1, Ordering::AcqRel);
        Ok(TinyGame)
    }
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
impl Agent<TinyGame> for HangingStartAgent {
    /// Deliberately remains pending until the runner's callback timeout cancels it.
    async fn start(&mut self, _initial_observation: TinyObservation) -> Result<()> {
        std::future::pending().await
    }

    /// Would yield immediately if execution incorrectly progressed past start.
    async fn respond(
        &mut self,
        _available_actions: Option<Vec<TinyAction>>,
    ) -> Result<Option<AgentResponse<TinyAction>>> {
        Ok(None)
    }
}

/// Creates timeout-test agents without coupling the ordinary scripted fixture to failures.
struct HangingStartFactory;

#[async_trait]
impl AgentFactory<TinyGame> for HangingStartFactory {
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
    async fn create(&self, _context: AgentContext) -> Result<Box<dyn Agent<TinyGame> + Send>> {
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
impl Agent<TinyGame> for TinyAgent {
    /// Records initial observation delivery.
    async fn start(&mut self, initial_observation: TinyObservation) -> Result<()> {
        anyhow::ensure!(initial_observation.role == self.role);
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
        _action: TinyAction,
        observation: TinyObservation,
    ) -> Result<()> {
        anyhow::ensure!(observation.role == self.role);
        self.events
            .lock()
            .unwrap()
            .push(format!("observe:{}", self.role.as_str()));
        Ok(())
    }

    /// Produces the next scripted increment from role A or yields.
    async fn respond(
        &mut self,
        _available_actions: Option<Vec<TinyAction>>,
    ) -> Result<Option<AgentResponse<TinyAction>>> {
        Ok(self
            .increments
            .pop_front()
            .map(|amount| AgentResponse::action(TinyAction { amount })))
    }

    /// Records normal completion delivery.
    async fn finish(&mut self, _completion: TinyCompletion) -> Result<()> {
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
impl AgentFactory<TinyGame> for TinyAgentFactory {
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
    async fn create(&self, context: AgentContext) -> Result<Box<dyn Agent<TinyGame> + Send>> {
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
fn test_runner(events: Arc<Mutex<Vec<String>>>) -> ExperimentRunner<TinyGame> {
    test_runner_with_increments(events, vec![1])
}

/// Builds a runner whose role-A agent proposes the supplied increments.
fn test_runner_with_increments(
    events: Arc<Mutex<Vec<String>>>,
    increments: Vec<u8>,
) -> ExperimentRunner<TinyGame> {
    ExperimentRunner::new(
        GameMetadata {
            id: "tiny".to_string(),
            name: "Tiny".to_string(),
            version: semver::Version::new(1, 0, 0),
            build_manifest: json!({"test": true}),
        },
        TinyGameFactory::default(),
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
fn failing_test_runner(events: Arc<Mutex<Vec<String>>>) -> ExperimentRunner<TinyGame> {
    ExperimentRunner::new(
        GameMetadata {
            id: "tiny".to_string(),
            name: "Tiny".to_string(),
            version: semver::Version::new(1, 0, 0),
            build_manifest: json!({"test": true}),
        },
        TinyGameFactory::default(),
    )
    .unwrap()
    .agent(TinyAgentFactory {
        events,
        fail_role: Some(PlayerRole::B),
        increments: vec![1],
    })
    .unwrap()
}
