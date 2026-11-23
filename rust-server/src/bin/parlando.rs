//! Author, validate, build, and run a component game from a local project.

#[path = "parlando/agent_project.rs"]
mod agent_project;
#[path = "parlando/agent_scaffold.rs"]
mod agent_scaffold;
#[path = "parlando/game_dependency.rs"]
mod game_dependency;
#[path = "parlando/project.rs"]
mod project;
#[path = "parlando/remote.rs"]
mod remote;
#[path = "parlando/scenarios.rs"]
mod scenarios;

use agent_project::AgentManifest;
use anyhow::Result;
use clap::{error::ErrorKind, Parser, Subcommand, ValueEnum};
use parlando::{agent::grpc::RemoteAgent, deployment, ExperimentFile, ExperimentRunner};
use project::Manifest;
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    process::ExitCode,
};

/// The public CLI can produce one structured result for each invocation.
#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// Emit structured JSON on stdout, including on failure.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

/// Selects a CLI operation on a project or packaged artifact.
#[derive(Subcommand)]
enum Command {
    /// Create a playable game.
    New {
        /// Game ID.
        name: String,
        /// Language of the generated game.
        #[arg(long, value_enum, default_value = "rust")]
        lang: Language,
        #[command(flatten)]
        options: ScaffoldOptions,
    },
    /// Create an agent or change its game package.
    Agent {
        #[command(subcommand)]
        operation: AgentCommand,
    },
    /// Compile a game or local agent and create its deployable ZIP in dist/.
    Build {
        /// Project manifest. Looks for game.yaml, then agent.yaml, in the current directory.
        #[arg(long)]
        manifest: Option<PathBuf>,
    },
    /// Verify a game or agent project or a built ZIP archive.
    Check {
        /// Project manifest. Looks for game.yaml, then agent.yaml, in the current directory.
        #[arg(long, conflicts_with = "package")]
        manifest: Option<PathBuf>,
        /// Game or agent ZIP archive produced by parlando build.
        #[arg(long)]
        package: Option<PathBuf>,
    },
    /// Run project tests and check game scenarios or agent compatibility.
    Test {
        /// Project manifest. Looks for game.yaml, then agent.yaml, in the current directory.
        #[arg(long)]
        manifest: Option<PathBuf>,
    },
    /// Run or resume an agent–agent evaluation.
    Run {
        /// Agent experiment YAML.
        experiment: PathBuf,
        /// Local agent ZIP or prebuilt WASM component. May be repeated.
        #[arg(long = "agent")]
        agents: Vec<PathBuf>,
        /// Game manifest or agent manifest with a game dependency.
        #[arg(long, conflicts_with = "package")]
        manifest: Option<PathBuf>,
        /// Built ZIP archive, defaulting to the game ID and version in game.yaml.
        #[arg(long)]
        package: Option<PathBuf>,
    },
    #[command(flatten)]
    Remote(remote::RemoteCommand),
}

/// Shared output and SDK selection for new projects.
#[derive(clap::Args)]
struct ScaffoldOptions {
    /// Destination directory, defaulting to the project ID.
    #[arg(long)]
    path: Option<PathBuf>,
    /// Local checkout of the SDK for the selected language.
    #[arg(long)]
    sdk_path: Option<PathBuf>,
}

/// Creates agent projects and selects their pinned game packages.
#[derive(Subcommand)]
enum AgentCommand {
    /// Create a Rust WASM policy against a game ZIP.
    New {
        /// Agent factory ID.
        id: String,
        /// Display name for the dashboard, defaulting to the words in the ID.
        #[arg(long)]
        name: Option<String>,
        /// Game package ZIP path or HTTP(S) URL.
        #[arg(long)]
        game: String,
        /// Language of the generated policy.
        #[arg(long, value_parser = ["rust"], default_value = "rust")]
        lang: String,
        #[command(flatten)]
        options: ScaffoldOptions,
    },
    /// Replace the declared dependency and refresh the agent's game bindings.
    SetGame {
        /// Game package ZIP path or HTTP(S) URL.
        game: String,
        #[arg(long, default_value = "agent.yaml")]
        manifest: PathBuf,
    },
}

/// Languages with scaffold and build support.
#[derive(Clone, ValueEnum)]
enum Language {
    Rust,
    Python,
}

impl Command {
    /// Identifies the operation in a structured error independently of clap's display text.
    fn name(&self) -> &'static str {
        match self {
            Self::New { .. } => "new",
            Self::Agent {
                operation: AgentCommand::New { .. },
            } => "agent new",
            Self::Agent {
                operation: AgentCommand::SetGame { .. },
            } => "agent set-game",
            Self::Build { .. } => "build",
            Self::Check { .. } => "check",
            Self::Test { .. } => "test",
            Self::Run { .. } => "run",
            Self::Remote(command) => command.name(),
        }
    }
}

/// Carries the terminal summary and the stable machine-readable result of one command.
#[derive(Debug)]
struct Report {
    message: String,
    data: Value,
}

impl Report {
    /// Keeps both renderings together where the command knows what its result means.
    fn new(message: impl Into<String>, data: Value) -> Self {
        Self {
            message: message.into(),
            data,
        }
    }
}

/// Resolves an explicit manifest or discovers a project in the working directory.
/// A game with a colocated policy keeps its usual default game build.
fn project_manifest(explicit: Option<&Path>) -> Result<&Path> {
    if let Some(path) = explicit {
        return Ok(path);
    }
    for name in ["game.yaml", "agent.yaml"] {
        let path = Path::new(name);
        if path.try_exists()? {
            return Ok(path);
        }
    }
    anyhow::bail!("no game.yaml or agent.yaml in the current directory; pass --manifest PATH")
}

