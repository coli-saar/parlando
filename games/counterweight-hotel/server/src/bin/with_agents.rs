//! Optional launcher for the Counterweight Hotel component and its authored agent policies.

use anyhow::Result;
use clap::Parser;
use parlando_counterweight_hotel::PorterFactory;

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
        "counterweight-hotel",
        "Counterweight Hotel",
        env!("CARGO_PKG_VERSION"),
    )?
    .agent(PorterFactory::new()?)?
    .serve((args.host, args.port).into())
    .await
}
