//! CLI entry points for deploying immutable packages and operating a deployment host.

use std::{collections::BTreeMap, env, net::SocketAddr, path::PathBuf};

use anyhow::{ensure, Context, Result};
use clap::Subcommand;
use parlando::deployment::{self, credentials, ExperimentSelection, HostOptions};
use serde_json::{json, Value};

/// Deployment commands exposed at the main Parlando CLI level.
#[derive(Subcommand)]
pub(super) enum RemoteCommand {
    /// Store an upload token in the system credential store.
    Login {
        /// Deployment host origin or saved server alias.
        #[arg(long, alias = "host")]
        server: String,
        /// Game ID this credential is authorized to publish.
        #[arg(long)]
        game: String,
    },
    /// Upload a built game package to a deployment host.
    Deploy {
        /// Deployment host origin or saved server alias.
        #[arg(long, alias = "server")]
        host: String,
        /// Package directory produced by parlando build.
        #[arg(long, default_value = "dist/parlando")]
        package: PathBuf,
    },
    /// Run a long-lived deployment and participant host.
    Host {
        /// Local TCP address, typically behind an HTTPS reverse proxy.
        #[arg(long, default_value = "127.0.0.1:8000")]
        bind: SocketAddr,
        /// Guest execution budget for every package on this host.
        #[arg(long, value_enum, default_value = "fuel")]
        wasm_metering: parlando::component::WasmMetering,
        /// Durable package, experiment, and database directory.
        #[arg(long)]
        store: PathBuf,
        /// Externally visible origin used to construct participant URLs.
        #[arg(long)]
        public_url: Option<String>,
        /// HTTPS dashboard origin on a different hostname from the participant site.
        #[arg(long)]
        admin_url: Option<String>,
        /// JSON object mapping game IDs to independent bearer tokens.
        #[arg(long)]
        grants: Option<PathBuf>,
    },
    /// Pin an experiment to one deployed revision on a running host.
    Select {
        /// Deployment host origin or saved server alias.
        #[arg(long, alias = "server")]
        host: String,
        /// New experiment ID used for the permanent participant URL.
        #[arg(long)]
        experiment: String,
        /// Game ID printed by parlando deploy.
        #[arg(long)]
        game: String,
        /// Exact immutable revision printed by parlando deploy.
        #[arg(long)]
        revision: String,
        /// Experiment configuration YAML. The default uses standard direct intake.
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Change intake lifecycle for a pinned experiment.
    Intake {
        /// Deployment host origin or saved server alias.
        #[arg(long, alias = "server")]
        host: String,
        /// Previously provisioned experiment ID.
        #[arg(long)]
        experiment: String,
        /// Lifecycle status to request (inactive, testing, active, completed, archived).
        status: String,
    },
}

impl RemoteCommand {
    /// Returns the stable JSON command identifier.
    pub(super) fn name(&self) -> &'static str {
        match self {
            Self::Login { .. } => "login",
            Self::Deploy { .. } => "deploy",
            Self::Host { .. } => "host",
            Self::Select { .. } => "select",
            Self::Intake { .. } => "intake",
        }
    }
}

/// Runs a remote deployment operation and returns the result for CLI formatting.
pub(super) async fn execute(command: RemoteCommand) -> Result<Value> {
    match command {
        RemoteCommand::Login { server, game } => {
            credentials::login(&server, &game)?;
            Ok(json!({"server": credentials::resolve_host(&server)?, "game": game}))
        }
        RemoteCommand::Deploy { host, package } => {
            let origin = credentials::resolve_host(&host)?;
            let game = deployment::package_game(&package)?;
            let token = credentials::upload_token(&origin, &game)?;
            let receipt = deployment::deploy(&origin, &package, &token).await?;
            Ok(json!({
                "server": origin,
                "game": receipt.game,
                "revision": receipt.revision,
                "version": receipt.version,
                "component_sha256": receipt.component_sha256,
            }))
        }
        RemoteCommand::Host {
            wasm_metering,
            bind,
            store,
            public_url,
            admin_url,
            grants,
        } => {
            let public_url = public_url.unwrap_or_else(|| format!("http://{bind}"));
            let admin_url = admin_url.or_else(|| env::var("PARLANDO_ADMIN_URL").ok());
            let grants = if let Some(path) = grants {
                serde_json::from_slice(
                    &std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?,
                )
                .context("expected a JSON object mapping game IDs to deployment tokens")?
            } else if admin_url.is_none()
                || env::var_os("PARLANDO_DEPLOY_GAME").is_some()
                || env::var_os("PARLANDO_DEPLOY_TOKEN").is_some()
            {
                BTreeMap::from([(
                    required_env("PARLANDO_DEPLOY_GAME")?,
                    required_env("PARLANDO_DEPLOY_TOKEN")?,
                )])
            } else {
                BTreeMap::new()
            };
            let options = HostOptions {
                store,
                public_url,
                grants,
                admin_token: required_env("PARLANDO_HOST_ADMIN_TOKEN")?,
            };
            deployment::serve_configured(bind, options, admin_url, wasm_metering).await?;
            Ok(json!({"status": "stopped"}))
        }
        RemoteCommand::Select {
            host,
            experiment,
            game,
            revision,
            config,
        } => {
            let config = config
                .map(|path| -> Result<Value> {
                    let bytes = std::fs::read(&path)
                        .with_context(|| format!("reading {}", path.display()))?;
                    serde_yaml::from_slice(&bytes)
                        .with_context(|| format!("parsing {}", path.display()))
                })
                .transpose()?
                .unwrap_or_else(|| json!({}));
            deployment::validate_experiment_id(&experiment)?;
            let origin = credentials::resolve_host(&host)?;
            let token = admin_token(&origin)?;
            deployment::control_request(
                &origin,
                "PUT",
                &format!("experiments/{experiment}"),
                &token,
                Some(&ExperimentSelection {
                    game,
                    revision,
                    config,
                }),
            )
            .await
        }
        RemoteCommand::Intake {
            host,
            experiment,
            status,
        } => {
            deployment::validate_experiment_id(&experiment)?;
            let origin = credentials::resolve_host(&host)?;
            let token = admin_token(&origin)?;
            deployment::control_request(
                &origin,
                "POST",
                &format!("experiments/{experiment}/admin/experiment/status"),
                &token,
                Some(&json!({"status": status})),
            )
            .await
        }
    }
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
        "operator credential belongs to another server"
    );
    required_env("PARLANDO_HOST_ADMIN_TOKEN")
}
