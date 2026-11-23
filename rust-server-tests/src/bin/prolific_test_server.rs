//! Minimal WebAssembly game used only by the standalone Prolific test runner.

use std::{net::SocketAddr, path::PathBuf};

use anyhow::Result;
use clap::Parser;
use parlando::{component::LoadedGame, GameMetadata, Server};
use serde_json::json;

/// Server-process arguments supplied by the independent scenario runner.
#[derive(Debug, Parser)]
#[command(about = "Dummy Parlando game server for Prolific integration tests")]
struct Args {
    /// Loopback address exposed to the test runner and signed launch audience.
    #[arg(long)]
    bind: SocketAddr,
    /// Fresh SQLite file used only for this test run.
    #[arg(long)]
    database: PathBuf,
}

/// Starts a production Parlando server around the test-only WebAssembly game.
#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    if !args.bind.ip().is_loopback() {
        anyhow::bail!("prolific-test-server may listen only on a loopback address");
    }
    let metadata = GameMetadata {
        id: "prolific-integration-fixture".to_string(),
        name: "Prolific Integration Fixture".to_string(),
        version: semver::Version::new(1, 0, 0),
        build_manifest: json!({"purpose": "prolific-integration-testing"}),
    };
    Server::new(
        LoadedGame::from_bytes(include_bytes!(
            "../../stress-fixture/embedded/contract.wasm"
        ))?,
        metadata,
    )?
    .database_url(format!("sqlite:///{}", args.database.display()))
    .public_url(format!("http://{}", args.bind))
    .serve(args.bind)
    .await
}
