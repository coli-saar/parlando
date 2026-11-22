//! Two synchronized optical puzzles for a projection booth and a backstage rail.

mod production;
use anyhow::Result;
use parlando_game_sdk::{Rejection as ActionRejection, Role as PlayerRole};
use production::PRODUCTION;
use rand::{rngs::StdRng, Rng, SeedableRng};
use serde::{Deserialize, Serialize};
#[cfg(test)]
use serde_json::json;
use std::borrow::Cow;

const POSITIONS: std::ops::RangeInclusive<u8> = 0..=6;

/// Interchangeable transparencies. Each physical slide can occupy only one lantern.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, parlando_game_sdk::GameValue,
)]
#[serde(rename_all = "camelCase")]
pub enum Slide {
    Empty,
    Moon,
    Light,
    Ship,
}

/// Physical rails position the lighthouse aperture and the opaque tower independently.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, parlando_game_sdk::GameValue,
)]
#[serde(rename_all = "camelCase")]
pub enum Flat {
    Window,
    Tower,
}

/// A single accepted manipulation. Detents range from zero at left to six at right.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, parlando_game_sdk::GameValue,
)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum Action {
    Aim { lantern: u8, position: u8 },
    Load { lantern: u8, slide: Slide },
    MoveFlat { flat: Flat, position: u8 },
    Exit,
}

/// A pair of flat detents, used for both a cue drawing and the actual scenery.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, parlando_game_sdk::GameValue,
)]
pub struct Marks {
    /// Window detent.
    pub window: u8,
    /// Tower detent.
    pub tower: u8,
}

/// The two lanterns have independent slide carriers and horizontal bearings.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, parlando_game_sdk::GameValue,
)]
pub struct Lantern {
    /// Stable lantern identifier within the booth.
    pub id: u8,
    /// Current bearing detent.
    pub aim: u8,
    /// Slide currently loaded in its carrier.
    pub slide: Slide,
}

/// What one participant can see during a reel or the final escape.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Observation<Text = Cow<'static, str>> {
    /// Authored reel identifier.
    pub level: Text,
    /// Authored cue label, or `escape` once both exit doors unlock.
    pub phase: Text,
    /// Screen shared by both rooms.
    pub screen: Screen,
    /// Every currently legal control for the observing participant.
    pub actions: Vec<Action>,
    /// Whether this participant has used their exit door.
    pub exited: bool,
    /// Room-local controls and cue supplied only to this participant.
    #[serde(flatten)]
    pub room: Room,
}

/// Component projection with the public role-tagged, flat observation shape.
#[derive(Serialize, parlando_game_sdk::GameValue)]
#[serde(tag = "role", rename_all = "lowercase")]
pub enum ScreeningView<Text = Cow<'static, str>> {
    /// Screen, booth controls, and tower cue visible to the projectionist.
    Projectionist {
        level: Text,
        phase: Text,
        screen: Screen,
        actions: Vec<Action>,
        exited: bool,
        cue: TowerCue,
        lanterns: [Lantern; 2],
    },
    /// Screen, scenery controls, and window cue visible to the stagehand.
    Stagehand {
        level: Text,
        phase: Text,
        screen: Screen,
        actions: Vec<Action>,
        exited: bool,
        cue: WindowCue,
        flats: Marks,
    },
}

impl<Text> From<Observation<Text>> for ScreeningView<Text> {
    /// Selects the compact role payload while retaining the participant's named view.
    fn from(view: Observation<Text>) -> Self {
        let Observation {
            level,
            phase,
            screen,
            actions,
            exited,
            room,
        } = view;
        match room {
            Room::Projectionist { cue, lanterns } => Self::Projectionist {
                level,
                phase,
                screen,
                actions,
                exited,
                cue,
                lanterns,
            },
            Room::Stagehand { cue, flats } => Self::Stagehand {
                level,
                phase,
                screen,
                actions,
                exited,
                cue,
                flats,
            },
        }
    }
}

