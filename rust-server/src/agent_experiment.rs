use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use futures_util::{stream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::time::timeout;
use uuid::Uuid;

use crate::{
    agents::{
        configuration_fingerprint, Agent, AgentContext, AgentFactory, AgentIdentity, AgentResponse,
    },
    game::{
        ActionRejection, Game, GameFactory, GameInitializationContext, GameMetadata,
        GameSessionContext, PlayerRole, SecretValues,
    },
    SessionLogger,
};

const EXPERIMENT_SCHEMA: &str = "parlando-agent-experiment/v1";
const PLAN_HASH_VERSION: u8 = 1;

/// Opaque learner-owned identity of one immutable policy checkpoint.
///
/// Parlando records and compares this value but never interprets it as a path, URI,
/// model version, or in-memory handle. Resolution and persistence belong to [`RLAgent`].
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(transparent)]
pub struct CheckpointId(String);

impl CheckpointId {
    /// Creates a non-empty opaque checkpoint identity.
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        if value.trim().is_empty() {
            bail!("checkpoint ID must not be empty");
        }
        Ok(Self(value))
    }

    /// Returns the learner-defined checkpoint identity without interpreting it.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Reward assigned to both fixed game roles after one accepted action.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct RoleRewards {
    /// Reward assigned to the agent occupying player A.
    pub player_a: f64,
    /// Reward assigned to the agent occupying player B.
    pub player_b: f64,
}

impl RoleRewards {
    /// Returns the reward associated with one fixed game role.
    pub fn for_role(self, role: PlayerRole) -> f64 {
        match role {
            PlayerRole::A => self.player_a,
            PlayerRole::B => self.player_b,
        }
    }
}

/// Game-specific reward calculation kept outside the base interactive game contract.
///
/// Rewards may inspect authoritative pre- and post-action states, but only their numeric
/// output enters a learner trajectory. Ordinary human and evaluation sessions need not
/// define rewards at all.
pub trait RewardFunction<G: Game>: Send + Sync + 'static {
    /// Returns the stable registry key selected by experiment YAML.
    fn id(&self) -> &'static str;

    /// Returns a semantic version recorded with every trajectory.
    fn version(&self) -> &'static str;

    /// Validates reward-specific YAML parameters before any session starts.
    fn validate_parameters(&self, parameters: &Value) -> Result<()>;

    /// Computes both role rewards after an accepted action.
    fn rewards(
        &self,
        before: &G::State,
        after: &G::State,
        actor: PlayerRole,
        action: &G::Action,
        completion: Option<&G::Completion>,
        parameters: &Value,
    ) -> RoleRewards;
}

/// One role-safe learner action transition captured by a training session.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TrajectoryStep {
    /// Experiment execution owning this step.
    pub run_id: String,
    /// Deterministic session plan which produced this step.
    pub plan_id: String,
    /// Ordinal call to `respond` within the session.
    pub decision: u64,
    /// Named scenario from the experiment specification.
    pub scenario: String,
    /// Fixed game role controlled by the learner for this decision.
    pub role: PlayerRole,
    /// Named YAML agent whose decision is represented.
    pub agent: String,
    /// Learner-owned policy checkpoint used for inference.
    pub checkpoint: CheckpointId,
    /// Stable reward implementation identity.
    pub reward: String,
    /// Semantic reward implementation version.
    pub reward_version: String,
    /// Role-safe game observation before the action.
    pub observation: Value,
    /// Optional discrete action affordance supplied to the agent.
    pub available_actions: Value,
    /// Action selected by the learner.
    pub action: Value,
    /// Whether the game accepted the selected action.
    pub accepted: bool,
    /// Stable rejection information for an invalid action.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rejection: Option<ActionRejection>,
    /// Rewards for both fixed roles; absent for rejected actions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rewards: Option<RoleRewards>,
    /// Role-safe observation following this action or rejection.
    pub next_observation: Value,
    /// Whether the accepted action completed the game.
    pub terminal: bool,
}

/// Immutable set of role-safe transitions supplied to one learner update.
#[derive(Clone, Debug)]
pub struct TrainingBatch {
    /// Stable idempotency key for the update within one experiment run.
    pub update_id: String,
    /// Completed training epochs represented by this update.
    pub completed_epochs: u64,
    /// Ordered learner transitions collected since the previous checkpoint.
    pub steps: Vec<TrajectoryStep>,
}

/// Normalized learner configuration supplied consistently to every training update.
///
/// `settings` is validated by the checkpoint-specific ordinary factory definition.
/// `factory_secrets` contains only secret values explicitly marked for local factory
/// or transport use; agent-instance secrets are deliberately excluded from training.
#[derive(Clone, Debug)]
pub struct RLTrainingContext {
    /// Normalized non-secret settings from the named YAML learner definition.
    pub settings: Value,
    /// Secret values authorized for the local learner transport or factory.
    pub factory_secrets: SecretValues,
}

