//! Serializable status, provenance, and outcomes of a headless session.
//!
//! The session runner builds a result from plan provenance, decision counters, completion or
//! failure, and any captured trace or training steps. Artifact storage serializes it, and
//! execution classifies its status to count fresh and resumed plans consistently.

use super::training::{CheckpointId, TrajectoryStep};
use crate::component::ComponentValue;
use crate::game::{ActionRejection, PlayerRole};
use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Deserialize, Serialize, Serializer};

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
pub struct SessionResult {
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
    pub completion: Option<ComponentValue>,
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
    #[serde(flatten)]
    pub(super) captures: SessionCaptures,
    /// Non-primary errors or timeouts encountered during unconditional shutdown.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub cleanup_diagnostics: Vec<String>,
}

impl SessionResult {
    /// Returns the role-safe learner transitions captured during this session.
    pub fn trajectory(&self) -> &[TrajectoryStep] {
        &self.captures.trajectory
    }
}

/// Keeps typed training and trace entries together so shared inputs serialize from one owner.
#[derive(Debug)]
pub(super) struct SessionCaptures {
    pub(super) trace: Option<Vec<DecisionTrace>>,
    pub(super) trajectory: Vec<TrajectoryStep>,
}

impl Serialize for SessionCaptures {
    /// Publishes only the captures requested for the session, in the stable result shape.
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut fields = serializer.serialize_map(None)?;
        if let Some(trace) = &self.trace {
            fields.serialize_entry(
                "trace",
                &TraceEntries {
                    trace,
                    trajectory: &self.trajectory,
                },
            )?;
        }
        if !self.trajectory.is_empty() {
            fields.serialize_entry("trajectory", &self.trajectory)?;
        }
        fields.end()
    }
}

/// Locates a full trace input in its owning decision or training step.
#[derive(Debug)]
pub(super) enum TraceInput {
    Decision(ComponentValue),
    Trajectory(usize),
}

/// One recorded headless decision with optional full input.
#[derive(Debug)]
pub(super) struct DecisionTrace {
    pub(super) decision: u64,
    pub(super) role: PlayerRole,
    pub(super) response: TraceResponse,
    pub(super) rejection: Option<ActionRejection>,
    pub(super) input: Option<TraceInput>,
}

/// Response variants and JSON tags used in headless result files.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum TraceResponse {
    Yield,
    Action {
        action: ComponentValue,
    },
    Message {
        message: String,
    },
    ActionAndMessage {
        action: ComponentValue,
        message: String,
    },
}

/// Borrows trace rows and resolves any inputs shared with learner trajectory steps.
struct TraceEntries<'a> {
    trace: &'a [DecisionTrace],
    trajectory: &'a [TrajectoryStep],
}

impl Serialize for TraceEntries<'_> {
    /// Writes one result-file row for each recorded decision.
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut rows = serializer.serialize_seq(Some(self.trace.len()))?;
        for row in self.trace {
            let input = match row.input.as_ref() {
                None => None,
                Some(TraceInput::Decision(observation)) => Some(TraceInputRef {
                    observation,
                    available_actions: None,
                }),
                Some(TraceInput::Trajectory(index)) => {
                    let step = self.trajectory.get(*index).ok_or_else(|| {
                        serde::ser::Error::custom("trace input refers to a missing training step")
                    })?;
                    Some(TraceInputRef {
                        observation: &step.observation,
                        available_actions: None,
                    })
                }
            };
            rows.serialize_element(&TraceRow {
                decision: row.decision,
                role: row.role,
                response: &row.response,
                rejection: &row.rejection,
                input,
            })?;
        }
        rows.end()
    }
}

/// Stable artifact record derived from one typed trace row.
#[derive(Serialize)]
struct TraceRow<'a> {
    decision: u64,
    role: PlayerRole,
    response: &'a TraceResponse,
    rejection: &'a Option<ActionRejection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    input: Option<TraceInputRef<'a>>,
}

/// Role-safe observation and optional action catalogue formatted in a full trace.
#[derive(Serialize)]
struct TraceInputRef<'a> {
    observation: &'a ComponentValue,
    available_actions: Option<()>,
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
