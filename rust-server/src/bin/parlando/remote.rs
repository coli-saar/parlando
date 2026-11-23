//! CLI entry points for deploying game and agent packages and operating a deployment host.

use std::{env, net::SocketAddr, path::PathBuf};

use super::{project, Report};
use anyhow::{Context, Result};
use clap::Subcommand;
use parlando::deployment::{self, credentials, HostOptions};
use serde_json::{json, Value};

/// Deployment commands exposed at the main Parlando CLI level.
#[derive(Subcommand)]
pub(super) enum RemoteCommand {
    /// Sign in through your browser to deploy packages to a server.
    Login {
        /// Parlando server URL (e.g. http://127.0.0.1:8000) or alias from ~/.parlando/servers.yaml.
        #[arg(
            long,
            alias = "host",
            value_name = "URL_OR_ALIAS",
            default_value = "default"
        )]
        server: String,
        /// Print the browser sign-in link without opening it automatically.
        #[arg(long)]
        no_browser: bool,
    },
    /// Upload a game or agent ZIP to a deployment host.
    Deploy {
        /// Parlando server URL (e.g. http://127.0.0.1:8000) or alias from ~/.parlando/servers.yaml.
        #[arg(
            long,
            alias = "server",
            value_name = "URL_OR_ALIAS",
            default_value = "default"
        )]
        host: String,
        /// Game or agent ZIP produced by parlando build. Defaults to the game release in game.yaml.
        #[arg(value_name = "ZIP")]
        archive: Option<PathBuf>,
        /// Supply the ZIP as a named option.
        #[arg(long, value_name = "ZIP", conflicts_with = "archive")]
        package: Option<PathBuf>,
    },
    /// Start a deployment host for games and their dashboards.
    ///
    /// With the default loopback origins, open the dashboard link printed at startup.
    /// To set the dashboard origin explicitly, pass --admin-url or set PARLANDO_ADMIN_URL.
    Serve {
        /// Local TCP address, typically behind an HTTPS reverse proxy.
        #[arg(long, default_value = "127.0.0.1:8000")]
        bind: SocketAddr,
        /// Guest execution budget for every package on this host.
        #[arg(long, value_enum, default_value = "fuel")]
        wasm_metering: parlando::component::WasmMetering,
        /// Durable package, experiment, and database directory, relative to the working directory.
        #[arg(long, default_value = ".")]
        store: PathBuf,
        /// Externally visible origin used to construct participant URLs.
        #[arg(long)]
        public_url: Option<String>,
        /// Dashboard origin. Use the public origin for trusted games or a separate dashboard hostname.
        #[arg(long)]
        admin_url: Option<String>,
    },
    /// Enable participant access to a deployed game.
    Activate(GameTarget),
    /// Disable participant access to a game once its experiments have stopped.
    Deactivate(GameTarget),
}

/// Identifies a deployed game and the host responsible for its availability.
#[derive(clap::Args)]
pub(super) struct GameTarget {
    /// Parlando server URL (e.g. http://127.0.0.1:8000) or alias from ~/.parlando/servers.yaml.
    #[arg(
        long,
        alias = "server",
        value_name = "URL_OR_ALIAS",
        default_value = "default"
    )]
    host: String,
    /// Game ID declared in the deployed ZIP.
    #[arg(long)]
    game: String,
}

impl RemoteCommand {
    /// Returns the stable JSON command identifier.
    pub(super) fn name(&self) -> &'static str {
        match self {
            Self::Login { .. } => "login",
            Self::Deploy { .. } => "deploy",
            Self::Serve { .. } => "serve",
            Self::Activate(_) => "activate",
            Self::Deactivate(_) => "deactivate",
        }
    }
}

/// Runs a remote deployment operation and returns the result for CLI formatting.
pub(super) async fn execute(command: RemoteCommand) -> Result<Report> {
    match command {
        RemoteCommand::Login { server, no_browser } => {
            let origin = credentials::resolve_host(&server)?;
            credentials::login(&origin, !no_browser).await?;
            Ok(Report::new(
                format!("Signed in to {origin}."),
                json!({"server": origin}),
            ))
        }
        RemoteCommand::Deploy {
            host,
            archive,
            package,
        } => {
            let package = project::resolve_archive(archive.or(package))?;
            let origin = credentials::resolve_host(&host)?;
            let receipt = deployment::deploy_with_saved_credentials(&origin, &package).await?;
            let mut data = serde_json::to_value(&receipt)?;
            data["server"] = json!(origin);
            let package_name = match receipt.agent.as_deref() {
                Some(agent) => format!("{agent} for {}", receipt.game),
                None => receipt.game.clone(),
            };
            Ok(Report::new(
                format!(
                    "Deployed {} {} to {origin}.\nRevision: {}\nComponent SHA-256: {}",
                    package_name, receipt.version, receipt.revision, receipt.component_sha256
                ),
                data,
            ))
        }
        RemoteCommand::Serve {
            wasm_metering,
            bind,
            store,
            public_url,
            admin_url,
        } => {
            let listener = tokio::net::TcpListener::bind(bind)
                .await
                .with_context(|| format!("binding deployment host to {bind}"))?;
            let bind = listener.local_addr()?;
            let use_local_dashboard = bind.ip().is_loopback() && public_url.is_none();
            let public_url = public_url.unwrap_or_else(|| format!("http://{bind}"));
            let admin_url = admin_url.or_else(|| env::var("PARLANDO_ADMIN_URL").ok());
            let admin_url = admin_url.or_else(|| {
                use_local_dashboard.then(|| format!("http://localhost:{}", bind.port()))
            });
            let store = std::path::absolute(store)?;
            let store_display = store.display().to_string();
            let options = HostOptions { store, public_url };
            let dashboard_url = admin_url
                .as_ref()
                .map(|origin| format!("{}/admin", origin.trim_end_matches('/')));
            let ready = || {
                eprintln!("Parlando deployment host listening on {bind}\nStore: {store_display}");
                if let Some(url) = &dashboard_url {
                    eprintln!("Administrator dashboard: {url}");
                } else {
                    eprintln!("Set --admin-url to enable dashboard sign-in and game uploads.");
                }
            };
            deployment::serve_on_listener(listener, options, admin_url, wasm_metering, ready)
                .await?;
            Ok(Report::new(
                format!("Deployment host on {bind} stopped."),
                json!({"status": "stopped", "bind": bind}),
            ))
        }
        RemoteCommand::Activate(target) => change_game_availability(target, true).await,
        RemoteCommand::Deactivate(target) => change_game_availability(target, false).await,
    }
}

/// Applies the requested availability to the game using the signed-in account.
async fn change_game_availability(target: GameTarget, active: bool) -> Result<Report> {
    deployment::validate_game_id(&target.game)?;
    let origin = credentials::resolve_host(&target.host)?;
    let token = credentials::upload_token(&origin)?;
    let action = if active { "activate" } else { "deactivate" };
    let response: Value = deployment::control_request(
        &origin,
        "POST",
        &format!("games/{}/{action}", target.game),
        &token,
        None::<&Value>,
    )
    .await?;
    let status = if active { "active" } else { "inactive" };
    Ok(Report::new(
        format!("Game {} is {status}.", target.game),
        response,
    ))
}
