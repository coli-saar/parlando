//! Author, validate, build, and run a component game from a local project.

#[path = "parlando/project.rs"]
mod project;
#[path = "parlando/remote.rs"]
mod remote;
#[path = "parlando/scenarios.rs"]
mod scenarios;

use anyhow::{ensure, Result};
use clap::{error::ErrorKind, Parser, Subcommand, ValueEnum};
use parlando::{
    agent::grpc::RemoteAgent, component::WasmMetering, ExperimentFile, ExperimentRunner, Server,
};
use project::Manifest;
use serde_json::{json, Value};
use std::{net::SocketAddr, path::PathBuf, process::ExitCode};

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

/// Selects an operation; project commands default to the current project's game.yaml.
#[derive(Subcommand)]
enum Command {
    /// Create a playable Parlando game.
    New {
        /// Lowercase game ID and new directory name.
        name: String,
        /// Language of the generated game.
        #[arg(long, value_enum, default_value = "rust")]
        lang: Language,
        /// Destination directory, defaulting to the game ID.
        #[arg(long)]
        path: Option<PathBuf>,
        /// Local game SDK checkout for the selected language.
        #[arg(long)]
        sdk_path: Option<PathBuf>,
    },
    /// Compile the game and stage a deployable package under dist/parlando.
    Build {
        #[arg(long, default_value = "game.yaml")]
        manifest: PathBuf,
    },
    /// Check the project and verify that the game can load.
    Check {
        #[arg(long, default_value = "game.yaml")]
        manifest: PathBuf,
    },
    /// Run native tests and the game scenarios in game.yaml.
    Test {
        #[arg(long, default_value = "game.yaml")]
        manifest: PathBuf,
    },
    /// Start a live game installation and its administrative dashboard.
    Serve {
        #[arg(long, default_value = "game.yaml")]
        manifest: PathBuf,
        /// Guest execution budget for this installation.
        #[arg(long, value_enum, default_value = "fuel")]
        wasm_metering: WasmMetering,
        /// Listener address, including the port.
        #[arg(long, default_value = "127.0.0.1:8000")]
        bind: SocketAddr,
        /// SQLite database URL for this installation.
        #[arg(long)]
        database_url: Option<String>,
        /// Override the package's compiled participant application.
        #[arg(long)]
        client_dist: Option<PathBuf>,
        /// Public origin when the service is behind a proxy.
        #[arg(long)]
        public_url: Option<String>,
    },
    /// Run or resume an agent–agent evaluation.
    Run {
        /// Agent experiment YAML.
        experiment: PathBuf,
        #[arg(long, default_value = "game.yaml")]
        manifest: PathBuf,
    },
    #[command(flatten)]
    Remote(remote::RemoteCommand),
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
            Self::Build { .. } => "build",
            Self::Check { .. } => "check",
            Self::Test { .. } => "test",
            Self::Serve { .. } => "serve",
            Self::Run { .. } => "run",
            Self::Remote(command) => command.name(),
        }
    }
}

/// Returns the result payload and whether a completed evaluation succeeded.
async fn execute(command: Command) -> Result<(Value, bool)> {
    let result = match command {
        Command::New {
            name,
            lang,
            path,
            sdk_path,
        } => project::scaffold(&name, lang, path.as_deref(), sdk_path.as_deref())?,
        Command::Build { manifest } => Manifest::read(&manifest)?.build()?,
        Command::Check { manifest } => Manifest::read(&manifest)?.check()?,
        Command::Test { manifest } => Manifest::read(&manifest)?.test()?,
        Command::Serve {
            manifest,
            bind,
            wasm_metering,
            database_url,
            client_dist,
            public_url,
        } => {
            let project = Manifest::read(&manifest)?;
            let (loaded_game, metadata) = project.load_with_metering(wasm_metering)?;
            let mut server = Server::new(loaded_game, metadata)?;
            if let Some(database_url) = database_url {
                server = server.database_url(database_url);
            }
            if let Some(client_dist) = client_dist.or_else(|| project.client_dist()) {
                ensure!(
                    client_dist.join("index.html").is_file(),
                    "participant app {} requires an index.html",
                    client_dist.display()
                );
                server = server.participant_app(client_dist);
            }
            if let Some(public_url) = public_url {
                server = server.public_url(public_url);
            }
            eprintln!("Starting Parlando on {bind}");
            server.serve(bind).await?;
            json!({"bind": bind})
        }
        Command::Run {
            manifest,
            experiment,
        } => {
            let experiment = ExperimentFile::read(experiment).await?;
            let (loaded_game, metadata) =
                Manifest::read(&manifest)?.load_with_metering(experiment.wasm_metering())?;
            let summary = ExperimentRunner::new(metadata, loaded_game)?
                .agent(RemoteAgent::new())?
                .run_file(experiment)
                .await?;
            return Ok((serde_json::to_value(&summary)?, summary.failed == 0));
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
                println!("{}", json!({"ok": ok, "command": name, "result": result}));
            } else {
                println!("{result:#}");
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
