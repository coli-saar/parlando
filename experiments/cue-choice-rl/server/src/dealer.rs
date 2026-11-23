//! The dealer acts once when seat B's view offers a deal.
use crate::{CueChoiceAction, CueChoiceCompletion, CueChoiceObservation};
use parlando_game_sdk::agent::{
    async_trait, Agent, Definition, InitContext, Observation, RespondReason, Response, Result, Role,
};

/// The dealer accepts no agent settings.
#[derive(serde::Deserialize)]
pub struct Config {}

/// Tracks whether the game has offered the dealer its turn.
pub struct DealerAgent {
    ready: bool,
}

#[async_trait]
impl Agent for DealerAgent {
    type Config = Config;
    type Action = CueChoiceAction;
    type View = CueChoiceObservation;
    type Completion = CueChoiceCompletion;

    /// Identifies this seat-B agent to the experiment runner and deployment host.
    fn definition() -> Definition {
        Definition {
            id: "cue_choice.dealer",
            name: "Cue Choice dealer",
            version: "1",
            description: "Deals the private cue with one deterministic typed action.",
            config_fields: "[]",
        }
    }
    /// Enforces the dealer's seat before receiving any game state.
    async fn init(context: InitContext<Config>) -> Result<Self> {
        if context.role != Role::B {
            return Err(anyhow::anyhow!("cue-choice dealer must occupy player B"));
        }
        Ok(Self { ready: false })
    }
    /// Reads the initial role-private view.
    async fn start(&mut self, view: Self::View) -> Result<()> {
        self.ready = matches!(view, CueChoiceObservation::ReadyToDeal);
        Ok(())
    }
    /// Updates readiness from each accepted game transition.
    async fn observe(&mut self, event: Observation<Self::Action, Self::View>) -> Result<()> {
        if let Some(transition) = event.transition {
            self.start(transition.view).await?;
        }
        Ok(())
    }
    /// Offers a deal whenever the current view permits it.
    async fn respond(&mut self, _: RespondReason) -> Result<Response<Self::Action>> {
        Ok(if self.ready {
            Response::action(CueChoiceAction::Deal)
        } else {
            Response::default()
        })
    }
}