/// Trainable agent capability layered over ordinary checkpoint-specific factories.
#[async_trait]
pub trait RLAgent<G: Game>: Send + 'static {
    /// Returns the stable factory ID used by every YAML agent definition for this learner.
    fn factory_id(&self) -> &str;

    /// Resolves learner-defined YAML into an opaque immutable checkpoint identity.
    fn resolve_checkpoint(&self, reference: &Value) -> Result<CheckpointId>;

    /// Exposes one checkpoint as the ordinary factory used by session execution.
    fn factory(&self, checkpoint: &CheckpointId) -> Result<Arc<dyn AgentFactory<G>>>;

    /// Updates the learner and returns the new immutable checkpoint identity.
    ///
    /// Implementations must treat `batch.update_id` idempotently because a process can
    /// fail after a remote update succeeds but before Parlando finalizes its checkpoint
    /// record. Checkpoint persistence cadence remains entirely learner-owned.
    async fn train(
        &mut self,
        context: &RLTrainingContext,
        base: &CheckpointId,
        batch: TrainingBatch,
    ) -> Result<CheckpointId>;
}

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

    /// Validates and executes an already parsed experiment specification.
    async fn run(mut self, spec: ExperimentFile) -> Result<ExperimentRunSummary> {
        self.validate_spec(&spec)?;
        let output = PathBuf::from(&spec.output.directory);
        tokio::fs::create_dir_all(output.join("results")).await?;

        let spec_hash = hash_serializable(&spec)?;
        let manifest_path = output.join("run.json");
        let manifest = load_or_create_manifest(
            &manifest_path,
            &self.game.id,
            &self.game.version.to_string(),
            &spec_hash,
        )
        .await?;
        let mut totals = ExecutionBatch::default();
        if let Some(sessions) = &spec.sessions {
            let plans = self.plan_set(
                &spec,
                &manifest.run_id,
                "evaluation",
                0,
                &sessions.scenarios,
                &sessions.seats,
                sessions.mirror_roles,
                &BTreeMap::new(),
                None,
            )?;
            totals.merge(
                self.execute_plans(plans, &spec.execution, &output, &manifest.run_id)
                    .await?,
            );
        }

        let mut checkpoints = 0u64;
        if let Some(training) = &spec.training {
            let configured = spec
                .agents
                .get(&training.learner)
                .expect("validated learner");
            let learner_factory_id = configured.factory.clone();
            let initial_reference = configured
                .checkpoint
                .as_ref()
                .expect("validated checkpoint");
            let mut checkpoint = self
                .rl_agents
                .get(&learner_factory_id)
                .expect("validated RL factory")
                .resolve_checkpoint(initial_reference)?;
            let (configured_factory, _) = self.configured_factory(configured, Some(&checkpoint))?;
            let definition = configured_factory.definition();
            let learner_settings = definition.normalize_settings(&configured.settings)?;
            let (learner_factory_secrets, _) =
                definition.resolve_secrets(&learner_settings, &self.secrets)?;
            let training_context = RLTrainingContext {
                settings: learner_settings,
                factory_secrets: learner_factory_secrets,
            };
            let reward = self
                .reward_functions
                .get(&training.reward.kind)
                .expect("validated reward")
                .clone();
            let mut pending_steps = Vec::new();

            for epoch in 1..=training.epochs {
                let overrides = BTreeMap::from([(training.learner.clone(), checkpoint.clone())]);
                let capture = TrainingCapture {
                    learner: training.learner.clone(),
                    checkpoint: checkpoint.clone(),
                    reward: reward.clone(),
                    reward_kind: training.reward.kind.clone(),
                    reward_version: reward.version().to_string(),
                    parameters: training.reward.parameters.clone(),
                };
                let plans = self.plan_set(
                    &spec,
                    &manifest.run_id,
                    "training",
                    epoch,
                    &training.scenarios,
                    &training.seats,
                    training.mirror_roles,
                    &overrides,
                    Some(capture),
                )?;
                let batch = self
                    .execute_plans(plans, &spec.execution, &output, &manifest.run_id)
                    .await?;
                pending_steps.extend(batch.steps.clone());
                let training_failed = batch.has_failure;
                totals.merge(batch);
                if training_failed {
                    bail!("training stopped because an epoch contains a failed session");
                }

                if !epoch.is_multiple_of(training.checkpoint_every_epochs) {
                    continue;
                }
                checkpoints += 1;
                let record_path = checkpoint_path(&output, checkpoints);
                checkpoint = if let Some(record) =
                    load_checkpoint_record(&record_path, &manifest.run_id, checkpoints).await?
                {
                    record.checkpoint
                } else {
                    let update_id = format!("{}:checkpoint:{checkpoints}", manifest.run_id);
                    let next = self
                        .rl_agents
                        .get_mut(&learner_factory_id)
                        .expect("validated RL factory")
                        .train(
                            &training_context,
                            &checkpoint,
                            TrainingBatch {
                                update_id: update_id.clone(),
                                completed_epochs: epoch,
                                steps: std::mem::take(&mut pending_steps),
                            },
                        )
                        .await?;
                    write_json_atomic(
                        &record_path,
                        &CheckpointRecord {
                            run_id: manifest.run_id.clone(),
                            checkpoint_number: checkpoints,
                            update_id,
                            base: checkpoint.clone(),
                            checkpoint: next.clone(),
                        },
                    )
                    .await?;
                    next
                };
                pending_steps.clear();

                if let Some(validation) = &spec.validation {
                    if checkpoints.is_multiple_of(validation.every_checkpoints) {
                        let overrides =
                            BTreeMap::from([(training.learner.clone(), checkpoint.clone())]);
                        let plans = self.plan_set(
                            &spec,
                            &manifest.run_id,
                            "validation",
                            checkpoints,
                            &validation.scenarios,
                            &validation.seats,
                            validation.mirror_roles,
                            &overrides,
                            None,
                        )?;
                        totals.merge(
                            self.execute_plans(plans, &spec.execution, &output, &manifest.run_id)
                                .await?,
                        );
                    }
                }
            }
        }

        Ok(ExperimentRunSummary {
            run_id: manifest.run_id,
            planned: totals.planned,
            completed: totals.completed,
            failed: totals.failed,
            skipped: totals.skipped,
            checkpoints,
            output_directory: output,
        })
    }

    /// Executes or resumes one set of already resolved session plans.
    async fn execute_plans(
        &self,
        plans: Vec<SessionPlan<G>>,
        execution: &ExecutionSpec,
        output: &Path,
        run_id: &str,
    ) -> Result<ExecutionBatch> {
        let mut batch = ExecutionBatch {
            planned: plans.len(),
            ..ExecutionBatch::default()
        };
        let mut pending = Vec::new();
        for plan in plans {
            let path = result_path(output, &plan.plan_id);
            if finalized_result_matches(&path, run_id, &plan.plan_id).await? {
                batch.skipped += 1;
                let (steps, succeeded) = load_result_data(&path).await?;
                batch.steps.extend(steps);
                batch.has_failure |= !succeeded;
            } else {
                pending.push(plan);
            }
        }
        let concurrency = if execution.fail_fast {
            1
        } else {
            execution.concurrency
        };
        let session_runner = Arc::new(SessionRunner {
            game_factory: self.game_factory.clone(),
        });
        let output = Arc::new(output.to_path_buf());
        let executions = stream::iter(pending.into_iter().map(|plan| {
            let session_runner = session_runner.clone();
            let output = output.clone();
            async move {
                let result = session_runner.run(plan).await;
                write_json_atomic(&result_path(&output, &result.plan_id), &result).await?;
                Ok::<_, anyhow::Error>((result.status.is_success(), result.trajectory))
            }
        }))
        .buffer_unordered(concurrency);
        futures_util::pin_mut!(executions);
        while let Some(result) = executions.next().await {
            let (succeeded, steps) = result?;
            batch.steps.extend(steps);
            if succeeded {
                batch.completed += 1;
            } else {
                batch.failed += 1;
                batch.has_failure = true;
                if execution.fail_fast {
                    bail!("agent experiment stopped after a failed session");
                }
            }
        }
        Ok(batch)
    }

    /// Checks cross-field constraints which serde cannot express.
    fn validate_spec(&self, spec: &ExperimentFile) -> Result<()> {
        if spec.schema != EXPERIMENT_SCHEMA {
            bail!(
                "unsupported agent experiment schema {:?}; expected {EXPERIMENT_SCHEMA:?}",
                spec.schema
            );
        }
        if spec.game.id != self.game.id {
            bail!(
                "experiment selects game {:?}, but this runner was compiled for {:?}",
                spec.game.id,
                self.game.id
            );
        }
        if spec.sessions.is_none() && spec.training.is_none() {
            bail!("an experiment must define sessions, training, or both");
        }
        if spec.validation.is_some() && spec.training.is_none() {
            bail!("validation requires a training section");
        }
        if spec.execution.concurrency == 0 {
            bail!("execution.concurrency must be positive");
        }
        for (key, reference) in &spec.secrets {
            if key.trim().is_empty() || key.starts_with("game.") {
                bail!("secret keys must be non-empty and omit the game. prefix");
            }
            if reference.environment.trim().is_empty() {
                bail!("secret {key:?} must name a non-empty environment variable");
            }
        }
        spec.limits.validate()?;
        for (name, configured) in &spec.agents {
            let (factory, _) = self
                .configured_factory(configured, None)
                .with_context(|| format!("invalid factory or checkpoint for agent {name:?}"))?;
            let definition = factory.definition();
            let settings = definition
                .normalize_settings(&configured.settings)
                .with_context(|| format!("invalid settings for agent {name:?}"))?;
            definition
                .resolve_secrets(&settings, &self.secrets)
                .with_context(|| format!("invalid secrets for agent {name:?}"))?;
            let identity = factory.identity(&settings)?;
            identity.validate()?;
        }
        if let Some(sessions) = &spec.sessions {
            self.validate_session_set(
                "sessions",
                &sessions.scenarios,
                &sessions.seats,
                &spec.agents,
            )?;
        }
        if let Some(training) = &spec.training {
            if training.epochs == 0 || training.checkpoint_every_epochs == 0 {
                bail!("training epochs and checkpoint cadence must be positive");
            }
            if training.epochs % training.checkpoint_every_epochs != 0 {
                bail!("training.epochs must be divisible by checkpoint_every_epochs");
            }
            let configured = spec.agents.get(&training.learner).with_context(|| {
                format!("training selects unknown learner {:?}", training.learner)
            })?;
            if !self.rl_agents.contains_key(&configured.factory) {
                bail!(
                    "training learner {:?} is not registered as an RL agent",
                    training.learner
                );
            }
            if training.seats.player_a != training.learner
                && training.seats.player_b != training.learner
            {
                bail!("training learner must occupy at least one training seat");
            }
            self.validate_session_set(
                "training",
                &training.scenarios,
                &training.seats,
                &spec.agents,
            )?;
            let reward = self
                .reward_functions
                .get(&training.reward.kind)
                .with_context(|| format!("unknown reward function {:?}", training.reward.kind))?;
            reward.validate_parameters(&training.reward.parameters)?;
        }
        if let Some(validation) = &spec.validation {
            if validation.every_checkpoints == 0 {
                bail!("validation.every_checkpoints must be positive");
            }
            self.validate_session_set(
                "validation",
                &validation.scenarios,
                &validation.seats,
                &spec.agents,
            )?;
            let learner = &spec.training.as_ref().expect("validated training").learner;
            if validation.seats.player_a != *learner && validation.seats.player_b != *learner {
                bail!("validation must seat the training learner");
            }
        }
        self.resolve_schedule(&spec.schedule)?;
        Ok(())
    }

    /// Validates named seats and all scenarios in one evaluation, training, or validation set.
    fn validate_session_set(
        &self,
        label: &str,
        scenarios: &[ScenarioSpec],
        seats: &SeatAssignments,
        agents: &BTreeMap<String, ConfiguredAgent>,
    ) -> Result<()> {
        if scenarios.is_empty() {
            bail!("{label}.scenarios must contain at least one scenario");
        }
        for key in [&seats.player_a, &seats.player_b] {
            if !agents.contains_key(key) {
                bail!("{label} seat references unknown agent {key:?}");
            }
        }
        for scenario in scenarios {
            if scenario.name.trim().is_empty()
                || scenario.seeds.is_empty()
                || scenario.repetitions == 0
            {
                bail!("{label} scenario names, seeds, and repetitions must be non-empty");
            }
            let config: G::Config =
                serde_json::from_value(scenario.config.clone()).with_context(|| {
                    format!("invalid config for {label} scenario {:?}", scenario.name)
                })?;
            self.game_factory.validate_config(&config)?;
        }
        Ok(())
    }

    /// Resolves a configured static or trainable factory and its optional checkpoint identity.
    fn configured_factory(
        &self,
        configured: &ConfiguredAgent,
        checkpoint_override: Option<&CheckpointId>,
    ) -> Result<(Arc<dyn AgentFactory<G>>, Option<CheckpointId>)> {
        if let Some(factory) = self.agent_factories.get(&configured.factory) {
            if configured.checkpoint.is_some() || checkpoint_override.is_some() {
                bail!(
                    "static factory {:?} does not accept a checkpoint",
                    configured.factory
                );
            }
            return Ok((factory.clone(), None));
        }
        let learner = self
            .rl_agents
            .get(&configured.factory)
            .with_context(|| format!("unregistered agent factory {:?}", configured.factory))?;
        let checkpoint = match checkpoint_override {
            Some(checkpoint) => checkpoint.clone(),
            None => learner.resolve_checkpoint(configured.checkpoint.as_ref().with_context(
                || format!("RL factory {:?} requires checkpoint", configured.factory),
            )?)?,
        };
        Ok((learner.factory(&checkpoint)?, Some(checkpoint)))
    }

    /// Expands one session set and role assignment into executable plans.
    #[allow(clippy::too_many_arguments)]
    fn plan_set(
        &self,
        spec: &ExperimentFile,
        run_id: &str,
        phase: &str,
        phase_index: u64,
        scenarios: &[ScenarioSpec],
        seats: &SeatAssignments,
        mirror_roles: bool,
        checkpoint_overrides: &BTreeMap<String, CheckpointId>,
        capture: Option<TrainingCapture<G>>,
    ) -> Result<Vec<SessionPlan<G>>> {
        let mut plans = Vec::new();
        let assignments = if mirror_roles && seats.player_a != seats.player_b {
            vec![
                (seats.player_a.as_str(), seats.player_b.as_str(), "original"),
                (seats.player_b.as_str(), seats.player_a.as_str(), "mirrored"),
            ]
        } else {
            vec![(seats.player_a.as_str(), seats.player_b.as_str(), "original")]
        };

        for scenario in scenarios {
            for &game_seed in &scenario.seeds {
                for repetition in 0..scenario.repetitions {
                    for &(player_a, player_b, mirror_leg) in &assignments {
                        let config: G::Config = serde_json::from_value(scenario.config.clone())?;
                        let seat_a = self.resolve_seat(
                            spec,
                            player_a,
                            PlayerRole::A,
                            game_seed,
                            repetition,
                            mirror_leg,
                            checkpoint_overrides.get(player_a),
                        )?;
                        let seat_b = self.resolve_seat(
                            spec,
                            player_b,
                            PlayerRole::B,
                            game_seed,
                            repetition,
                            mirror_leg,
                            checkpoint_overrides.get(player_b),
                        )?;
                        let identity = json!({
                            "version": PLAN_HASH_VERSION,
                            "phase": phase,
                            "phase_index": phase_index,
                            "schema": spec.schema,
                            "game": {"id": spec.game.id, "version": self.game.version},
                            "scenario": scenario.name,
                            "config": scenario.config,
                            "game_seed": game_seed,
                            "repetition": repetition,
                            "mirror_leg": mirror_leg,
                            "player_a": seat_a.identity_document(),
                            "player_b": seat_b.identity_document(),
                            "schedule": spec.schedule,
                            "limits": spec.limits,
                            "trace": spec.output.trace,
                        });
                        let plan_id = hash_value(&identity)?;
                        let schedule_factory = self
                            .schedule_factories
                            .get(&spec.schedule.kind)
                            .with_context(|| {
                                format!("unknown agent schedule {:?}", spec.schedule.kind)
                            })?
                            .clone();
                        plans.push(SessionPlan {
                            run_id: run_id.to_string(),
                            plan_id,
                            phase: phase.to_string(),
                            phase_index,
                            scenario: scenario.name.clone(),
                            game_id: self.game.id.clone(),
                            game_version: self.game.version.to_string(),
                            configuration_fingerprint: hash_value(&scenario.config)?,
                            game_config: config,
                            game_seed,
                            game_secrets: SecretValues::new(
                                self.secrets.clone().into_iter().collect(),
                            ),
                            seats: [seat_a, seat_b],
                            schedule: spec.schedule.clone(),
                            schedule_factory,
                            limits: spec.limits.clone(),
                            trace: spec.output.trace,
                            training_capture: capture.clone(),
                        });
                    }
                }
            }
        }
        let mut ids = BTreeSet::new();
        for plan in &plans {
            if !ids.insert(plan.plan_id.clone()) {
                bail!("experiment expands to duplicate plan ID {}", plan.plan_id);
            }
        }
        Ok(plans)
    }

    /// Constructs one schedule to validate its kind and parameters before planning.
    fn resolve_schedule(&self, spec: &ScheduleSpec) -> Result<Box<dyn AgentSchedule<G>>> {
        self.schedule_factories
            .get(&spec.kind)
            .with_context(|| format!("unknown agent schedule {:?}", spec.kind))?(
            spec.first.role(),
            &spec.parameters,
        )
    }

    /// Resolves one named agent definition for a concrete role and derived seed.
    #[allow(clippy::too_many_arguments)]
    fn resolve_seat(
        &self,
        spec: &ExperimentFile,
        name: &str,
        role: PlayerRole,
        game_seed: u64,
        repetition: u32,
        mirror_leg: &str,
        checkpoint_override: Option<&CheckpointId>,
    ) -> Result<ResolvedSeat<G>> {
        let configured = spec
            .agents
            .get(name)
            .with_context(|| format!("unknown agent {name:?}"))?;
        let (factory, checkpoint) = self.configured_factory(configured, checkpoint_override)?;
        let definition = factory.definition();
        let settings = definition.normalize_settings(&configured.settings)?;
        let (factory_secrets, agent_instance_secrets) =
            definition.resolve_secrets(&settings, &self.secrets)?;
        let identity = factory.identity(&settings)?;
        identity.validate()?;
        let seed = derive_seed(&[
            &spec.schema,
            &self.game.id,
            name,
            role.as_str(),
            &game_seed.to_string(),
            &repetition.to_string(),
            mirror_leg,
        ]);
        Ok(ResolvedSeat {
            name: name.to_string(),
            factory_id: configured.factory.clone(),
            factory,
            settings_fingerprint: configuration_fingerprint(&configured.factory, &settings)?,
            settings,
            factory_secrets,
            agent_instance_secrets,
            identity,
            checkpoint,
            role,
            seed,
        })
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

/// Mutable aggregate for one or more execution phases.
#[derive(Default)]
struct ExecutionBatch {
    planned: usize,
    completed: usize,
    failed: usize,
    skipped: usize,
    steps: Vec<TrajectoryStep>,
    has_failure: bool,
}

impl ExecutionBatch {
    /// Adds another phase's counts and trajectory steps to this aggregate.
    fn merge(&mut self, other: Self) {
        self.planned += other.planned;
        self.completed += other.completed;
        self.failed += other.failed;
        self.skipped += other.skipped;
        self.steps.extend(other.steps);
        self.has_failure |= other.has_failure;
    }
}

/// Durable learner-update boundary used to resume without repeating training.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CheckpointRecord {
    run_id: String,
    checkpoint_number: u64,
    update_id: String,
    base: CheckpointId,
    checkpoint: CheckpointId,
}

