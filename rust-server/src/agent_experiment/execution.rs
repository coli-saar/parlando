//! Executes evaluation and training phases and resumes finalized plans.
//!
//! The runner supplies a validated specification. Execution resolves its plans, recovers
//! matching finalized results, and sends pending plans to the session runner. A fresh result
//! contributes to counts after publication. Fail-fast execution finalizes the first failure
//! before stopping subsequent plans.

use super::artifacts::RunArtifacts;
use super::planning::{hash_serializable, SessionPlan};
use super::session::SessionRunner;
use super::spec::{ExecutionSpec, ExperimentFile};
use super::training::TrajectoryStep;
use super::{ExperimentRunSummary, ExperimentRunner};
use anyhow::{bail, Result};
use futures_util::{stream, StreamExt};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

impl ExperimentRunner {
    /// Validates and executes an already parsed experiment specification.
    pub(super) async fn run(mut self, spec: ExperimentFile) -> Result<ExperimentRunSummary> {
        let reward = self.validate_spec(&spec)?;
        let output = PathBuf::from(&spec.output.directory);

        // A changed policy binary invalidates cached results even when its settings
        // and semantic version are identical. Native and remote runs retain their identity.
        let policy_artifacts: BTreeMap<_, _> = spec
            .agents
            .values()
            .filter_map(|config| {
                let digest = self.agent_factories.get(&config.factory)?.wasm_sha256()?;
                Some((&config.factory, digest))
            })
            .collect();
        let spec_hash = if policy_artifacts.is_empty() {
            hash_serializable(&spec)?
        } else {
            hash_serializable(&(&spec, policy_artifacts))?
        };
        let artifacts = RunArtifacts::open(
            &output,
            &self.game.id,
            &self.game.version.to_string(),
            &spec_hash,
            self.loaded_game.digest(),
        )
        .await?;
        let mut totals = ExecutionCounts::default();
        if let Some(sessions) = &spec.sessions {
            let plans = self.plan_set(
                &spec,
                &artifacts.run_id,
                "evaluation",
                0,
                &sessions.scenarios,
                &sessions.seats,
                sessions.mirror_roles,
                &BTreeMap::new(),
                None,
            )?;
            totals.merge(
                self.execute_plans(plans, &spec.execution, &artifacts)
                    .await?
                    .counts,
            );
        }
        let checkpoints = match &spec.training {
            Some(training) => {
                self.train_epochs(
                    &spec,
                    training,
                    reward.expect("validated training reward"),
                    &artifacts,
                    &mut totals,
                )
                .await?
            }
            None => 0,
        };

        Ok(ExperimentRunSummary {
            run_id: artifacts.run_id,
            planned: totals.planned,
            completed: totals.completed,
            failed: totals.failed,
            skipped: totals.skipped,
            checkpoints,
            output_directory: output,
        })
    }

    /// Classifies existing result files as skipped and runs only the remaining plans.
    /// Each fresh result is finalized before it contributes to the counts. Fail-fast
    /// runs pending plans sequentially and returns after finalizing the first failure.
    pub(super) async fn execute_plans(
        &self,
        plans: Vec<SessionPlan>,
        execution: &ExecutionSpec,
        artifacts: &RunArtifacts,
    ) -> Result<ExecutionBatch> {
        let mut batch = ExecutionBatch {
            counts: ExecutionCounts::default(),
            steps: Vec::new(),
            has_failure: false,
        };
        batch.counts.planned = plans.len();
        let mut pending = Vec::new();
        for plan in plans {
            if let Some((status, trajectory)) = artifacts
                .result(&plan.plan_id, plan.training_capture.is_some())
                .await?
            {
                batch.counts.skipped += 1;
                batch.steps.extend(trajectory);
                batch.has_failure |= !status.is_success();
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
            loaded_game: self.loaded_game.clone(),
        });
        let executions = stream::iter(pending.into_iter().map(|plan| {
            let session_runner = session_runner.clone();
            async move {
                let result = session_runner.run(plan).await;
                artifacts.finalize_result(&result).await?;
                Ok::<_, anyhow::Error>((result.status.is_success(), result.captures.trajectory))
            }
        }))
        .buffer_unordered(concurrency);
        futures_util::pin_mut!(executions);
        while let Some(result) = executions.next().await {
            let (succeeded, steps) = result?;
            batch.steps.extend(steps);
            if succeeded {
                batch.counts.completed += 1;
            } else {
                batch.counts.failed += 1;
                batch.has_failure = true;
                if execution.fail_fast {
                    bail!("agent experiment stopped after a failed session");
                }
            }
        }
        Ok(batch)
    }
}

/// Training data and outcomes for a single execution phase.
pub(super) struct ExecutionBatch {
    pub(super) counts: ExecutionCounts,
    pub(super) steps: Vec<TrajectoryStep>,
    pub(super) has_failure: bool,
}

/// Run-wide summary independent of learner trajectory ownership.
#[derive(Default)]
pub(super) struct ExecutionCounts {
    planned: usize,
    completed: usize,
    failed: usize,
    skipped: usize,
}

impl ExecutionCounts {
    /// Includes the counts from one execution phase.
    pub(super) fn merge(&mut self, other: Self) {
        self.planned += other.planned;
        self.completed += other.completed;
        self.failed += other.failed;
        self.skipped += other.skipped;
    }
}
