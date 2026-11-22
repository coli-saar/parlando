//! CLI entry points for deploying game packages and operating a deployment host.

use std::{collections::BTreeMap, env, net::SocketAddr, path::PathBuf};

use super::{project, Report};
use anyhow::{ensure, Context, Result};
use clap::Subcommand;
use parlando::deployment::{self, credentials, HostOptions};
use serde_json::{json, Value};

/// Deployment commands exposed at the main Parlando CLI level.
#[derive(Subcommand)]
pub(super) enum RemoteCommand {
    /// Store an upload token in the system credential store.
    Login {
        /// Parlando server URL (e.g. http://127.0.0.1:8000) or saved server alias.
        #[arg(long, alias = "host", value_name = "URL_OR_ALIAS")]
        server: String,
        /// Game ID this credential is authorized to publish.
        #[arg(long)]
        game: String,
    },
    /// Upload a built game package to a deployment host.
    Deploy {
        /// Parlando server URL (e.g. http://127.0.0.1:8000) or saved server alias.
        #[arg(long, alias = "server", value_name = "URL_OR_ALIAS")]
        host: String,
        /// ZIP archive produced by parlando build, defaulting to the game ID and version in game.yaml.
        #[arg(long)]
        package: Option<PathBuf>,
    },
    /// Start a deployment host for games and their dashboards.
    ///
    /// With the default loopback origins, open the dashboard link printed at startup.
    /// To set the dashboard origin explicitly, pass --admin-url or set PARLANDO_ADMIN_URL.
    /// For an explicit dashboard origin or upload grant, set PARLANDO_HOST_ADMIN_TOKEN.
    /// To grant CLI upload access, pass --grants or set PARLANDO_DEPLOY_GAME and
    /// PARLANDO_DEPLOY_TOKEN for one game.
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
        /// Dashboard origin on a different hostname from the participant site.
        #[arg(long)]
        admin_url: Option<String>,
        /// Path to a JSON file mapping game IDs to independent bearer tokens.
        #[arg(long)]
        grants: Option<PathBuf>,
    },
    /// Enable participant access to a deployed game.
    Activate(GameTarget),
    /// Disable participant access to a game once its experiments have stopped.
    Deactivate(GameTarget),
}

/// Identifies a deployed game and the host responsible for its availability.
#[derive(clap::Args)]
pub(super) struct GameTarget {
    /// Parlando server URL (e.g. http://127.0.0.1:8000) or saved server alias.
    #[arg(long, alias = "server", value_name = "URL_OR_ALIAS")]
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
        RemoteCommand::Login { server, game } => {
            credentials::login(&server, &game)?;
            let origin = credentials::resolve_host(&server)?;
            Ok(Report::new(
                format!("Saved upload token for {game} on {origin}."),
                json!({"server": origin, "game": game}),
            ))
        }
        RemoteCommand::Deploy { host, package } => {
            let package = project::resolve_archive(package)?;
            let origin = credentials::resolve_host(&host)?;
            let receipt = deployment::deploy_with_saved_credentials(&origin, &package).await?;
            Ok(Report::new(
                format!(
                    "Deployed {} {} to {origin}.\nRevision: {}\nComponent SHA-256: {}",
                    receipt.game, receipt.version, receipt.revision, receipt.component_sha256
                ),
                json!({
                    "server": origin,
                    "game": receipt.game,
                    "revision": receipt.revision,
                    "version": receipt.version,
                    "component_sha256": receipt.component_sha256,
                }),
            ))
        }
        RemoteCommand::Serve {
            wasm_metering,
            bind,
            store,
            public_url,
            admin_url,
            grants,
        } => {
            let listener = tokio::net::TcpListener::bind(bind)
                .await
                .with_context(|| format!("binding deployment host to {bind}"))?;
            let bind = listener.local_addr()?;
            let use_local_dashboard = bind.ip().is_loopback() && public_url.is_none();
            let public_url = public_url.unwrap_or_else(|| format!("http://{bind}"));
            let admin_url = admin_url.or_else(|| env::var("PARLANDO_ADMIN_URL").ok());
            let grants = if let Some(path) = grants {
                serde_json::from_slice(
                    &std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?,
                )
                .context("expected a JSON object mapping game IDs to deployment tokens")?
            } else if env::var_os("PARLANDO_DEPLOY_GAME").is_some()
                || env::var_os("PARLANDO_DEPLOY_TOKEN").is_some()
            {
                BTreeMap::from([(
                    required_env("PARLANDO_DEPLOY_GAME")?,
                    required_env("PARLANDO_DEPLOY_TOKEN")?,
                )])
            } else {
                BTreeMap::new()
            };
            let management_configured = !grants.is_empty()
                || admin_url.is_some()
                || env::var_os("PARLANDO_HOST_ADMIN_TOKEN").is_some();
            let admin_token = management_configured
                .then(|| required_env("PARLANDO_HOST_ADMIN_TOKEN"))
                .transpose()?;
            let local_dashboard = use_local_dashboard && admin_url.is_none();
            let admin_url = admin_url
                .or_else(|| local_dashboard.then(|| format!("http://localhost:{}", bind.port())));
            let store = std::path::absolute(store)?;
            let store_display = store.display().to_string();
            let options = HostOptions {
                store,
                public_url,
                grants,
                admin_token,
            };
            let dashboard_url = admin_url
                .as_ref()
                .map(|origin| format!("{}/admin", origin.trim_end_matches('/')));
            let ready = || {
                eprintln!("Parlando deployment host listening on {bind}\nStore: {store_display}");
                if let Some(url) = &dashboard_url {
                    eprintln!("Administrator dashboard: {url}");
                } else if !management_configured {
                    eprintln!(
                        "Uploads and experiment management are disabled (no credentials configured)."
                    );
                }
            };
            match admin_url {
                Some(origin) if local_dashboard => {
                    deployment::serve_local_dashboard_on_listener(
                        listener,
                        options,
                        origin,
                        wasm_metering,
                        ready,
                    )
                    .await?;
                }
                origin => {
                    deployment::serve_on_listener(listener, options, origin, wasm_metering, ready)
                        .await?;
                }
            }
            Ok(Report::new(
                format!("Deployment host on {bind} stopped."),
                json!({"status": "stopped", "bind": bind}),
            ))
        }
        RemoteCommand::Activate(target) => change_game_availability(target, true).await,
        RemoteCommand::Deactivate(target) => change_game_availability(target, false).await,
    }
}

/// Applies the requested availability to the game using the host administrator credential.
async fn change_game_availability(target: GameTarget, active: bool) -> Result<Report> {
    deployment::validate_game_id(&target.game)?;
    let origin = credentials::resolve_host(&target.host)?;
    let token = admin_token(&origin)?;
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

/// Retrieves an explicitly configured secret without exposing its value in a failure.
fn required_env(key: &str) -> Result<String> {
    let value = env::var(key).with_context(|| format!("set {key}"))?;
    ensure!(!value.trim().is_empty(), "{key} is empty");
    Ok(value)
}

/// Ensures an ambient operator token is only sent to its configured origin.
fn admin_token(origin: &str) -> Result<String> {
    let expected = required_env("PARLANDO_HOST_ADMIN_ORIGIN")?;
    ensure!(
        deployment::checked_origin(&expected)? == origin,
        "administrator credential belongs to another server"
    );
    required_env("PARLANDO_HOST_ADMIN_TOKEN")
}
