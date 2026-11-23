//! Reads, validates, and atomically publishes finalized run artifacts.
//!
//! Execution checks the run manifest and looks up finalized outcomes by run and plan identity
//! before scheduling work. Epochs recover a checkpoint only when its number, update ID, and
//! base match the expected update. Results and checkpoints become visible by renaming a
//! completed file in the output directory.

use super::results::{SessionResult, SessionStatus};
use super::spec::EXPERIMENT_SCHEMA;
use super::training::{CheckpointId, TrajectoryStep};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Identifies the finalized files belonging to one validated run directory.
pub(super) struct RunArtifacts {
    directory: PathBuf,
    pub(super) run_id: String,
}

impl RunArtifacts {
    /// Opens the selected output directory and verifies that its manifest matches the inputs.
    pub(super) async fn open(
        directory: &Path,
        game_id: &str,
        version: &str,
        spec_hash: &str,
        component_sha256: &str,
    ) -> Result<Self> {
        tokio::fs::create_dir_all(directory.join("results")).await?;
        let manifest = load_or_create_manifest(
            &directory.join("run.json"),
            game_id,
            version,
            spec_hash,
            component_sha256,
        )
        .await?;
        Ok(Self {
            directory: directory.to_owned(),
            run_id: manifest.run_id,
        })
    }

