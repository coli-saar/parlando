use std::{
    net::{IpAddr, SocketAddr},
    path::PathBuf,
};

use anyhow::Result;
use clap::Parser;
use parlando_server::{serve, ExperimentConfig, ServeOptions};
use parlando_space_game::{
    agents::{available_agent_options, factory_from_config},
    SpaceGameAdapter,
};
use serde_json::{json, Value};

#[derive(Debug, Parser)]
#[command(name = "parlando-space-game")]
struct Cli {
    #[arg(long, short)]
    config: Option<PathBuf>,
    #[arg(long, default_value = "127.0.0.1")]
    host: IpAddr,
    #[arg(long)]
    port: Option<u16>,
    #[arg(long)]
    experiment_id: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let cli = Cli::parse();
    let mut config = if let Some(path) = cli.config {
        ExperimentConfig::from_yaml(path)?
    } else {
        ExperimentConfig::default()
    };
    if cli.experiment_id.is_some() {
        config.experiment.id = cli.experiment_id;
    }
    let port = cli
        .port
        .or_else(|| {
            std::env::var("PORT")
                .ok()
                .and_then(|value| value.parse().ok())
        })
        .unwrap_or(8000);
    let agent_factory = factory_from_config(&config)?;
    let adapter = SpaceGameAdapter::new();
    serve(
        adapter,
        config,
        SocketAddr::new(cli.host, port),
        ServeOptions {
            agent_factory,
            game_version_manifest: Some(space_game_version_manifest()),
            admin_agent_options: available_agent_options(),
            ..ServeOptions::default()
        },
    )
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
