//! Expands scenarios into deterministic, secret-safe session plans.

use super::results::ResultAgent;
use super::schedule::AgentSchedule;
use super::spec::{
    ConfiguredAgent, ExperimentFile, ScenarioSpec, ScheduleSpec, SeatAssignments, SessionLimits,
    TraceLevel,
};
use super::training::{CheckpointId, TrainingCapture};
use super::{ExperimentRunner, ScheduleFactory};
use crate::agents::{configuration_fingerprint, AgentFactory, AgentIdentity};
use crate::canonical_json::canonical_value;
use crate::game::{Game, PlayerRole, SecretValues};
use anyhow::{bail, Context, Result};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

const PLAN_HASH_VERSION: u8 = 1;

/// One resolved headless session, including orchestration identity and runtime handles.
///
/// `plan_id` is computed before this value is constructed. It is result provenance and is
/// never supplied to game mechanics or agent decisions.
pub(super) struct SessionPlan<G: Game> {
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
    pub(super) game_id: String,
    pub(super) game_version: String,
    pub(super) configuration_fingerprint: String,
    pub(super) game_config: G::Config,
    pub(super) game_seed: u64,
    pub(super) game_secrets: SecretValues,
    pub(super) seats: [ResolvedSeat<G>; 2],
    pub(super) schedule: ScheduleSpec,
    pub(super) schedule_factory: ScheduleFactory<G>,
    pub(super) limits: SessionLimits,
    pub(super) trace: TraceLevel,
    pub(super) training_capture: Option<TrainingCapture<G>>,
}

/// Resolved construction data for one session seat.
pub(super) struct ResolvedSeat<G: Game> {
    pub(super) name: String,
    pub(super) factory_id: String,
    pub(super) factory: Arc<dyn AgentFactory<G>>,
    pub(super) settings: Value,
    pub(super) settings_fingerprint: String,
    pub(super) factory_secrets: SecretValues,
    pub(super) agent_instance_secrets: SecretValues,
    pub(super) identity: AgentIdentity,
    pub(super) checkpoint: Option<CheckpointId>,
    pub(super) role: PlayerRole,
    pub(super) seed: u64,
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
    pub(super) fn result_identity(&self) -> ResultAgent {
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

impl<G: Game> ExperimentRunner<G> {
    /// Resolves a configured static or trainable factory and its optional checkpoint identity.
    pub(super) fn configured_factory(
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
    pub(super) fn plan_set(
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
    pub(super) fn resolve_schedule(
        &self,
        spec: &ScheduleSpec,
    ) -> Result<Box<dyn AgentSchedule<G>>> {
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
pub(super) fn hash_serializable(value: &impl Serialize) -> Result<String> {
    hash_value(&serde_json::to_value(value)?)
}

/// Hashes one JSON value after canonical object ordering.
pub(super) fn hash_value(value: &Value) -> Result<String> {
    let bytes = serde_json::to_vec(&canonical_value(value))?;
    Ok(format!("sha256-{:x}", Sha256::digest(bytes)))
}