    /// Recovers the status and training steps from this plan's finalized result.
    pub(super) async fn result(
        &self,
        plan_id: &str,
        load_trajectory: bool,
    ) -> Result<Option<(SessionStatus, Vec<TrajectoryStep>)>> {
        let path = result_path(&self.directory, plan_id);
        let bytes = match tokio::fs::read(&path).await {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error).with_context(|| format!("failed to read {}", path.display()))
            }
        };
        let result: FinalizedResult<'_> = serde_json::from_slice(&bytes)
            .with_context(|| format!("invalid finalized result {}", path.display()))?;
        if result.run_id != self.run_id || result.plan_id != plan_id {
            bail!(
                "result {} has inconsistent run or plan identity",
                path.display()
            );
        }
        let trajectory = match (load_trajectory, result.trajectory) {
            (true, Some(trajectory)) => serde_json::from_str(trajectory.get())
                .with_context(|| format!("invalid finalized trajectory {}", path.display()))?,
            _ => Vec::new(),
        };
        Ok(Some((result.status, trajectory)))
    }

    /// Publishes all result fields together once the session has shut down.
    pub(super) async fn finalize_result(&self, result: &SessionResult) -> Result<()> {
        write_json_atomic(&result_path(&self.directory, &result.plan_id), result).await
    }

    /// Restores a checkpoint only for the expected run, number, update ID and base.
    /// A missing record permits the caller to submit the idempotent learner update.
    /// An unreadable, malformed or mismatched record stops resumption.
    pub(super) async fn checkpoint(
        &self,
        number: u64,
        update_id: &str,
        base: &CheckpointId,
    ) -> Result<Option<CheckpointId>> {
        let path = checkpoint_path(&self.directory, number);
        let bytes = match tokio::fs::read(&path).await {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let record: CheckpointRecord = serde_json::from_slice(&bytes)
            .with_context(|| format!("invalid checkpoint record {}", path.display()))?;
        if record.run_id != self.run_id
            || record.checkpoint_number != number
            || record.update_id != update_id
            || &record.base != base
        {
            bail!(
                "checkpoint record {} has inconsistent identity",
                path.display()
            );
        }
        Ok(Some(record.checkpoint))
    }

    /// Records the starting and resulting learner checkpoints for a completed update.
    pub(super) async fn finalize_checkpoint(
        &self,
        number: u64,
        update_id: String,
        base: &CheckpointId,
        checkpoint: &CheckpointId,
    ) -> Result<()> {
        write_json_atomic(
            &checkpoint_path(&self.directory, number),
            &CheckpointRecord {
                run_id: self.run_id.clone(),
                checkpoint_number: number,
                update_id,
                base: base.clone(),
                checkpoint: checkpoint.clone(),
            },
        )
        .await
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

/// Durable identity of one invocation and the normalized input it belongs to.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RunManifest {
    schema: String,
    run_id: String,
    game_id: String,
    game_version: String,
    spec_hash: String,
    /// Digest of the loaded game component. A missing digest cannot match a run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    component_sha256: Option<String>,
}

/// Loads the existing run manifest or atomically creates a new run identity.
async fn load_or_create_manifest(
    path: &Path,
    game_id: &str,
    game_version: &str,
    spec_hash: &str,
    component_sha256: &str,
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
            if manifest.component_sha256.as_deref() != Some(component_sha256) {
                bail!(
                    "run manifest {} belongs to a different game component",
                    path.display()
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
                component_sha256: Some(component_sha256.to_owned()),
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

/// Projection of the durable session fields needed to resume a plan.
#[derive(Deserialize)]
struct FinalizedResult<'a> {
    run_id: String,
    plan_id: String,
    status: SessionStatus,
    #[serde(borrow)]
    trajectory: Option<&'a RawValue>,
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
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    /// A moved result from a different run or plan cannot cause a session to be skipped.
    #[tokio::test]
    async fn resume_result_requires_both_identity_components() {
        let directory = tempdir().unwrap();
        let artifacts = RunArtifacts::open(directory.path(), "game", "1", "spec", "component")
            .await
            .unwrap();
        assert!(artifacts
            .result("requested", false)
            .await
            .unwrap()
            .is_none());
        let path = result_path(directory.path(), "requested");
        for (run, plan) in [
            ("other-run", "requested"),
            (artifacts.run_id.as_str(), "other-plan"),
        ] {
            write_json_atomic(
                &path,
                &json!({"run_id": run, "plan_id": plan, "status": "completed"}),
            )
            .await
            .unwrap();
            assert!(artifacts.result("requested", false).await.is_err());
        }
        write_json_atomic(
            &path,
            &json!({"run_id": artifacts.run_id, "plan_id": "requested", "status": "quiescent"}),
        )
        .await
        .unwrap();
        assert_eq!(
            artifacts
                .result("requested", false)
                .await
                .unwrap()
                .unwrap()
                .0,
            SessionStatus::Quiescent
        );
    }

    /// A checkpoint is reusable only at its original numbered update and base.
    #[tokio::test]
    async fn checkpoint_resume_validates_the_update_number_id_and_base() {
        let directory = tempdir().unwrap();
        let artifacts = RunArtifacts::open(directory.path(), "game", "1", "spec", "component")
            .await
            .unwrap();
        let base = CheckpointId::new("initial").unwrap();
        let next = CheckpointId::new("trained").unwrap();
        artifacts
            .finalize_checkpoint(1, "update".into(), &base, &next)
            .await
            .unwrap();
        assert_eq!(
            artifacts.checkpoint(1, "update", &base).await.unwrap(),
            Some(next.clone())
        );
        assert!(artifacts.checkpoint(1, "other", &base).await.is_err());
        assert!(artifacts.checkpoint(1, "update", &next).await.is_err());
        tokio::fs::copy(
            checkpoint_path(directory.path(), 1),
            checkpoint_path(directory.path(), 2),
        )
        .await
        .unwrap();
        assert!(artifacts.checkpoint(2, "update", &base).await.is_err());
    }

    /// A resumed component run must use the exact executable that created it.
    #[tokio::test]
    async fn component_run_pins_the_loaded_digest() {
        let directory = tempdir().unwrap();
        let path = directory.path();
        let artifacts = RunArtifacts::open(path, "game", "1", "spec", "original")
            .await
            .unwrap();
        let resumed = RunArtifacts::open(path, "game", "1", "spec", "original")
            .await
            .unwrap();
        assert_eq!(resumed.run_id, artifacts.run_id);
        let error = RunArtifacts::open(path, "game", "1", "spec", "different")
            .await
            .err()
            .expect("run accepted a different game artifact");
        assert!(error.to_string().contains("different game component"));

        let manifest_path = path.join("run.json");
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&tokio::fs::read(&manifest_path).await.unwrap()).unwrap();
        manifest.as_object_mut().unwrap().remove("component_sha256");
        write_json_atomic(&manifest_path, &manifest).await.unwrap();
        let error = RunArtifacts::open(path, "game", "1", "spec", "original")
            .await
            .err()
            .expect("run accepted a manifest without a component digest");
        assert!(error.to_string().contains("different game component"));
    }
}
