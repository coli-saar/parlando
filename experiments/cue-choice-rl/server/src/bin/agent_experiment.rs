use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use parlando::{
    agent::grpc::RemoteAgent, component::LoadedGame, ExperimentFile, ExperimentRunner, GameMetadata,
};
use parlando_cue_choice_rl::{CorrectnessReward, DealerAgentFactory};

/// Command-line input for one cue-choice headless experiment.
#[derive(Debug, Parser)]
#[command(name = "cue-choice-agent-experiment")]
struct Cli {
    /// Versioned Parlando agent-experiment YAML file.
    experiment: PathBuf,
    /// Built Cue Choice WebAssembly component.
    #[arg(long, conflicts_with = "package")]
    component: Option<PathBuf>,
    /// Game ZIP built by the Parlando CLI.
    #[arg(long)]
    package: Option<PathBuf>,
    /// Packaged dealer to use in place of the native dealer.
    #[arg(long)]
    agent: Option<PathBuf>,
}

/// Registers the compiled game, dealer, reward, and Python learner before execution.
#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let experiment = ExperimentFile::read(&cli.experiment).await?;
    let metering = experiment.wasm_metering();
    let (game, metadata) = if let Some(component) = cli.component {
        let game = LoadedGame::load_with_metering(&component, metering)?;
        let metadata = GameMetadata {
            id: "cue-choice".into(),
            name: "Cue Choice".into(),
            version: env!("CARGO_PKG_VERSION").parse()?,
            build_manifest: serde_json::json!({"component_sha256": game.digest()}),
        };
        (game, metadata)
    } else {
        let package = cli
            .package
            .unwrap_or_else(|| PathBuf::from("dist/cue-choice-0.1.0.zip"));
        let (game, metadata) = parlando::deployment::load_package(&package, metering)?;
        anyhow::ensure!(metadata.id == "cue-choice", "expected a Cue Choice package");
        (game, metadata)
    };
    let dealer = cli
        .agent
        .map(|path| parlando::deployment::load_agent_package(&path, &game, &metadata.id, metering))
        .transpose()?;
    let runner = ExperimentRunner::new(metadata, game)?;
    let runner = match dealer {
        Some(factory) => runner.agent(factory)?,
        None => runner.agent(DealerAgentFactory::new()?)?,
    };
    let summary = runner
        .rl_agent(RemoteAgent::new())?
        .reward(CorrectnessReward)?
        .run_file(experiment)
        .await?;
    println!("{}", serde_json::to_string_pretty(&summary)?);
    Ok(())
}
