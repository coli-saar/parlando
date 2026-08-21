use std::net::{IpAddr, SocketAddr};

use anyhow::Result;
use clap::Parser;
use parlando::{agent::grpc::RemoteAgent, GameMetadata, Server};
use parlando_space_game::{BackAndForthAgentFactory, SpaceGameFactory};
use serde_json::{json, Value};

#[derive(Debug, Parser)]
#[command(name = "parlando-space-game")]
struct Cli {
    #[arg(long, default_value = "127.0.0.1")]
    host: IpAddr,
    /// Explicit listener port; hosting platforms may provide the `PORT` environment variable.
    #[arg(long, env = "PORT")]
    port: u16,
    /// Durable SQLite location needed before dashboard-owned settings can be loaded.
    #[arg(
        long,
        env = "PARLANDO_DATABASE_URL",
        default_value = "sqlite:///./parlando-space-game.sqlite"
    )]
    database_url: String,
    /// Compiled browser-client directory served for every experiment.
    #[arg(long, env = "PARLANDO_CLIENT_DIST", default_value = "./client/dist")]
    client_dist: String,
}

/// Starts one compiled Space Game host with database-backed experiment runtimes.
#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let cli = Cli::parse();
    let build_manifest = space_game_version_manifest();
    let descriptor = GameMetadata {
        id: "space-game".to_string(),
        name: "Space Game".to_string(),
        version: semver::Version::parse(env!("CARGO_PKG_VERSION"))?,
        build_manifest: build_manifest.clone(),
    };
    Server::new(SpaceGameFactory, descriptor)?
        .database_url(cli.database_url)
        .participant_app(cli.client_dist)
        .agent(BackAndForthAgentFactory)?
        .agent(RemoteAgent::new())?
        .serve(SocketAddr::new(cli.host, cli.port))
        .await
}

fn space_game_version_manifest() -> Value {
    let cargo_toml = include_str!("../Cargo.toml");
    let client_package = include_str!("../../client/package.json");
    let local_warnings = local_dependency_warnings(env!("CARGO_MANIFEST_DIR"), cargo_toml);
    let client_package_json: Value = serde_json::from_str(client_package).unwrap_or(Value::Null);
    json!({
        "name": env!("CARGO_PKG_NAME"),
        "version": env!("CARGO_PKG_VERSION"),
        "build_time": option_env!("PARLANDO_SPACE_GAME_BUILD_TIME"),
        "git_sha": option_env!("PARLANDO_SPACE_GAME_GIT_SHA"),
        "git_dirty": option_env!("PARLANDO_SPACE_GAME_GIT_DIRTY").unwrap_or("unknown"),
        "client": {
            "name": client_package_json.get("name").and_then(Value::as_str).unwrap_or("parlando-space-game-client"),
            "version": client_package_json.get("version").and_then(Value::as_str).unwrap_or("unknown"),
            "build_time": option_env!("PARLANDO_SPACE_GAME_BUILD_TIME"),
            "git_sha": option_env!("PARLANDO_SPACE_GAME_GIT_SHA"),
            "git_dirty": option_env!("PARLANDO_SPACE_GAME_GIT_DIRTY").unwrap_or("unknown"),
            "package": "@coli-saar/parlando-client",
            "package_version": client_package_json
                .get("dependencies")
                .and_then(|deps| deps.get("@coli-saar/parlando-client"))
                .and_then(Value::as_str)
                .unwrap_or("unknown"),
        },
        "local_dependency_warnings": local_warnings,
        "warnings": local_warnings,
    })
}

fn local_dependency_warnings(manifest_dir: &str, cargo_toml: &str) -> Vec<Value> {
    cargo_toml
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            !trimmed.starts_with('#') && trimmed.contains("path") && trimmed.contains('=')
        })
        .map(|line| {
            json!({
                "level": "warning",
                "message": "Game is linked against a local development dependency; use a published package or pinned Git revision for reproducibility.",
                "manifest_dir": manifest_dir,
                "dependency": line.trim(),
            })
        })
        .collect()
}
