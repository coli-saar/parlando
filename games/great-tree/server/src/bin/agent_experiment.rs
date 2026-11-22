use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use parlando::{
    agent::grpc::RemoteAgent, component::LoadedGame, ExperimentFile, ExperimentRunner, GameMetadata,
};
use parlando_great_tree::{IdleAgentFactory, LlmAgentFactory, RootBotAgentFactory};

/// Command-line arguments for one headless Great Tree agent experiment.
#[derive(Debug, Parser)]
#[command(name = "parlando-great-tree-agent-experiment")]
struct Cli {
    /// Versioned YAML experiment specification.
    experiment: PathBuf,
    /// Built Great Tree component from the game package.
    #[arg(long, default_value = "dist/parlando/game.wasm")]
    component: PathBuf,
}

/// Registers the compiled Great Tree implementations and executes the YAML run.
#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let cli = Cli::parse();
    let experiment = ExperimentFile::read(&cli.experiment).await?;
    let game = LoadedGame::load_with_metering(&cli.component, experiment.wasm_metering())?;
    let metadata = GameMetadata {
        id: "great-tree".to_string(),
        name: "The Great Tree".to_string(),
        version: env!("CARGO_PKG_VERSION").parse()?,
        build_manifest: serde_json::json!({
            "name": env!("CARGO_PKG_NAME"),
            "version": env!("CARGO_PKG_VERSION"),
            "component_sha256": game.digest(),
        }),
    };
    let summary = ExperimentRunner::new(metadata, game)?
        .agent(IdleAgentFactory)?
        .agent(LlmAgentFactory)?
        .agent(RootBotAgentFactory)?
        .agent(RemoteAgent::new())?
        .run_file(experiment)
        .await?;
    println!("{}", serde_json::to_string_pretty(&summary)?);
    Ok(())
}
