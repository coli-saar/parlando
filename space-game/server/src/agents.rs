use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{bail, Result};
use async_trait::async_trait;
use parlando_server::{
    AdminAgentOption, AgentFactory, AgentInitContext, AgentParticipantIdentity, AgentResponse,
    AgentUtteranceKind, ExperimentConfig, GameAgent, PlayerRole, RemoteGrpcAgentConfig,
    RemoteGrpcAgentFactory,
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

/// Returns the Space Game agent selectors supported by `factory_from_config`.
pub fn available_agent_options() -> Vec<AdminAgentOption> {
    vec![
        AdminAgentOption {
            selector: "space_game.back_and_forth".to_string(),
            label: "Back-and-forth agent".to_string(),
            description: Some("Built-in deterministic Space Game movement agent.".to_string()),
            requires_config: false,
            default_config: serde_json::json!({}),
        },
        AdminAgentOption {
            selector: "remote_grpc".to_string(),
            label: "Remote gRPC agent".to_string(),
            description: Some(
                "External agent service using the Parlando agent protocol.".to_string(),
            ),
            requires_config: true,
            default_config: serde_json::json!({
                "endpoint": "http://127.0.0.1:50051",
                "agent_name": "space-game-remote-agent",
                "agent_version": null
            }),
        },
    ]
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
    "parlando-agent-v2".to_string()
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

/// Deterministic Space Game agent that moves, comments, and answers visible-world questions.
pub struct BackAndForthAgent {
    role: String,
    rng: StdRng,
    step_index: usize,
    last_step_at: Instant,
    last_other_position: Option<(i64, i64)>,
    pending_step: bool,
    other_player_moved: bool,
    utterances: Vec<&'static str>,
    current_observation: Option<SpaceObservation>,
    pending_question: Option<(PlayerRole, String)>,
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
            pending_step: true,
            other_player_moved: false,
            utterances: vec![
                "Ich habe deine Bewegung gesehen.",
                "Okay, ich reagiere auf deinen Schritt.",
                "Gut, ich passe meine Route an.",
                "Alles klar, ich habe dich gesehen.",
                "Verstanden, ich mache weiter.",
            ],
            current_observation: None,
            pending_question: None,
        }
    }

    /// Answers one participant question using only the agent's visible world observation.
    fn answer_world_question(&self, speaker: PlayerRole, question: &str) -> String {
        let Some(world) = &self.current_observation else {
            return "Ich habe noch keine Weltinformationen erhalten.".to_string();
        };
        let normalized = question.to_lowercase();
        let agent = if self.role == "A" {
            &world.players.a
        } else {
            &world.players.b
        };
        let human = if speaker == PlayerRole::A {
            &world.players.a
        } else {
            &world.players.b
        };
        if normalized.contains("wo bist du") || normalized.contains("deine position") {
            return format!(
                "Ich bin im Raum {} bei Position {}, {}.",
                agent.room, agent.position.x, agent.position.y
            );
        }
        if normalized.contains("wo bin ich") || normalized.contains("meine position") {
            return format!(
                "Du bist im Raum {} bei Position {}, {}.",
                human.room, human.position.x, human.position.y
            );
        }
        if normalized.contains("spieler a") || normalized.contains("player a") {
            return format!(
                "Spieler A ist im Raum {} bei Position {}, {}.",
                world.players.a.room, world.players.a.position.x, world.players.a.position.y
            );
        }
        if normalized.contains("spieler b") || normalized.contains("player b") {
            return format!(
                "Spieler B ist im Raum {} bei Position {}, {}.",
                world.players.b.room, world.players.b.position.x, world.players.b.position.y
            );
        }
        if normalized.contains("start")
            || normalized.contains("launch")
            || normalized.contains("bereit")
        {
            let mut missing = Vec::new();
            if !world.systems.power_stable {
                missing.push("stabile Energie");
            }
            if !world.systems.oxygen_stable {
                missing.push("stabiler Sauerstoff");
            }
            if !world.systems.door_access {
                missing.push("Türzugang");
            }
            if !world.systems.signal_routed {
                missing.push("Signal zur Antenne");
            }
            return if missing.is_empty() {
                "Alle sichtbaren Systeme sind bereit. Das Leuchtfeuer kann gestartet werden."
                    .to_string()
            } else {
                format!("Noch nicht startbereit. Es fehlen: {}.", missing.join(", "))
            };
        }
        if normalized.contains("batter") {
            return format!(
                "Die Batterie ist bei {}, {} und {}.",
                world.battery.location,
                if world.battery.charged {
                    "geladen"
                } else {
                    "nicht geladen"
                },
                if world.battery.spent {
                    "verbraucht"
                } else {
                    "noch verwendbar"
                }
            );
        }
        if normalized.contains("sicherung") || normalized.contains("fuse") {
            return format!(
                "Sicherungen: blau {}, gelb {}, rot {}.",
                on_off(world.fuses.blue),
                on_off(world.fuses.yellow),
                on_off(world.fuses.red)
            );
        }
        if normalized.contains("schalter") || normalized.contains("breaker") {
            return format!(
                "Schutzschalter: main {}, aux {}.",
                on_off(world.breakers.main),
                on_off(world.breakers.aux)
            );
        }
        if normalized.contains("ventil") || normalized.contains("valve") {
            return format!(
                "Ventile: A {}, C {}, Fluttor {}.",
                open_closed(world.valves.a),
                open_closed(world.valves.c),
                open_closed(world.valves.floodgate)
            );
        }
        if normalized.contains("relais")
            || normalized.contains("relay")
            || normalized.contains("signal")
        {
            return format!(
                "Das Relais steht auf {}. Das Signal ist {}geroutet.",
                world.relay,
                if world.systems.signal_routed {
                    ""
                } else {
                    "nicht "
                }
            );
        }
        if normalized.contains("weißt")
            || normalized.contains("weisst")
            || normalized.contains("hinweis")
            || normalized.contains("wissen")
        {
            return if world.private_knowledge.is_empty() {
                "Ich habe keine privaten Hinweise entdeckt.".to_string()
            } else {
                format!("Ich weiß Folgendes: {}", world.private_knowledge.join(" "))
            };
        }
        format!(
            "Ich sehe: Energie {}, Sauerstoff {}, Türzugang {}, Signal {}. Frag mich auch nach Positionen, Batterie, Sicherungen, Ventilen oder Hinweisen.",
            ready_not(world.systems.power_stable),
            ready_not(world.systems.oxygen_stable),
            ready_not(world.systems.door_access),
            ready_not(world.systems.signal_routed),
        )
    }
}

