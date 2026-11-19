//! Executes evaluation and training phases and resumes finalized plans.

use super::artifacts::RunArtifacts;
use super::planning::{hash_serializable, SessionPlan};
use super::session::SessionRunner;
use super::spec::{ExecutionSpec, ExperimentFile};
use super::training::TrajectoryStep;
use super::{ExperimentRunSummary, ExperimentRunner};
use crate::game::Game;
use anyhow::{bail, Result};
use futures_util::{stream, StreamExt};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

impl<G: Game> ExperimentRunner<G> {
    /// Validates and executes an already parsed experiment specification.
    pub(super) async fn run(mut self, spec: ExperimentFile) -> Result<ExperimentRunSummary> {
        self.validate_spec(&spec)?;
        let output = PathBuf::from(&spec.output.directory);

        let spec_hash = hash_serializable(&spec)?;
        let artifacts = RunArtifacts::open(
            &output,
            &self.game.id,
            &self.game.version.to_string(),
            &spec_hash,
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
                self.train_epochs(&spec, training, &artifacts, &mut totals)
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

    /// Executes or resumes one set of already resolved session plans.
    pub(super) async fn execute_plans(
        &self,
        plans: Vec<SessionPlan<G>>,
        execution: &ExecutionSpec,
        artifacts: &RunArtifacts,
    ) -> Result<ExecutionBatch> {
        let mut batch = ExecutionBatch::default();
        batch.counts.planned = plans.len();
        let mut pending = Vec::new();
        for plan in plans {
            if let Some(result) = artifacts.result(&plan.plan_id).await? {
                batch.counts.skipped += 1;
                batch.steps.extend(result.trajectory);
                batch.has_failure |= !result.status.is_success();
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
        let executions = stream::iter(pending.into_iter().map(|plan| {
            let session_runner = session_runner.clone();
            async move {
                let result = session_runner.run(plan).await;
                artifacts.finalize_result(&result).await?;
                Ok::<_, anyhow::Error>((result.status.is_success(), result.trajectory))
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
#[derive(Default)]
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
