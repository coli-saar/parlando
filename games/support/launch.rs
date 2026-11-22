//! Starts the shared component runtime with opt-in, game-specific participant agents.

use std::{net::IpAddr, path::PathBuf};

use anyhow::Result;
use clap::Parser;
use parlando::{
    component::{LoadedGame, WasmMetering},
    GameMetadata, Server,
};

/// Inputs for the optional host binary that registers the authored agents.
#[derive(Parser)]
pub(crate) struct Launch {
    /// Built component from a Parlando game package.
    #[arg(long)]
    component: PathBuf,
    /// Guest execution budget for the local installation.
    #[arg(long, value_enum, default_value = "fuel")]
    wasm_metering: WasmMetering,
    /// Compiled participant browser application.
    #[arg(long, env = "PARLANDO_CLIENT_DIST")]
    client_dist: PathBuf,
    /// SQLite URL for this component installation.
    #[arg(long, env = "PARLANDO_DATABASE_URL")]
    database_url: String,
    /// Listener interface.
    #[arg(long, default_value = "127.0.0.1")]
    pub(crate) host: IpAddr,
    /// Listener port.
    #[arg(long, env = "PORT", default_value_t = 8080)]
    pub(crate) port: u16,
    /// Public origin when serving behind a reverse proxy.
    #[arg(long, env = "PARLANDO_PUBLIC_URL")]
    public_url: Option<String>,
}

impl Launch {
    /// Opens the component and binds its verified digest to the game identity.
    pub(crate) fn server(&self, id: &str, name: &str, version: &str) -> Result<Server> {
        let game = LoadedGame::load_with_metering(&self.component, self.wasm_metering)?;
        let metadata = GameMetadata {
            id: id.into(),
            name: name.into(),
            version: version.parse()?,
            build_manifest: serde_json::json!({"component_sha256": game.digest()}),
        };
        let mut server = Server::new(game, metadata)?
            .database_url(self.database_url.clone())
            .participant_app(self.client_dist.clone());
        if let Some(origin) = &self.public_url {
            server = server.public_url(origin.clone());
        }
        Ok(server)
    }
}
