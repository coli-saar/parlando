//! Project manifests, guest compilation, and browser packaging.

use super::{scenarios, Language, Report};
use anyhow::{anyhow, bail, ensure, Context, Result};
use fs2::FileExt;
use parlando::{
    component::{LoadedGame, WasmMetering},
    deployment, GameMetadata,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    env,
    fs::{self, OpenOptions},
    io::{ErrorKind, Write},
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
};

/// Project inputs and built package assets resolve independently from the manifest directory.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Manifest {
    id: String,
    name: String,
    version: semver::Version,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    component: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cargo_manifest: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    python: Option<PythonProject>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    client_project: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    client_dist: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    scenarios: Option<PathBuf>,
    #[serde(skip)]
    root: PathBuf,
}

/// Python entry module and optional source checkout of the guest SDK.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PythonProject {
    module: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sdk_path: Option<PathBuf>,
}

impl PythonProject {
    /// Resolves the configured SDK checkout to the directory to add to Python's search path.
    fn sdk_source(&self, root: &Path) -> Result<Option<PathBuf>> {
        let Some(sdk) = self.sdk_path.as_ref() else {
            return Ok(None);
        };
        let sdk = root.join(sdk);
        ensure!(
            Language::Python.contains_sdk(&sdk),
            "Python SDK path {} requires {}",
            sdk.display(),
            Language::Python.sdk_marker()
        );
        Ok(Some(sdk.join("src")))
    }
}

impl Manifest {
    /// Reads the manifest and resolves its project root without changing the caller's directory.
    pub(super) fn read(path: &Path) -> Result<Self> {
        let path = path
            .canonicalize()
            .with_context(|| format!("locating {}", path.display()))?;
        let input = fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        let mut manifest: Self = serde_yaml::from_slice(&input)
            .with_context(|| format!("decoding {}", path.display()))?;
        ensure!(
            manifest
                .component
                .as_ref()
                .is_none_or(|path| !path.as_os_str().is_empty()),
            "manifest component path cannot be empty"
        );
        ensure!(
            manifest.cargo_manifest.is_some()
                || manifest.python.is_some()
                || manifest.component.is_some(),
            "manifest requires a component path or game source"
        );
        ensure!(
            manifest.cargo_manifest.is_none() || manifest.python.is_none(),
            "manifest cannot declare both cargo_manifest and python"
        );
        manifest.root = path
            .parent()
            .context("manifest must have a parent directory")?
            .into();
        Ok(manifest)
    }

    /// Resolves an existing participant build relative to the project manifest.
    fn client_dist(&self) -> Option<PathBuf> {
        self.client_dist.as_ref().map(|path| self.root.join(path))
    }

    /// Resolves the executable input for a manifest that uses a prebuilt component.
    fn component_path(&self) -> Result<&Path> {
        self.component
            .as_deref()
            .context("manifest requires a component path")
    }

    /// Compiles or loads the project game with the operator-selected execution budget.
    pub(super) fn load_with_metering(
        &self,
        metering: WasmMetering,
    ) -> Result<(LoadedGame, GameMetadata)> {
        if self.cargo_manifest.is_none() && self.python.is_none() {
            return self.load_component(&self.root.join(self.component_path()?), metering);
        }
        let temporary = tempfile::tempdir()?;
        let component = temporary.path().join("game.wasm");
        self.compile_component(&component)?;
        self.load_component(&component, metering)
    }

    /// Binds an executable's digest to the declared game identity.
    fn load_component(
        &self,
        component: &Path,
        metering: WasmMetering,
    ) -> Result<(LoadedGame, GameMetadata)> {
        let loaded_game = LoadedGame::load_with_metering(component, metering)
            .with_context(|| format!("loading game component {}", component.display()))?;
        let metadata = GameMetadata {
            id: self.id.clone(),
            name: self.name.clone(),
            version: self.version.clone(),
            build_manifest: json!({"component_sha256": loaded_game.digest()}),
        };
        metadata.validate()?;
        Ok((loaded_game, metadata))
    }