/// One resolved headless session, including orchestration identity and runtime handles.
///
/// `plan_id` is computed before this value is constructed. It is result provenance and is
/// never supplied to game mechanics or agent decisions.
struct SessionPlan<G: Game> {
    /// Identity of this complete experiment execution.
    pub run_id: String,
    /// Deterministic identity of this expanded plan, independent of `run_id`.
    pub plan_id: String,
    /// Execution phase: evaluation, training, or validation.
    pub phase: String,
    /// Epoch or checkpoint coordinate within the phase.
    pub phase_index: u64,
    /// Human-readable scenario key from YAML.
    pub scenario: String,
    game_id: String,
    game_version: String,
    configuration_fingerprint: String,
    game_config: G::Config,
    game_seed: u64,
    game_secrets: SecretValues,
    seats: [ResolvedSeat<G>; 2],
    schedule: ScheduleSpec,
    schedule_factory: ScheduleFactory<G>,
    limits: SessionLimits,
    trace: TraceLevel,
    training_capture: Option<TrainingCapture<G>>,
}

/// Session-local instructions for capturing one named learner's action transitions.
struct TrainingCapture<G: Game> {
    learner: String,
    checkpoint: CheckpointId,
    reward: Arc<dyn RewardFunction<G>>,
    reward_kind: String,
    reward_version: String,
    parameters: Value,
}

impl<G: Game> Clone for TrainingCapture<G> {
    /// Clones immutable capture configuration for another expanded session.
    fn clone(&self) -> Self {
        Self {
            learner: self.learner.clone(),
            checkpoint: self.checkpoint.clone(),
            reward: self.reward.clone(),
            reward_kind: self.reward_kind.clone(),
            reward_version: self.reward_version.clone(),
            parameters: self.parameters.clone(),
        }
    }
}

/// Stable terminal classification stored with every headless session result.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    /// The game returned its shared completion value.
    Completed,
    /// The activation schedule found no further eligible agent.
    Quiescent,
    /// The maximum number of calls to `respond` was reached.
    DecisionLimit,
    /// The maximum number of accepted actions was reached.
    ActionLimit,
    /// The maximum number of delivered messages was reached.
    MessageLimit,
    /// The maximum number of rejected actions was reached.
    RejectionLimit,
    /// The wall-clock session deadline elapsed.
    Deadline,
    /// Construction or an agent lifecycle callback failed.
    Failed,
}

impl SessionStatus {
    /// Returns whether the status represents an ordinary experiment outcome.
    fn is_success(self) -> bool {
        matches!(self, Self::Completed | Self::Quiescent)
    }
}

/// Compact immutable output from one planned headless session.
#[derive(Debug, Serialize)]
pub struct SessionResult<C: Serialize> {
    /// Identity of the experiment execution.
    pub run_id: String,
    /// Deterministic identity of the expanded plan.
    pub plan_id: String,
    /// Execution phase: evaluation, training, or validation.
    pub phase: String,
    /// Epoch or checkpoint coordinate within the phase.
    pub phase_index: u64,
    /// Scenario key from the input file.
    pub scenario: String,
    /// Stable compiled game identifier.
    pub game_id: String,
    /// Exact compiled game version.
    pub game_version: String,
    /// Canonical fingerprint of the scenario configuration.
    pub configuration_fingerprint: String,
    /// Recorded deterministic game seed.
    pub game_seed: u64,
    /// Resolved identity and role provenance for both agents.
    pub agents: Vec<ResultAgent>,
    /// Stable compiled activation schedule name.
    pub schedule: String,
    /// Stable terminal status.
    pub status: SessionStatus,
    /// Shared game completion for normal completion.
    pub completion: Option<C>,
    /// Stable lifecycle phase for a failure, when applicable.
    pub failure_phase: Option<String>,
    /// Responsible role for a role-specific failure.
    pub failure_role: Option<PlayerRole>,
    /// Bounded human-readable diagnostic for infrastructure failures.
    pub diagnostic: Option<String>,
    /// Number of calls made to `Agent::respond`.
    pub decisions: u64,
    /// Number of actions accepted by the game.
    pub accepted_actions: u64,
    /// Number of actions rejected by the game.
    pub rejected_actions: u64,
    /// Number of messages delivered to the other agent.
    pub messages: u64,
    /// Total wall-clock execution time in milliseconds.
    pub elapsed_ms: u64,
    /// Optional ordered trace selected by `output.trace`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace: Option<Vec<Value>>,
    /// Role-safe learner transitions, present only for training sessions.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub trajectory: Vec<TrajectoryStep>,
    /// Non-primary errors or timeouts encountered during unconditional shutdown.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub cleanup_diagnostics: Vec<String>,
}

/// Chooses which software agent receives each headless decision opportunity.
pub trait AgentSchedule<G: Game>: Send {
    /// Returns the role selected for the first call to `respond`.
    fn first(&mut self) -> PlayerRole;

    /// Returns the next role, or `None` when the session is quiescent.
    fn next(
        &mut self,
        previous: PlayerRole,
        response: Option<&AgentResponse<G::Action>>,
        rejection: Option<&ActionRejection>,
    ) -> Option<PlayerRole>;
}

/// Alternates after every response and stops after both agents yield consecutively.
pub struct AlternateEveryResponse {
    first: PlayerRole,
    previous_yield: Option<PlayerRole>,
}

impl AlternateEveryResponse {
    /// Creates the schedule with one deterministic first role.
    pub fn new(first: PlayerRole) -> Self {
        Self {
            first,
            previous_yield: None,
        }
    }
}

impl<G: Game> AgentSchedule<G> for AlternateEveryResponse {
    /// Returns the configured first role.
    fn first(&mut self) -> PlayerRole {
        self.first
    }

