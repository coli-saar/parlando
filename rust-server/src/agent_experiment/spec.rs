//! Strict YAML inputs and validation against the compiled game and registered agents.
//!
//! The runner parses the YAML file into these types and validates its game, seats, schedules,
//! training settings, and limits against the registry. Planning consumes the validated
//! specification before any session starts.

use super::{ExperimentRunner, RewardFunction};
use crate::game::{parse_game_config, PlayerRole};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

pub(super) const EXPERIMENT_SCHEMA: &str = "parlando-agent-experiment/v1";

/// Parsed agent–agent experiment specification shared by component setup and execution.
/// Read the file before loading the game to compile it with the requested metering
/// mode, then pass the specification to [`super::ExperimentRunner::run_file`].
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExperimentFile {
    pub(super) schema: String,
    pub(super) game: ExperimentGame,
    #[serde(default)]
    pub(super) secrets: BTreeMap<String, SecretReference>,
    pub(super) agents: BTreeMap<String, ConfiguredAgent>,
    #[serde(default)]
    pub(super) sessions: Option<SessionsSpec>,
    #[serde(default)]
    pub(super) training: Option<TrainingSpec>,
    #[serde(default)]
    pub(super) validation: Option<ValidationSpec>,
    pub(super) schedule: ScheduleSpec,
    #[serde(default)]
    pub(super) limits: SessionLimits,
    #[serde(default)]
    pub(super) execution: ExecutionSpec,
    pub(super) output: OutputSpec,
}

/// Non-secret reference used to obtain one headless experiment secret.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SecretReference {
    pub(super) environment: String,
}

/// Stable game selector in an experiment file.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ExperimentGame {
    pub(super) id: String,
}

/// One reusable named agent definition from YAML.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ConfiguredAgent {
    pub(super) factory: String,
    #[serde(default)]
    pub(super) checkpoint: Option<Value>,
    #[serde(default = "empty_object")]
    pub(super) settings: Value,
}

/// Repeated training data and checkpoint cadence for one named learner.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TrainingSpec {
    pub(super) learner: String,
    pub(super) scenarios: Vec<ScenarioSpec>,
    pub(super) seats: SeatAssignments,
    #[serde(default)]
    pub(super) mirror_roles: bool,
    pub(super) epochs: u64,
    pub(super) checkpoint_every_epochs: u64,
    pub(super) reward: RewardSpec,
}

/// Held-out session data evaluated at a checkpoint cadence.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ValidationSpec {
    pub(super) scenarios: Vec<ScenarioSpec>,
    pub(super) seats: SeatAssignments,
    #[serde(default)]
    pub(super) mirror_roles: bool,
    pub(super) every_checkpoints: u64,
}

/// Registered reward function and its small game-specific parameter object.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RewardSpec {
    pub(super) kind: String,
    #[serde(default = "empty_object")]
    pub(super) parameters: Value,
}

/// Scenario expansion and role assignment input.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SessionsSpec {
    pub(super) scenarios: Vec<ScenarioSpec>,
    pub(super) seats: SeatAssignments,
    #[serde(default)]
    pub(super) mirror_roles: bool,
}

/// One game configuration and its deterministic expansion coordinates.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ScenarioSpec {
    pub(super) name: String,
    pub(super) config: Value,
    pub(super) seeds: Vec<u64>,
    #[serde(default = "one_repetition")]
    pub(super) repetitions: u32,
}

/// Named agents assigned independently to the two game roles.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SeatAssignments {
    pub(super) player_a: String,
    pub(super) player_b: String,
}

/// A compiled activation schedule selected by stable name and first role.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ScheduleSpec {
    pub(super) kind: String,
    pub(super) first: ConfiguredRole,
    #[serde(default = "empty_object")]
    pub(super) parameters: Value,
}

/// YAML spelling of Parlando's two player roles.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ConfiguredRole {
    PlayerA,
    PlayerB,
}

impl ConfiguredRole {
    /// Converts the YAML spelling to the public runtime role.
    pub(super) fn role(self) -> PlayerRole {
        match self {
            Self::PlayerA => PlayerRole::A,
            Self::PlayerB => PlayerRole::B,
        }
    }
}