impl<Text> From<ScreeningView<Text>> for Observation<Text> {
    /// Restores the policy's shared view with the room-local controls grouped together.
    fn from(view: ScreeningView<Text>) -> Self {
        match view {
            ScreeningView::Projectionist {
                level,
                phase,
                screen,
                actions,
                exited,
                cue,
                lanterns,
            } => Self {
                level,
                phase,
                screen,
                actions,
                exited,
                room: Room::Projectionist { cue, lanterns },
            },
            ScreeningView::Stagehand {
                level,
                phase,
                screen,
                actions,
                exited,
                cue,
                flats,
            } => Self {
                level,
                phase,
                screen,
                actions,
                exited,
                room: Room::Stagehand { cue, flats },
            },
        }
    }
}

/// Public progress recorded on each accepted cinema operation.
#[derive(Clone, Copy, Serialize, parlando_game_sdk::GameValue)]
pub struct ScreeningEvent {
    level: &'static str,
    phase: &'static str,
}

/// What both rooms can see on the screen. A masked or missing image is absent.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, parlando_game_sdk::GameValue,
)]
pub struct Screen {
    /// Window position on the screen.
    pub window: u8,
    /// Tower position on the screen.
    pub tower: u8,
    /// Detent of the visible ship image, if present.
    pub ship: Option<u8>,
    /// Detent of the visible moon image, if present.
    pub moon: Option<u8>,
    /// Detent of the visible lamp image, if present.
    pub light: Option<u8>,
    /// Whether the lamp illuminates the window.
    pub lit: bool,
}

/// Room-local controls and the cue for the other room's apparatus.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "lowercase")]
pub enum Room {
    /// Booth controls and the cue for the stagehand's tower.
    Projectionist {
        /// Desired tower detent.
        cue: TowerCue,
        /// Current settings of each booth lantern.
        lanterns: [Lantern; 2],
    },
    /// Backstage controls and the cue for the projectionist's aim.
    Stagehand {
        /// Desired window detent.
        cue: WindowCue,
        /// Current detents of both scenery flats.
        flats: Marks,
    },
}

/// The tower detent drawn on the projectionist's cue.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, parlando_game_sdk::GameValue,
)]
pub struct TowerCue {
    /// Desired tower detent.
    pub tower: u8,
}

/// The window detent drawn on the stagehand's cue.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, parlando_game_sdk::GameValue,
)]
pub struct WindowCue {
    /// Desired window detent.
    pub window: u8,
}

/// The result shared after both participants escape the cinema.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, parlando_game_sdk::GameValue,
)]
#[serde(rename_all = "camelCase")]
pub struct Completion {
    /// Whether both participants used their exit doors.
    pub escaped: bool,
    /// Number of authored reels completed.
    pub levels_completed: usize,
}

/// The runtime persists the two seeded setups and the players' accepted manipulations.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    #[serde(default, skip_serializing_if = "ProjectionistSeat::is_a")]
    projectionist_seat: ProjectionistSeat,
    reel: usize,
    frame: usize,
    unlocked: bool,
    cues: Vec<Marks>,
    flats: Marks,
    starting_flats: Vec<Marks>,
    lanterns: [Lantern; 2],
    exits: [bool; 2],
}

/// Game-owned settings. The runtime supplies the seed and experiment controls.
#[derive(Default, Serialize, Deserialize, parlando_game_sdk::GameValue)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Seat assigned the projection booth. Defaults to A. The other seat runs the stage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projectionist_seat: Option<ProjectionistSeat>,
}

/// Seat that controls the lanterns and receives the private tower cue.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, parlando_game_sdk::GameValue,
)]
#[serde(rename_all = "lowercase")]
pub enum ProjectionistSeat {
    /// Assigns the projection booth to player A.
    #[default]
    A,
    /// Assigns the projection booth to player B.
    B,
}