/// Returns the result payload and whether a completed evaluation succeeded.
async fn execute(command: Command) -> Result<(Report, bool)> {
    let result = match command {
        Command::New {
            name,
            lang,
            options,
        } => project::scaffold(
            &name,
            lang,
            options.path.as_deref(),
            options.sdk_path.as_deref(),
        )?,
        Command::Agent { operation } => match operation {
            AgentCommand::New {
                id,
                name,
                game,
                options,
                ..
            } => {
                agent_project::create(
                    &id,
                    name.as_deref(),
                    &game,
                    options.path.as_deref(),
                    options.sdk_path.as_deref(),
                )
                .await?
            }
            AgentCommand::SetGame { game, manifest } => {
                agent_project::set_game(&manifest, &game).await?
            }
        },
        Command::Build { manifest } => {
            let manifest = project_manifest(manifest.as_deref())?;
            if agent_project::is_agent_manifest(manifest)? {
                AgentManifest::open(manifest)?.build().await?
            } else {
                Manifest::read(manifest)?.build()?
            }
        }
        Command::Check {
            package: Some(package),
            ..
        } => {
            let mut details =
                deployment::check_archive(&package, parlando::component::WasmMetering::Fuel)?;
            let message = match details["kind"].as_str() {
                Some("agent") => format!(
                    "Agent package check passed for {}.",
                    details["id"].as_str().unwrap_or("the policy")
                ),
                _ => format!(
                    "Package check passed for {}.",
                    details["game"].as_str().unwrap_or("the game")
                ),
            };
            details["package"] = json!(package);
            Report::new(message, details)
        }
        Command::Check {
            manifest,
            package: None,
        } => {
            let manifest = project_manifest(manifest.as_deref())?;
            if agent_project::is_agent_manifest(manifest)? {
                AgentManifest::open(manifest)?.check(false).await?
            } else {
                Manifest::read(manifest)?.check()?
            }
        }
        Command::Test { manifest } => {
            let manifest = project_manifest(manifest.as_deref())?;
            if agent_project::is_agent_manifest(manifest)? {
                AgentManifest::open(manifest)?.check(true).await?
            } else {
                Manifest::read(manifest)?.test()?
            }
        }
        Command::Run {
            manifest,
            experiment,
            package,
            agents,
        } => {
            let experiment = ExperimentFile::read(experiment).await?;
            let metering = experiment.wasm_metering();
            let mut factories = Vec::new();
            let (loaded_game, metadata) = match manifest {
                Some(path) if agent_project::is_agent_manifest(&path)? => {
                    let (resolved, factory) = AgentManifest::open(&path)?.for_run(metering).await?;
                    factories.push(factory);
                    (resolved.game, resolved.metadata)
                }
                Some(manifest) => Manifest::read(&manifest)?.load_with_metering(metering)?,
                None => deployment::load_package(&project::resolve_archive(package)?, metering)?,
            };
            for path in agents {
                let factory = if path
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"))
                {
                    deployment::load_agent_package(&path, &loaded_game, &metadata.id, metering)?
                } else {
                    parlando::agent::wasm::WasmAgentFactory::load_with_metering(
                        &path,
                        &loaded_game,
                        metering,
                    )?
                };
                factories.push(factory);
            }
            let mut runner =
                ExperimentRunner::new(metadata, loaded_game)?.agent(RemoteAgent::new())?;
            for factory in factories {
                runner = runner.agent(factory)?;
            }
            let summary = runner.run_file(experiment).await?;
            let report = Report::new(format!(
                "Run {} finished: {} planned, {} completed, {} failed, {} skipped.\nCheckpoints: {}\nResults: {}",
                summary.run_id, summary.planned, summary.completed, summary.failed, summary.skipped,
                summary.checkpoints, summary.output_directory.display()
            ), serde_json::to_value(&summary)?);
            return Ok((report, summary.failed == 0));
        }
        Command::Remote(command) => remote::execute(command).await?,
    };
    Ok((result, true))
}

/// Keeps subprocess diagnostics off stdout and maps errors to a stable JSON envelope.
#[tokio::main]
async fn main() -> ExitCode {
    let json_requested = std::env::args_os()
        .skip(1)
        .take_while(|arg| arg != "--")
        .any(|arg| arg == "--json");
    let args = match Args::try_parse() {
        Ok(args) => args,
        Err(error)
            if json_requested
                && !matches!(
                    error.kind(),
                    ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
                ) =>
        {
            println!(
                "{}",
                json!({"ok": false, "error": {"code": "invalid_arguments", "message": error.to_string()}})
            );
            return ExitCode::from(2);
        }
        Err(error) => error.exit(),
    };

    let name = args.command.name();
    match execute(args.command).await {
        Ok((result, ok)) => {
            if args.json {
                println!(
                    "{}",
                    json!({"ok": ok, "command": name, "result": result.data})
                );
            } else {
                println!("{}", result.message);
            }
            if ok {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(error) => {
            if args.json {
                println!(
                    "{}",
                    json!({"ok": false, "command": name, "error": {"code": "operation_failed", "message": format!("{error:#}")}})
                );
            } else {
                eprintln!("{error:#}");
            }
            ExitCode::FAILURE
        }
    }
}