    /// Alternates after output and recognizes two consecutive yields as quiescence.
    fn next(
        &mut self,
        previous: PlayerRole,
        response: Option<&AgentResponse<G::Action>>,
        _rejection: Option<&ActionRejection>,
    ) -> Option<PlayerRole> {
        if response.is_none() {
            if self.previous_yield == Some(other_role(previous)) {
                return None;
            }
            self.previous_yield = Some(previous);
        } else {
            self.previous_yield = None;
        }
        Some(other_role(previous))
    }
}

/// Keeps control after messages and rejections, and alternates after actions or yields.
pub struct AlternateAfterAction {
    first: PlayerRole,
    previous_yield: Option<PlayerRole>,
}

impl AlternateAfterAction {
    /// Creates the schedule with one deterministic first role.
    pub fn new(first: PlayerRole) -> Self {
        Self {
            first,
            previous_yield: None,
        }
    }
}

impl<G: Game> AgentSchedule<G> for AlternateAfterAction {
    /// Returns the configured first role.
    fn first(&mut self) -> PlayerRole {
        self.first
    }

    /// Applies the action-oriented handoff rule and detects two-agent yielding.
    fn next(
        &mut self,
        previous: PlayerRole,
        response: Option<&AgentResponse<G::Action>>,
        rejection: Option<&ActionRejection>,
    ) -> Option<PlayerRole> {
        match response {
            None => {
                if self.previous_yield == Some(other_role(previous)) {
                    return None;
                }
                self.previous_yield = Some(previous);
                Some(other_role(previous))
            }
            Some(AgentResponse::Message(_)) => {
                self.previous_yield = None;
                Some(previous)
            }
            Some(AgentResponse::Action(_)) | Some(AgentResponse::ActionAndMessage { .. }) => {
                self.previous_yield = None;
                if rejection.is_some() {
                    Some(previous)
                } else {
                    Some(other_role(previous))
                }
            }
        }
    }
}

/// Single-session driver shared by batch evaluation workers.
struct SessionRunner<G: Game> {
    game_factory: Arc<dyn GameFactory<Game = G>>,
}

impl<G: Game> SessionRunner<G> {
    /// Constructs, drives, cleans up, and summarizes one session exactly once.
    async fn run(&self, plan: SessionPlan<G>) -> SessionResult<G::Completion> {
        let started = Instant::now();
        let mut result = SessionResult {
            run_id: plan.run_id.clone(),
            plan_id: plan.plan_id.clone(),
            phase: plan.phase.clone(),
            phase_index: plan.phase_index,
            scenario: plan.scenario.clone(),
            game_id: plan.game_id.clone(),
            game_version: plan.game_version.clone(),
            configuration_fingerprint: plan.configuration_fingerprint.clone(),
            game_seed: plan.game_seed,
            agents: plan
                .seats
                .iter()
                .map(ResolvedSeat::result_identity)
                .collect(),
            schedule: plan.schedule.kind.clone(),
            status: SessionStatus::Failed,
            completion: None,
            failure_phase: None,
            failure_role: None,
            diagnostic: None,
            decisions: 0,
            accepted_actions: 0,
            rejected_actions: 0,
            messages: 0,
            elapsed_ms: 0,
            trace: (plan.trace != TraceLevel::Results).then(Vec::new),
            trajectory: Vec::new(),
            cleanup_diagnostics: Vec::new(),
        };
        let logger = SessionLogger::headless();
        let game = match self.game_factory.create(GameSessionContext {
            logger: logger.clone(),
        }) {
            Ok(game) => game,
            Err(error) => {
                record_failure(&mut result, "game_factory", None, error);
                result.elapsed_ms = elapsed_ms(started);
                return result;
            }
        };

        let mut first = match create_agent(
            &plan.seats[0],
            logger.clone(),
            plan.limits.callback_timeout(),
        )
        .await
        {
            Ok(agent) => agent,
            Err(error) => {
                record_failure(&mut result, "agent_factory", Some(PlayerRole::A), error);
                result.elapsed_ms = elapsed_ms(started);
                return result;
            }
        };
        let second =
            match create_agent(&plan.seats[1], logger, plan.limits.callback_timeout()).await {
                Ok(agent) => agent,
                Err(error) => {
                    record_failure(&mut result, "agent_factory", Some(PlayerRole::B), error);
                    if let Some(error) =
                        shutdown_agent(&mut first, plan.limits.shutdown_timeout()).await
                    {
                        result.cleanup_diagnostics.push(error);
                    }
                    result.elapsed_ms = elapsed_ms(started);
                    return result;
                }
            };
        let mut agents = [first, second];

        let state = match game.initial_state(GameInitializationContext {
            config: &plan.game_config,
            seed: plan.game_seed,
            secrets: &plan.game_secrets,
        }) {
            Ok(state) => state,
            Err(error) => {
                record_failure(&mut result, "initial_state", None, error);
                result.cleanup_diagnostics =
                    shutdown_agents(&mut agents, plan.limits.shutdown_timeout()).await;
                result.elapsed_ms = elapsed_ms(started);
                return result;
            }
        };

        let execution = drive_session(&game, state, &mut agents, &plan, &mut result).await;
        match execution {
            Ok(outcome) => {
                result.status = outcome.status;
                result.completion = outcome.completion;
            }
            Err(failure) => record_failure(&mut result, failure.phase, failure.role, failure.error),
        }
        result.cleanup_diagnostics =
            shutdown_agents(&mut agents, plan.limits.shutdown_timeout()).await;
        result.elapsed_ms = elapsed_ms(started);
        result
    }
}

/// Successful exit from the decision loop.
struct SessionOutcome<C> {
    status: SessionStatus,
    completion: Option<C>,
}

/// Structured internal failure preserving lifecycle attribution.
struct SessionFailure {
    phase: &'static str,
    role: Option<PlayerRole>,
    error: anyhow::Error,
}

/// Runs initialization callbacks and the ordered decision loop.
async fn drive_session<G: Game>(
    game: &G,
    mut state: G::State,
    agents: &mut [Box<dyn Agent<G> + Send>; 2],
    plan: &SessionPlan<G>,
    result: &mut SessionResult<G::Completion>,
) -> std::result::Result<SessionOutcome<G::Completion>, SessionFailure> {
    for role in [PlayerRole::A, PlayerRole::B] {
        let observation = game.observation(&state, role);
        timeout(
            plan.limits.callback_timeout(),
            agents[role_index(role)].start(observation),
        )
        .await
        .map_err(|_| callback_timeout("start", role))?
        .map_err(|error| callback_failure("start", role, error))?;
    }

    if let Some(completion) = game.completion(&state) {
        finish_agents(agents, &completion, plan.limits.callback_timeout()).await?;
        return Ok(SessionOutcome {
            status: SessionStatus::Completed,
            completion: Some(completion),
        });
    }

    let mut schedule =
        (plan.schedule_factory)(plan.schedule.first.role(), &plan.schedule.parameters).map_err(
            |error| SessionFailure {
                phase: "schedule",
                role: None,
                error,
            },
        )?;
    let mut role = schedule.first();
    let deadline = Instant::now() + plan.limits.session_timeout();

    loop {
        if Instant::now() >= deadline {
            return Ok(limit_outcome(SessionStatus::Deadline));
        }
        if result.decisions >= plan.limits.decisions {
            return Ok(limit_outcome(SessionStatus::DecisionLimit));
        }
        result.decisions += 1;
        let affordances = game.available_actions(&state, role);
        let learner_input = if plan
            .training_capture
            .as_ref()
            .is_some_and(|capture| capture.learner == plan.seats[role_index(role)].name)
        {
            Some((
                serde_json::to_value(game.observation(&state, role)).map_err(|error| {
                    SessionFailure {
                        phase: "trajectory",
                        role: Some(role),
                        error: error.into(),
                    }
                })?,
                serde_json::to_value(&affordances).map_err(|error| SessionFailure {
                    phase: "trajectory",
                    role: Some(role),
                    error: error.into(),
                })?,
            ))
        } else {
            None
        };
        let trace_context = if plan.trace == TraceLevel::Full {
            let observation =
                serde_json::to_value(game.observation(&state, role)).map_err(|error| {
                    SessionFailure {
                        phase: "trace",
                        role: Some(role),
                        error: error.into(),
                    }
                })?;
            let available_actions =
                serde_json::to_value(&affordances).map_err(|error| SessionFailure {
                    phase: "trace",
                    role: Some(role),
                    error: error.into(),
                })?;
            Some(json!({
                "observation": observation,
                "available_actions": available_actions,
            }))
        } else {
            None
        };
        let response = timeout(
            plan.limits.callback_timeout(),
            agents[role_index(role)].respond(affordances),
        )
        .await
        .map_err(|_| callback_timeout("respond", role))?
        .map_err(|error| callback_failure("respond", role, error))?;

        if learner_input.is_some() && !matches!(response, Some(AgentResponse::Action(_))) {
            return Err(SessionFailure {
                phase: "training_response",
                role: Some(role),
                error: anyhow!(
                    "the v1 RL trajectory contract requires action-only learner responses"
                ),
            });
        }

        let mut rejection = None;
        let mut completed = None;
        if let Some(response) = response.as_ref() {
            match response {
                AgentResponse::Action(action) => {
                    completed = apply_action(
                        game,
                        &mut state,
                        agents,
                        role,
                        action,
                        None,
                        plan,
                        result,
                        &mut rejection,
                        learner_input.as_ref(),
                    )
                    .await?;
                }
                AgentResponse::Message(message) => {
                    deliver_message(agents, role, message, plan.limits.callback_timeout()).await?;
                    result.messages += 1;
                }
                AgentResponse::ActionAndMessage { action, message } => {
                    completed = apply_action(
                        game,
                        &mut state,
                        agents,
                        role,
                        action,
                        Some(message),
                        plan,
                        result,
                        &mut rejection,
                        learner_input.as_ref(),
                    )
                    .await?;
                }
            }
        }
        record_trace::<G>(
            result,
            role,
            response.as_ref(),
            rejection.as_ref(),
            trace_context,
        );

        if let Some(completion) = completed {
            finish_agents(agents, &completion, plan.limits.callback_timeout()).await?;
            return Ok(SessionOutcome {
                status: SessionStatus::Completed,
                completion: Some(completion),
            });
        }
        if result.accepted_actions >= plan.limits.accepted_actions {
            return Ok(limit_outcome(SessionStatus::ActionLimit));
        }
        if result.messages >= plan.limits.messages {
            return Ok(limit_outcome(SessionStatus::MessageLimit));
        }
        if result.rejected_actions >= plan.limits.rejected_actions {
            return Ok(limit_outcome(SessionStatus::RejectionLimit));
        }
        let Some(next) = schedule.next(role, response.as_ref(), rejection.as_ref()) else {
            return Ok(limit_outcome(SessionStatus::Quiescent));
        };
        role = next;
    }
}