impl ProjectionistSeat {
    /// Omits the original seat assignment from persisted state to preserve its representation.
    fn is_a(&self) -> bool {
        *self == Self::A
    }

    /// Identifies the runtime role authorized to operate the booth.
    fn role(self) -> PlayerRole {
        match self {
            Self::A => PlayerRole::A,
            Self::B => PlayerRole::B,
        }
    }
}

/// Owns the state and last accepted event for one Screening session.
pub struct Screening {
    state: State,
    metadata: Option<ScreeningEvent>,
}

impl State {
    /// Deals two reproducible cue drawings, each requiring movement of scenery and light.
    fn deal(seed: u64) -> Self {
        let mut rng = StdRng::seed_from_u64(seed);
        let cues: Vec<Marks> = PRODUCTION
            .iter()
            .map(|_| {
                let tower = rng.gen_range(1..=5);
                let window = (tower + rng.gen_range(1..7)) % 7;
                Marks { window, tower }
            })
            .collect();
        let off_mark = |cue: Marks| Marks {
            window: (cue.window + rng.gen_range(1..7)) % 7,
            tower: (cue.tower + rng.gen_range(1..7)) % 7,
        };
        let starting_flats: Vec<_> = cues.iter().copied().map(off_mark).collect();
        Self {
            projectionist_seat: ProjectionistSeat::default(),
            reel: 0,
            frame: 0,
            unlocked: false,
            flats: starting_flats[0],
            starting_flats,
            cues,
            lanterns: std::array::from_fn(|i| Lantern {
                id: i as u8,
                aim: rng.gen_range(POSITIONS),
                slide: Slide::Empty,
            }),
            exits: [false; 2],
        }
    }
    /// Looks up the current cue without disclosing the following setup.
    fn cue(&self) -> Marks {
        self.cues[self.reel]
    }
    /// Finds an image's projected detent before masking by the foreground tower.
    fn image(&self, slide: Slide) -> Option<u8> {
        self.lanterns
            .iter()
            .find(|l| l.slide == slide)
            .map(|l| l.aim)
    }
    /// A silhouette blocks all projections at its detent.
    fn visible(&self, slide: Slide) -> Option<u8> {
        self.image(slide).filter(|x| *x != self.flats.tower)
    }
    /// Identifies the reel in the authored production for both participants.
    fn level(&self) -> &'static str {
        PRODUCTION[self.reel].id
    }
    /// Reports the current authored task or the unlocked door.
    fn phase(&self) -> &'static str {
        if self.unlocked {
            "escape"
        } else {
            PRODUCTION[self.reel].phase(self.frame)
        }
    }
    /// Checks one projected picture, then installs the next reel or unlocks the exit.
    fn evaluate(&mut self) {
        if let Some(frame) = PRODUCTION[self.reel].advance(self, self.frame) {
            self.frame = frame;
        } else if self.reel + 1 < PRODUCTION.len() {
            self.reel += 1;
            self.frame = 0;
            self.flats = self.starting_flats[self.reel];
        } else {
            self.unlocked = true;
        }
    }
    /// Validates ownership, ranges and slide inventory before changing the copied state.
    fn step(&self, action: Action, role: PlayerRole) -> Result<Self, ActionRejection> {
        let reject = |code| Err(ActionRejection::new(code));
        let seat = usize::from(role == PlayerRole::B);
        let projectionist = role == self.projectionist_seat.role();
        let mut next = self.clone();
        if self.unlocked {
            if action != Action::Exit {
                return reject("show_finished");
            }
            if self.exits[seat] {
                return reject("already_exited");
            }
            next.exits[seat] = true;
            return Ok(next);
        }
        match action {
            Action::Exit => return reject("door_locked"),
            Action::MoveFlat { .. } if projectionist => return reject("wrong_role"),
            Action::Aim { .. } | Action::Load { .. } if !projectionist => {
                return reject("wrong_role")
            }
            Action::MoveFlat { flat, position } => {
                if !POSITIONS.contains(&position) {
                    return reject("invalid_position");
                }
                let current = match flat {
                    Flat::Window => &mut next.flats.window,
                    Flat::Tower => &mut next.flats.tower,
                };
                *current = position;
            }
            Action::Aim { lantern, position } => {
                if !POSITIONS.contains(&position) {
                    return reject("invalid_position");
                }
                next.lanterns
                    .get_mut(lantern as usize)
                    .ok_or_else(|| ActionRejection::new("unknown_lantern"))?
                    .aim = position;
            }
            Action::Load { lantern, slide } => {
                let index = lantern as usize;
                if index >= 2 {
                    return reject("unknown_lantern");
                }
                if slide != Slide::Empty && self.lanterns[1 - index].slide == slide {
                    return reject("slide_in_use");
                }
                next.lanterns[index].slide = slide;
            }
        }
        if next == *self {
            return reject("unchanged");
        }
        next.evaluate();
        Ok(next)
    }
    /// Enumerates physical controls through the same rule checks used for submitted actions.
    fn actions(&self, role: PlayerRole) -> Vec<Action> {
        let mut choices = vec![Action::Exit];
        if role == self.projectionist_seat.role() {
            for lantern in 0..2 {
                choices.extend(POSITIONS.map(|position| Action::Aim { lantern, position }));
                choices.extend(
                    [Slide::Empty, Slide::Moon, Slide::Light, Slide::Ship]
                        .map(|slide| Action::Load { lantern, slide }),
                );
            }
        } else {
            for flat in [Flat::Window, Flat::Tower] {
                choices.extend(POSITIONS.map(|position| Action::MoveFlat { flat, position }));
            }
        }
        choices.retain(|a| self.step(*a, role).is_ok());
        choices
    }
}
impl Screening {
    /// Applies precisely one authorized operation.
    fn apply_action(s: &State, a: &Action, r: PlayerRole) -> Result<State, ActionRejection> {
        s.step(*a, r)
    }
    /// Shows both rooms the projected frame. The booth holds the tower cue, while the
    /// stagehand holds the window cue needed for aiming a lantern.
    fn observation(s: &State, r: PlayerRole) -> Observation {
        Self::project(s, r)
    }