    /// Executes Cargo on the configured local Rust project and returns its process builder.
    fn cargo(&self, subcommand: &str) -> Result<Command> {
        let path = self
            .cargo_manifest
            .as_ref()
            .context("manifest requires cargo_manifest")?;
        let mut cargo = build_tool("cargo");
        cargo
            .current_dir(&self.root)
            .arg(subcommand)
            .arg("--manifest-path")
            .arg(self.root.join(path));
        Ok(cargo)
    }

    /// Builds the game component without enabling policy-specific Cargo features.
    fn compile_rust(&self, component: &Path) -> Result<()> {
        compile_rust_component(
            &self.root,
            self.cargo_manifest
                .as_deref()
                .context("manifest requires cargo_manifest")?,
            &[],
            component,
        )
    }

    /// Compiles Python against the host's WIT using a private, temporary input file.
    fn compile_python(&self, project: &PythonProject, component: &Path) -> Result<()> {
        let wit = tempfile::Builder::new().suffix(".wit").tempfile()?;
        fs::write(wit.path(), include_bytes!("../../../wit/game.wit"))?;
        let mut command = python_tool(&self.root, "componentize-py");
        command
            .arg("--wit-path")
            .arg(wit.path())
            .args(["--world", "game", "componentize", "--stub-wasi", "-p"])
            .arg(&self.root);
        if let Some(sdk_source) = project.sdk_source(&self.root)? {
            command.arg("-p").arg(sdk_source);
        }
        run(
            command
                .arg("--output")
                .arg(component)
                .arg("--")
                .arg(&project.module),
            "componentize-py (install version 0.25.1 in the Python environment)",
        )
    }

    /// Compiles source if this is an authoring project; packages already contain their executable.
    fn compile_component(&self, component: &Path) -> Result<bool> {
        if let Some(project) = self.python.as_ref() {
            self.compile_python(project, component)?;
        } else if self.cargo_manifest.is_some() {
            self.compile_rust(component)?;
        } else {
            return Ok(false);
        }
        Ok(true)
    }

    /// Validates native compilation, the guest ABI and schema, and any declared scenario file.
    pub(super) fn check(&self) -> Result<Report> {
        if self.cargo_manifest.is_some() {
            run(&mut self.cargo("check")?, "cargo check")?;
        }
        let loaded_game = self.for_validation()?;
        let scenarios = self.read_scenarios()?;
        Ok(Report::new(
            format!(
                "Game check passed for {}. Scenarios found: {}.",
                self.id,
                scenarios.len()
            ),
            json!({"component_sha256": loaded_game.digest(), "scenarios": scenarios.len()}),
        ))
    }

    /// Verifies native unit tests and runs the same executable the live host would load.
    pub(super) fn test(&self) -> Result<Report> {
        let native = if self.cargo_manifest.is_some() {
            run(&mut self.cargo("test")?, "cargo test")?;
            true
        } else if let Some(project) = self
            .python
            .as_ref()
            .filter(|_| self.root.join("tests").is_dir())
        {
            let inherited = env::var_os("PYTHONPATH");
            let paths = std::iter::once(self.root.clone())
                .chain(project.sdk_source(&self.root)?)
                .chain(inherited.iter().flat_map(env::split_paths));
            run(
                python_tool(&self.root, "python")
                    .env("PYTHONPATH", env::join_paths(paths)?)
                    .args(["-m", "unittest", "discover", "-s", "tests"]),
                "Python unittest discovery",
            )?;
            true
        } else {
            false
        };
        let loaded_game = self.for_validation()?;
        let scenarios = self.read_scenarios()?;
        for scenario in &scenarios {
            scenario.run(&loaded_game)?;
        }
        let native_status = if native { "passed" } else { "not run" };
        Ok(Report::new(
            format!("Tests finished for {}.\nNative tests: {native_status}\nComponent scenarios passed: {}", self.id, scenarios.len()),
            json!({"native_tests_run": native, "component_scenarios_passed": scenarios.len(), "component_sha256": loaded_game.digest()}),
        ))
    }

    /// Checks declared assets for a standalone component and loads the game.
    fn for_validation(&self) -> Result<LoadedGame> {
        let (loaded_game, _) = if self.cargo_manifest.is_none()
            && self.python.is_none()
            && self.client_project.is_none()
        {
            self.load_component(&self.validate_package_assets()?, WasmMetering::Fuel)?
        } else {
            self.load_with_metering(WasmMetering::Fuel)?
        };
        Ok(loaded_game)
    }

