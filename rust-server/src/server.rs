//! Launchers use the server builder to supply a WebAssembly game, agent factories,
//! participant assets, and installation options. Serving hands these inputs to the HTTP host,
//! which loads experiment settings and mounts the participant and administrative routes.

use std::{net::SocketAddr, path::PathBuf, sync::Arc};

use crate::{
    agents::{AgentFactory, SharedAgentFactory},
    app::{serve_game, ServeOptions},
    component::LoadedGame,
    config::{AgentsMode, ExperimentConfig},
    game::GameMetadata,
    remote_agent::RemoteAgent,
};
use anyhow::{anyhow, Result};

/// Configures one WebAssembly game and runs its participant and administrator routes.
pub struct Server {
    loaded_game: LoadedGame,
    metadata: GameMetadata,
    database_url: String,
    participant_app: Option<PathBuf>,
    public_url: Option<String>,
    agents: Vec<SharedAgentFactory>,
}

impl Server {
    /// Creates a server for one component and pins its verified SHA-256 in the build manifest.
    /// A manifest with a conflicting digest is rejected before the server opens its database.
    pub fn new(loaded_game: LoadedGame, mut metadata: GameMetadata) -> Result<Self> {
        metadata.validate()?;
        metadata.pin_component(&loaded_game)?;
        Ok(Self {
            loaded_game,
            metadata,
            database_url: "sqlite:///./parlando.sqlite".to_string(),
            participant_app: None,
            public_url: None,
            agents: Vec::new(),
        })
    }

    /// Selects the installation's durable SQLite database URL.
    pub fn database_url(mut self, database_url: impl Into<String>) -> Self {
        self.database_url = database_url.into();
        self
    }

    /// Serves an optional compiled participant application as a deployment convenience.
    ///
    /// The server protocol does not depend on these assets or on their framework.
    pub fn participant_app(mut self, directory: impl Into<PathBuf>) -> Self {
        self.participant_app = Some(directory.into());
        self
    }

    /// Sets the externally visible origin when it cannot be inferred from a loopback address.
    pub fn public_url(mut self, public_url: impl Into<String>) -> Self {
        self.public_url = Some(public_url.into());
        self
    }

    /// Registers one compiled agent implementation for dashboard selection.
    pub fn agent(mut self, factory: impl AgentFactory) -> Result<Self> {
        let prepared = factory.with_host_wasm_metering(self.loaded_game.metering())?;
        let factory = prepared.unwrap_or_else(|| Arc::new(factory));
        let definition = factory.definition();
        definition.validate()?;
        let id = definition.id;
        if self
            .agents
            .iter()
            .any(|registered| registered.definition().id == id)
        {
            return Err(anyhow!(
                "agent definition {id:?} is registered more than once"
            ));
        }
        self.agents.push(factory);
        Ok(self)
    }

    /// Runs the frontend-neutral HTTP, JSON, and WebSocket server.
    pub async fn serve(mut self, address: SocketAddr) -> Result<()> {
        let mut bootstrap = ExperimentConfig::default();
        bootstrap.database.url = self.database_url;
        bootstrap.server.client_dist_path = self
            .participant_app
            .map(|path| path.to_string_lossy().into_owned());
        bootstrap.server.public_base_url = self.public_url.unwrap_or_else(|| {
            let host = if address.ip().is_unspecified() {
                "127.0.0.1".to_string()
            } else {
                address.ip().to_string()
            };
            format!("http://{host}:{}", address.port())
        });
        register_default_remote_agent(&mut self.agents);
        let registered = Arc::new(self.agents);
        serve_game(
            self.loaded_game,
            bootstrap,
            self.metadata,
            address,
            move |experiment| experiment_serve_options(&registered, experiment),
        )
        .await
    }
}

/// Selects the registered agent factory and advertises all supported agent definitions.
pub(crate) fn experiment_serve_options(
    registered: &[SharedAgentFactory],
    experiment: &ExperimentConfig,
) -> Result<ServeOptions> {
    let definitions = registered
        .iter()
        .map(|factory| factory.definition())
        .collect();
    let agent_factory = if experiment.agents.mode == AgentsMode::HumanVsAgent {
        let selected = experiment
            .agents
            .human_vs_agent
            .as_ref()
            .and_then(|config| config.factory.as_deref())
            .ok_or_else(|| anyhow!("human-versus-agent mode requires an explicit agent factory"))?;
        Some(
            registered
                .iter()
                .find(|factory| factory.definition().id == selected)
                .cloned()
                .ok_or_else(|| anyhow!("unknown registered agent {selected:?}"))?,
        )
    } else {
        None
    };
    Ok(ServeOptions {
        agent_factory,
        agent_definitions: definitions,
        ..ServeOptions::default()
    })
}

/// Adds Parlando's standard remote transport unless the server already registered it explicitly.
pub(crate) fn register_default_remote_agent(agents: &mut Vec<SharedAgentFactory>) {
    let remote_id = <RemoteAgent as AgentFactory>::definition(&RemoteAgent::new()).id;
    if agents
        .iter()
        .any(|registered| registered.definition().id == remote_id)
    {
        return;
    }
    agents.push(Arc::new(RemoteAgent::new()));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Records the loaded executable identity and refuses a false caller-supplied pin.
    #[test]
    fn server_validates_component_build_provenance() -> Result<()> {
        let game =
            LoadedGame::from_bytes(include_bytes!("../tests/fixtures/live-game/fixture.wasm"))?;
        let metadata = GameMetadata {
            id: "build-pin-test".to_string(),
            name: "Build pin test".to_string(),
            version: semver::Version::new(1, 0, 0),
            build_manifest: serde_json::json!({"caller": "fixture"}),
        };
        let server = Server::new(game.clone(), metadata.clone())?;
        assert_eq!(server.metadata.build_manifest["caller"], "fixture");
        assert_eq!(
            server.metadata.build_manifest["component_sha256"],
            game.digest()
        );
        let mut conflicting = metadata;
        conflicting.build_manifest["component_sha256"] = serde_json::json!("other");
        let error = Server::new(game, conflicting)
            .err()
            .expect("server must reject a false digest");
        assert!(error.to_string().contains("digest does not match"));
        Ok(())
    }
}
