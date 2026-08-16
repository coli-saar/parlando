use std::time::{Duration, Instant};

use anyhow::Result;
use async_trait::async_trait;
use parlando_server::{
    agent::{
        Agent, Context as AgentContext, Definition as AgentDefinition, Factory as AgentFactory,
        Identity as AgentIdentity, Response,
    },
    PlayerRole,
};
use rand::{rngs::StdRng, Rng, SeedableRng};
use serde_json::Value;

use crate::{
    game::state_engine::{SpaceAction, SpaceObservation},
    SpaceGame,
};

/// Factory for the simple deterministic Space Game back-and-forth agent.
pub struct BackAndForthAgentFactory;

#[async_trait]
impl AgentFactory<SpaceGame> for BackAndForthAgentFactory {
    fn definition(&self) -> AgentDefinition {
        AgentDefinition {
            id: "space_game.back_and_forth".to_string(),
            name: "Back and forth".to_string(),
            description:
                "Built-in deterministic Space Game agent for local experiments and testing."
                    .to_string(),
            config_fields: Vec::new(),
        }
    }

    async fn create(&self, context: AgentContext) -> Result<Box<dyn Agent<SpaceGame> + Send>> {
        Ok(Box::new(BackAndForthAgent::new(
            context.role.as_str().to_string(),
            context.seed,
        )))
    }

    fn identity(&self, _settings: &Value) -> Result<AgentIdentity> {
        Ok(AgentIdentity {
            name: "BackAndForthAgent".to_string(),
            version: Some(env!("CARGO_PKG_VERSION").to_string()),
        })
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
    fn new(role: String, seed: u64) -> Self {
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
impl Agent<SpaceGame> for BackAndForthAgent {
    /// Receives the first role-safe world snapshot after agent initialization.
    async fn start(&mut self, initial_observation: SpaceObservation) -> Result<()> {
        self.update_other_player_position(&initial_observation);
        self.current_observation = Some(initial_observation);
        Ok(())
    }

    /// Updates movement behavior and the latest role-safe world snapshot after an action.
    async fn observe_transition(
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
    async fn observe_message(&mut self, speaker: PlayerRole, text: String) -> Result<()> {
        if speaker.as_str() != self.role && !text.trim().is_empty() {
            self.pending_question = Some((speaker, text));
        }
        Ok(())
    }

    /// Answers a pending question before considering the agent's next movement.
    async fn respond(
        &mut self,
        _available_actions: Option<Vec<SpaceAction>>,
    ) -> Result<Option<Response<SpaceAction>>> {
        if let Some((speaker, question)) = self.pending_question.take() {
            return Ok(Some(Response::message(
                self.answer_world_question(speaker, &question),
            )));
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
        Ok(Some(match message {
            Some(message) => Response::action_and_message(action, message),
            None => Response::action(action),
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
    use parlando_server::{agent::Context as AgentContext, Game, PlayerRole};

    use crate::game::state_engine::initial_state;

    use super::*;

    fn init_context(role: PlayerRole) -> AgentContext {
        AgentContext {
            role,
            seed: 1,
            settings: Value::Null,
        }
    }

    /// Creates and starts one test agent for the given role.
    async fn started_agent(role: PlayerRole) -> BackAndForthAgent {
        let adapter = SpaceGame::new();
        let mut agent = BackAndForthAgent::new(role.as_str().to_string(), 1);
        agent
            .start(adapter.observation(&initial_state(), role))
            .await
            .unwrap();
        agent
    }

    #[tokio::test]
    async fn back_and_forth_factory_returns_fresh_agents() {
        let factory = BackAndForthAgentFactory;
        let mut first = factory.create(init_context(PlayerRole::A)).await.unwrap();
        let mut second = factory.create(init_context(PlayerRole::A)).await.unwrap();
        let initial = SpaceGame::new().observation(&initial_state(), PlayerRole::A);
        first.start(initial.clone()).await.unwrap();
        second.start(initial).await.unwrap();

        let first_result = first.respond(Some(vec![])).await.unwrap().unwrap();
        let second_result = second.respond(Some(vec![])).await.unwrap().unwrap();

        assert!(matches!(
            first_result,
            Response::Action(SpaceAction::MoveStep { direction, .. }) if direction == "left"
        ));
        assert!(matches!(
            second_result,
            Response::Action(SpaceAction::MoveStep { direction, .. }) if direction == "left"
        ));
    }

    #[tokio::test]
    async fn back_and_forth_agent_speaks_when_other_player_moves() {
        let adapter = SpaceGame::new();
        let mut agent = started_agent(PlayerRole::B).await;
        let _ = agent.respond(Some(vec![])).await.unwrap();
        agent.last_step_at = Instant::now() - Duration::from_secs(60);
        let mut moved_state = initial_state();
        moved_state.players.a.position.x += 1;
        let second_observation = adapter.observation(&moved_state, PlayerRole::B);
        agent
            .observe_transition(
                PlayerRole::A,
                SpaceAction::MoveStep {
                    player: "A".to_string(),
                    direction: "right".to_string(),
                },
                second_observation,
            )
            .await
            .unwrap();

        let result = agent.respond(Some(vec![])).await.unwrap().unwrap();

        assert!(matches!(
            result,
            Response::ActionAndMessage { action: SpaceAction::MoveStep { direction, .. }, ref message }
                if direction == "down" && !message.is_empty()
        ));
    }

    #[tokio::test]
    async fn back_and_forth_agent_does_not_chain_after_its_own_move() {
        let adapter = SpaceGame::new();
        let observation = adapter.observation(&initial_state(), PlayerRole::B);
        let mut agent = started_agent(PlayerRole::B).await;
        let first_result = agent.respond(Some(vec![])).await.unwrap();
        assert!(matches!(first_result, Some(Response::Action(_))));
        agent.last_step_at = Instant::now() - Duration::from_secs(60);
        agent
            .observe_transition(
                PlayerRole::B,
                SpaceAction::MoveStep {
                    player: "B".to_string(),
                    direction: "up".to_string(),
                },
                observation,
            )
            .await
            .unwrap();

        assert!(agent.respond(Some(vec![])).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn back_and_forth_agent_answers_spoken_world_questions_without_moving() {
        let mut agent = started_agent(PlayerRole::B).await;
        let _ = agent.respond(Some(vec![])).await.unwrap();
        agent
            .observe_message(PlayerRole::A, "Wo bist du?".to_string())
            .await
            .unwrap();

        let response = agent.respond(Some(vec![])).await.unwrap().unwrap();

        assert_eq!(
            response,
            Response::Message("Ich bin im Raum valve bei Position 9, 6.".to_string())
        );
    }

    #[tokio::test]
    async fn back_and_forth_agent_reports_visible_launch_blockers() {
        let mut agent = started_agent(PlayerRole::B).await;
        agent
            .observe_message(PlayerRole::A, "Sind wir startbereit?".to_string())
            .await
            .unwrap();

        let response = agent.respond(Some(vec![])).await.unwrap().unwrap();
        let Response::Message(message) = response else {
            panic!("expected a message-only response")
        };

        assert!(message.contains("Noch nicht startbereit"));
        assert!(message.contains("stabile Energie"));
        assert!(message.contains("stabiler Sauerstoff"));
    }
}