    /// Checks the package's declared files before loading its executable.
    fn validate_package_assets(&self) -> Result<PathBuf> {
        if let Some(client) = &self.client_dist {
            self.package_asset(client, true)?;
            self.package_asset(&client.join("index.html"), false)?;
        }
        if let Some(scenarios) = &self.scenarios {
            self.package_asset(scenarios, false)?;
        }
        self.package_asset(self.component_path()?, false)
    }

    /// Requires a relative asset path whose resolved target belongs to this package.
    fn package_asset(&self, path: &Path, directory: bool) -> Result<PathBuf> {
        ensure!(
            !path.as_os_str().is_empty()
                && path
                    .components()
                    .all(|part| matches!(part, Component::Normal(_))),
            "invalid package asset path {}",
            path.display()
        );
        let asset = self
            .root
            .join(path)
            .canonicalize()
            .with_context(|| format!("locating package asset {}", path.display()))?;
        ensure!(
            asset.starts_with(&self.root),
            "package asset {} escapes the package",
            path.display()
        );
        ensure!(
            if directory {
                asset.is_dir()
            } else {
                asset.is_file()
            },
            "package asset {} must be a {}",
            path.display(),
            if directory {
                "directory"
            } else {
                "regular file"
            }
        );
        Ok(asset)
    }

    /// Parses the project's scenario declarations when present.
    fn read_scenarios(&self) -> Result<Vec<scenarios::Scenario>> {
        match &self.scenarios {
            Some(path) => scenarios::read(&self.root.join(path)),
            None => Ok(Vec::new()),
        }
    }

    /// Resolves the specific archive this manifest builds, including the full game version.
    pub(super) fn archive_path(&self) -> Result<PathBuf> {
        deployment::validate_game_id(&self.id)?;
        Ok(self
            .root
            .join("dist")
            .join(format!("{}-{}.zip", self.id, self.version)))
    }

    /// Stages a verified package, then replaces the previous package under a build lock.
    pub(super) fn build(&self) -> Result<Report> {
        let package = self.archive_path()?;
        let output = self.root.join("dist");
        fs::create_dir_all(&output)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(output.join(".parlando-build.lock"))?;
        lock.lock_exclusive()
            .context("locking the package output directory")?;
        check_output_path(&package, &self.id)?;
        let temporary = tempfile::tempdir_in(&output)?;
        let staging = temporary.path().join("staging");
        fs::create_dir(&staging)?;
        let component = staging.join("game.wasm");
        if !self.compile_component(&component)? {
            fs::copy(self.root.join(self.component_path()?), &component)?;
        }
        let (loaded_game, _) = self.load_component(&component, WasmMetering::Fuel)?;

        let client_dir = if let Some(project) = &self.client_project {
            let project = self.root.join(project);
            let install = if project.join("package-lock.json").is_file() {
                "ci"
            } else {
                "install"
            };
            run(
                build_tool("npm")
                    .current_dir(&project)
                    .args([install, "--no-audit", "--no-fund"]),
                "installing browser dependencies",
            )?;
            run(
                build_tool("npm")
                    .current_dir(&project)
                    .args(["run", "build"]),
                "building the browser client",
            )?;
            Some(project.join("dist"))
        } else {
            self.client_dist()
        };

        if let Some(client_dir) = &client_dir {
            ensure!(
                client_dir.join("index.html").is_file(),
                "browser assets {} require index.html",
                client_dir.display()
            );
            copy_tree(client_dir, &staging.join("client"))?;
        }
        let built_manifest = Self {
            id: self.id.clone(),
            name: self.name.clone(),
            version: self.version.clone(),
            component: Some("game.wasm".into()),
            cargo_manifest: None,
            python: None,
            client_project: None,
            client_dist: client_dir.map(|_| "client".into()),
            scenarios: None,
            root: staging.clone(),
        };
        fs::write(
            staging.join("game.yaml"),
            serde_yaml::to_string(&built_manifest)?,
        )?;
        let archive = temporary.path().join("package.zip");
        deployment::write_package_archive(&staging, &archive)?;
        fs::rename(archive, &package).context("publishing the package archive")?;
        let client_entry = if built_manifest.client_dist.is_some() {
            "\n  client/     Browser application and assets"
        } else {
            ""
        };
        Ok(Report::new(
            format!(
                "\nBuild complete for {} {}.\n  game.yaml   Package metadata\n  game.wasm   Game component{client_entry}\n\nZIP archive is ready for deployment.\n{}",
                self.id,
                self.version,
                package.display()
            ),
            json!({"package": package, "component_sha256": loaded_game.digest()}),
        ))
    }
}

