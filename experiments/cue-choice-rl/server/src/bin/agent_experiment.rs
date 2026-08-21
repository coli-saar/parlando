use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use parlando::{agent::grpc::RemoteAgent, ExperimentRunner, GameMetadata};
use parlando_cue_choice_rl::{CorrectnessReward, CueChoiceFactory, DealerAgentFactory};

/// Command-line input for one cue-choice headless experiment.
#[derive(Debug, Parser)]
#[command(name = "cue-choice-agent-experiment")]
struct Cli {
    /// Versioned Parlando agent-experiment YAML file.
    experiment: PathBuf,
}

/// Registers the compiled game, dealer, reward, and Python learner before execution.
#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let metadata = GameMetadata {
        id: "cue-choice".to_string(),
        name: "Cue Choice".to_string(),
        version: semver::Version::parse(env!("CARGO_PKG_VERSION"))?,
        build_manifest: serde_json::json!({
            "name": env!("CARGO_PKG_NAME"),
            "version": env!("CARGO_PKG_VERSION"),
        }),
    };
    let summary = ExperimentRunner::new(metadata, CueChoiceFactory)?
        .agent(DealerAgentFactory)?
        .rl_agent(RemoteAgent::new())?
        .reward(CorrectnessReward)?
        .run_yaml(cli.experiment)
        .await?;
    println!("{}", serde_json::to_string_pretty(&summary)?);
    Ok(())
}
