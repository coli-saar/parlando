//! Builds standalone local policy archives for games already deployed to a host.

use super::{project::compile_rust_component, Report};
use anyhow::{ensure, Context, Result};
use fs2::FileExt;
use parlando::{component::WasmMetering, deployment};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    fs::{self, OpenOptions},
    io::ErrorKind,
    path::{Path, PathBuf},
};

/// Project-level source or prebuilt component settings for one agent.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AgentManifest {
    game: String,
    id: String,
    name: String,
    version: semver::Version,
    component: Option<PathBuf>,
    cargo_manifest: Option<PathBuf>,
    #[serde(default)]
    features: Vec<String>,
    #[serde(skip)]
    root: PathBuf,
}

/// Self-contained manifest written beside the compiled policy in the archive.
#[derive(Serialize)]
struct ArchiveManifest<'a> {
    game: &'a str,
    id: &'a str,
    name: &'a str,
    version: &'a semver::Version,
    component: &'static str,
}

/// Identifies an agent manifest by its required target-game field.
pub(super) fn is_agent_manifest(path: &Path) -> Result<bool> {
    let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let manifest: serde_yaml::Value =
        serde_yaml::from_slice(&bytes).with_context(|| format!("decoding {}", path.display()))?;
    Ok(manifest.get("game").is_some())
}

impl AgentManifest {
    /// Resolves trusted build inputs relative to the manifest and validates its identity.
    pub(super) fn read(path: &Path) -> Result<Self> {
        let path = path
            .canonicalize()
            .with_context(|| format!("locating {}", path.display()))?;
        let input = fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        let mut manifest: Self = serde_yaml::from_slice(&input)
            .with_context(|| format!("decoding {}", path.display()))?;
        deployment::validate_game_id(&manifest.game)?;
        deployment::validate_agent_id(&manifest.id)?;
        ensure!(
            manifest.component.is_some() != manifest.cargo_manifest.is_some(),
            "agent manifest requires exactly one of component or cargo_manifest"
        );
        ensure!(
            manifest.features.is_empty() || manifest.cargo_manifest.is_some(),
            "agent features require cargo_manifest"
        );
        manifest.root = path
            .parent()
            .context("manifest has no parent directory")?
            .into();
        Ok(manifest)
    }

    /// Checks a project by building and validating a private archive.
    pub(super) fn check(&self) -> Result<Report> {
        let temporary = tempfile::tempdir()?;
        let archive = self.stage(temporary.path())?;
        let report = deployment::check_agent_package(&archive, WasmMetering::Fuel)?;
        Ok(Report::new(
            format!("Agent check passed for {}.", self.id),
            report,
        ))
    }

    /// Publishes a validated ZIP alongside game archives after staging all inputs.
    pub(super) fn build(&self) -> Result<Report> {
        let output = self.root.join("dist");
        fs::create_dir_all(&output)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(output.join(".parlando-build.lock"))?;
        lock.lock_exclusive()
            .context("locking the package output directory")?;

        let archive = output.join(format!("agent-{}-{}.zip", self.id, self.version));
        self.check_output_path(&archive)?;
        let temporary = tempfile::tempdir_in(&output)?;
        let staged = self.stage(temporary.path())?;
        let details = deployment::check_agent_package(&staged, WasmMetering::Fuel)?;
        fs::rename(staged, &archive).context("publishing the agent archive")?;
        Ok(Report::new(
            format!(
                "\nBuild complete for {} {}.\nAgent ZIP for {} is ready for deployment.\n{}",
                self.id,
                self.version,
                self.game,
                archive.display()
            ),
            json!({
                "package": archive,
                "game": self.game,
                "id": self.id,
                "version": self.version,
                "component_sha256": details["component_sha256"],
            }),
        ))
    }

    /// Protects an existing output unless it already contains this agent for this game.
    fn check_output_path(&self, archive: &Path) -> Result<()> {
        let kind = match fs::symlink_metadata(archive) {
            Ok(metadata) => metadata.file_type(),
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error).context("inspecting the agent archive output path"),
        };
        ensure!(
            kind.is_file(),
            "agent archive output must be a regular file"
        );
        let existing = deployment::check_agent_package(archive, WasmMetering::Fuel)
            .with_context(|| format!("output file {} already exists", archive.display()))?;
        ensure!(
            existing["game"].as_str() == Some(self.game.as_str())
                && existing["id"].as_str() == Some(self.id.as_str()),
            "output file {} belongs to a different agent package",
            archive.display()
        );
        Ok(())
    }

    /// Builds or copies the component and serializes the deployable archive manifest.
    fn stage(&self, directory: &Path) -> Result<PathBuf> {
        let staging = directory.join("staging");
        fs::create_dir(&staging)?;
        let component = staging.join("agent.wasm");
        if let Some(manifest) = &self.cargo_manifest {
            compile_rust_component(&self.root, manifest, &self.features, &component)?;
        } else {
            let source = self
                .root
                .join(self.component.as_ref().expect("validated agent input"));
            ensure!(
                source.is_file(),
                "agent component {} must be a regular file",
                source.display()
            );
            fs::copy(source, &component)?;
        }
        let packed = ArchiveManifest {
            game: &self.game,
            id: &self.id,
            name: &self.name,
            version: &self.version,
            component: "agent.wasm",
        };
        fs::write(staging.join("agent.yaml"), serde_yaml::to_string(&packed)?)?;
        let archive = directory.join("package.zip");
        deployment::write_agent_archive(&staging, &archive)?;
        Ok(archive)
    }
}
