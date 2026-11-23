//! Standalone policy for the generated Beacon Relay release candidate game.

use parlando_game_sdk::{
    agent::{
        async_trait, Agent, Definition, InitContext, Observation, RespondReason, Response, Result,
    },
    export_agent, GameValue, Role,
};
use serde::Deserialize;

const PARTNER_MESSAGE: &str = "One tick from the Beacon partner.";

/// The release proof needs no policy settings.
#[derive(Deserialize)]
pub struct Config {}

/// A charge pulse matching the generated game.
#[derive(GameValue)]
pub struct Pulse {
    ticks: u8,
}

/// Seed-derived target and turn information visible to this seat.
#[derive(GameValue)]
pub struct View {
    charge: u8,
    target: u8,
    your_turn: bool,
}

/// The charge at which the generated game completed.
#[derive(GameValue)]
pub struct Outcome {
    activated_at: u8,
}

/// A policy that charges one tick on every turn.
pub struct BeaconPartner {
    view: Option<View>,
    requests_ticks: bool,
    first_request_pending: bool,
}

#[async_trait]
impl Agent for BeaconPartner {
    type Config = Config;
    type Action = Pulse;
    type View = View;
    type Completion = Outcome;

    /// Names the standalone policy for package checking and host selection.
    fn definition() -> Definition {
        Definition {
            id: "release.beacon",
            name: "Release Beacon partner",
            version: "1",
            description: "Adds one tick when the beacon is ready for this player.",
            config_fields: "[]",
        }
    }

    /// Allocates policy state before the game provides its first view.
    async fn init(_context: InitContext<Config>) -> Result<Self> {
        Ok(Self {
            view: None,
            requests_ticks: false,
            first_request_pending: false,
        })
    }

    /// Stores the initial view, including permission to act first.
    async fn start(&mut self, view: View) -> Result<()> {
        self.view = Some(view);
        Ok(())
    }

    /// Updates turn permission and charge from each accepted transition.
    async fn observe(&mut self, observation: Observation<Pulse, View>) -> Result<()> {
        if observation
            .message
            .as_ref()
            .is_some_and(|message| message.text.contains("“charge the beacon”"))
        {
            self.requests_ticks = true;
            self.first_request_pending = true;
        }
        if let Some(transition) = observation.transition {
            if transition.view.charge == 2
                && transition.view.your_turn
                && !observation.message.as_ref().is_some_and(|message| {
                    message.sender == Role::B && message.text == PARTNER_MESSAGE
                })
            {
                return Err(std::io::Error::other(
                    "the second tick and the partner message were not delivered together",
                )
                .into());
            }
            self.view = Some(transition.view);
        }
        Ok(())
    }

    /// Plays one legal tick or yields until the game's next update.
    async fn respond(&mut self, _reason: RespondReason) -> Result<Response<Pulse>> {
        if self.requests_ticks {
            let first = std::mem::take(&mut self.first_request_pending);
            return Ok(if self.view.as_ref().is_some_and(|view| view.your_turn) {
                Response::action_and_message(Pulse { ticks: 1 }, "charge the beacon")
            } else if first {
                Response::message("charge the beacon")
            } else {
                Response::default()
            });
        }
        match &self.view {
            Some(view) if view.your_turn && view.charge == 1 => Ok(Response::action_and_message(
                Pulse { ticks: 1 },
                PARTNER_MESSAGE,
            )),
            Some(view) if view.your_turn && view.charge < view.target => {
                Ok(Response::action(Pulse { ticks: 1 }))
            }
            _ => Ok(Response::default()),
        }
    }

    /// Checks that the terminal result agrees with the last committed transition.
    async fn finish(&mut self, outcome: Outcome) -> Result<()> {
        if !self.view.as_ref().is_some_and(|view| {
            view.charge == outcome.activated_at && view.target == outcome.activated_at
        }) {
            return Err(
                std::io::Error::other("beacon completion disagrees with the final view").into(),
            );
        }
        Ok(())
    }
}

export_agent!(BeaconPartner);
