//! Optional launcher for the Space Game component and its authored agent policies.

use anyhow::Result;
use clap::Parser;
use parlando_space_game::BackAndForthAgentFactory;

#[path = "../../../../games/support/launch.rs"]
mod launch;

/// Runs the shared host with the local agent factories registered.
#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let args = launch::Launch::parse();
    args.server("space-game", "Space Game", env!("CARGO_PKG_VERSION"))?
        .agent(BackAndForthAgentFactory::new()?)?
        .serve((args.host, args.port).into())
        .await
}