/// Applies one action and delivers its accepted transition and optional message.
#[allow(clippy::too_many_arguments)]
async fn apply_action<G: Game>(
    game: &G,
    state: &mut G::State,
    agents: &mut [Box<dyn Agent<G> + Send>; 2],
    actor: PlayerRole,
    action: &G::Action,
    message: Option<&String>,
    plan: &SessionPlan<G>,
    result: &mut SessionResult<G::Completion>,
    rejection: &mut Option<ActionRejection>,
    learner_input: Option<&(Value, Value)>,
) -> std::result::Result<Option<G::Completion>, SessionFailure> {
    let next = match game.apply_action(state, action, actor) {
        Ok(next) => next,
        Err(error) => {
            result.rejected_actions += 1;
            *rejection = Some(error.clone());
            record_trajectory(
                game,
                state,
                actor,
                action,
                learner_input,
                None,
                Some(error),
                plan,
                result,
            )?;
            return Ok(None);
        }
    };
    result.accepted_actions += 1;
    for role in [PlayerRole::A, PlayerRole::B] {
        let observation = game.observation(&next, role);
        timeout(
            plan.limits.callback_timeout(),
            agents[role_index(role)].observe_transition(actor, action.clone(), observation),
        )
        .await
        .map_err(|_| callback_timeout("observe_transition", role))?
        .map_err(|error| callback_failure("observe_transition", role, error))?;
    }
    let completion = game.completion(&next);
    record_trajectory(
        game,
        &next,
        actor,
        action,
        learner_input,
        Some((state, completion.as_ref())),
        None,
        plan,
        result,
    )?;
    *state = next;
    if completion.is_none() {
        if let Some(message) = message {
            deliver_message(agents, actor, message, plan.limits.callback_timeout()).await?;
            result.messages += 1;
        }
    }
    Ok(completion)
}

/// Records one learner transition using only role-safe observations and numeric rewards.
#[allow(clippy::too_many_arguments)]
fn record_trajectory<G: Game>(
    game: &G,
    next_state: &G::State,
    actor: PlayerRole,
    action: &G::Action,
    learner_input: Option<&(Value, Value)>,
    accepted: Option<(&G::State, Option<&G::Completion>)>,
    rejection: Option<ActionRejection>,
    plan: &SessionPlan<G>,
    result: &mut SessionResult<G::Completion>,
) -> std::result::Result<(), SessionFailure> {
    let (Some((observation, available_actions)), Some(capture)) =
        (learner_input, plan.training_capture.as_ref())
    else {
        return Ok(());
    };
    let completion = accepted.and_then(|(_, completion)| completion);
    let rewards = accepted.map(|(before, _)| {
        capture.reward.rewards(
            before,
            next_state,
            actor,
            action,
            completion,
            &capture.parameters,
        )
    });
    let next_observation =
        serde_json::to_value(game.observation(next_state, actor)).map_err(|error| {
            SessionFailure {
                phase: "trajectory",
                role: Some(actor),
                error: error.into(),
            }
        })?;
    let action = serde_json::to_value(action).map_err(|error| SessionFailure {
        phase: "trajectory",
        role: Some(actor),
        error: error.into(),
    })?;
    result.trajectory.push(TrajectoryStep {
        run_id: plan.run_id.clone(),
        plan_id: plan.plan_id.clone(),
        decision: result.decisions,
        scenario: plan.scenario.clone(),
        role: actor,
        agent: capture.learner.clone(),
        checkpoint: capture.checkpoint.clone(),
        reward: capture.reward_kind.clone(),
        reward_version: capture.reward_version.clone(),
        observation: observation.clone(),
        available_actions: available_actions.clone(),
        action,
        accepted: accepted.is_some(),
        rejection,
        rewards,
        next_observation,
        terminal: completion.is_some(),
    });
    Ok(())
}

/// Delivers one player message only to the opposite role.
async fn deliver_message<G: Game>(
    agents: &mut [Box<dyn Agent<G> + Send>; 2],
    sender: PlayerRole,
    message: &str,
    callback_timeout: Duration,
) -> std::result::Result<(), SessionFailure> {
    let receiver = other_role(sender);
    timeout(
        callback_timeout,
        agents[role_index(receiver)].observe_message(sender, message.to_string()),
    )
    .await
    .map_err(|_| callback_timeout_failure("observe_message", receiver))?
    .map_err(|error| callback_failure("observe_message", receiver, error))
}

/// Delivers normal completion to both agents in stable role order.
async fn finish_agents<G: Game>(
    agents: &mut [Box<dyn Agent<G> + Send>; 2],
    completion: &G::Completion,
    callback_timeout: Duration,
) -> std::result::Result<(), SessionFailure> {
    for role in [PlayerRole::A, PlayerRole::B] {
        timeout(
            callback_timeout,
            agents[role_index(role)].finish(completion.clone()),
        )
        .await
        .map_err(|_| callback_timeout_failure("finish", role))?
        .map_err(|error| callback_failure("finish", role, error))?;
    }
    Ok(())
}

/// Shuts down both successfully constructed agents without replacing the primary result.
async fn shutdown_agents<G: Game>(
    agents: &mut [Box<dyn Agent<G> + Send>; 2],
    shutdown_timeout: Duration,
) -> Vec<String> {
    let mut diagnostics = Vec::new();
    for (index, agent) in agents.iter_mut().enumerate() {
        if let Some(error) = shutdown_agent(agent, shutdown_timeout).await {
            let role = if index == 0 { "A" } else { "B" };
            diagnostics.push(format!("role {role}: {error}"));
        }
    }
    diagnostics
}

/// Applies the bounded best-effort shutdown contract to one agent.
async fn shutdown_agent<G: Game>(
    agent: &mut Box<dyn Agent<G> + Send>,
    duration: Duration,
) -> Option<String> {
    match timeout(duration, agent.shutdown()).await {
        Ok(Ok(())) => None,
        Ok(Err(error)) => Some(format!("shutdown failed: {error:#}")),
        Err(_) => Some("shutdown timed out".to_string()),
    }
}

/// Creates one role-bound agent using its resolved settings and secrets.
async fn create_agent<G: Game>(
    seat: &ResolvedSeat<G>,
    logger: SessionLogger,
    callback_timeout: Duration,
) -> Result<Box<dyn Agent<G> + Send>> {
    timeout(
        callback_timeout,
        seat.factory.create(AgentContext {
            role: seat.role,
            seed: seat.seed,
            settings: seat.settings.clone(),
            factory_secrets: seat.factory_secrets.clone(),
            agent_instance_secrets: seat.agent_instance_secrets.clone(),
            logger,
        }),
    )
    .await
    .context("agent factory timed out")?
}

/// Adds one structured response entry when tracing is enabled.
fn record_trace<G: Game>(
    result: &mut SessionResult<G::Completion>,
    role: PlayerRole,
    response: Option<&AgentResponse<G::Action>>,
    rejection: Option<&ActionRejection>,
    context: Option<Value>,
) {
    let Some(trace) = result.trace.as_mut() else {
        return;
    };
    let response = match response {
        None => json!({"type": "yield"}),
        Some(AgentResponse::Action(action)) => json!({"type": "action", "action": action}),
        Some(AgentResponse::Message(message)) => json!({"type": "message", "message": message}),
        Some(AgentResponse::ActionAndMessage { action, message }) => {
            json!({"type": "action_and_message", "action": action, "message": message})
        }
    };
    let mut entry = json!({
        "decision": result.decisions,
        "role": role,
        "response": response,
        "rejection": rejection,
    });
    if let (Some(entry), Some(context)) = (entry.as_object_mut(), context) {
        entry.insert("input".to_string(), context);
    }
    trace.push(entry);
}

/// Converts one lifecycle error into the compact public result fields.
fn record_failure<C: Serialize>(
    result: &mut SessionResult<C>,
    phase: &str,
    role: Option<PlayerRole>,
    error: impl Into<anyhow::Error>,
) {
    result.status = SessionStatus::Failed;
    result.failure_phase = Some(phase.to_string());
    result.failure_role = role;
    let diagnostic = format!("{:#}", error.into());
    result.diagnostic = Some(diagnostic.chars().take(4096).collect());
}

/// Constructs a successful non-completion loop exit.
fn limit_outcome<C>(status: SessionStatus) -> SessionOutcome<C> {
    SessionOutcome {
        status,
        completion: None,
    }
}

/// Builds a role-attributed callback error.
fn callback_failure(phase: &'static str, role: PlayerRole, error: anyhow::Error) -> SessionFailure {
    SessionFailure {
        phase,
        role: Some(role),
        error,
    }
}

/// Builds a role-attributed callback timeout error.
fn callback_timeout(phase: &'static str, role: PlayerRole) -> SessionFailure {
    callback_timeout_failure(phase, role)
}

/// Constructs the common timeout diagnostic without exposing callback internals.
fn callback_timeout_failure(phase: &'static str, role: PlayerRole) -> SessionFailure {
    SessionFailure {
        phase,
        role: Some(role),
        error: anyhow!("agent callback timed out"),
    }
}

/// Returns elapsed wall time using a saturating millisecond conversion.
fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// Returns the array index assigned to one public player role.
fn role_index(role: PlayerRole) -> usize {
    match role {
        PlayerRole::A => 0,
        PlayerRole::B => 1,
    }
}

/// Returns the other role in Parlando's fixed two-player model.
fn other_role(role: PlayerRole) -> PlayerRole {
    match role {
        PlayerRole::A => PlayerRole::B,
        PlayerRole::B => PlayerRole::A,
    }
}

