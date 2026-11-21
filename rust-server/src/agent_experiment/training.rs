//! Defines reward, trajectory, and learner checkpoint contracts.
//!
//! The decision loop records role-specific inputs and rewards in trajectory steps. Epoch
//! execution batches these steps for the registered learner, which trains from a named base
//! checkpoint and returns the resulting checkpoint ID. Validation uses the corresponding
//! policy factory.

use crate::agents::AgentFactory;
use crate::game::{ActionRejection, Game, PlayerRole, SecretValues};
use anyhow::{bail, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

/// Opaque learner-owned identity of one immutable policy checkpoint.
///
/// Construction and deserialization reject empty and whitespace-only IDs. Every
/// other ID retains its exact string representation, including surrounding spaces.
///
/// Parlando records and compares this value but never interprets it as a path, URI,
/// model version, or in-memory handle. Resolution and persistence belong to [`RLAgent`].
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(try_from = "String")]
pub struct CheckpointId(String);

impl TryFrom<String> for CheckpointId {
    type Error = anyhow::Error;

    /// Applies the same identity contract to stored and newly returned checkpoints.
    fn try_from(value: String) -> Result<Self> {
        Self::new(value)
    }
}

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

    /// Builds the configured reward before the runner plans any sessions.
    ///
    /// Parse and validate the YAML parameters here, retaining settings as typed
    /// fields on the returned reward. Preserve the registered ID and version.
    /// All training sessions in this run share the configured instance.
    fn configure(&self, parameters: &Value) -> Result<Self>
    where
        Self: Sized;

    /// Computes both role rewards after an accepted action.
    fn rewards(
        &self,
        before: &G::State,
        after: &G::State,
        actor: PlayerRole,
        action: &G::Action,
        completion: Option<&G::Completion>,
    ) -> RoleRewards;
}

/// Reads persisted learner steps using the same game types used during live capture.
pub(super) type TrajectoryDecoder<O, A> = fn(&str) -> serde_json::Result<Vec<TrajectoryStep<O, A>>>;

/// One role-safe learner action transition captured by a training session.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TrajectoryStep<O, A> {
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
    pub observation: O,
    /// Optional discrete action affordance supplied to the agent.
    pub available_actions: Option<Vec<A>>,
    /// Action selected by the learner.
    pub action: A,
    /// Whether the game accepted the selected action.
    pub accepted: bool,
    /// Stable rejection information for an invalid action.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rejection: Option<ActionRejection>,
    /// Rewards for both fixed roles; absent for rejected actions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rewards: Option<RoleRewards>,
    /// Role-safe observation following this action or rejection.
    pub next_observation: O,
    /// Whether the accepted action completed the game.
    pub terminal: bool,
}

/// Immutable set of role-safe transitions supplied to one learner update.
#[derive(Clone, Debug)]
pub struct TrainingBatch<O, A> {
    /// Stable idempotency key for the update within one experiment run.
    pub update_id: String,
    /// Completed training epochs represented by this update.
    pub completed_epochs: u64,
    /// Ordered learner transitions collected since the previous checkpoint.
    pub steps: Vec<TrajectoryStep<O, A>>,
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
        batch: TrainingBatch<G::Observation, G::Action>,
    ) -> Result<CheckpointId>;
}

/// Session-local instructions for capturing one named learner's action transitions.
pub(super) struct TrainingCapture<G: Game> {
    pub(super) learner: String,
    pub(super) checkpoint: CheckpointId,
    pub(super) reward: Arc<dyn RewardFunction<G>>,
    pub(super) reward_kind: String,
    pub(super) reward_version: String,
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
        }
    }
}