#[async_trait]
impl GameAgent<SpaceGameAdapter> for BackAndForthAgent {
    /// Stores the latest role-safe world snapshot used to answer questions.
    async fn observe_state(&mut self, current_observation: SpaceObservation) -> Result<()> {
        self.update_other_player_position(&current_observation);
        self.current_observation = Some(current_observation);
        Ok(())
    }

    /// Updates movement behavior and the latest role-safe world snapshot after an action.
    async fn observe_action(
        &mut self,
        actor: PlayerRole,
        _action: SpaceAction,
        resulting_observation: SpaceObservation,
    ) -> Result<()> {
        if actor.as_str() != self.role {
            self.pending_step = true;
            self.other_player_moved = true;
        }
        self.update_other_player_position(&resulting_observation);
        self.current_observation = Some(resulting_observation);
        Ok(())
    }

    /// Queues a typed or spoken participant utterance for a message-only response.
    async fn observe_message(
        &mut self,
        speaker: PlayerRole,
        _kind: AgentUtteranceKind,
        text: String,
    ) -> Result<()> {
        if speaker.as_str() != self.role && !text.trim().is_empty() {
            self.pending_question = Some((speaker, text));
        }
        Ok(())
    }

    /// Answers a pending question before considering the agent's next movement.
    async fn maybe_act(
        &mut self,
        _available_actions: Option<Vec<SpaceAction>>,
    ) -> Result<Option<AgentResponse<SpaceAction>>> {
        if let Some((speaker, question)) = self.pending_question.take() {
            return Ok(Some(AgentResponse {
                message: Some(self.answer_world_question(speaker, &question)),
                action: None,
            }));
        }
        if !self.pending_step {
            return Ok(None);
        }
        if self.last_step_at.elapsed() < Duration::from_secs(1) {
            return Ok(None);
        }
        self.last_step_at = Instant::now();
        self.pending_step = false;
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
        let message = if self.other_player_moved {
            self.other_player_moved = false;
            Some(self.utterances[self.rng.gen_range(0..self.utterances.len())].to_string())
        } else {
            None
        };
        Ok(Some(AgentResponse {
            message,
            action: Some(action),
        }))
    }
}

/// Formats a binary component state for a spoken German status response.
fn on_off(value: bool) -> &'static str {
    if value {
        "an"
    } else {
        "aus"
    }
}

/// Formats a valve state for a spoken German status response.
fn open_closed(value: bool) -> &'static str {
    if value {
        "offen"
    } else {
        "geschlossen"
    }
}

/// Formats whether one derived system has reached its required state.
fn ready_not(value: bool) -> &'static str {
    if value {
        "bereit"
    } else {
        "nicht bereit"
    }
}

impl BackAndForthAgent {
    // Stores the other player's current position for future observations.
    fn update_other_player_position(&mut self, observation: &SpaceObservation) {
        let position = if self.role == "A" {
            observation.players.b.position
        } else {
            observation.players.a.position
        };
        self.last_other_position = Some((position.x, position.y));
    }
}

