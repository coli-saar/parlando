//! Back and forth uses the same typed callbacks for native and component play.
use crate::game::state_engine::{SpaceAction, SpaceCompletion, SpaceObservation};
use parlando_game_sdk::agent::{
    async_trait, Agent, Definition, InitContext, Observation, RespondReason, Response, Result,
};
use parlando_game_sdk::Role as PlayerRole;
use rand::{rngs::StdRng, Rng, SeedableRng};
use std::time::Duration;

/// The movement agent has no dashboard settings.
#[derive(serde::Deserialize)]
pub struct Config {}

/// Deterministic Space Game agent that moves, comments, and answers visible-world questions.
pub struct BackAndForthAgent {
    role: String,
    rng: StdRng,
    step_index: usize,
    cooling_down: bool,
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
            cooling_down: false,
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
impl Agent for BackAndForthAgent {
    type Config = Config;
    type Action = SpaceAction;
    type View = SpaceObservation;
    type Completion = SpaceCompletion;

    /// Identifies the agent for native and uploaded games.
    fn definition() -> Definition {
        Definition {
            id: "space_game.back_and_forth",
            name: "Back and forth",
            version: "1",
            description:
                "Moves in response to its partner and answers questions about its visible station.",
            config_fields: "[]",
        }
    }

    /// Seeds one seat's route and choice of messages.
    async fn init(context: InitContext<Config>) -> Result<Self> {
        Ok(Self::new(context.role.as_str().into(), context.seed))
    }

    /// Retains the current role-private snapshot for station questions.
    async fn start(&mut self, view: SpaceObservation) -> Result<()> {
        self.current_observation = Some(view);
        Ok(())
    }

    /// Reacts to partner moves and answers questions against the latest view.
    async fn observe(&mut self, event: Observation<SpaceAction, SpaceObservation>) -> Result<()> {
        if let Some(transition) = event.transition {
            if transition.actor.as_str() != self.role {
                self.pending_step = true;
                self.other_player_moved = true;
            }
            self.start(transition.view).await?;
        }
        if let Some(message) = event.message {
            if message.sender.as_str() != self.role && !message.text.trim().is_empty() {
                self.pending_question = Some((message.sender, message.text));
            }
        }
        Ok(())
    }

    /// Uses the live host timer to space moves. A headless schedule supplies its own pacing.
    async fn respond(&mut self, reason: RespondReason) -> Result<Response<SpaceAction>> {
        let live = matches!(
            reason,
            RespondReason::SessionStarted
                | RespondReason::AfterObservation
                | RespondReason::WakeupDue
        );
        if matches!(reason, RespondReason::WakeupDue | RespondReason::Scheduled) {
            self.cooling_down = false;
        }
        if let Some((speaker, question)) = self.pending_question.take() {
            let response = Response::message(self.answer_world_question(speaker, &question));
            return Ok(if live && self.pending_step && !self.cooling_down {
                response.wake_after(Duration::ZERO)
            } else {
                response
            });
        }
        if !self.pending_step || self.cooling_down {
            return Ok(Response::default());
        }
        self.cooling_down = live;
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
        let response = match message {
            Some(message) => Response::action_and_message(action, message),
            None => Response::action(action),
        };
        Ok(if live {
            response.wake_after(Duration::from_secs(1))
        } else {
            response
        })
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::state_engine::initial_state;
    use parlando_game_sdk::agent::{Message, Transition};

    /// Starts an independent agent using the same callbacks as a packaged policy.
    async fn started_agent(role: PlayerRole) -> BackAndForthAgent {
        let mut agent = BackAndForthAgent::init(InitContext {
            config: Config {},
            role,
            seed: 1,
        })
        .await
        .unwrap();
        agent
            .start(crate::SpaceGame::observation(&initial_state(), role))
            .await
            .unwrap();
        agent
    }

    /// Delivers a message from the other seat.
    async fn ask(agent: &mut BackAndForthAgent, text: &str) -> String {
        agent
            .observe(Observation {
                transition: None,
                message: Some(Message {
                    sender: PlayerRole::A,
                    text: text.into(),
                }),
            })
            .await
            .unwrap();
        let response = agent
            .respond(RespondReason::AfterObservation)
            .await
            .unwrap();
        assert!(response.action.is_none());
        response.message.unwrap()
    }

    #[tokio::test]
    async fn back_and_forth_agent_speaks_when_other_player_moves() {
        let mut agent = started_agent(PlayerRole::B).await;
        let first = agent.respond(RespondReason::SessionStarted).await.unwrap();
        assert!(first.action.is_some());
        assert!(first.wakeup.is_some());
        let view = crate::SpaceGame::observation(&initial_state(), PlayerRole::B);
        agent
            .observe(Observation {
                message: None,
                transition: Some(Transition {
                    actor: PlayerRole::A,
                    action: SpaceAction::MoveStep {
                        player: "A".into(),
                        direction: "right".into(),
                    },
                    view,
                }),
            })
            .await
            .unwrap();
        assert!(agent
            .respond(RespondReason::AfterObservation)
            .await
            .unwrap()
            .action
            .is_none());
        let next = agent.respond(RespondReason::WakeupDue).await.unwrap();
        assert!(
            matches!(next.action, Some(SpaceAction::MoveStep { direction, .. }) if direction == "down")
        );
        assert!(next.message.is_some());
        assert!(agent
            .respond(RespondReason::WakeupDue)
            .await
            .unwrap()
            .action
            .is_none());
    }

    /// Every fresh session starts at the first step of the selected seat's route.
    #[tokio::test]
    async fn fresh_agents_start_their_own_routes() {
        for (role, first) in [
            (PlayerRole::A, "left"),
            (PlayerRole::B, "up"),
            (PlayerRole::A, "left"),
        ] {
            let mut agent = started_agent(role).await;
            let response = agent.respond(RespondReason::SessionStarted).await.unwrap();
            assert!(
                matches!(response.action, Some(SpaceAction::MoveStep { direction, .. }) if direction == first)
            );
        }
    }

    /// Headless sessions reject timer requests and control scheduling themselves.
    #[tokio::test]
    async fn scheduled_move_has_no_live_wakeup() {
        let mut agent = started_agent(PlayerRole::A).await;
        let response = agent.respond(RespondReason::Scheduled).await.unwrap();
        assert!(response.action.is_some());
        assert!(response.wakeup.is_none());
    }

    #[tokio::test]
    async fn back_and_forth_agent_answers_spoken_world_questions_without_moving() {
        let mut agent = started_agent(PlayerRole::B).await;
        assert_eq!(
            ask(&mut agent, "Wo bist du?").await,
            "Ich bin im Raum valve bei Position 9, 6."
        );
        let blockers = ask(&mut agent, "Sind wir startbereit?").await;
        assert!(blockers.contains("Noch nicht startbereit"));
        assert!(blockers.contains("stabile Energie"));
        assert!(blockers.contains("stabiler Sauerstoff"));
    }
}