    /// Shares the named wire layout between borrowed game output and owned policy inputs.
    fn project<Text: From<&'static str>>(s: &State, r: PlayerRole) -> Observation<Text> {
        let light = s.visible(Slide::Light);
        let cue = s.cue();
        let room = if r == s.projectionist_seat.role() {
            Room::Projectionist {
                cue: TowerCue { tower: cue.tower },
                lanterns: s.lanterns,
            }
        } else {
            Room::Stagehand {
                cue: WindowCue { window: cue.window },
                flats: s.flats,
            }
        };
        Observation {
            actions: s.actions(r),
            level: s.level().into(),
            phase: s.phase().into(),
            screen: Screen {
                window: s.flats.window,
                tower: s.flats.tower,
                ship: s.visible(Slide::Ship),
                moon: s.visible(Slide::Moon),
                light,
                lit: light == Some(s.flats.window),
            },
            exited: s.exits[usize::from(r == PlayerRole::B)],
            room,
        }
    }
    /// Records the public cue detector result per accepted action.
    fn transition_metadata(s: &State) -> ScreeningEvent {
        ScreeningEvent {
            level: s.level(),
            phase: s.phase(),
        }
    }
    /// The cinema is escaped only once both players have used their own door.
    fn outcome(s: &State) -> Option<Completion> {
        s.exits.iter().all(|x| *x).then_some(Completion {
            escaped: true,
            levels_completed: PRODUCTION.len(),
        })
    }
}

impl parlando_game_sdk::Game for Screening {
    type Config = Config;
    type Action = Action;
    type ViewA<'a> = ScreeningView;
    type ViewB<'a> = ScreeningView;
    type Completion<'a> = Completion;
    type Metadata<'a> = ScreeningEvent;