/// Builds one Rust cdylib and wraps the core module with its embedded WIT metadata.
pub(super) fn compile_rust_component(
    root: &Path,
    manifest_path: &Path,
    features: &[String],
    component: &Path,
) -> Result<()> {
    let manifest_path = root
        .join(manifest_path)
        .canonicalize()
        .context("locating the Rust component manifest")?;
    let mut cargo = build_tool("cargo");
    cargo
        .current_dir(root)
        .args(["build", "--manifest-path"])
        .arg(&manifest_path);
    if !features.is_empty() {
        cargo.arg("--features").arg(features.join(","));
    }
    let output = cargo
        .args([
            "--lib",
            "--release",
            "--target",
            "wasm32-unknown-unknown",
            "--message-format=json-render-diagnostics",
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::inherit())
        .output()
        .context("starting cargo to build the Rust component")?;
    let mut candidates = Vec::new();
    for line in String::from_utf8(output.stdout)?.lines() {
        let message: Value = serde_json::from_str(line).context("decoding Cargo JSON output")?;
        match message["reason"].as_str() {
            Some("compiler-message") => {
                if let Some(diagnostic) = message["message"]["rendered"].as_str() {
                    eprint!("{diagnostic}");
                }
            }
            Some("compiler-artifact")
                if message["manifest_path"]
                    .as_str()
                    .is_some_and(|path| Path::new(path) == manifest_path) =>
            {
                if let Some(files) = message["filenames"].as_array() {
                    candidates.extend(
                        files
                            .iter()
                            .filter_map(Value::as_str)
                            .map(PathBuf::from)
                            .filter(|path| path.extension().is_some_and(|ext| ext == "wasm")),
                    );
                }
            }
            _ => {}
        }
    }
    ensure!(
        output.status.success(),
        "Cargo component build exited with {}",
        output.status
    );
    let [artifact] = candidates.as_slice() else {
        bail!("the Rust component must build exactly one WebAssembly cdylib");
    };
    run(
        build_tool("wasm-tools")
            .args(["component", "new"])
            .arg(artifact)
            .arg("-o")
            .arg(component),
        "wasm-tools component new",
    )
}

/// Resolves a supplied archive or the exact release declared by the current project.
pub(super) fn resolve_archive(package: Option<PathBuf>) -> Result<PathBuf> {
    match package {
        Some(package) => Ok(package),
        None => Manifest::read(Path::new("game.yaml"))
            .context("pass --package to use a ZIP outside a Parlando project")?
            .archive_path(),
    }
}

/// Protects existing files by allowing replacement only for a package of this game.
fn check_output_path(package: &Path, game_id: &str) -> Result<()> {
    let kind = match fs::symlink_metadata(package) {
        Ok(metadata) => metadata.file_type(),
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).context("inspecting the package output path"),
    };
    ensure!(
        kind.is_file(),
        "package output {} must be a regular file",
        package.display()
    );
    ensure!(
        !deployment::is_agent_package(package)?,
        "output file {} is an agent package",
        package.display()
    );
    let previous = deployment::package_game(package)
        .with_context(|| format!("output file {} already exists", package.display()))?;
    ensure!(
        previous == game_id,
        "output file {} belongs to game {previous:?}, expected game {game_id:?}",
        package.display()
    );
    Ok(())
}

