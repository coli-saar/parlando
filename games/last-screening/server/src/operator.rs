//! Deterministic cooperative partner. Its inputs are its own observation,
//! messages from the other player, and the runtime's legal-action catalogue.

#[cfg(test)]
use crate::Screening;
use crate::{Action, Completion, Flat, Lantern, Room, ScreeningView, Slide, POSITIONS};
use parlando_game_sdk::agent::{
    Agent, Definition, InitContext, Observation, RespondReason, Response, Result,
};
use parlando_game_sdk::Role as PlayerRole;

type View = crate::Observation<String>;

/// The screening partner accepts no policy settings.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScreeningPartnerConfig {}

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
pub struct ScreeningPartner {
    role: PlayerRole,
    view: Option<View>,
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
    fn update_view(&mut self, view: View) {
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
        view: &View,
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
    fn start(&mut self, observation: View) -> Result<()> {
        self.update_view(observation);
        Ok(())
    }

    /// Refreshes the view after any accepted manipulation, including its own.
    fn observe_transition(&mut self, _: PlayerRole, _: Action, view: View) -> Result<()> {
        self.update_view(view);
        Ok(())
    }

    /// Accepts a new or corrected mark and responds to malformed or impossible cues.
    fn observe_message(&mut self, sender: PlayerRole, text: String) -> Result<()> {
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
    fn respond(&mut self) -> Result<Response<Action>> {
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
        Ok(Response {
            action,
            message,
            wakeup: None,
        })
    }
}
#[parlando_game_sdk::agent::async_trait]
impl Agent for ScreeningPartner {
    type Config = ScreeningPartnerConfig;
    type Action = Action;
    type View = ScreeningView<String>;
    type Completion = Completion;

    /// Declares compatible settings and the stable identity shared by both builds.
    fn definition() -> Definition {
        Definition {
            id: "last_screening.operator",
            name: "Screening partner",
            version: "1",
            description: "Shares its cue in chat and operates the booth or backstage controls.",
            config_fields: "[]",
        }
    }

    /// Initializes private policy state for this seat.
    async fn init(context: InitContext<Self::Config>) -> Result<Self> {
        Ok(Self::new(context.role))
    }

    /// Receives the initial typed observation.
    async fn start(&mut self, view: Self::View) -> Result<()> {
        Self::start(self, view.into())
    }

    /// Updates the policy from the accepted action first and then any accompanying chat.
    async fn observe(&mut self, observation: Observation<Action, Self::View>) -> Result<()> {
        if let Some(transition) = observation.transition {
            self.observe_transition(transition.actor, transition.action, transition.view.into())?;
        }
        if let Some(message) = observation.message {
            self.observe_message(message.sender, message.text)?;
        }
        Ok(())
    }

    /// Produces the next typed response for the assigned seat.
    async fn respond(&mut self, _: RespondReason) -> Result<Response<Action>> {
        Self::respond(self)
    }
}

#[cfg(test)]
mod tests {
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
    #[test]
    fn early_and_corrected_marks_do_not_lose_the_other_cue() {
        let state = State::deal(82);
        let mut partner = ScreeningPartner::new(PlayerRole::B);
        partner
            .start(Screening::project(&state, PlayerRole::B))
            .unwrap();
        let cue = format!("tower {}", state.cue().tower + 1);
        let window = Action::MoveFlat {
            flat: Flat::Window,
            position: state.cue().window,
        };
        partner.observe_message(PlayerRole::A, cue.clone()).unwrap();
        assert!(matches!(
            partner.respond().unwrap(),
            Response { action: Some(action), message: Some(message), .. }
                if action == window && message.starts_with(&format!("window {}.", state.cue().window + 1))
        ));
        partner
            .observe_message(PlayerRole::A, "What is my cue?".into())
            .unwrap();
        assert!(matches!(
            partner.respond().unwrap(),
            Response { action: Some(action), message: Some(message), .. }
                if action == window && message.starts_with(&format!("window {}.", state.cue().window + 1))
        ));
        partner.observe_message(PlayerRole::A, cue).unwrap();
        let response = partner.respond().unwrap();
        assert_eq!(response.action, Some(window));
        assert!(response.message.is_none());
        assert!(response.wakeup.is_none());
    }

    /// Two independent partners share only what each human seat could see and finish both reels.
    #[test]
    fn partners_clear_both_reels_with_either_seat_assignment() {
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
                        .start(Screening::project(&state, role))
                        .unwrap();
                }
                for _ in 0..40 {
                    for (index, role) in roles.into_iter().enumerate() {
                        let Response {
                            action, message, ..
                        } = partners[index].respond().unwrap();
                        if let Some(action) = action {
                            state = Screening::apply_action(&state, &action, role).unwrap();
                            for (partner_index, recipient) in roles.into_iter().enumerate() {
                                partners[partner_index]
                                    .observe_transition(
                                        role,
                                        action,
                                        Screening::project(&state, recipient),
                                    )
                                    .unwrap();
                            }
                        }
                        if let Some(message) = message {
                            partners[1 - index].observe_message(role, message).unwrap();
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