    /// Deals deterministic state and initializes the session-local projections.
    fn init(config: Self::Config, seed: u64) -> Result<Self, String> {
        Ok(Self {
            state: State {
                projectionist_seat: config.projectionist_seat.unwrap_or_default(),
                ..State::deal(seed)
            },
            metadata: None,
        })
    }

    /// Validates and applies one operation before emitting new observations.
    fn step(&mut self, actor: PlayerRole, action: Self::Action) -> Result<(), ActionRejection> {
        self.state = Self::apply_action(&self.state, &action, actor)?;
        self.metadata = Some(Self::transition_metadata(&self.state));
        Ok(())
    }

    /// Projects information visible to the first seat.
    fn view_a(&self) -> Self::ViewA<'_> {
        Self::observation(&self.state, PlayerRole::A).into()
    }

    /// Projects information visible to the second seat.
    fn view_b(&self) -> Self::ViewB<'_> {
        Self::observation(&self.state, PlayerRole::B).into()
    }

    /// Reports the game-defined success outcome when reached.
    fn completion(&self) -> Option<Self::Completion<'_>> {
        Self::outcome(&self.state)
    }

    /// Exposes only public event information from the last accepted action.
    fn metadata(&self) -> Option<Self::Metadata<'_>> {
        self.metadata
    }
}

#[cfg(not(all(feature = "agent-component", target_arch = "wasm32")))]
parlando_game_sdk::export!(Screening);

#[cfg(test)]
mod tests {
    use super::*;
    use PlayerRole::{A, B};

    /// The bare dashboard configuration decodes through the component ABI and assigns seat A.
    #[test]
    fn default_booth_assignment_accepts_empty_component_config() {
        use parlando_game_sdk::{ComponentValue, Game, GameValue};

        let value = ComponentValue::from_typed(&json!({})).unwrap();
        let bytes = value.to_schema_messagepack(&Config::schema()).unwrap();
        let config = parlando_game_sdk::decode_declared(&bytes).unwrap();
        let game = Screening::init(config, 7).unwrap();
        assert!(matches!(game.view_a(), ScreeningView::Projectionist { .. }));
        assert!(matches!(game.view_b(), ScreeningView::Stagehand { .. }));
    }

    /// Applies a control setting only when it differs, as a player following the cue would.
    fn set(s: &mut State, a: Action, r: PlayerRole) {
        if s.actions(r).contains(&a) {
            *s = s.step(a, r).unwrap();
        }
    }

    /// Plays Moonrise from arbitrary seeded equipment positions.
    fn moonrise(s: &mut State) {
        let cue = s.cue();
        set(
            s,
            Action::MoveFlat {
                flat: Flat::Tower,
                position: cue.tower,
            },
            B,
        );
        set(
            s,
            Action::MoveFlat {
                flat: Flat::Window,
                position: cue.window,
            },
            B,
        );
        set(
            s,
            Action::Aim {
                lantern: 0,
                position: cue.window,
            },
            A,
        );
        set(
            s,
            Action::Load {
                lantern: 0,
                slide: Slide::Moon,
            },
            A,
        );
        assert_eq!(s.level(), "crossing");
    }

    /// Stages the first crossing frame with the ship on the left edge.
    fn crossing_start(s: &mut State) {
        let cue = s.cue();
        set(
            s,
            Action::MoveFlat {
                flat: Flat::Window,
                position: cue.window,
            },
            B,
        );
        set(
            s,
            Action::MoveFlat {
                flat: Flat::Tower,
                position: cue.tower,
            },
            B,
        );
        set(
            s,
            Action::Aim {
                lantern: 1,
                position: 0,
            },
            A,
        );
        set(
            s,
            Action::Load {
                lantern: 1,
                slide: Slide::Ship,
            },
            A,
        );
        set(
            s,
            Action::Aim {
                lantern: 0,
                position: cue.window,
            },
            A,
        );
        set(
            s,
            Action::Load {
                lantern: 0,
                slide: Slide::Light,
            },
            A,
        );
        assert_eq!(s.phase(), "hide");
    }

