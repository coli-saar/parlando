//! Advances learner updates at checkpoint boundaries and schedules held-out validation.

use super::artifacts::RunArtifacts;
use super::execution::ExecutionCounts;
use super::spec::{ExperimentFile, TrainingSpec};
use super::training::{RLTrainingContext, TrainingBatch, TrainingCapture};
use super::ExperimentRunner;
use crate::game::Game;
use anyhow::{bail, Result};
use std::collections::BTreeMap;

impl<G: Game> ExperimentRunner<G> {
    /// Accumulates learner steps until a checkpoint boundary, then trains or restores
    /// that update. A restored checkpoint skips the learner callback. Validation
    /// uses the resulting policy and its steps never enter the next training batch.
    pub(super) async fn train_epochs(
        &mut self,
        spec: &ExperimentFile,
        training: &TrainingSpec,
        artifacts: &RunArtifacts,
        totals: &mut ExecutionCounts,
    ) -> Result<u64> {
        let run_id = &artifacts.run_id;
        let mut checkpoints = 0;
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
                spec,
                run_id,
                "training",
                epoch,
                &training.scenarios,
                &training.seats,
                training.mirror_roles,
                &overrides,
                Some(capture),
            )?;
            let batch = self
                .execute_plans(plans, &spec.execution, artifacts)
                .await?;
            pending_steps.extend(batch.steps);
            let training_failed = batch.has_failure;
            totals.merge(batch.counts);
            if training_failed {
                bail!("training stopped because an epoch contains a failed session");
            }

            if !epoch.is_multiple_of(training.checkpoint_every_epochs) {
                continue;
            }
            checkpoints += 1;
            let update_id = format!("{run_id}:checkpoint:{checkpoints}");
            checkpoint = if let Some(existing) = artifacts
                .checkpoint(checkpoints, &update_id, &checkpoint)
                .await?
            {
                existing
            } else {
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
                artifacts
                    .finalize_checkpoint(checkpoints, update_id, &checkpoint, &next)
                    .await?;
                next
            };
            pending_steps.clear();

            if let Some(validation) = &spec.validation {
                if checkpoints.is_multiple_of(validation.every_checkpoints) {
                    let overrides =
                        BTreeMap::from([(training.learner.clone(), checkpoint.clone())]);
                    let plans = self.plan_set(
                        spec,
                        run_id,
                        "validation",
                        checkpoints,
                        &validation.scenarios,
                        &validation.seats,
                        validation.mirror_roles,
                        &overrides,
                        None,
                    )?;
                    totals.merge(
                        self.execute_plans(plans, &spec.execution, artifacts)
                            .await?
                            .counts,
                    );
                }
            }
        }
        Ok(checkpoints)
    }
}
