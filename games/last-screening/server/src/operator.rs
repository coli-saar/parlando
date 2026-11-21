//! Deterministic cooperative partner. Its inputs are its own observation,
//! messages from the other player, and the runtime's legal-action catalogue.

use crate::agent_response::Response;
use parlando::component::ComponentValue;

use anyhow::Result;
use async_trait::async_trait;
use parlando::{
    agent::{Agent, Context, Definition, Factory, Identity},
    PlayerRole,
};
use serde_json::Value;

#[cfg(test)]
use crate::Screening;
use crate::{Action, Flat, Lantern, Observation, Room, Slide, POSITIONS};

/// Registers a configuration-free cooperative partner that can play either seat.
pub struct ScreeningPartnerFactory;

#[async_trait]
impl Factory for ScreeningPartnerFactory {
    /// Lists the in-process partner among the server's available agent factories.
    fn definition(&self) -> Definition {
        Definition {
            id: "last_screening.operator".into(),
            name: "Screening partner".into(),
            description: "Shares its cue in chat and operates the booth or backstage controls."
                .into(),
            config_fields: Vec::new(),
        }
    }

    /// Builds a fresh partner with the cues and controls for its assigned seat.
    async fn create(&self, context: Context) -> Result<Box<dyn Agent + Send>> {
        Ok(Box::new(ScreeningPartner::new(context.role)))
    }

    /// Identifies the fixed implementation independently of game sessions.
    fn identity(&self, _: &Value) -> Result<Identity> {
        Ok(Identity {
            name: "ScreeningPartner".into(),
            version: "1".into(),
        })
    }
}

impl Room {
    /// Returns the room's cue label and mark, followed by the label it must get in chat.
    fn cue_exchange(self) -> (&'static str, u8, &'static str) {
        match self {
            Self::Projectionist { cue, .. } => ("tower", cue.tower, "window"),
            Self::Stagehand { cue, .. } => ("window", cue.window, "tower"),
        }
    }
}

/// Session-local cue exchange and control of one room.
struct ScreeningPartner {
    role: PlayerRole,
    view: Option<Observation>,
    other_mark: Option<u8>,
    announce_cue: bool,
}

impl ScreeningPartner {
    /// Initializes a seat before the runtime supplies the first observation.
    fn new(role: PlayerRole) -> Self {
        Self {
            role,
            view: None,
            other_mark: None,
            announce_cue: false,
        }
    }

    /// Keeps the current reel's conversation and prompts when a new reel begins.
    fn update_view(&mut self, view: Observation) {
        if view.phase == "escape" {
            self.announce_cue = false;
        } else if self.view.as_ref().is_none_or(|old| old.level != view.level) {
            self.other_mark = None;
            self.announce_cue = true;
        }
        self.view = Some(view);
    }

    /// Chooses the next action using the exchanged cues and current controls.
    fn next_action(&self, available: &[Action]) -> Option<Action> {
        let view = self.view.as_ref()?;
        if view.phase == "escape" {
            return available.contains(&Action::Exit).then_some(Action::Exit);
        }
        let other_mark = self.other_mark?;
        match view.room {
            Room::Projectionist { cue, lanterns } => {
                Self::projection_action(view, &lanterns, cue.tower, other_mark, available)
            }
            Room::Stagehand { cue, .. } => [
                Action::MoveFlat {
                    flat: Flat::Window,
                    position: cue.window,
                },
                Action::MoveFlat {
                    flat: Flat::Tower,
                    position: other_mark,
                },
            ]
            .into_iter()
            .find(|choice| available.contains(choice)),
        }
    }

    /// Aims through the shared window and follows the reel's ship-crossing cues.
    fn projection_action(
        view: &Observation,
        lanterns: &[Lantern; 2],
        tower: u8,
        window: u8,
        available: &[Action],
    ) -> Option<Action> {
        let window_slide = match view.level.as_ref() {
            "moonrise" => Slide::Moon,
            "crossing" => Slide::Light,
            _ => return None,
        };
        // Keep a slide in its existing carrier so an occupied carrier never
        // has to be emptied just to transfer the same slide between lanterns.
        let window_lantern = lanterns
            .iter()
            .find(|lantern| lantern.slide == window_slide)
            .or_else(|| lanterns.iter().find(|lantern| lantern.slide != Slide::Ship))?;
        let window_actions = Self::project(window_lantern.id, window_slide, window);
        if view.level == "moonrise" {
            return window_actions
                .into_iter()
                .find(|choice| available.contains(choice));
        }

        let ship_lantern = lanterns.iter().find(|item| item.id != window_lantern.id)?;
        let bearing = match view.phase.as_ref() {
            "align" => *POSITIONS.start(),
            "hide" => tower,
            "emerge" => *POSITIONS.end(),
            _ => return None,
        };
        window_actions
            .into_iter()
            .chain(Self::project(ship_lantern.id, Slide::Ship, bearing))
            .find(|choice| available.contains(choice))
    }

