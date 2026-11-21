//! Optional launcher for the ['Space Game', 'The Great Tree', 'Counterweight Hotel', 'The Last Screening'] component and its authored agent policies.

use anyhow::Result;
use clap::Parser;
use parlando_great_tree::{IdleAgentFactory, LlmAgentFactory, RootBotAgentFactory};

#[path = "../../../../support/launch.rs"]
mod launch;

/// Runs the shared host with the local agent factories registered.
#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let args = launch::Launch::parse();
    args.server(
        "great-tree",
        "['Space Game', 'The Great Tree', 'Counterweight Hotel', 'The Last Screening']",
        env!("CARGO_PKG_VERSION"),
    )?
    .agent(IdleAgentFactory)?
    .agent(LlmAgentFactory)?
    .agent(RootBotAgentFactory)?
    .serve((args.host, args.port).into())
    .await
}