/// Resolved construction data for one session seat.
struct ResolvedSeat<G: Game> {
    name: String,
    factory_id: String,
    factory: Arc<dyn AgentFactory<G>>,
    settings: Value,
    settings_fingerprint: String,
    factory_secrets: SecretValues,
    agent_instance_secrets: SecretValues,
    identity: AgentIdentity,
    checkpoint: Option<CheckpointId>,
    role: PlayerRole,
    seed: u64,
}

impl<G: Game> ResolvedSeat<G> {
    /// Returns the secret-free canonical fields which affect plan identity.
    fn identity_document(&self) -> Value {
        json!({
            "name": self.name,
            "factory": self.factory_id,
            "settings_fingerprint": self.settings_fingerprint,
            "identity": {"name": self.identity.name, "version": self.identity.version},
            "checkpoint": self.checkpoint,
            "role": self.role,
            "seed": self.seed,
        })
    }

    /// Returns the durable, secret-free provenance stored with a session result.
    fn result_identity(&self) -> ResultAgent {
        ResultAgent {
            key: self.name.clone(),
            factory: self.factory_id.clone(),
            name: self.identity.name.clone(),
            version: self.identity.version.clone(),
            checkpoint: self.checkpoint.clone(),
            settings_fingerprint: self.settings_fingerprint.clone(),
            role: self.role,
            seed: self.seed,
        }
    }
}

/// Secret-free provenance for one resolved agent seat.
#[derive(Clone, Debug, Serialize)]
pub struct ResultAgent {
    /// Named agent key from YAML.
    pub key: String,
    /// Stable registered factory ID.
    pub factory: String,
    /// Semantic implementation or model name.
    pub name: String,
    /// Required semantic implementation version.
    pub version: String,
    /// Opaque checkpoint identity for a trainable agent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<CheckpointId>,
    /// Canonical non-secret settings fingerprint.
    pub settings_fingerprint: String,
    /// Role occupied in this session.
    pub role: PlayerRole,
    /// Deterministically derived agent seed.
    pub seed: u64,
}

/// Strict top-level representation of the versioned YAML input.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ExperimentFile {
    schema: String,
    game: ExperimentGame,
    #[serde(default)]
    secrets: BTreeMap<String, SecretReference>,
    agents: BTreeMap<String, ConfiguredAgent>,
    #[serde(default)]
    sessions: Option<SessionsSpec>,
    #[serde(default)]
    training: Option<TrainingSpec>,
    #[serde(default)]
    validation: Option<ValidationSpec>,
    schedule: ScheduleSpec,
    #[serde(default)]
    limits: SessionLimits,
    #[serde(default)]
    execution: ExecutionSpec,
    output: OutputSpec,
}

/// Non-secret reference used to obtain one headless experiment secret.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SecretReference {
    environment: String,
}

/// Stable game selector in an experiment file.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ExperimentGame {
    id: String,
}

/// One reusable named agent definition from YAML.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ConfiguredAgent {
    factory: String,
    #[serde(default)]
    checkpoint: Option<Value>,
    #[serde(default = "empty_object")]
    settings: Value,
}

/// Repeated training data and checkpoint cadence for one named learner.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct TrainingSpec {
    learner: String,
    scenarios: Vec<ScenarioSpec>,
    seats: SeatAssignments,
    #[serde(default)]
    mirror_roles: bool,
    epochs: u64,
    checkpoint_every_epochs: u64,
    reward: RewardSpec,
}

/// Held-out session data evaluated at a checkpoint cadence.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ValidationSpec {
    scenarios: Vec<ScenarioSpec>,
    seats: SeatAssignments,
    #[serde(default)]
    mirror_roles: bool,
    every_checkpoints: u64,
}

/// Registered reward function and its small game-specific parameter object.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RewardSpec {
    kind: String,
    #[serde(default = "empty_object")]
    parameters: Value,
}

/// Scenario expansion and role assignment input.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SessionsSpec {
    scenarios: Vec<ScenarioSpec>,
    seats: SeatAssignments,
    #[serde(default)]
    mirror_roles: bool,
}

/// One game configuration and its deterministic expansion coordinates.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ScenarioSpec {
    name: String,
    config: Value,
    seeds: Vec<u64>,
    #[serde(default = "one_repetition")]
    repetitions: u32,
}

/// Named agents assigned independently to the two game roles.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SeatAssignments {
    player_a: String,
    player_b: String,
}

/// A compiled activation schedule selected by stable name and first role.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ScheduleSpec {
    kind: String,
    first: ConfiguredRole,
    #[serde(default = "empty_object")]
    parameters: Value,
}

/// YAML spelling of Parlando's two player roles.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum ConfiguredRole {
    PlayerA,
    PlayerB,
}

impl ConfiguredRole {
    /// Converts the YAML spelling to the public runtime role.
    fn role(self) -> PlayerRole {
        match self {
            Self::PlayerA => PlayerRole::A,
            Self::PlayerB => PlayerRole::B,
        }
    }
}

/// Hard per-session bounds which remain independent of activation policy.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
struct SessionLimits {
    callback_timeout_seconds: u64,
    shutdown_timeout_seconds: u64,
    session_timeout_seconds: u64,
    decisions: u64,
    accepted_actions: u64,
    messages: u64,
    rejected_actions: u64,
}

impl Default for SessionLimits {
    /// Supplies conservative finite defaults for every headless session.
    fn default() -> Self {
        Self {
            callback_timeout_seconds: 60,
            shutdown_timeout_seconds: 10,
            session_timeout_seconds: 900,
            decisions: 200,
            accepted_actions: 100,
            messages: 200,
            rejected_actions: 10,
        }
    }
}

impl SessionLimits {
    /// Rejects zero bounds and a callback timeout longer than the session deadline.
    fn validate(&self) -> Result<()> {
        if self.callback_timeout_seconds == 0
            || self.shutdown_timeout_seconds == 0
            || self.session_timeout_seconds == 0
            || self.decisions == 0
            || self.accepted_actions == 0
            || self.messages == 0
            || self.rejected_actions == 0
        {
            bail!("all session limits must be positive");
        }
        if self.callback_timeout_seconds > self.session_timeout_seconds {
            bail!("callback timeout must not exceed the session timeout");
        }
        Ok(())
    }

    /// Returns the callback timeout duration.
    fn callback_timeout(&self) -> Duration {
        Duration::from_secs(self.callback_timeout_seconds)
    }

    /// Returns the shutdown timeout duration.
    fn shutdown_timeout(&self) -> Duration {
        Duration::from_secs(self.shutdown_timeout_seconds)
    }

    /// Returns the wall-clock session deadline duration.
    fn session_timeout(&self) -> Duration {
        Duration::from_secs(self.session_timeout_seconds)
    }
}

/// Cross-session executor settings.
#[derive(Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
struct ExecutionSpec {
    concurrency: usize,
    fail_fast: bool,
}

impl Default for ExecutionSpec {
    /// Runs one session at a time unless YAML explicitly requests concurrency.
    fn default() -> Self {
        Self {
            concurrency: 1,
            fail_fast: false,
        }
    }
}

/// Result location and structured trace level.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct OutputSpec {
    directory: String,
    #[serde(default)]
    trace: TraceLevel,
}

/// Amount of structured per-decision data retained with each compact result.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum TraceLevel {
    /// Stores no per-decision records.
    #[default]
    Results,
    /// Stores actions, messages, yields, and rejections.
    Decisions,
    /// Adds the acting role's observation and available-action affordance.
    Full,
}

/// Durable identity of one invocation and the normalized input it belongs to.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RunManifest {
    schema: String,
    run_id: String,
    game_id: String,
    game_version: String,
    spec_hash: String,
}

/// Produces an empty JSON object for omitted agent settings.
fn empty_object() -> Value {
    Value::Object(serde_json::Map::new())
}

/// Rejects parameters for built-in schedules which deliberately have none.
fn require_empty_parameters(parameters: &Value) -> Result<()> {
    if parameters
        .as_object()
        .is_some_and(serde_json::Map::is_empty)
    {
        Ok(())
    } else {
        bail!("this agent schedule accepts no parameters")
    }
}

/// Supplies the default single repetition for one scenario seed.
fn one_repetition() -> u32 {
    1
}

/// Derives one stable 64-bit seed from labelled identity components.
fn derive_seed(parts: &[&str]) -> u64 {
    let mut hasher = Sha256::new();
    hasher.update(b"parlando-agent-seed-v1\0");
    for part in parts {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part.as_bytes());
    }
    let digest = hasher.finalize();
    u64::from_be_bytes(
        digest[..8]
            .try_into()
            .expect("SHA-256 has at least eight bytes"),
    )
}

/// Hashes any serializable value after recursively canonicalizing object keys.
fn hash_serializable(value: &impl Serialize) -> Result<String> {
    hash_value(&serde_json::to_value(value)?)
}

/// Hashes one JSON value after canonical object ordering.
fn hash_value(value: &Value) -> Result<String> {
    let bytes = serde_json::to_vec(&canonical_value(value))?;
    Ok(format!("sha256-{:x}", Sha256::digest(bytes)))
}

/// Recursively sorts JSON object keys while preserving array order.
fn canonical_value(value: &Value) -> Value {
    match value {
        Value::Object(object) => {
            let mut entries = object.iter().collect::<Vec<_>>();
            entries.sort_by_key(|(key, _)| *key);
            Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key.clone(), canonical_value(value)))
                    .collect(),
            )
        }
        Value::Array(values) => Value::Array(values.iter().map(canonical_value).collect()),
        _ => value.clone(),
    }
}

