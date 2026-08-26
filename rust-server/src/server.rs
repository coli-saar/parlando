use std::{net::SocketAddr, path::PathBuf, sync::Arc};

use crate::{
    agents::{AgentFactory, SharedAgentFactory},
    app::{serve_game, ServeOptions},
    config::{AgentsMode, ExperimentConfig},
    game::{GameFactory, GameMetadata},
    remote_agent::RemoteAgent,
};
use anyhow::{anyhow, Result};

/// Configures and runs one compiled game without exposing dashboard-owned policy.
pub struct Server<F: GameFactory> {
    game_factory: F,
    metadata: GameMetadata,
    database_url: String,
    participant_app: Option<PathBuf>,
    public_url: Option<String>,
    agents: Vec<SharedAgentFactory<F::Game>>,
}

impl<F: GameFactory> Server<F> {
    /// Creates a server for one compiled game and validates its stable identity.
    pub fn new(game_factory: F, metadata: GameMetadata) -> Result<Self> {
        metadata.validate()?;
        Ok(Self {
            game_factory,
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
    pub fn agent<AF>(mut self, factory: AF) -> Result<Self>
    where
        AF: AgentFactory<F::Game>,
    {
        let factory: Arc<dyn AgentFactory<F::Game>> = Arc::new(factory);
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
            self.game_factory,
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
                        .ok_or_else(|| {
                            anyhow!("human-versus-agent mode requires an explicit agent factory")
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

/// Adds Parlando's standard remote transport unless the server already registered it explicitly.
fn register_default_remote_agent<A: crate::Game>(agents: &mut Vec<SharedAgentFactory<A>>) {
    let remote_id = <RemoteAgent as AgentFactory<A>>::definition(&RemoteAgent::new()).id;
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
    use crate::{ActionRejection, Game, GameInitializationContext, PlayerRole};
    use serde_json::Value;

    /// Minimal typed game used to inspect generic server-side agent registration.
    struct TestGame;

    impl Game for TestGame {
        type Config = Value;
        type State = Value;
        type Action = Value;
        type Observation = Value;
        type Completion = Value;

        /// Creates an empty state because registration does not execute game mechanics.
        fn initial_state(
            &self,
            _context: GameInitializationContext<'_, Self::Config>,
        ) -> Result<Self::State> {
            Ok(Value::Null)
        }

        /// Preserves the state because registration does not execute game mechanics.
        fn apply_action(
            &self,
            state: &Self::State,
            _action: &Self::Action,
            _actor: PlayerRole,
        ) -> std::result::Result<Self::State, ActionRejection> {
            Ok(state.clone())
        }

        /// Returns the state as the role-neutral test observation.
        fn observation(&self, state: &Self::State, _role: PlayerRole) -> Self::Observation {
            state.clone()
        }

        /// Keeps the test game nonterminal.
        fn completion(&self, _state: &Self::State) -> Option<Self::Completion> {
            None
        }
    }

    /// The default registration makes the remote factory available exactly once.
    #[test]
    fn default_remote_agent_is_registered_once() {
        let mut agents: Vec<SharedAgentFactory<TestGame>> = Vec::new();
        register_default_remote_agent(&mut agents);
        register_default_remote_agent(&mut agents);

        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].definition().id, "remote_grpc");
    }
}