    /// Sets the desired bearing before loading, avoiding an unintended exposure.
    fn project(lantern: u8, slide: Slide, position: u8) -> [Action; 2] {
        [
            Action::Aim { lantern, position },
            Action::Load { lantern, slide },
        ]
    }
}

/// Reads a one-based cue as either a number or the expected cue name and number.
fn parse_mark(text: &str, cue: &str) -> Option<u8> {
    let text = text.split_once(". Send me ").map_or(text, |(mark, _)| mark);
    let mut words = text.trim().trim_end_matches(['.', '!']).split_whitespace();
    let first = words.next()?;
    let mark = if first.eq_ignore_ascii_case(cue) {
        match words.next()? {
            at if at.eq_ignore_ascii_case("at") => words.next()?,
            mark => mark,
        }
    } else {
        first
    };
    let position = mark.parse::<u8>().ok()?.checked_sub(1)?;
    (words.next().is_none()
        && POSITIONS.contains(&position)
        && (cue != "tower" || position > *POSITIONS.start() && position < *POSITIONS.end()))
    .then_some(position)
}

impl ScreeningPartner {
    /// Receives the partner's first role-filtered cue and controls.
    async fn start(&mut self, observation: Observation) -> Result<()> {
        self.update_view(observation);
        Ok(())
    }

    /// Refreshes the view after any accepted manipulation, including its own.
    async fn observe_transition(
        &mut self,
        _: PlayerRole,
        _: Action,
        view: Observation,
    ) -> Result<()> {
        self.update_view(view);
        Ok(())
    }

    /// Accepts a new or corrected mark and responds to malformed or impossible cues.
    async fn observe_message(&mut self, sender: PlayerRole, text: String) -> Result<()> {
        let Some(view) = self.view.as_ref() else {
            return Ok(());
        };
        if self.role == sender || view.phase == "escape" {
            return Ok(());
        }
        let (_, own_mark, cue) = view.room.cue_exchange();
        let mark = parse_mark(&text, cue).filter(|mark| *mark != own_mark);
        if let Some(mark) = mark {
            self.other_mark = Some(mark);
        } else {
            self.announce_cue = true;
        }
        Ok(())
    }

    /// Sends the requested cue and takes any immediately available step.
    async fn respond(&mut self) -> Result<Option<Response<Action>>> {
        let action = self.next_action(
            self.view
                .as_ref()
                .map_or(&[], |view| view.actions.as_slice()),
        );
        let message = std::mem::take(&mut self.announce_cue)
            .then(|| self.view.as_ref())
            .flatten()
            .map(|view| {
                let (own, mark, other) = view.room.cue_exchange();
                format!(
                    "{own} {}. Send me the number on your {other} cue.",
                    mark + 1
                )
            });
        Ok(match (action, message) {
            (Some(action), Some(message)) => Some(Response::action_and_message(action, message)),
            (Some(action), None) => Some(Response::action(action)),
            (None, Some(message)) => Some(Response::message(message)),
            (None, None) => None,
        })
    }
}
#[async_trait]
impl Agent for ScreeningPartner {
    /// Delivers the decoded starting view to the policy.
    async fn start(&mut self, view: ComponentValue) -> Result<()> {
        Self::start(self, view.decode()?).await
    }

    /// Delivers the accepted action and this seat's decoded view.
    async fn observe_transition(
        &mut self,
        actor: PlayerRole,
        action: ComponentValue,
        view: ComponentValue,
    ) -> Result<()> {
        Self::observe_transition(self, actor, action.decode()?, view.decode()?).await
    }

    /// Delivers chat without modifying game state.
    async fn observe_message(&mut self, actor: PlayerRole, text: String) -> Result<()> {
        Self::observe_message(self, actor, text).await
    }

    /// Encodes a typed policy decision for the shared component host.
    async fn respond(&mut self) -> Result<Option<parlando::agent::Response>> {
        Self::respond(self)
            .await?
            .map(Response::into_wire)
            .transpose()
    }
}

#[cfg(test)]
mod tests {
    /// Maps the transport seat to a component rules seat.
    fn guest_role(role: PlayerRole) -> parlando_game_sdk::Role {
        match role {
            PlayerRole::A => parlando_game_sdk::Role::A,
            PlayerRole::B => parlando_game_sdk::Role::B,
        }
    }

    use super::*;
    use crate::{Config, ProjectionistSeat, State};

