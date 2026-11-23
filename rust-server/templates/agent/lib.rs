//! Implement your policy here. Regenerate the game types separately.

use parlando_game_sdk::agent::{
    async_trait, export_agent, Agent, Definition, InitContext, Observation, RespondReason,
    Response, Result,
};

/// The game wire types generated from the target game schema.
pub mod game;

/// Dashboard settings read when the policy starts.
#[derive(serde::Deserialize)]
struct Settings {}

/// Session-local state for the policy.
struct Policy {
    view: Option<<Self as Agent>::View>,
}

#[async_trait]
impl Agent for Policy {
    type Config = Settings;
    type Action = game::Action;
    // Use game::ViewB here to target seat B when the layouts differ.
    type View = game::ViewA;
    type Completion = game::Completion;

    /// Identifies the policy in an agent archive and on the dashboard.
    fn definition() -> Definition {
        Definition {
            id: "__AGENT_ID__",
            name: __AGENT_NAME__,
            version: env!("CARGO_PKG_VERSION"),
            description: "A Parlando game policy.",
            config_fields: "[]",
        }
    }

    /// Initializes session state before the host delivers the first view.
    async fn init(_context: InitContext<Self::Config>) -> Result<Self> {
        Ok(Self { view: None })
    }

    /// Stores the complete initial view.
    async fn start(&mut self, view: Self::View) -> Result<()> {
        self.view = Some(view);
        Ok(())
    }

    /// Replaces the retained view when the game accepts an action.
    async fn observe(&mut self, event: Observation<Self::Action, Self::View>) -> Result<()> {
        if let Some(transition) = event.transition {
            self.view = Some(transition.view);
        }
        Ok(())
    }

    /// Returns a decision using the most recent view. Add the policy's logic here.
    async fn respond(&mut self, _reason: RespondReason) -> Result<Response<Self::Action>> {
        Ok(Response::default())
    }
}

export_agent!(Policy);