/// Loads the existing run manifest or atomically creates a new run identity.
async fn load_or_create_manifest(
    path: &Path,
    game_id: &str,
    game_version: &str,
    spec_hash: &str,
) -> Result<RunManifest> {
    match tokio::fs::read(path).await {
        Ok(bytes) => {
            let manifest: RunManifest = serde_json::from_slice(&bytes)
                .with_context(|| format!("invalid run manifest {}", path.display()))?;
            if manifest.schema != EXPERIMENT_SCHEMA
                || manifest.game_id != game_id
                || manifest.game_version != game_version
                || manifest.spec_hash != spec_hash
            {
                bail!(
                    "output directory {} belongs to a different experiment specification",
                    path.parent().unwrap_or_else(|| Path::new(".")).display()
                );
            }
            Ok(manifest)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let manifest = RunManifest {
                schema: EXPERIMENT_SCHEMA.to_string(),
                run_id: Uuid::new_v4().to_string(),
                game_id: game_id.to_string(),
                game_version: game_version.to_string(),
                spec_hash: spec_hash.to_string(),
            };
            write_json_atomic(path, &manifest).await?;
            Ok(manifest)
        }
        Err(error) => Err(error).with_context(|| format!("failed to read {}", path.display())),
    }
}

/// Returns the immutable result path for one plan ID.
fn result_path(output: &Path, plan_id: &str) -> PathBuf {
    output.join("results").join(format!("{plan_id}.json"))
}

/// Returns the immutable record path for one numbered learner update.
fn checkpoint_path(output: &Path, checkpoint_number: u64) -> PathBuf {
    output
        .join("checkpoints")
        .join(format!("{checkpoint_number:08}.json"))
}

/// Loads and validates one finalized checkpoint record when it exists.
async fn load_checkpoint_record(
    path: &Path,
    run_id: &str,
    checkpoint_number: u64,
) -> Result<Option<CheckpointRecord>> {
    let bytes = match tokio::fs::read(path).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let record: CheckpointRecord = serde_json::from_slice(&bytes)
        .with_context(|| format!("invalid checkpoint record {}", path.display()))?;
    if record.run_id != run_id || record.checkpoint_number != checkpoint_number {
        bail!(
            "checkpoint record {} has inconsistent identity",
            path.display()
        );
    }
    Ok(Some(record))
}

/// Reads learner transitions from a finalized result while resuming training.
async fn load_result_data(path: &Path) -> Result<(Vec<TrajectoryStep>, bool)> {
    let value: Value = serde_json::from_slice(&tokio::fs::read(path).await?)?;
    let steps = match value.get("trajectory") {
        Some(steps) => serde_json::from_value(steps.clone())
            .with_context(|| format!("invalid trajectory in {}", path.display())),
        None => Ok(Vec::new()),
    }?;
    let succeeded = matches!(
        value.get("status").and_then(Value::as_str),
        Some("completed" | "quiescent")
    );
    Ok((steps, succeeded))
}

/// Verifies whether one existing result belongs to the requested run and plan.
async fn finalized_result_matches(path: &Path, run_id: &str, plan_id: &str) -> Result<bool> {
    let bytes = match tokio::fs::read(path).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", path.display()))
        }
    };
    let value: Value = serde_json::from_slice(&bytes)
        .with_context(|| format!("invalid finalized result {}", path.display()))?;
    if value.get("run_id").and_then(Value::as_str) != Some(run_id)
        || value.get("plan_id").and_then(Value::as_str) != Some(plan_id)
    {
        bail!(
            "result {} has inconsistent run or plan identity",
            path.display()
        );
    }
    Ok(true)
}