/// Copies only regular, self-contained files into a package.
fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let to = destination.join(entry.file_name());
        if kind.is_dir() {
            copy_tree(&entry.path(), &to)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), to)?;
        } else {
            bail!(
                "browser package input {} is not a regular file or directory",
                entry.path().display()
            );
        }
    }
    Ok(())
}

/// Builds trusted project code without passing the CLI's deployment authority to child processes.
fn build_tool(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut command = Command::new(program);
    command.env_remove("PARLANDO_HOST_ADMIN_TOKEN");
    command
}

/// Selects the project's virtual environment, the active environment, or a tool on PATH.
fn python_tool(root: &Path, program: &str) -> Command {
    let local = root.join(".venv");
    let environment = if local.join("pyvenv.cfg").is_file() {
        Some(local)
    } else {
        env::var_os("VIRTUAL_ENV").map(PathBuf::from)
    };
    if let Some(environment) = environment {
        let prefix = if cfg!(windows) { "Scripts" } else { "bin" };
        let executable = format!("{program}{}", env::consts::EXE_SUFFIX);
        let mut command = build_tool(environment.join(prefix).join(executable));
        command.current_dir(root).env("VIRTUAL_ENV", environment);
        command
    } else {
        let program = if program == "python" && !cfg!(windows) {
            "python3"
        } else {
            program
        };
        let mut command = build_tool(program);
        command.current_dir(root);
        command
    }
}

/// Runs a noninteractive tool, reserving stdout for the CLI result.
fn run(command: &mut Command, description: &str) -> Result<()> {
    let output = command
        .stdin(Stdio::null())
        .stderr(Stdio::inherit())
        .output()
        .with_context(|| format!("starting {description}"))?;
    std::io::stderr().write_all(&output.stdout)?;
    ensure!(
        output.status.success(),
        "{description} exited with {}",
        output.status
    );
    Ok(())
}

const RUST_TEMPLATE: &[(&str, &str)] = &[
    (
        "Cargo.toml",
        include_str!("../../../templates/rust/Cargo.toml.template"),
    ),
    (
        "game.yaml",
        include_str!("../../../templates/rust/game.yaml"),
    ),
    (
        "src/lib.rs",
        include_str!("../../../templates/rust/src/lib.rs"),
    ),
    (
        "README.md",
        include_str!("../../../templates/rust/README.md"),
    ),
];

const PYTHON_TEMPLATE: &[(&str, &str)] = &[
    (
        "game.yaml",
        include_str!("../../../templates/python/game.yaml"),
    ),
    ("game.py", include_str!("../../../templates/python/game.py")),
    (
        "tests/test_game.py",
        include_str!("../../../templates/python/tests/test_game.py"),
    ),
    (
        "README.md",
        include_str!("../../../templates/python/README.md"),
    ),
];

const SHARED_TEMPLATE: &[(&str, &str)] = &[
    (
        "scenarios.yaml",
        include_str!("../../../templates/rust/scenarios.yaml"),
    ),
    (
        "client/index.html",
        include_str!("../../../templates/rust/client/index.html"),
    ),
    (
        "client/package.json",
        include_str!("../../../templates/rust/client/package.json"),
    ),
    (
        "client/beacon.css",
        include_str!("../../../templates/rust/client/beacon.css"),
    ),
    (
        "client/main.jsx",
        include_str!("../../../templates/rust/client/main.jsx"),
    ),
];

impl Language {
    /// Names the portable SDK directory and checkout for this language.
    fn sdk_name(&self) -> &'static str {
        match self {
            Self::Rust => "parlando-game-sdk",
            Self::Python => "parlando-python-sdk",
        }
    }

    /// Identifies a checkout by a source file the build will need.
    fn sdk_marker(&self) -> &'static str {
        match self {
            Self::Rust => "Cargo.toml",
            Self::Python => "src/parlando_game_sdk/__init__.py",
        }
    }

    /// Determines whether a directory supplies the expected SDK.
    fn contains_sdk(&self, path: &Path) -> bool {
        path.join(self.sdk_marker()).is_file()
    }

    /// Renders the dependency value expected by the chosen project template.
    fn sdk_dependency(&self, path: Option<&Path>) -> Result<String> {
        let requirement = format!("={}", env!("CARGO_PKG_VERSION"));
        match (self, path) {
            (Self::Rust, Some(path)) => Ok(format!(
                "{{ version = {requirement:?}, path = {} }}",
                serde_json::to_string(path)?
            )),
            (Self::Rust, None) => Ok(serde_json::to_string(&requirement)?),
            (Self::Python, path) => Ok(serde_json::to_string(&path)?),
        }
    }
}

