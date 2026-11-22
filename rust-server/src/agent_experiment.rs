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
pub use self::spec::ExperimentFile;
use crate::agents::AgentFactory;
use crate::component::LoadedGame;
use crate::game::{GameMetadata, PlayerRole};
use anyhow::{bail, Context, Result};
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Builds a typed reward for the selected YAML parameters during run validation.
type RewardFactory = Box<dyn Fn(&Value) -> Result<Arc<dyn RewardFunction>> + Send + Sync>;

/// Runs versioned YAML agent-agent experiments for one compiled game.
///
/// The registry contains reusable factories. Every planned session receives a fresh
/// game and two fresh agents, and no headless state is inserted into the live database.
pub struct ExperimentRunner {
    game: GameMetadata,
    loaded_game: LoadedGame,
    agent_factories: BTreeMap<String, Arc<dyn AgentFactory>>,
    rl_agents: BTreeMap<String, Box<dyn RLAgent>>,
    reward_functions: BTreeMap<String, RewardFactory>,
    schedule_factories: BTreeMap<String, ScheduleFactory>,
    secrets: HashMap<String, String>,
}

/// Constructor for one fresh activation schedule from validated YAML parameters.
type ScheduleFactory =
    Arc<dyn Fn(PlayerRole, &Value) -> Result<Box<dyn AgentSchedule>> + Send + Sync>;

impl ExperimentRunner {
    /// Creates a runner for one compiled component and pins its digest in the game metadata.
    pub fn new(mut game: GameMetadata, loaded_game: LoadedGame) -> Result<Self> {
        game.validate()?;
        game.pin_component(&loaded_game)?;
        let mut schedule_factories = BTreeMap::<String, ScheduleFactory>::new();
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
            loaded_game,
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
    /// trainable, local, remote, in memory, or loaded from disk. Finalized trajectories are loaded from the run artifacts on resume.
    pub fn rl_agent<L>(mut self, learner: L) -> Result<Self>
    where
        L: RLAgent,
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
        R: RewardFunction,
    {
        let id = reward.id().to_string();
        if id.trim().is_empty() || reward.version().trim().is_empty() {
            bail!("reward ID and version must not be empty");
        }
        if self
            .reward_functions
            .insert(
                id.clone(),
                Box::new(move |parameters| Ok(Arc::new(reward.configure(parameters)?))),
            )
            .is_some()
        {
            bail!("reward function {id:?} is registered more than once");
        }
        Ok(self)
    }

    /// Registers one agent factory under its existing stable definition ID.
    pub fn agent<F>(mut self, factory: F) -> Result<Self>
    where
        F: AgentFactory,
    {
        let factory: Arc<dyn AgentFactory> = Arc::new(factory);
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
        F: Fn(PlayerRole, &Value) -> Result<Box<dyn AgentSchedule>> + Send + Sync + 'static,
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
    pub async fn run_yaml(self, path: impl AsRef<Path>) -> Result<ExperimentRunSummary> {
        self.run_file(ExperimentFile::read(path).await?).await
    }

    /// Executes an already parsed specification. Its metering mode applies to every
    /// session, including training and validation, before any artifacts are opened.
    pub async fn run_file(mut self, spec: ExperimentFile) -> Result<ExperimentRunSummary> {
        let metering = spec.wasm_metering();
        self.loaded_game = self.loaded_game.with_metering(metering)?;
        for configured in spec.agents.values() {
            if let Some(factory) = self.agent_factories.get_mut(&configured.factory) {
                if let Some(replacement) = factory.with_host_wasm_metering(metering)? {
                    *factory = replacement;
                }
            }
        }
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

impl ExperimentFile {
    /// Reads the versioned agent experiment YAML before compiling the component.
    pub async fn read(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let bytes = tokio::fs::read(path)
            .await
            .with_context(|| format!("failed to read agent experiment {}", path.display()))?;
        serde_yaml::from_slice(&bytes)
            .with_context(|| format!("invalid agent experiment YAML in {}", path.display()))
    }

    /// Selects the Wasmtime instrumentation for the component used by this experiment.
    pub fn wasm_metering(&self) -> crate::component::WasmMetering {
        self.execution.wasm_metering
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
