use std::{net::SocketAddr, path::PathBuf, sync::Arc};

use crate::{
    agents::{AgentFactory, SharedAgentFactory},
    app::{serve_game, ServeOptions},
    config::{AgentsMode, ExperimentConfig},
    game::{Game, GameMetadata},
};
use anyhow::{anyhow, Result};

/// Configures and runs one compiled game without exposing dashboard-owned policy.
pub struct Server<G: Game> {
    game: G,
    metadata: GameMetadata,
    database_url: String,
    participant_app: Option<PathBuf>,
    public_url: Option<String>,
    agents: Vec<SharedAgentFactory<G>>,
}

impl<G: Game> Server<G> {
    /// Creates a server for one compiled game and validates its stable identity.
    pub fn new(game: G, metadata: GameMetadata) -> Result<Self> {
        metadata.validate()?;
        Ok(Self {
            game,
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
    pub fn agent<F>(mut self, factory: F) -> Result<Self>
    where
        F: AgentFactory<G>,
    {
        let factory: Arc<dyn AgentFactory<G>> = Arc::new(factory);
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
    pub async fn serve(self, address: SocketAddr) -> Result<()> {
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
        let registered = Arc::new(self.agents);
        serve_game(
            self.game,
            bootstrap,
            self.metadata,
            address,
            move |experiment| {
                let definitions = registered
                    .iter()
                    .map(|factory| factory.definition())
                    .collect::<Vec<_>>();
                let agent_factory = if experiment.agents.mode == AgentsMode::HumanVsAgent {
                    let selected = experiment
                        .agents
                        .human_vs_agent
                        .as_ref()
                        .and_then(|config| config.factory.as_deref())
                        .or_else(|| definitions.first().map(|definition| definition.id.as_str()))
                        .ok_or_else(|| {
                            anyhow!("human-versus-agent mode has no registered agent")
                        })?;
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
            },
        )
        .await
    }
}