/// Locates the release-specific asset directory next to the running CLI.
fn bundled_assets(executable: &Path) -> Option<PathBuf> {
    Some(
        executable
            .parent()?
            .join("share")
            .join("parlando")
            .join(env!("CARGO_PKG_VERSION")),
    )
}

/// Resolves an SDK bundled with a portable CLI or supplied by a source checkout.
fn installed_sdk(language: &Language) -> Option<PathBuf> {
    let executable = env::current_exe().ok();
    let versioned = executable
        .as_deref()
        .and_then(bundled_assets)
        .map(|directory| directory.join(language.sdk_name()));
    let portable = executable.and_then(|path| {
        path.parent()
            .map(|directory| directory.join("share").join(language.sdk_name()))
    });
    let checkout = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join(language.sdk_name());
    versioned
        .into_iter()
        .chain(portable)
        .chain([checkout])
        .filter_map(|path| path.canonicalize().ok())
        .find(|path| language.contains_sdk(path))
}

/// Vendors the matching browser tarball when supplied beside the CLI.
fn client_dependency(output: &Path) -> Result<String> {
    let version = env!("CARGO_PKG_VERSION");
    let archive = format!("coli-saar-parlando-client-{version}.tgz");
    let bundled = env::current_exe()
        .ok()
        .as_deref()
        .and_then(bundled_assets)
        .map(|directory| directory.join(&archive))
        .filter(|path| path.is_file());
    let Some(source) = bundled else {
        return Ok(version.to_owned());
    };
    let vendor = output.join("vendor");
    fs::create_dir_all(&vendor)?;
    fs::copy(&source, vendor.join(&archive))
        .with_context(|| format!("copying the bundled browser client {}", source.display()))?;
    Ok(format!("file:../vendor/{archive}"))
}

/// Creates a self-contained project structure without overwriting any existing destination.
pub(super) fn scaffold(
    name: &str,
    language: Language,
    output: Option<&Path>,
    sdk: Option<&Path>,
) -> Result<Report> {
    ensure!(
        !name.is_empty() && name.len() <= 64 && name.starts_with(|c: char| c.is_ascii_lowercase())
            && name.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'),
        "game ID must start with a lowercase letter and use up to 64 lowercase letters, digits, and dashes"
    );
    let output = output.unwrap_or_else(|| Path::new(name));
    let selected_sdk = sdk
        .map(|path| -> Result<PathBuf> {
            let path = path.canonicalize().context("locating the local game SDK")?;
            ensure!(
                language.contains_sdk(&path),
                "SDK path requires {}",
                language.sdk_marker()
            );
            Ok(path)
        })
        .transpose()?;
    let bundled_sdk = if selected_sdk.is_none() {
        installed_sdk(&language)
    } else {
        None
    };
    let vendor_path = Path::new("vendor").join(language.sdk_name());
    let dependency_path = selected_sdk
        .as_deref()
        .or_else(|| bundled_sdk.as_ref().map(|_| vendor_path.as_path()));
    let dependency = language.sdk_dependency(dependency_path)?;
    if let Some(parent) = output.parent().filter(|path| !path.as_os_str().is_empty()) {
        ensure!(
            parent.is_dir(),
            "project parent directory {} does not exist",
            parent.display()
        );
    }
    fs::create_dir(output).with_context(|| format!("creating project {}", output.display()))?;
    if let Some(sdk) = bundled_sdk {
        let destination = output.join(vendor_path);
        match language {
            Language::Rust => vendor_rust_sdk(&sdk, &destination)?,
            Language::Python => vendor_python_sdk(&sdk, &destination)?,
        }
    }
    let template = match language {
        Language::Rust => RUST_TEMPLATE,
        Language::Python => PYTHON_TEMPLATE,
    };
    let client_dependency = client_dependency(output)?;
    for (relative, template) in template.iter().chain(SHARED_TEMPLATE) {
        let path = output.join(relative);
        fs::create_dir_all(
            path.parent()
                .ok_or_else(|| anyhow!("invalid template path"))?,
        )?;
        let content = template
            .replace("__GAME_ID__", name)
            .replace("__SDK_DEPENDENCY__", &dependency)
            .replace("__SDK_VERSION__", env!("CARGO_PKG_VERSION"))
            .replace("__CLIENT_DEPENDENCY__", &client_dependency);
        fs::write(path, content)?;
    }
    let manifest = output.join("game.yaml");
    Ok(Report::new(
        format!(
            "Created game project in {}.\nManifest: {}",
            output.display(),
            manifest.display()
        ),
        json!({"project": output, "manifest": manifest}),
    ))
}

