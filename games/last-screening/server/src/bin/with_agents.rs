//! Optional launcher for The Last Screening component and its authored agent policies.

use anyhow::Result;
use clap::Parser;
use parlando_last_screening::ScreeningPartnerFactory;

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
        "last-screening",
        "The Last Screening",
        env!("CARGO_PKG_VERSION"),
    )?
    .agent(ScreeningPartnerFactory::new()?)?
    .serve((args.host, args.port).into())
    .await
}