    /// Every sampled cue solves through the catalog and both exits, including edge windows.
    #[test]
    fn seeded_shows_solve_through_player_actions() {
        let mut pairs = std::collections::HashSet::new();
        for seed in 0..256 {
            let mut s = State::deal(seed);
            assert_eq!(s, State::deal(seed));
            assert_ne!(s.flats.window, s.cue().window);
            moonrise(&mut s);
            let cue = s.cue();
            pairs.insert((cue.window, cue.tower));
            crossing_start(&mut s);
            set(
                &mut s,
                Action::Aim {
                    lantern: 1,
                    position: cue.tower,
                },
                A,
            );
            assert_eq!(s.phase(), "emerge");
            assert_eq!(Screening::observation(&s, B).screen.ship, None);
            set(
                &mut s,
                Action::Aim {
                    lantern: 1,
                    position: 6,
                },
                A,
            );
            assert_eq!(s.phase(), "escape");
            for r in [B, A] {
                assert_eq!(Screening::outcome(&s), None);
                assert_eq!(s.actions(r), vec![Action::Exit]);
                s = s.step(Action::Exit, r).unwrap();
                assert!(s.actions(r).is_empty());
            }
            assert_eq!(
                serde_json::to_value(Screening::outcome(&s)).unwrap(),
                json!({"escaped":true,"levelsCompleted":2})
            );
        }
        assert_eq!(pairs.len(), 30);
    }

    /// A skipped occultation, changed scenery, or dark frame invalidates crossing progress.
    #[test]
    fn crossing_checks_order_and_continuity() {
        let mut s = State::deal(2);
        moonrise(&mut s);
        crossing_start(&mut s);
        let cue = s.cue();
        set(
            &mut s,
            Action::Aim {
                lantern: 1,
                position: 6,
            },
            A,
        );
        assert_eq!(s.phase(), "align");
        for interruption in [
            Action::Aim {
                lantern: 0,
                position: (cue.window + 1) % 7,
            },
            Action::MoveFlat {
                flat: Flat::Tower,
                position: (cue.tower + 1) % 7,
            },
        ] {
            crossing_start(&mut s);
            set(
                &mut s,
                Action::Aim {
                    lantern: 1,
                    position: cue.tower,
                },
                A,
            );
            assert_eq!(s.phase(), "emerge");
            let r = if matches!(interruption, Action::MoveFlat { .. }) {
                B
            } else {
                A
            };
            set(&mut s, interruption, r);
            assert_eq!(s.phase(), "align");
        }
    }

    /// A solid flat masks the moon despite the lantern and window being on their cue mark.
    #[test]
    fn opaque_scenery_blocks_moonrise() {
        let mut s = State::deal(42);
        let cue = s.cue();
        set(
            &mut s,
            Action::MoveFlat {
                flat: Flat::Tower,
                position: cue.window,
            },
            B,
        );
        set(
            &mut s,
            Action::MoveFlat {
                flat: Flat::Window,
                position: cue.window,
            },
            B,
        );
        set(
            &mut s,
            Action::Aim {
                lantern: 0,
                position: cue.window,
            },
            A,
        );
        set(
            &mut s,
            Action::Load {
                lantern: 0,
                slide: Slide::Moon,
            },
            A,
        );
        assert_eq!(s.level(), "moonrise");
        assert_eq!(Screening::observation(&s, A).screen.moon, None);
        set(
            &mut s,
            Action::MoveFlat {
                flat: Flat::Tower,
                position: cue.tower,
            },
            B,
        );
        assert_eq!(s.level(), "crossing");
    }

