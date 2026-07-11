use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{bail, Result};
use async_trait::async_trait;
use parlando_server::{
    AgentActContext, AgentFactory, AgentInitContext, AgentResult, ExperimentConfig, GameAgent,
};
use rand::{rngs::StdRng, Rng, SeedableRng};
use serde_json::Value;

use crate::{
    game::state_engine::{SpaceAction, SpaceObservation},
    SpaceGameAdapter,
};

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
        other => bail!("unknown Space Game agent factory selector: {other}"),
    }
}

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
}

pub struct BackAndForthAgent {
    role: String,
    rng: StdRng,
    step_index: usize,
    last_step_at: Instant,
    last_other_position: Option<(i64, i64)>,
    utterances: Vec<&'static str>,
}

impl BackAndForthAgent {
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
        _available_actions: Vec<SpaceAction>,
        _context: AgentActContext,
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
