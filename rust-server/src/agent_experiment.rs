//! Runs reproducible headless sessions for a compiled game and its registered agents.
//!
//! Game binaries register their game, agent factories, schedules, and optional learners with
//! `ExperimentRunner`. The runner validates a YAML specification, resolves session plans, and
//! hands them to execution. It returns run-wide counts and writes per-plan outcomes to the
//! selected artifact directory.

mod artifacts;
mod decisions;
mod epochs;
mod execution;
mod planning;
mod results;
mod schedule;
mod session;
mod spec;
mod training;

#[cfg(test)]
mod tests;

pub use results::{ResultAgent, SessionResult, SessionStatus};
pub use schedule::{AgentSchedule, AlternateAfterAction, AlternateEveryResponse};
pub use training::{
    CheckpointId, RLAgent, RLTrainingContext, RewardFunction, RoleRewards, TrainingBatch,
    TrajectoryStep,
};

use self::schedule::require_empty_parameters;
use self::spec::ExperimentFile;
use crate::agents::AgentFactory;
use crate::game::{Game, GameFactory, GameMetadata, PlayerRole};
use anyhow::{bail, Context, Result};
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Runs versioned YAML agent-agent experiments for one compiled game.
///
/// The registry contains reusable factories. Every planned session receives a fresh
/// game and two fresh agents, and no headless state is inserted into the live database.
pub struct ExperimentRunner<G: Game> {
    game: GameMetadata,
    game_factory: Arc<dyn GameFactory<Game = G>>,
    agent_factories: BTreeMap<String, Arc<dyn AgentFactory<G>>>,
    rl_agents: BTreeMap<String, Box<dyn RLAgent<G>>>,
    reward_functions: BTreeMap<String, Arc<dyn RewardFunction<G>>>,
    schedule_factories: BTreeMap<String, ScheduleFactory<G>>,
    secrets: HashMap<String, String>,
}

/// Constructor for one fresh activation schedule from validated YAML parameters.
type ScheduleFactory<G> =
    Arc<dyn Fn(PlayerRole, &Value) -> Result<Box<dyn AgentSchedule<G>>> + Send + Sync>;

impl<G: Game> ExperimentRunner<G> {
    /// Creates an empty runner registry for one stable game ID and factory.
    pub fn new<F>(game: GameMetadata, game_factory: F) -> Result<Self>
    where
        F: GameFactory<Game = G>,
    {
        game.validate()?;
        let mut schedule_factories = BTreeMap::<String, ScheduleFactory<G>>::new();
        schedule_factories.insert(
            "alternate_every_response".to_string(),
            Arc::new(|first, parameters| {
                require_empty_parameters(parameters)?;
                Ok(Box::new(AlternateEveryResponse::new(first)))
            }),
        );
        schedule_factories.insert(
            "alternate_after_action".to_string(),
            Arc::new(|first, parameters| {
                require_empty_parameters(parameters)?;
                Ok(Box::new(AlternateAfterAction::new(first)))
            }),
        );
        Ok(Self {
            game,
            game_factory: Arc::new(game_factory),
            agent_factories: BTreeMap::new(),
            rl_agents: BTreeMap::new(),
            reward_functions: BTreeMap::new(),
            schedule_factories,
            secrets: HashMap::new(),
        })
    }

    /// Registers a trainable agent under the same factory ID used by ordinary agent YAML.
    ///
    /// The learner resolves opaque checkpoints and exposes each checkpoint as a normal
    /// [`AgentFactory`]. Consequently session execution does not know whether an agent is
    /// trainable, local, remote, in memory, or loaded from disk.
    pub fn rl_agent<L>(mut self, learner: L) -> Result<Self>
    where
        L: RLAgent<G>,
    {
        let id = learner.factory_id().to_string();
        if id.trim().is_empty() {
            bail!("RL agent factory ID must not be empty");
        }
        if self.agent_factories.contains_key(&id) || self.rl_agents.contains_key(&id) {
            bail!("agent factory {id:?} is registered more than once");
        }
        self.rl_agents.insert(id, Box::new(learner));
        Ok(self)
    }