#[cfg(test)]
mod tests {
    use parlando_server::config::{AgentsConfig, AgentsMode, HumanVsAgentConfig};
    use parlando_server::{AgentInitContext, GameAdapter, PlayerRole};

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
                ..AgentsConfig::default()
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
            ..AgentsConfig::default()
        };

        assert!(factory_from_config(&config).unwrap().is_some());
    }

    #[test]
    fn available_agent_options_include_supported_selectors() {
        let selectors = available_agent_options()
            .into_iter()
            .map(|option| option.selector)
            .collect::<Vec<_>>();
        assert!(selectors.contains(&"space_game.back_and_forth".to_string()));
        assert!(selectors.contains(&"remote_grpc".to_string()));
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
            ..AgentsConfig::default()
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
        first.observe_state(observation.clone()).await.unwrap();
        second.observe_state(observation).await.unwrap();

        let first_result = first.act(Some(vec![])).await.unwrap();
        let second_result = second.act(Some(vec![])).await.unwrap();

        assert!(matches!(
            first_result.action,
            Some(SpaceAction::MoveStep { direction, .. }) if direction == "left"
        ));
        assert!(matches!(
            second_result.action,
            Some(SpaceAction::MoveStep { direction, .. }) if direction == "left"
        ));
    }

    #[tokio::test]
    async fn back_and_forth_agent_speaks_when_other_player_moves() {
        let adapter = SpaceGameAdapter::new();
        let mut agent = BackAndForthAgent::new("B".to_string(), 1, Value::Null);
        let first_observation = adapter.observe_state(&initial_state(), PlayerRole::B);
        agent.observe_state(first_observation).await.unwrap();
        let _ = agent.act(Some(vec![])).await.unwrap();
        agent.last_step_at = Instant::now() - Duration::from_secs(60);
        let mut moved_state = initial_state();
        moved_state.players.a.position.x += 1;
        let second_observation = adapter.observe_state(&moved_state, PlayerRole::B);
        agent
            .observe_action(
                PlayerRole::A,
                SpaceAction::MoveStep {
                    player: "A".to_string(),
                    direction: "right".to_string(),
                },
                second_observation,
            )
            .await
            .unwrap();

        let result = agent.act(Some(vec![])).await.unwrap();

        assert!(matches!(
            result.action,
            Some(SpaceAction::MoveStep { direction, .. }) if direction == "down"
        ));
        assert!(result.message.is_some_and(|message| !message.is_empty()));
    }

    #[tokio::test]
    async fn back_and_forth_agent_does_not_chain_after_its_own_move() {
        let adapter = SpaceGameAdapter::new();
        let mut agent = BackAndForthAgent::new("B".to_string(), 1, Value::Null);
        let observation = adapter.observe_state(&initial_state(), PlayerRole::B);
        agent.observe_state(observation.clone()).await.unwrap();
        let first_result = agent.act(Some(vec![])).await.unwrap();
        assert!(first_result.action.is_some());
        agent.last_step_at = Instant::now() - Duration::from_secs(60);
        agent
            .observe_action(
                PlayerRole::B,
                SpaceAction::MoveStep {
                    player: "B".to_string(),
                    direction: "up".to_string(),
                },
                observation,
            )
            .await
            .unwrap();

        assert!(agent.maybe_act(Some(vec![])).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn back_and_forth_agent_answers_spoken_world_questions_without_moving() {
        let adapter = SpaceGameAdapter::new();
        let mut agent = BackAndForthAgent::new("B".to_string(), 1, Value::Null);
        let observation = adapter.observe_state(&initial_state(), PlayerRole::B);
        agent.observe_state(observation).await.unwrap();
        let _ = agent.act(Some(vec![])).await.unwrap();
        agent
            .observe_message(
                PlayerRole::A,
                AgentUtteranceKind::Spoken,
                "Wo bist du?".to_string(),
            )
            .await
            .unwrap();

        let response = agent.maybe_act(Some(vec![])).await.unwrap().unwrap();

        assert!(response.action.is_none());
        assert_eq!(
            response.message.as_deref(),
            Some("Ich bin im Raum valve bei Position 9, 6.")
        );
    }

    #[tokio::test]
    async fn back_and_forth_agent_reports_visible_launch_blockers() {
        let adapter = SpaceGameAdapter::new();
        let mut agent = BackAndForthAgent::new("B".to_string(), 1, Value::Null);
        let observation = adapter.observe_state(&initial_state(), PlayerRole::B);
        agent.observe_state(observation).await.unwrap();
        agent
            .observe_message(
                PlayerRole::A,
                AgentUtteranceKind::Typed,
                "Sind wir startbereit?".to_string(),
            )
            .await
            .unwrap();

        let response = agent.maybe_act(Some(vec![])).await.unwrap().unwrap();
        let message = response.message.unwrap();

        assert!(message.contains("Noch nicht startbereit"));
        assert!(message.contains("stabile Energie"));
        assert!(message.contains("stabiler Sauerstoff"));
    }
}
