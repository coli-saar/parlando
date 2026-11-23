//! A Beacon partner that spends one chat request per one-tick pulse.

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
    greeting_pending: bool,
    your_turn: bool,
    pending_ticks: usize,
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
            description: "Adds one Beacon tick on request.",
            config_fields: "[]",
        }
    }

    /// Initializes session state before the host delivers the first view.
    async fn init(_context: InitContext<Self::Config>) -> Result<Self> {
        Ok(Self {
            greeting_pending: true,
            your_turn: false,
            pending_ticks: 0,
        })
    }

    /// Records whether the agent can take the opening turn.
    async fn start(&mut self, view: Self::View) -> Result<()> {
        self.your_turn = view.your_turn;
        Ok(())
    }

    /// Updates turn ownership and queues one pulse per partner command.
    async fn observe(&mut self, event: Observation<Self::Action, Self::View>) -> Result<()> {
        if let Some(transition) = event.transition {
            self.your_turn = transition.view.your_turn;
        }
        if let Some(message) = event.message {
            let command = message
                .text
                .trim()
                .trim_end_matches(['!', '.', '?'])
                .trim_end();
            if ["tick", "do a tick", "please tick", "tick please"]
                .iter()
                .any(|phrase| command.eq_ignore_ascii_case(phrase))
            {
                self.pending_ticks = self.pending_ticks.saturating_add(1);
            }
        }
        Ok(())
    }

    /// Greets the partner once and consumes at most one request on each legal turn.
    async fn respond(&mut self, _reason: RespondReason) -> Result<Response<Self::Action>> {
        let mut response = Response::default();
        if self.greeting_pending {
            self.greeting_pending = false;
            response.message = Some("Send me “tick” in the chat whenever you want me to add one tick to the beacon. If it’s your turn, I’ll wait for mine.".into());
        }
        if self.your_turn && self.pending_ticks > 0 {
            self.pending_ticks -= 1;
            self.your_turn = false;
            response.action = Some(game::Action { ticks: 1 });
        }
        Ok(response)
    }
}

export_agent!(Policy);