    /// Registers one game-specific reward calculation for training experiments.
    pub fn reward<R>(mut self, reward: R) -> Result<Self>
    where
        R: RewardFunction<G>,
    {
        let id = reward.id().to_string();
        if id.trim().is_empty() || reward.version().trim().is_empty() {
            bail!("reward ID and version must not be empty");
        }
        if self
            .reward_functions
            .insert(id.clone(), Arc::new(reward))
            .is_some()
        {
            bail!("reward function {id:?} is registered more than once");
        }
        Ok(self)
    }

    /// Registers one agent factory under its existing stable definition ID.
    pub fn agent<F>(mut self, factory: F) -> Result<Self>
    where
        F: AgentFactory<G>,
    {
        let factory: Arc<dyn AgentFactory<G>> = Arc::new(factory);
        let definition = factory.definition();
        definition.validate()?;
        if self.rl_agents.contains_key(&definition.id) {
            bail!(
                "agent factory {:?} is registered more than once",
                definition.id
            );
        }
        if self
            .agent_factories
            .insert(definition.id.clone(), factory)
            .is_some()
        {
            bail!(
                "agent factory {:?} is registered more than once",
                definition.id
            );
        }
        Ok(self)
    }

    /// Supplies one experiment secret without placing its value in YAML or result artifacts.
    pub fn secret(mut self, key: impl Into<String>, value: impl Into<String>) -> Result<Self> {
        let key = key.into();
        if key.trim().is_empty() || key.starts_with("game.") {
            bail!("secret keys must be non-empty and omit the game. prefix");
        }
        self.secrets.insert(key, value.into());
        Ok(self)
    }

    /// Registers one compiled activation policy under a stable YAML name.
    pub fn schedule<F>(mut self, kind: impl Into<String>, factory: F) -> Result<Self>
    where
        F: Fn(PlayerRole, &Value) -> Result<Box<dyn AgentSchedule<G>>> + Send + Sync + 'static,
    {
        let kind = kind.into();
        if kind.trim().is_empty() {
            bail!("agent schedule kind must not be empty");
        }
        if self
            .schedule_factories
            .insert(kind.clone(), Arc::new(factory))
            .is_some()
        {
            bail!("agent schedule {kind:?} is registered more than once");
        }
        Ok(self)
    }

    /// Reads, validates, plans, and executes one YAML experiment file.
    pub async fn run_yaml(mut self, path: impl AsRef<Path>) -> Result<ExperimentRunSummary> {
        let path = path.as_ref();
        let bytes = tokio::fs::read(path)
            .await
            .with_context(|| format!("failed to read agent experiment {}", path.display()))?;
        let spec: ExperimentFile = serde_yaml::from_slice(&bytes)
            .with_context(|| format!("invalid agent experiment YAML in {}", path.display()))?;
        for (key, reference) in &spec.secrets {
            if !self.secrets.contains_key(key) {
                let value = std::env::var(&reference.environment).with_context(|| {
                    format!(
                        "secret {key:?} requires environment variable {:?}",
                        reference.environment
                    )
                })?;
                self.secrets.insert(key.clone(), value);
            }
        }
        self.run(spec).await
    }
}

/// Summary returned after one experiment invocation finishes or resumes.
#[derive(Clone, Debug, Serialize)]
pub struct ExperimentRunSummary {
    /// Identity shared by the original invocation and its resumptions.
    pub run_id: String,
    /// Number of plans expanded from YAML.
    pub planned: usize,
    /// Newly executed sessions which completed normally or quiescently.
    pub completed: usize,
    /// Newly executed sessions which ended abnormally or at a hard limit.
    pub failed: usize,
    /// Previously finalized plans skipped during this invocation.
    pub skipped: usize,
    /// Number of learner checkpoint boundaries reached by this run.
    pub checkpoints: u64,
    /// Directory containing the run manifest and immutable result files.
    pub output_directory: PathBuf,
}
