//! Agent fixture for the host's headless counter game and metering tests.

use parlando_game_sdk::{
    agent::{async_trait, Agent, Definition, InitContext, RespondReason, Response, Result},
    export_agent, GameValue, Role,
};
use serde::Deserialize;

/// Empty settings accepted by the counter policy.
#[derive(Deserialize)]
pub struct Config {}

/// Action exchanged with the counter game.
#[derive(GameValue)]
pub struct Action {
    amount: u8,
}

/// Complete role-private view of the counter.
#[derive(GameValue)]
pub struct View {
    count: u8,
    role: String,
}

/// Shared terminal counter.
#[derive(GameValue)]
pub struct Completion {
    count: u8,
}

/// Plays one increment from the first seat.
pub struct CounterPolicy(Role);

#[async_trait]
impl Agent for CounterPolicy {
    type Config = Config;
    type Action = Action;
    type View = View;
    type Completion = Completion;

    /// Registers the counter fixture under the ID used by the runner tests.
    fn definition() -> Definition {
        Definition {
            id: "tiny.agent",
            name: "Counter policy fixture",
            version: "1",
            description: "Increments the counter from seat A.",
            config_fields: "[]",
        }
    }

    /// Stores the assigned seat so only seat A offers an increment.
    async fn init(context: InitContext<Config>) -> Result<Self> {
        Ok(Self(context.role))
    }

    /// Proposes an increment from the first seat and yields from the other seat.
    async fn respond(&mut self, _reason: RespondReason) -> Result<Response<Action>> {
        Ok(match self.0 {
            Role::A => Response::action(Action { amount: 1 }),
            Role::B => Response::default(),
        })
    }
}

export_agent!(CounterPolicy);
