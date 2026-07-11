use std::{
    net::{IpAddr, SocketAddr},
    path::PathBuf,
};

use anyhow::Result;
use clap::Parser;
use parlando_server::{serve, ExperimentConfig, ServeOptions};
use parlando_space_game::{agents::factory_from_config, SpaceGameAdapter};

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
            ..ServeOptions::default()
        },
    )
    .await
}