    /// Role-local controls remain private and invalid commands cannot alter the source frame.
    #[test]
    fn controls_are_private_and_validate_ownership_range_inventory_and_noops() {
        let s = State::deal(42)
            .step(
                Action::Load {
                    lantern: 0,
                    slide: Slide::Ship,
                },
                A,
            )
            .unwrap();
        let observation_a = Screening::observation(&s, A);
        let observation_b = Screening::observation(&s, B);
        let a = serde_json::to_value(&observation_a).unwrap();
        let b = serde_json::to_value(&observation_b).unwrap();
        assert_eq!(
            serde_json::from_value::<Observation>(a.clone()).unwrap(),
            observation_a
        );
        assert_eq!(
            serde_json::from_value::<Observation>(b.clone()).unwrap(),
            observation_b
        );
        assert_eq!(
            serde_json::to_value(ScreeningView::from(observation_a)).unwrap(),
            a
        );
        assert_eq!(
            serde_json::to_value(ScreeningView::from(observation_b)).unwrap(),
            b
        );
        let light = s.visible(Slide::Light);
        let screen = json!({
            "window": s.flats.window,
            "tower": s.flats.tower,
            "ship": s.visible(Slide::Ship),
            "moon": s.visible(Slide::Moon),
            "light": light,
            "lit": light == Some(s.flats.window),
        });
        assert_eq!(
            a,
            json!({
                "role": "projectionist", "level": "moonrise", "phase": "align",
                "screen": screen, "actions": s.actions(A), "cue": {"tower": s.cue().tower},
                "lanterns": s.lanterns, "exited": false,
            })
        );
        assert_eq!(
            b,
            json!({
                "role": "stagehand", "level": "moonrise", "phase": "align",
                "screen": screen, "actions": s.actions(B), "cue": {"window": s.cue().window},
                "flats": s.flats, "exited": false,
            })
        );
        for (cmd, role) in [
            (Action::Exit, A),
            (
                Action::MoveFlat {
                    flat: Flat::Window,
                    position: 3,
                },
                A,
            ),
            (
                Action::MoveFlat {
                    flat: Flat::Tower,
                    position: 8,
                },
                B,
            ),
            (
                Action::Aim {
                    lantern: 2,
                    position: 1,
                },
                A,
            ),
            (
                Action::Aim {
                    lantern: 0,
                    position: 7,
                },
                A,
            ),
            (
                Action::Aim {
                    lantern: 0,
                    position: 1,
                },
                B,
            ),
            (
                Action::Load {
                    lantern: 2,
                    slide: Slide::Moon,
                },
                A,
            ),
            (
                Action::Load {
                    lantern: 1,
                    slide: Slide::Ship,
                },
                A,
            ),
            (
                Action::Load {
                    lantern: 0,
                    slide: Slide::Ship,
                },
                A,
            ),
            (
                Action::Load {
                    lantern: 0,
                    slide: Slide::Moon,
                },
                B,
            ),
        ] {
            assert!(s.step(cmd, role).is_err());
            assert!(!s.actions(role).contains(&cmd));
        }
        assert!(serde_json::from_value::<Config>(json!({"unexpected":true})).is_err());
        let decoded: State = serde_json::from_value(serde_json::to_value(&s).unwrap()).unwrap();
        assert_eq!(decoded, s);
    }
}

#[cfg(any(
    all(feature = "bundled-agents", not(target_arch = "wasm32")),
    all(feature = "agent-component", any(target_arch = "wasm32", test)),
))]
mod operator;

#[cfg(all(feature = "bundled-agents", not(target_arch = "wasm32")))]
pub use operator::ScreeningPartner;

/// Registers the game's authored native policy with Parlando.
#[cfg(all(feature = "bundled-agents", not(target_arch = "wasm32")))]
pub type ScreeningPartnerFactory = parlando::agent::native::TypedAgentFactory<ScreeningPartner>;

#[cfg(all(feature = "agent-component", target_arch = "wasm32"))]
parlando_game_sdk::export_agent!(operator::ScreeningPartner);
