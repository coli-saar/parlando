use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{bail, Result};
use async_trait::async_trait;
use parlando_server::{
    AgentFactory, AgentInitContext, AgentParticipantIdentity, AgentResult, ExperimentConfig,
    GameAgent, RemoteGrpcAgentConfig, RemoteGrpcAgentFactory,
};
use rand::{rngs::StdRng, Rng, SeedableRng};
use serde::Deserialize;
use serde_json::Value;

use crate::{
    game::state_engine::{SpaceAction, SpaceObservation},
    SpaceGameAdapter,
};

/// Builds the Space Game agent factory requested by the shared experiment config.
pub fn factory_from_config(
    config: &ExperimentConfig,
) -> Result<Option<Arc<dyn AgentFactory<SpaceGameAdapter>>>> {
    if config.agents.mode != parlando_server::config::AgentsMode::HumanVsAgent {
        return Ok(None);
    }
    let human_vs_agent = config.agents.human_vs_agent.as_ref().ok_or_else(|| {
        anyhow::anyhow!("agents.human_vs_agent is required when agents.mode is human_vs_agent")
    })?;
    match human_vs_agent
        .factory
        .as_deref()
        .unwrap_or("space_game.back_and_forth")
    {
        "space_game.back_and_forth" | "space_game.agents:create_back_and_forth_agent" => {
            Ok(Some(Arc::new(BackAndForthAgentFactory {
                seed: human_vs_agent.seed,
                config: human_vs_agent.config.clone(),
            })))
        }
        "remote_grpc" | "parlando.remote_grpc" => {
            let config = RemoteAgentSelectorConfig::from_value(
                human_vs_agent.config.clone(),
                human_vs_agent.act_timeout_seconds,
            )?;
            Ok(Some(Arc::new(
                RemoteGrpcAgentFactory::<SpaceGameAdapter>::new(config),
            )))
        }
        other => bail!("unknown Space Game agent factory selector: {other}"),
    }
}

#[derive(Debug, Deserialize)]
struct RemoteAgentSelectorConfig {
    endpoint: String,
    #[serde(default = "default_remote_agent_name")]
    agent_name: String,
    agent_version: Option<String>,
    #[serde(default = "default_remote_agent_protocol")]
    protocol_version: String,
}

impl RemoteAgentSelectorConfig {
    // Converts YAML agent config into the reusable remote gRPC factory config.
    fn from_value(value: Value, act_timeout_seconds: f64) -> Result<RemoteGrpcAgentConfig> {
        let selector: Self = serde_json::from_value(value)?;
        let mut config = RemoteGrpcAgentConfig::new(selector.endpoint, selector.agent_name);
        config.agent_version = selector.agent_version;
        config.protocol_version = selector.protocol_version;
        config.request_timeout = Duration::from_secs_f64(act_timeout_seconds.max(0.1));
        Ok(config)
    }
}

fn default_remote_agent_name() -> String {
    "space-game-remote-agent".to_string()
}

fn default_remote_agent_protocol() -> String {
    "parlando-agent-v1".to_string()
}

/// Factory for the simple deterministic Space Game back-and-forth agent.
pub struct BackAndForthAgentFactory {
    seed: Option<u64>,
    config: Value,
}

impl AgentFactory<SpaceGameAdapter> for BackAndForthAgentFactory {
    fn create(
        &self,
        context: AgentInitContext,
    ) -> Result<Box<dyn GameAgent<SpaceGameAdapter> + Send>> {
        let seed = context.seed.or(self.seed).unwrap_or(0);
        Ok(Box::new(BackAndForthAgent::new(
            context.role,
            seed,
            self.config.clone(),
        )))
    }

    fn participant_identity(&self) -> AgentParticipantIdentity {
        AgentParticipantIdentity {
            identity_provider: "space_game".to_string(),
            external_id: Some(format!(
                "space_game.back_and_forth@{}",
                env!("CARGO_PKG_VERSION")
            )),
            metadata: serde_json::json!({
                "agent_type": "space_game.back_and_forth",
                "agent_name": "BackAndForthAgent",
                "agent_version": env!("CARGO_PKG_VERSION"),
                "agent_version_source": "space-game crate version",
            }),
        }
    }
}

/// Space Game agent that alternates movement and comments when the human moves.
pub struct BackAndForthAgent {
    role: String,
    rng: StdRng,
    step_index: usize,
    last_step_at: Instant,
    last_other_position: Option<(i64, i64)>,
    utterances: Vec<&'static str>,
}

impl BackAndForthAgent {
    // Creates one mutable agent instance for a single room participant.
    fn new(role: String, seed: u64, _config: Value) -> Self {
        Self {
            role,
            rng: StdRng::seed_from_u64(seed),
            step_index: 0,
            last_step_at: Instant::now() - Duration::from_secs(60),
            last_other_position: None,
            utterances: vec![
                "Ich habe deine Bewegung gesehen.",
                "Okay, ich reagiere auf deinen Schritt.",
                "Gut, ich passe meine Route an.",
                "Alles klar, ich habe dich gesehen.",
                "Verstanden, ich mache weiter.",
            ],
        }
    }
}

