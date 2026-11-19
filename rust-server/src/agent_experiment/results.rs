//! Serializable status, provenance, and outcomes of a headless session.

use super::training::{CheckpointId, TrajectoryStep};
use crate::game::PlayerRole;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Stable terminal classification stored with every headless session result.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
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
    pub(super) fn is_success(self) -> bool {
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
