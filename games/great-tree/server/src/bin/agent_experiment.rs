use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use parlando::{ExperimentRunner, GameMetadata};
use parlando_great_tree::{
    GreatTreeFactory, IdleAgentFactory, LlmAgentFactory, RootBotAgentFactory,
};

/// Command-line arguments for one headless Great Tree agent experiment.
#[derive(Debug, Parser)]
#[command(name = "parlando-great-tree-agent-experiment")]
struct Cli {
    /// Versioned YAML experiment specification.
    experiment: PathBuf,
}

/// Registers the compiled Great Tree implementations and executes the YAML run.
#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let cli = Cli::parse();
    let metadata = GameMetadata {
        id: "great-tree".to_string(),
        name: "The Great Tree".to_string(),
        version: semver::Version::parse(env!("CARGO_PKG_VERSION"))?,
        build_manifest: serde_json::json!({
            "name": env!("CARGO_PKG_NAME"),
            "version": env!("CARGO_PKG_VERSION"),
        }),
    };
    let summary = ExperimentRunner::new(metadata, GreatTreeFactory)?
        .agent(IdleAgentFactory)?
        .agent(LlmAgentFactory)?
        .agent(RootBotAgentFactory)?
        .run_yaml(cli.experiment)
        .await?;
    println!("{}", serde_json::to_string_pretty(&summary)?);
    Ok(())
}
