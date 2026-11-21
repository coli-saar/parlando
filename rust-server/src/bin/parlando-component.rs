//! Starts one packaged component game in Parlando's live or headless runtime.

use anyhow::{ensure, Context, Result};
use clap::{Parser, Subcommand};
use parlando::{
    agent::grpc::RemoteAgent, component::ComponentGameFactory, ExperimentRunner, GameMetadata,
    Server,
};
use serde::Deserialize;
use serde_json::json;
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
};

/// Selects one local game package and the runtime that will host it.
#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// YAML package manifest describing the component and its game identity.
    #[arg(long)]
    manifest: PathBuf,
    #[command(subcommand)]
    command: Command,
}

/// Starts the standard live server or executes a headless evaluation.
#[derive(Subcommand)]
enum Command {
    /// Start the live server with the administrative dashboard.
    Serve {
        /// Listener address, including the port.
        #[arg(long, default_value = "127.0.0.1:8000")]
        bind: SocketAddr,
        /// SQLite database URL for this game installation.
        #[arg(long)]
        database_url: String,
        /// Optional compiled participant web application.
        #[arg(long)]
        client_dist: Option<PathBuf>,
        /// Public origin when the service is behind a proxy.
        #[arg(long)]
        public_url: Option<String>,
    },
    /// Run or resume an agent–agent evaluation from YAML.
    Run {
        /// Path to a parlando-agent-experiment/v1 YAML file.
        experiment: PathBuf,
    },
}

/// Public package identity and its executable location, relative to this manifest.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PackageManifest {
    id: String,
    name: String,
    version: semver::Version,
    component: PathBuf,
}

impl PackageManifest {
    /// Reads the local package description and resolves its component path.
    fn read(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path)
            .with_context(|| format!("reading component manifest {}", path.display()))?;
        let mut manifest: Self = serde_yaml::from_slice(&bytes)
            .with_context(|| format!("invalid component manifest {}", path.display()))?;
        ensure!(
            !manifest.component.as_os_str().is_empty(),
            "component manifest {} requires a component path",
            path.display()
        );
        if let Some(directory) = path.parent() {
            manifest.component = directory.join(manifest.component);
        }
        Ok(manifest)
    }

    /// Binds the declared game identity to the exact bytes loaded by the host.
    fn load(self) -> Result<(ComponentGameFactory, GameMetadata)> {
        let factory = ComponentGameFactory::load(&self.component)
            .with_context(|| format!("loading game component {}", self.component.display()))?;
        let metadata = GameMetadata {
            id: self.id,
            name: self.name,
            version: self.version,
            build_manifest: json!({"component_sha256": factory.digest()}),
        };
        Ok((factory, metadata))
    }
}

/// Uses the same game factory in both runtimes and exposes failed evaluations to scripts.
#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let (factory, metadata) = PackageManifest::read(&args.manifest)?.load()?;
    match args.command {
        Command::Serve {
            bind,
            database_url,
            client_dist,
            public_url,
        } => {
            let mut server = Server::new(factory, metadata)?.database_url(database_url);
            if let Some(client_dist) = client_dist {
                server = server.participant_app(client_dist);
            }
            if let Some(public_url) = public_url {
                server = server.public_url(public_url);
            }
            server.serve(bind).await
        }
        Command::Run { experiment } => {
            let summary = ExperimentRunner::new(metadata, factory)?
                .agent(RemoteAgent::new())?
                .run_yaml(experiment)
                .await?;
            println!("{}", serde_json::to_string_pretty(&summary)?);
            ensure!(summary.failed == 0, "{} session(s) failed", summary.failed);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Package assets resolve from the manifest and its schema rejects typos.
    #[test]
    fn reads_a_manifest_independent_of_working_directory() {
        let package = tempfile::tempdir().unwrap();
        let path = package.path().join("game.yaml");
        let content = "id: counter\nname: Counter\nversion: 1.0.0\ncomponent: bin/game.wasm\n";
        std::fs::write(&path, content).unwrap();
        let manifest = PackageManifest::read(&path).unwrap();
        assert_eq!(manifest.id, "counter");
        assert_eq!(manifest.name, "Counter");
        assert_eq!(manifest.version, semver::Version::new(1, 0, 0));
        assert_eq!(manifest.component, package.path().join("bin/game.wasm"));

        std::fs::write(&path, format!("{content}unknown: true\n")).unwrap();
        assert!(PackageManifest::read(&path).is_err());
    }
}