/// Copies the SDK sources required by a movable starter, excluding local build and test state.
fn vendor_rust_sdk(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination.join("derive"))?;
    for manifest in ["Cargo.toml", "derive/Cargo.toml"] {
        let from = source.join(manifest);
        ensure!(
            fs::symlink_metadata(&from)?.is_file(),
            "SDK manifest {} must be a regular file",
            from.display()
        );
        fs::copy(from, destination.join(manifest))?;
    }
    for directory in ["src", "derive/src", "wit"] {
        let from = source.join(directory);
        ensure!(
            fs::symlink_metadata(&from)?.is_dir(),
            "SDK source {} must be a directory",
            from.display()
        );
        copy_tree(&from, &destination.join(directory))?;
    }
    Ok(())
}

/// Vendors the Python SDK's pure-Python package without local interpreter artifacts.
fn vendor_python_sdk(source: &Path, destination: &Path) -> Result<()> {
    let package = Path::new("src/parlando_game_sdk");
    let destination = destination.join(package);
    fs::create_dir_all(&destination)?;
    for entry in fs::read_dir(source.join(package))? {
        let entry = entry?;
        if entry.path().extension().is_some_and(|ext| ext == "py")
            || entry.file_name() == "py.typed"
        {
            ensure!(
                entry.file_type()?.is_file(),
                "SDK source {} must be a regular file",
                entry.path().display()
            );
            fs::copy(entry.path(), destination.join(entry.file_name()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A package is incomplete without its configured participant HTML entry point.
    #[test]
    fn checks_declared_participant_index() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        fs::create_dir(root.join("client")).unwrap();
        fs::write(root.join("game.wasm"), []).unwrap();
        let manifest_path = root.join("game.yaml");
        fs::write(
            &manifest_path,
            "id: test\nname: Test\nversion: 1.0.0\ncomponent: game.wasm\nclient_dist: client\n",
        )
        .unwrap();
        let manifest = Manifest::read(&manifest_path).unwrap();
        let failure = manifest.check().unwrap_err();
        assert!(
            failure.to_string().contains("client/index.html"),
            "{failure:#}"
        );
        fs::write(root.join("client/index.html"), []).unwrap();
        assert_eq!(
            manifest.validate_package_assets().unwrap(),
            root.canonicalize().unwrap().join("game.wasm")
        );
        assert!(manifest
            .package_asset(Path::new("../outside.wasm"), false)
            .is_err());
        assert!(manifest
            .package_asset(&root.join("game.wasm"), false)
            .is_err());
    }

    /// Package inputs cannot follow symlinks to files outside the package directory.
    #[cfg(unix)]
    #[test]
    fn rejects_external_symlink_asset() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("package");
        fs::create_dir(&root).unwrap();
        let manifest = root.join("game.yaml");
        fs::write(
            &manifest,
            "id: test\nname: Test\nversion: 1.0.0\ncomponent: game.wasm\n",
        )
        .unwrap();
        let outside = temporary.path().join("outside.wasm");
        fs::write(&outside, []).unwrap();
        std::os::unix::fs::symlink(outside, root.join("game.wasm")).unwrap();
        let failure = Manifest::read(&manifest).unwrap().check().unwrap_err();
        assert!(
            failure.to_string().contains("escapes the package"),
            "{failure:#}"
        );
    }
}