#[async_trait]
impl GameAgent<SpaceGameAdapter> for BackAndForthAgent {
    async fn act(
        &mut self,
        observation: SpaceObservation,
        _available_actions: Option<Vec<SpaceAction>>,
    ) -> Result<AgentResult<SpaceAction>> {
        if self.last_step_at.elapsed() < Duration::from_secs(1) {
            return Ok(AgentResult::None);
        }
        self.last_step_at = Instant::now();
        let directions: &[&str] = if self.role == "A" {
            &["left", "right"]
        } else {
            &["up", "down"]
        };
        let direction = directions[self.step_index % directions.len()];
        self.step_index += 1;
        let action = SpaceAction::MoveStep {
            player: self.role.clone(),
            direction: direction.to_string(),
        };
        if self.other_player_moved(&observation) {
            let message = self.utterances[self.rng.gen_range(0..self.utterances.len())].to_string();
            Ok(AgentResult::ActionWithMessage { action, message })
        } else {
            Ok(AgentResult::Action(action))
        }
    }
}

impl BackAndForthAgent {
    // Detects whether the other player moved since the previous agent turn.
    fn other_player_moved(&mut self, observation: &SpaceObservation) -> bool {
        let position = if self.role == "A" {
            observation.players.b.position
        } else {
            observation.players.a.position
        };
        let current = (position.x, position.y);
        let moved = self
            .last_other_position
            .is_some_and(|previous| previous != current);
        self.last_other_position = Some(current);
        moved
    }
}

#[cfg(test)]
mod tests {
    use parlando_server::config::{AgentsConfig, AgentsMode, HumanVsAgentConfig};
    use parlando_server::{AgentInitContext, AgentResult, GameAdapter, PlayerRole};

    use crate::game::state_engine::initial_state;

    use super::*;

    fn init_context(role: &str) -> AgentInitContext {
        AgentInitContext {
            role: role.to_string(),
            seed: None,
            config: Value::Null,
        }
    }

    #[test]
    fn factory_from_config_is_absent_for_human_vs_human() {
        let config = ExperimentConfig::default();

        assert!(factory_from_config(&config).unwrap().is_none());
    }

    #[test]
    fn factory_from_config_accepts_default_and_legacy_selectors() {
        for selector in [None, Some("space_game.agents:create_back_and_forth_agent")] {
            let mut config = ExperimentConfig::default();
            config.agents = AgentsConfig {
                mode: AgentsMode::HumanVsAgent,
                human_vs_agent: Some(HumanVsAgentConfig {
                    factory: selector.map(str::to_string),
                    ..HumanVsAgentConfig::default()
                }),
            };

            assert!(factory_from_config(&config).unwrap().is_some());
        }
    }

    #[test]
    fn factory_from_config_accepts_remote_grpc_selector() {
        let mut config = ExperimentConfig::default();
        config.agents = AgentsConfig {
            mode: AgentsMode::HumanVsAgent,
            human_vs_agent: Some(HumanVsAgentConfig {
                factory: Some("remote_grpc".to_string()),
                act_timeout_seconds: 0.5,
                config: serde_json::json!({
                    "endpoint": "http://127.0.0.1:50051",
                    "agent_name": "test-python-agent",
                    "agent_version": "v1"
                }),
                ..HumanVsAgentConfig::default()
            }),
        };

        assert!(factory_from_config(&config).unwrap().is_some());
    }

    #[test]
    fn factory_from_config_rejects_unknown_selectors() {
        let mut config = ExperimentConfig::default();
        config.agents = AgentsConfig {
            mode: AgentsMode::HumanVsAgent,
            human_vs_agent: Some(HumanVsAgentConfig {
                factory: Some("python.module:factory".to_string()),
                ..HumanVsAgentConfig::default()
            }),
        };

        assert!(factory_from_config(&config).is_err());
    }

    #[tokio::test]
    async fn back_and_forth_factory_returns_fresh_agents() {
        let factory = BackAndForthAgentFactory {
            seed: Some(1),
            config: Value::Null,
        };
        let adapter = SpaceGameAdapter::new();
        let observation = adapter.observe_state(&initial_state(), PlayerRole::A);
        let mut first = factory.create(init_context("A")).unwrap();
        let mut second = factory.create(init_context("A")).unwrap();

        let first_result = first.act(observation.clone(), Some(vec![])).await.unwrap();
        let second_result = second.act(observation, Some(vec![])).await.unwrap();

        assert!(matches!(
            first_result,
            AgentResult::Action(SpaceAction::MoveStep { direction, .. }) if direction == "left"
        ));
        assert!(matches!(
            second_result,
            AgentResult::Action(SpaceAction::MoveStep { direction, .. }) if direction == "left"
        ));
    }

    #[tokio::test]
    async fn back_and_forth_agent_speaks_when_other_player_moves() {
        let adapter = SpaceGameAdapter::new();
        let mut agent = BackAndForthAgent::new("B".to_string(), 1, Value::Null);
        let first_observation = adapter.observe_state(&initial_state(), PlayerRole::B);
        let _ = agent.act(first_observation, Some(vec![])).await.unwrap();
        agent.last_step_at = Instant::now() - Duration::from_secs(60);
        let mut moved_state = initial_state();
        moved_state.players.a.position.x += 1;
        let second_observation = adapter.observe_state(&moved_state, PlayerRole::B);

        let result = agent.act(second_observation, Some(vec![])).await.unwrap();

        assert!(matches!(
            result,
            AgentResult::ActionWithMessage {
                action: SpaceAction::MoveStep { direction, .. },
                message
            } if direction == "down" && !message.is_empty()
        ));
    }
}
