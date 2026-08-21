use std::net::{IpAddr, SocketAddr};

use anyhow::Result;
use clap::Parser;
use parlando::{GameMetadata, Server};
use parlando_great_tree::{
    GreatTreeFactory, IdleAgentFactory, LlmAgentFactory, RootBotAgentFactory,
};

#[derive(Debug, Parser)]
#[command(name = "parlando-great-tree")]
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
        default_value = "sqlite:///./parlando-great-tree.sqlite"
    )]
    database_url: String,
    /// Compiled browser-client directory served for every experiment.
    #[arg(long, env = "PARLANDO_CLIENT_DIST", default_value = "./client/dist")]
    client_dist: String,
}

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
    Server::new(GreatTreeFactory, metadata)?
        .database_url(cli.database_url)
        .participant_app(cli.client_dist)
        .agent(IdleAgentFactory)?
        .agent(LlmAgentFactory)?
        .agent(RootBotAgentFactory)?
        .serve(SocketAddr::new(cli.host, cli.port))
        .await
}
