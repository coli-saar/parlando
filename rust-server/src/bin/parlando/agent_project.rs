//! Resolves agent project inputs, verifies the policy against its game, and builds agent archives.

use super::{
    agent_scaffold,
    game_dependency::{self, Dependency, ResolvedGame},
    project::{self, compile_rust_component},
    Report,
};
use anyhow::{ensure, Context, Result};
use fs2::FileExt;
use parlando::{component::WasmMetering, deployment};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    fs::{self, File, OpenOptions},
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
};

/// Project-level source or prebuilt component settings for one agent.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AgentManifest {
    game: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    dependency: Option<Dependency>,
    /// Optional generated Rust module, relative to the author's manifest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    bindings: Option<PathBuf>,
    id: String,
    name: String,
    version: semver::Version,
    #[serde(skip_serializing_if = "Option::is_none")]
    component: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cargo_manifest: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    features: Vec<String>,
    #[serde(skip)]
    root: PathBuf,
    #[serde(skip)]
    _lock: Option<File>,
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
    pub(super) fn open(path: &Path) -> Result<Self> {
        Self::read_locked(path, |bytes| Ok(serde_yaml::from_slice(bytes)?))
    }

    /// Holds the project lock while loading build inputs or replacing the dependency declaration.
    fn read_locked(path: &Path, parse: impl FnOnce(&[u8]) -> Result<Self>) -> Result<Self> {
        let path = path
            .canonicalize()
            .with_context(|| format!("locating {}", path.display()))?;
        let root = path.parent().context("manifest has no parent directory")?;
        let output = root.join("dist");
        fs::create_dir_all(&output)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(output.join(".parlando-build.lock"))?;
        lock.lock_exclusive().context("locking agent project")?;
        let input = fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        let mut manifest = parse(&input).with_context(|| format!("decoding {}", path.display()))?;
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
        manifest.root = root.into();
        manifest._lock = Some(lock);
        Ok(manifest)
    }

    /// Writes the optional Rust binding module at the location chosen by the project.
    fn generate_bindings(&self, game: &ResolvedGame) -> Result<()> {
        if let Some(path) = &self.bindings {
            agent_scaffold::bindings(&self.root.join(path), game.game.schema())?;
        }
        Ok(())
    }

    /// Resolves once and regenerates the bindings consumed by this compilation.
    async fn refresh(&self, metering: WasmMetering) -> Result<ResolvedGame> {
        let dep = self.dependency.as_ref().context("agent builds require a pinned game ZIP; run parlando agent set-game <game.zip> with the agent manifest")?;
        let game = game_dependency::resolve(&self.root, &self.game, dep, metering).await?;
        self.generate_bindings(&game)?;
        Ok(game)
    }

    /// Loads the just-built policy against the same game instance used for code generation.
    pub(super) async fn for_run(
        &self,
        metering: WasmMetering,
    ) -> Result<(ResolvedGame, parlando::agent::wasm::WasmAgentFactory)> {
        let game = self.refresh(metering).await?;
        let temporary = tempfile::tempdir()?;
        let archive = self.stage(temporary.path())?;
        let factory = deployment::load_agent_package(&archive, &game.game, &self.game, metering)?;
        Ok((game, factory))
    }

    /// Checks component compatibility and optionally runs the policy's native unit tests.
    pub(super) async fn check(&self, test: bool) -> Result<Report> {
        let game = self.refresh(WasmMetering::Fuel).await?;
        if let Some(manifest) = self.cargo_manifest.as_ref().filter(|_| test) {
            let mut cargo = std::process::Command::new("cargo");
            cargo
                .current_dir(&self.root)
                .args(["test", "--manifest-path"])
                .arg(manifest);
            if !self.features.is_empty() {
                cargo.arg("--features").arg(self.features.join(","));
            }
            project::run(&mut cargo, "agent unit tests")?;
        }
        let temporary = tempfile::tempdir()?;
        let archive = self.stage(temporary.path())?;
        let report = self.validate(&archive, &game)?;
        Ok(Report::new(
            format!("Agent check passed for {}.", self.id),
            report,
        ))
    }

    /// Verifies the package identity and its layouts against the selected game package.
    fn validate(&self, archive: &Path, game: &ResolvedGame) -> Result<serde_json::Value> {
        deployment::load_agent_package(archive, &game.game, &self.game, WasmMetering::Fuel)?;
        deployment::check_agent_package(archive, WasmMetering::Fuel)
    }

    /// Publishes a validated ZIP alongside game archives after staging all inputs.
    pub(super) async fn build(&self) -> Result<Report> {
        let game = self.refresh(WasmMetering::Fuel).await?;
        let output = self.root.join("dist");
        let archive = output.join(format!("agent-{}-{}.zip", self.id, self.version));
        self.check_output_path(&archive)?;
        let temporary = tempfile::tempdir_in(&output)?;
        let staged = self.stage(temporary.path())?;
        let details = self.validate(&staged, &game)?;
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

/// Creates a standalone agent against a local or remote game archive.
pub(super) async fn create(
    id: &str,
    name: Option<&str>,
    input: &str,
    output: Option<&Path>,
    sdk: Option<&Path>,
) -> Result<Report> {
    deployment::validate_agent_id(id)?;
    let name = match name {
        Some(name) => {
            ensure!(
                !name.trim().is_empty() && !name.chars().any(char::is_control),
                "agent name must contain visible text and no control characters"
            );
            name.trim().to_owned()
        }
        None => id
            .split(['.', '-', '_'])
            .filter(|word| !word.is_empty())
            .map(|word| {
                let mut chars = word.chars();
                let first = chars.next().expect("nonempty word").to_ascii_uppercase();
                format!("{first}{}", chars.as_str())
            })
            .collect::<Vec<_>>()
            .join(" "),
    };
    let output = output.unwrap_or_else(|| Path::new(id));
    ensure!(
        !output.try_exists()?,
        "destination {} already exists",
        output.display()
    );
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let temporary =
        tempfile::tempdir_in(parent).context("creating a sibling agent project directory")?;
    let root = temporary.path();
    let (dependency, game) = game_dependency::prepare(root, input, WasmMetering::Fuel).await?;
    agent_scaffold::scaffold(root, id, &name, game.game.schema(), sdk)?;
    let bindings = PathBuf::from("src/game.rs");
    agent_scaffold::bindings(&root.join(&bindings), game.game.schema())?;
    let manifest = AgentManifest {
        game: game.metadata.id,
        dependency: Some(dependency),
        bindings: Some(bindings),
        id: id.to_string(),
        name,
        version: semver::Version::new(0, 1, 0),
        component: None,
        cargo_manifest: Some("Cargo.toml".into()),
        features: vec![],
        root: root.into(),
        _lock: None,
    };
    fs::write(root.join("agent.yaml"), serde_yaml::to_string(&manifest)?)?;
    // Reserve the name with create_dir so even a concurrently created empty project survives.
    fs::create_dir(output).with_context(|| format!("creating {}", output.display()))?;
    if let Err(error) = fs::rename(root, output) {
        let _ = fs::remove_dir(output);
        return Err(error).context("publishing the agent project");
    }
    Ok(Report::new(
        format!(
            "Created agent {id} for {} in {}.",
            manifest.game,
            output.display()
        ),
        json!({"project": output, "manifest": output.join("agent.yaml"), "game": manifest.game, "dependency": manifest.dependency}),
    ))
}

/// Replaces a single project file only after its full contents have been written.
fn replace(path: &Path, bytes: &[u8]) -> Result<()> {
    let directory = path.parent().context("missing project directory")?;
    fs::create_dir_all(directory)?;
    let mut staged = tempfile::NamedTempFile::new_in(directory)?;
    if let Ok(metadata) = path.metadata() {
        staged.as_file().set_permissions(metadata.permissions())?;
    }
    staged.write_all(bytes)?;
    staged.persist(path)?;
    Ok(())
}

/// Validates a replacement dependency before recording it in the portable agent manifest.
pub(super) async fn set_game(path: &Path, input: &str) -> Result<Report> {
    // Only the replacement pin is an input to this operation. This also lets an
    // author repair a missing or invalid pin without editing generated metadata.
    let mut document = serde_yaml::Value::Null;
    let project = AgentManifest::read_locked(path, |bytes| {
        document = serde_yaml::from_slice(bytes)?;
        let mut inputs = document.clone();
        inputs
            .as_mapping_mut()
            .context("expected an agent manifest mapping")?
            .remove("dependency");
        Ok(serde_yaml::from_value(inputs)?)
    })?;
    let (dep, game) = game_dependency::prepare(&project.root, input, WasmMetering::Fuel).await?;
    ensure!(
        game.metadata.id == project.game,
        "expected game {}, found {}",
        project.game,
        game.metadata.id
    );
    project.generate_bindings(&game)?;
    let path = path.canonicalize()?;
    document["dependency"] = serde_yaml::to_value(&dep)?;
    replace(&path, serde_yaml::to_string(&document)?.as_bytes())?;
    Ok(Report::new(
        format!("Set game dependency to {} {}.", project.game, dep.version),
        json!({"game":project.game,"dependency":dep}),
    ))
}