    /// Exercises accepted speech forms and rejects wrong cue names and unusable tower marks.
    #[test]
    fn numbered_marks_match_the_requested_room() {
        assert_eq!(parse_mark("4", "tower"), Some(3));
        assert_eq!(parse_mark("Tower at 4.", "tower"), Some(3));
        assert_eq!(parse_mark("window 7", "window"), Some(6));
        assert_eq!(parse_mark("window 4", "tower"), None);
        assert_eq!(parse_mark("tower 7", "tower"), None);
        assert_eq!(parse_mark("0", "window"), None);
    }

    /// A cue arriving early or after a typo still leaves the agent's own cue available.
    #[tokio::test]
    async fn early_and_corrected_marks_do_not_lose_the_other_cue() {
        let state = State::deal(82);
        let mut partner = ScreeningPartner::new(PlayerRole::B);
        partner
            .start(Screening::observation(&state, guest_role(PlayerRole::B)))
            .await
            .unwrap();
        let cue = format!("tower {}", state.cue().tower + 1);
        let window = Action::MoveFlat {
            flat: Flat::Window,
            position: state.cue().window,
        };
        partner
            .observe_message(PlayerRole::A, cue.clone())
            .await
            .unwrap();
        assert!(matches!(
            partner.respond().await.unwrap(),
            Some(Response::ActionAndMessage { action, message })
                if action == window && message.starts_with(&format!("window {}.", state.cue().window + 1))
        ));
        partner
            .observe_message(PlayerRole::A, "What is my cue?".into())
            .await
            .unwrap();
        assert!(matches!(
            partner.respond().await.unwrap(),
            Some(Response::ActionAndMessage { action, message })
                if action == window && message.starts_with(&format!("window {}.", state.cue().window + 1))
        ));
        partner.observe_message(PlayerRole::A, cue).await.unwrap();
        assert_eq!(
            partner.respond().await.unwrap(),
            Some(Response::action(window))
        );
    }

    /// Two independent partners share only what each human seat could see and finish both reels.
    #[tokio::test]
    async fn partners_clear_both_reels_with_either_seat_assignment() {
        for projectionist_seat in [ProjectionistSeat::A, ProjectionistSeat::B] {
            for seed in 0..20 {
                let mut state = State {
                    projectionist_seat,
                    ..State::deal(seed)
                };
                let roles = [PlayerRole::A, PlayerRole::B];
                let mut partners = roles.map(ScreeningPartner::new);
                for (index, role) in roles.into_iter().enumerate() {
                    partners[index]
                        .start(Screening::observation(&state, guest_role(role)))
                        .await
                        .unwrap();
                }
                for _ in 0..40 {
                    for (index, role) in roles.into_iter().enumerate() {
                        let (action, message) = match partners[index].respond().await.unwrap() {
                            Some(Response::Action(action)) => (Some(action), None),
                            Some(Response::Message(message)) => (None, Some(message)),
                            Some(Response::ActionAndMessage { action, message }) => {
                                (Some(action), Some(message))
                            }
                            None => (None, None),
                        };
                        if let Some(action) = action {
                            state =
                                Screening::apply_action(&state, &action, guest_role(role)).unwrap();
                            for (partner_index, recipient) in roles.into_iter().enumerate() {
                                partners[partner_index]
                                    .observe_transition(
                                        role,
                                        action,
                                        Screening::observation(&state, guest_role(recipient)),
                                    )
                                    .await
                                    .unwrap();
                            }
                        }
                        if let Some(message) = message {
                            partners[1 - index]
                                .observe_message(role, message)
                                .await
                                .unwrap();
                        }
                    }
                    if Screening::outcome(&state).is_some() {
                        break;
                    }
                }
                assert!(
                    Screening::outcome(&state).is_some(),
                    "seat {projectionist_seat:?}, seed {seed}"
                );
            }
        }
    }

    /// Config accepts explicit seat B, and historical persisted states imply seat A.
    #[test]
    fn projectionist_seat_defaults_and_round_trips() {
        assert_eq!(
            serde_json::from_str::<Config>("{}")
                .unwrap()
                .projectionist_seat,
            None
        );
        assert_eq!(
            serde_json::from_str::<Config>(r#"{"projectionist_seat":"b"}"#)
                .unwrap()
                .projectionist_seat,
            Some(ProjectionistSeat::B)
        );
        let previous = State::deal(5);
        let serialized = serde_json::to_value(&previous).unwrap();
        assert!(serialized.get("projectionist_seat").is_none());
        assert_eq!(
            serde_json::from_value::<State>(serialized).unwrap(),
            previous
        );
        let swapped = State {
            projectionist_seat: ProjectionistSeat::B,
            ..previous
        };
        let serialized = serde_json::to_value(&swapped).unwrap();
        assert_eq!(serialized["projectionist_seat"], "b");
        assert_eq!(
            serde_json::from_value::<State>(serialized).unwrap(),
            swapped
        );
    }
}