/// Writes one JSON file through an adjacent temporary path and atomic rename.
async fn write_json_atomic(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path
        .parent()
        .with_context(|| format!("path {} has no parent", path.display()))?;
    tokio::fs::create_dir_all(parent).await?;
    let temporary = parent.join(format!(".{}.tmp", Uuid::new_v4()));
    let bytes = serde_json::to_vec_pretty(value)?;
    tokio::fs::write(&temporary, bytes)
        .await
        .with_context(|| format!("failed to write {}", temporary.display()))?;
    tokio::fs::rename(&temporary, path)
        .await
        .with_context(|| format!("failed to finalize {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use serde::Deserialize;
    use tempfile::TempDir;

    use super::*;
    use crate::game::AgentDefinition;

    /// Minimal deterministic game used to exercise the complete headless lifecycle.
    struct TinyGame;

    /// Reusable constructor for a fresh tiny game per planned session.
    struct TinyGameFactory;

    /// Strict game configuration used by the YAML fixture.
    #[derive(Deserialize, Serialize)]
    #[serde(deny_unknown_fields)]
    struct TinyConfig {
        target: u8,
    }

    /// Authoritative state which completes when its counter reaches the target.
    #[derive(Serialize)]
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
    #[derive(Serialize)]
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

        /// Applies positive increments and rejects zero increments.
        fn apply_action(
            &self,
            state: &Self::State,
            action: &Self::Action,
            _actor: PlayerRole,
        ) -> std::result::Result<Self::State, ActionRejection> {
            if action.amount == 0 {
                return Err(ActionRejection::new("zero_increment"));
            }
            Ok(TinyState {
                count: state.count.saturating_add(action.amount),
                target: state.target,
            })
        }

        /// Reveals the public count and the viewer's own role.
        fn observation(&self, state: &Self::State, role: PlayerRole) -> Self::Observation {
            TinyObservation {
                count: state.count,
                role,
            }
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
            Ok(TinyGame)
        }
    }

    /// Factory which creates role-aware scripted agents and records lifecycle calls.
    struct TinyAgentFactory {
        events: Arc<Mutex<Vec<String>>>,
        fail_role: Option<PlayerRole>,
    }

    /// Agent which completes the game from role A and yields from role B.
    struct TinyAgent {
        role: PlayerRole,
        acted: bool,
        events: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl Agent<TinyGame> for TinyAgent {
        /// Records initial observation delivery.
        async fn start(&mut self, _initial_observation: TinyObservation) -> Result<()> {
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
            _observation: TinyObservation,
        ) -> Result<()> {
            self.events
                .lock()
                .unwrap()
                .push(format!("observe:{}", self.role.as_str()));
            Ok(())
        }

        /// Produces one completing action from role A and otherwise yields.
        async fn respond(
            &mut self,
            _available_actions: Option<Vec<TinyAction>>,
        ) -> Result<Option<AgentResponse<TinyAction>>> {
            if self.role == PlayerRole::A && !self.acted {
                self.acted = true;
                Ok(Some(AgentResponse::action(TinyAction { amount: 1 })))
            } else {
                Ok(None)
            }
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
                acted: false,
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

    /// Checkpoint-specific ordinary factory exposed by the mock learner.
    struct TinyCheckpointFactory {
        checkpoint: CheckpointId,
        events: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl AgentFactory<TinyGame> for TinyCheckpointFactory {
        /// Exposes the same uniform factory ID used by learner YAML.
        fn definition(&self) -> AgentDefinition {
            AgentDefinition {
                id: "tiny.rl".to_string(),
                name: "Tiny RL agent".to_string(),
                description: "Checkpoint-specific tiny learner policy.".to_string(),
                config_fields: vec![crate::game::AgentConfigField {
                    key: "mode".to_string(),
                    label: "Mode".to_string(),
                    help: "Normalized learner setting used by the runner contract test."
                        .to_string(),
                    value: crate::game::AgentConfigValue::String {
                        format: crate::game::StringFormat::Plain,
                    },
                    required: true,
                    default_value: json!("default"),
                }],
            }
        }

        /// Creates the same deterministic action policy for each mock checkpoint.
        async fn create(&self, context: AgentContext) -> Result<Box<dyn Agent<TinyGame> + Send>> {
            self.events.lock().unwrap().push(format!(
                "create-rl:{}:{}",
                self.checkpoint.as_str(),
                context.role.as_str()
            ));
            Ok(Box::new(TinyAgent {
                role: context.role,
                acted: false,
                events: self.events.clone(),
            }))
        }

        /// Includes the checkpoint in stable policy provenance.
        fn identity(&self, _settings: &Value) -> Result<AgentIdentity> {
            Ok(AgentIdentity {
                name: "TinyRLAgent".to_string(),
                version: self.checkpoint.as_str().to_string(),
            })
        }
    }

    /// Mock learner which records idempotent updates and creates numbered checkpoints.
    struct TinyLearner {
        events: Arc<Mutex<Vec<String>>>,
        updates: Arc<Mutex<Vec<(String, String, usize)>>>,
    }

    #[async_trait]
    impl RLAgent<TinyGame> for TinyLearner {
        /// Uses one stable registry ID with no YAML learner discriminator.
        fn factory_id(&self) -> &str {
            "tiny.rl"
        }

        /// Treats the YAML string as the learner-owned initial checkpoint reference.
        fn resolve_checkpoint(&self, reference: &Value) -> Result<CheckpointId> {
            CheckpointId::new(
                reference
                    .as_str()
                    .context("tiny checkpoint reference must be a string")?,
            )
        }

        /// Produces an ordinary factory bound to one resolved checkpoint.
        fn factory(&self, checkpoint: &CheckpointId) -> Result<Arc<dyn AgentFactory<TinyGame>>> {
            Ok(Arc::new(TinyCheckpointFactory {
                checkpoint: checkpoint.clone(),
                events: self.events.clone(),
            }))
        }

        /// Records the update and returns its deterministic numbered checkpoint.
        async fn train(
            &mut self,
            context: &RLTrainingContext,
            base: &CheckpointId,
            batch: TrainingBatch,
        ) -> Result<CheckpointId> {
            anyhow::ensure!(
                context.settings["mode"] == "configured",
                "runner did not deliver normalized learner settings"
            );
            let mut updates = self.updates.lock().unwrap();
            updates.push((
                batch.update_id,
                base.as_str().to_string(),
                batch.steps.len(),
            ));
            CheckpointId::new(format!("checkpoint-{}", updates.len()))
        }
    }

    /// Dense test reward equal to the accepted counter increment for both roles.
    struct TinyReward;

    impl RewardFunction<TinyGame> for TinyReward {
        /// Returns the YAML registry key.
        fn id(&self) -> &'static str {
            "count_delta"
        }

        /// Returns stable test reward provenance.
        fn version(&self) -> &'static str {
            "1"
        }

        /// Accepts no reward parameters.
        fn validate_parameters(&self, parameters: &Value) -> Result<()> {
            require_empty_parameters(parameters)
        }

        /// Rewards the public counter change symmetrically.
        fn rewards(
            &self,
            before: &TinyState,
            after: &TinyState,
            _actor: PlayerRole,
            _action: &TinyAction,
            _completion: Option<&TinyCompletion>,
            _parameters: &Value,
        ) -> RoleRewards {
            let reward = f64::from(after.count.saturating_sub(before.count));
            RoleRewards {
                player_a: reward,
                player_b: reward,
            }
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

    /// Renders a two-epoch learner run with validation after every checkpoint.
    fn training_yaml(output: &Path) -> String {
        format!(
            r#"schema: parlando-agent-experiment/v1
game:
  id: tiny
agents:
  learner:
    factory: tiny.rl
    checkpoint: initial
    settings:
      mode: configured
  baseline:
    factory: tiny.agent
training:
  learner: learner
  scenarios:
    - name: train-one-step
      config: {{ target: 1 }}
      seeds: [7]
  seats:
    player_a: learner
    player_b: baseline
  epochs: 2
  checkpoint_every_epochs: 1
  reward:
    kind: count_delta
validation:
  scenarios:
    - name: held-out
      config: {{ target: 1 }}
      seeds: [11]
  seats:
    player_a: learner
    player_b: baseline
  every_checkpoints: 1
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
output:
  directory: '{}'
"#,
            output.display()
        )
    }

    /// Builds the test runner with a fresh lifecycle event sink.
    fn test_runner(events: Arc<Mutex<Vec<String>>>) -> ExperimentRunner<TinyGame> {
        ExperimentRunner::new(
            GameMetadata {
                id: "tiny".to_string(),
                name: "Tiny".to_string(),
                version: semver::Version::new(1, 0, 0),
                build_manifest: json!({"test": true}),
            },
            TinyGameFactory,
        )
        .unwrap()
        .agent(TinyAgentFactory {
            events,
            fail_role: None,
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
            TinyGameFactory,
        )
        .unwrap()
        .agent(TinyAgentFactory {
            events,
            fail_role: Some(PlayerRole::B),
        })
        .unwrap()
    }

    /// Builds a runner containing one ordinary opponent, learner, and reward function.
    fn training_runner(
        events: Arc<Mutex<Vec<String>>>,
        updates: Arc<Mutex<Vec<(String, String, usize)>>>,
    ) -> ExperimentRunner<TinyGame> {
        test_runner(events.clone())
            .rl_agent(TinyLearner { events, updates })
            .unwrap()
            .reward(TinyReward)
            .unwrap()
    }

    /// Exercises training, opaque checkpoints, validation cadence, trajectories, and resume.
    #[tokio::test]
    async fn training_updates_and_validates_without_repeating_updates_on_resume() {
        let temporary = TempDir::new().unwrap();
        let output = temporary.path().join("training-output");
        let yaml_path = temporary.path().join("training.yaml");
        tokio::fs::write(&yaml_path, training_yaml(&output))
            .await
            .unwrap();
        let updates = Arc::new(Mutex::new(Vec::new()));

        let first = training_runner(Arc::new(Mutex::new(Vec::new())), updates.clone())
            .run_yaml(&yaml_path)
            .await
            .unwrap();
        assert_eq!(first.planned, 4);
        assert_eq!(first.completed, 4);
        assert_eq!(first.checkpoints, 2);
        assert_eq!(
            updates
                .lock()
                .unwrap()
                .iter()
                .map(|(_, base, steps)| (base.as_str(), *steps))
                .collect::<Vec<_>>(),
            [("initial", 1), ("checkpoint-1", 1)]
        );

        let training_result = std::fs::read_dir(output.join("results"))
            .unwrap()
            .filter_map(std::result::Result::ok)
            .map(|entry| std::fs::read(entry.path()).unwrap())
            .map(|bytes| serde_json::from_slice::<Value>(&bytes).unwrap())
            .find(|value| value["phase"] == "training")
            .unwrap();
        assert_eq!(training_result["trajectory"][0]["rewards"]["player_a"], 1.0);
        assert_eq!(training_result["trajectory"][0]["accepted"], true);
        assert_eq!(training_result["trajectory"][0]["observation"]["role"], "A");

        let resumed_updates = Arc::new(Mutex::new(Vec::new()));
        let second = training_runner(Arc::new(Mutex::new(Vec::new())), resumed_updates.clone())
            .run_yaml(&yaml_path)
            .await
            .unwrap();
        assert_eq!(second.run_id, first.run_id);
        assert_eq!(second.skipped, 4);
        assert!(resumed_updates.lock().unwrap().is_empty());
    }

    /// Verifies execution, immutable result identity, lifecycle cleanup, and resume.
    #[tokio::test]
    async fn yaml_run_executes_once_and_resume_skips_finalized_result() {
        let temporary = TempDir::new().unwrap();
        let output = temporary.path().join("output");
        let yaml_path = temporary.path().join("experiment.yaml");
        tokio::fs::write(&yaml_path, experiment_yaml(&output))
            .await
            .unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));

        let first = test_runner(events.clone())
            .run_yaml(&yaml_path)
            .await
            .unwrap();
        assert_eq!(first.planned, 1);
        assert_eq!(first.completed, 1);
        assert_eq!(first.failed, 0);
        assert_eq!(first.skipped, 0);

        let result_path = std::fs::read_dir(output.join("results"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let result: Value =
            serde_json::from_slice(&tokio::fs::read(result_path).await.unwrap()).unwrap();
        assert_eq!(result["run_id"], first.run_id);
        assert!(result["plan_id"].as_str().unwrap().starts_with("sha256-"));
        assert!(result.get("session_id").is_none());
        assert_eq!(result["status"], "completed");

        let second = test_runner(events.clone())
            .run_yaml(&yaml_path)
            .await
            .unwrap();
        assert_eq!(second.run_id, first.run_id);
        assert_eq!(second.skipped, 1);
        assert_eq!(second.completed, 0);
        assert_eq!(
            events.lock().unwrap().as_slice(),
            [
                "create:A",
                "create:B",
                "start:A",
                "start:B",
                "observe:A",
                "observe:B",
                "finish:A",
                "finish:B",
                "shutdown:A",
                "shutdown:B",
            ]
        );
    }

    /// Confirms a new run ID does not change an equivalent plan ID.
    #[tokio::test]
    async fn plan_identity_is_stable_across_distinct_runs() {
        let temporary = TempDir::new().unwrap();
        let first_output = temporary.path().join("first");
        let second_output = temporary.path().join("second");
        let first_yaml = temporary.path().join("first.yaml");
        let second_yaml = temporary.path().join("second.yaml");
        tokio::fs::write(&first_yaml, experiment_yaml(&first_output))
            .await
            .unwrap();
        tokio::fs::write(&second_yaml, experiment_yaml(&second_output))
            .await
            .unwrap();

        let first = test_runner(Arc::new(Mutex::new(Vec::new())))
            .run_yaml(first_yaml)
            .await
            .unwrap();
        let second = test_runner(Arc::new(Mutex::new(Vec::new())))
            .run_yaml(second_yaml)
            .await
            .unwrap();
        assert_ne!(first.run_id, second.run_id);

        let first_name = std::fs::read_dir(first_output.join("results"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .file_name();
        let second_name = std::fs::read_dir(second_output.join("results"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .file_name();
        assert_eq!(first_name, second_name);
    }

    /// Verifies constructor failure is finalized once and cleans up the first agent.
    #[tokio::test]
    async fn failed_session_is_finalized_and_not_retried_on_resume() {
        let temporary = TempDir::new().unwrap();
        let output = temporary.path().join("output");
        let yaml_path = temporary.path().join("experiment.yaml");
        tokio::fs::write(&yaml_path, experiment_yaml(&output))
            .await
            .unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));

        let first = failing_test_runner(events.clone())
            .run_yaml(&yaml_path)
            .await
            .unwrap();
        assert_eq!(first.failed, 1);
        assert_eq!(first.skipped, 0);
        assert_eq!(
            events.lock().unwrap().as_slice(),
            ["create:A", "create:B", "shutdown:A"]
        );

        let second = failing_test_runner(events.clone())
            .run_yaml(&yaml_path)
            .await
            .unwrap();
        assert_eq!(second.run_id, first.run_id);
        assert_eq!(second.failed, 0);
        assert_eq!(second.skipped, 1);
        assert_eq!(
            events.lock().unwrap().as_slice(),
            ["create:A", "create:B", "shutdown:A"]
        );
    }

    /// Confirms YAML does not expose the internal ordinary-versus-RL distinction.
    #[test]
    fn yaml_rejects_obsolete_agent_kind_discriminator() {
        let yaml = experiment_yaml(Path::new("results")).replace(
            "    factory: tiny.agent",
            "    kind: factory\n    factory: tiny.agent",
        );
        let error = serde_yaml::from_str::<ExperimentFile>(&yaml).unwrap_err();
        assert!(error.to_string().contains("unknown field `kind`"));
    }

    /// Exercises the built-in policy's distinct message, rejection, action, and yield handoffs.
    #[test]
    fn alternate_after_action_uses_the_original_agent_response() {
        let mut schedule = AlternateAfterAction::new(PlayerRole::A);
        assert_eq!(
            <AlternateAfterAction as AgentSchedule<TinyGame>>::next(
                &mut schedule,
                PlayerRole::A,
                Some(&AgentResponse::message("hello")),
                None,
            ),
            Some(PlayerRole::A)
        );
        let action = AgentResponse::action(TinyAction { amount: 1 });
        assert_eq!(
            <AlternateAfterAction as AgentSchedule<TinyGame>>::next(
                &mut schedule,
                PlayerRole::A,
                Some(&action),
                Some(&ActionRejection::new("blocked")),
            ),
            Some(PlayerRole::A)
        );
        assert_eq!(
            <AlternateAfterAction as AgentSchedule<TinyGame>>::next(
                &mut schedule,
                PlayerRole::A,
                Some(&action),
                None,
            ),
            Some(PlayerRole::B)
        );
        assert_eq!(
            <AlternateAfterAction as AgentSchedule<TinyGame>>::next(
                &mut schedule,
                PlayerRole::A,
                None,
                None,
            ),
            Some(PlayerRole::B)
        );
        assert_eq!(
            <AlternateAfterAction as AgentSchedule<TinyGame>>::next(
                &mut schedule,
                PlayerRole::B,
                None,
                None,
            ),
            None
        );
    }
}