/// Hard per-session bounds which remain independent of activation policy.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct SessionLimits {
    /// Maximum time in seconds for one agent lifecycle callback.
    pub callback_timeout_seconds: u64,
    /// Maximum time in seconds for each agent shutdown.
    pub shutdown_timeout_seconds: u64,
    /// Wall-clock limit in seconds for decisions and gameplay.
    pub session_timeout_seconds: u64,
    /// Maximum number of decision opportunities across both roles.
    pub decisions: u64,
    /// Maximum number of game actions accepted across both roles.
    pub accepted_actions: u64,
    /// Maximum number of chat deliveries across both roles.
    pub messages: u64,
    /// Maximum number of rejected game actions across both roles.
    pub rejected_actions: u64,
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
    pub fn validate(&self) -> Result<()> {
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
        let now = std::time::Instant::now();
        for timeout in [
            self.callback_timeout(),
            self.shutdown_timeout(),
            self.session_timeout(),
        ] {
            anyhow::ensure!(
                now.checked_add(timeout).is_some(),
                "session timeout exceeds the supported clock range"
            );
        }
        Ok(())
    }

    /// Selects the first count limit reached after one complete response.
    ///
    /// Check game completion before calling this: completion takes priority
    /// over all count limits reached on that same decision.
    pub fn reached_after_response(
        &self,
        accepted_actions: u64,
        messages: u64,
        rejected_actions: u64,
    ) -> Option<super::SessionStatus> {
        use super::SessionStatus;

        if accepted_actions >= self.accepted_actions {
            Some(SessionStatus::ActionLimit)
        } else if messages >= self.messages {
            Some(SessionStatus::MessageLimit)
        } else if rejected_actions >= self.rejected_actions {
            Some(SessionStatus::RejectionLimit)
        } else {
            None
        }
    }

    /// Returns the callback timeout duration.
    pub fn callback_timeout(&self) -> Duration {
        Duration::from_secs(self.callback_timeout_seconds)
    }

    /// Returns the shutdown timeout duration.
    pub fn shutdown_timeout(&self) -> Duration {
        Duration::from_secs(self.shutdown_timeout_seconds)
    }

    /// Returns the wall-clock session deadline duration.
    pub fn session_timeout(&self) -> Duration {
        Duration::from_secs(self.session_timeout_seconds)
    }
}

/// Cross-session executor settings.
#[derive(Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct ExecutionSpec {
    #[serde(skip_serializing_if = "crate::component::WasmMetering::is_fuel")]
    pub(super) wasm_metering: crate::component::WasmMetering,
    pub(super) concurrency: usize,
    pub(super) fail_fast: bool,
}

impl Default for ExecutionSpec {
    /// Runs one session at a time unless YAML explicitly requests concurrency.
    fn default() -> Self {
        Self {
            wasm_metering: Default::default(),
            concurrency: 1,
            fail_fast: false,
        }
    }
}

/// Result location and structured trace level.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct OutputSpec {
    pub(super) directory: String,
    #[serde(default)]
    pub(super) trace: TraceLevel,
}

/// Amount of structured per-decision data retained with each compact result.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum TraceLevel {
    /// Stores no per-decision records.
    #[default]
    Results,
    /// Stores actions, messages, yields, and rejections.
    Decisions,
    /// Adds the acting role's observation and available-action affordance.
    Full,
}

/// Produces an empty JSON object for omitted agent settings.
fn empty_object() -> Value {
    Value::Object(serde_json::Map::new())
}

/// Supplies the default single repetition for one scenario seed.
fn one_repetition() -> u32 {
    1
}

impl ExperimentRunner {
    /// Checks cross-field constraints and prepares the selected training reward.
    pub(super) fn validate_spec(
        &self,
        spec: &ExperimentFile,
    ) -> Result<Option<Arc<dyn RewardFunction>>> {
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
        let reward = if let Some(training) = &spec.training {
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
            Some(reward(&training.reward.parameters).with_context(|| {
                format!("invalid parameters for reward {:?}", training.reward.kind)
            })?)
        } else {
            None
        };
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
        Ok(reward)
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
            parse_game_config(&self.loaded_game, &scenario.config).with_context(|| {
                format!("invalid config for {label} scenario {:?}", scenario.name)
            })?;
        }
        Ok(())
    }
}
