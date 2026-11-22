//! The hotel porter plays either car using role-visible controls and chatted entries.

#[cfg(test)]
use crate::Hotel;
use crate::{Action, BridgePose, Completion, FreightStatus, Seal, CABIN_CAPACITY};
use parlando_game_sdk::agent::{
    Agent, Definition, InitContext, Observation, RespondReason, Response, Result,
};
use parlando_game_sdk::agent::{LazyView, WakeupRequest};
use parlando_game_sdk::Role as PlayerRole;
use serde::Deserialize;
use std::{sync::LazyLock, time::Duration};

type View = crate::Observation<String>;

/// Controls whether mechanics and crossing animations wait for live presentation.
#[derive(Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PorterConfigPacing {
    /// Pace mechanical actions for a participant watching the lift.
    #[default]
    Live,
    /// Respond immediately throughout headless runs.
    Instant,
}

/// Settings for one porter instance.
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PorterConfig {
    pacing: PorterConfigPacing,
}

/// Facts the guest has shared so far, possibly across separate messages.
#[derive(Default)]
struct EntryReport {
    floor: Option<u8>,
    seal: Option<Seal>,
    cases: Option<u8>,
}
impl EntryReport {
    /// Reads explicit report fields in any order and across sentences. A bare
    /// seal is accepted without treating ordinary mentions of the brake key as seals.
    fn parse(text: &str, needs_cases: bool) -> Self {
        let text = text.to_lowercase();
        let words: Vec<_> = text
            .split(|c: char| !c.is_alphanumeric())
            .filter(|word| !word.is_empty() && *word != "is")
            .collect();
        let floor = words
            .windows(2)
            .find_map(|pair| {
                if pair[0] == "floor" {
                    spoken_number(pair[1])
                } else if pair[1] == "floor" {
                    spoken_number(pair[0])
                } else {
                    None
                }
            })
            .or_else(|| words.contains(&"ground").then_some(0))
            .filter(|floor| *floor <= 4);
        let seal_word = |word| match word {
            "stag" => Some(Seal::Stag),
            "rose" => Some(Seal::Rose),
            "key" => Some(Seal::Key),
            _ => None,
        };
        let seal = words
            .windows(2)
            .find_map(|pair| {
                if matches!(pair[0], "seal" | "engraving" | "stamp") {
                    seal_word(pair[1])
                } else if matches!(pair[1], "seal" | "engraving" | "stamp") {
                    seal_word(pair[0])
                } else {
                    None
                }
            })
            .or_else(|| {
                words.iter().find_map(|&word| {
                    (word != "key" || words.len() == 1 || floor.is_some())
                        .then(|| seal_word(word))
                        .flatten()
                })
            });
        let cases = if needs_cases {
            words
                .windows(2)
                .find_map(|pair| {
                    matches!(pair[1], "case" | "cases" | "suitcase" | "suitcases")
                        .then(|| {
                            if pair[0] == "a" {
                                Some(1)
                            } else {
                                spoken_number(pair[0])
                            }
                        })
                        .flatten()
                })
                .filter(|count| *count <= CABIN_CAPACITY)
        } else {
            None
        };
        let floor = floor.or_else(|| {
            seal.and_then(|_| words.first().and_then(|word| spoken_number(word)))
                .filter(|floor| *floor <= 4)
        });
        Self { floor, seal, cases }
    }

    /// Identifies whether the message supplied any usable dispatch facts.
    fn is_empty(&self) -> bool {
        self.floor.is_none() && self.seal.is_none() && self.cases.is_none()
    }

    /// Replaces only the reported fields, permitting a correction to a single fact.
    fn merge(&mut self, other: Self) {
        self.floor = other.floor.or(self.floor);
        self.seal = other.seal.or(self.seal);
        self.cases = other.cases.or(self.cases);
    }

    /// Returns an entry once all required facts have been received.
    fn complete(&self, needs_cases: bool) -> Option<Entry> {
        Some(Entry {
            floor: self.floor?,
            seal: self.seal?,
            cases: if needs_cases { Some(self.cases?) } else { None },
        })
    }

    /// Requests only the dispatch facts still missing.
    fn request(&self, needs_cases: bool) -> String {
        match (
            self.floor.is_none(),
            self.seal.is_none(),
            needs_cases && self.cases.is_none(),
        ) {
            (true, true, true) => "I’m missing my bridge floor, seal, and suitcase count.",
            (true, true, false) => "I’m missing my bridge floor and seal.",
            (true, false, true) => {
                "I have my seal. I’m missing my bridge floor and suitcase count."
            }
            (false, true, true) => {
                "I have my bridge floor. I’m missing my seal and suitcase count."
            }
            (true, false, false) => "I’m missing my bridge floor.",
            (false, true, false) => "I’m missing my seal.",
            (false, false, true) => "I’m missing my suitcase count.",
            (false, false, false) => "I’m waiting for a corrected bridge entry.",
        }
        .into()
    }
}

/// A dispatch entry conveyed by the other guest.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Entry {
    floor: u8,
    seal: Seal,
    cases: Option<u8>,
}
impl Entry {
    /// Reads back a bridge entry so the guest can verify the porter copied it correctly.
    fn read_back(self) -> String {
        format!("My bridge entry is {}.", self.details())
    }

    /// Formats a bridge entry consistently in announcements and confirmations.
    fn details(self) -> String {
        let seal = match self.seal {
            Seal::Stag => "stag",
            Seal::Rose => "rose",
            Seal::Key => "key",
        };
        let luggage = match self.cases {
            Some(1) => ", 1 suitcase".into(),
            Some(n) => format!(", {n} suitcases"),
            None => String::new(),
        };
        format!("{}, {seal} seal{luggage}", floor_name(self.floor))
    }
}

/// Gives chat the same names as the landing labels.
fn floor_name(floor: u8) -> String {
    if floor == 0 {
        "Ground".into()
    } else {
        format!("Floor {floor}")
    }
}

/// Accepts numbers as typed digits or words transcribed from speech.
fn spoken_number(word: &str) -> Option<u8> {
    match word {
        "ground" | "zero" | "no" | "none" => Some(0),
        "one" | "first" | "1st" => Some(1),
        "two" | "second" | "2nd" => Some(2),
        "three" | "third" | "3rd" => Some(3),
        "four" | "fourth" | "4th" => Some(4),
        _ => word.parse().ok(),
    }
}

impl View {
    /// The coupled floors sum to four, so the partner’s bridge entry supplies both destinations.
    fn destination(&self) -> u8 {
        4 - self.partner_destination
    }

    /// Reconstructs the entry for the other guest from the public-to-this-role ledger.
    fn partner_entry(&self) -> Entry {
        Entry {
            floor: self.partner_destination,
            seal: self.partner_seal,
            cases: self.partner_delivery,
        }
    }

    /// Checks whether releasing the brake would take this car toward the given landing.
    fn can_travel_towards(&self, goal: u8) -> bool {
        (self.floor > goal && self.cargo > self.partner_cargo)
            || (self.floor < goal && self.cargo < self.partner_cargo)
    }

    /// Prepares a receipt for an available transfer. The runner posts it after commit.
    fn transfer_report(&self, action: Action) -> Option<String> {
        let (transfer, count) = match action {
            Action::Load => (
                format!(
                    "I loaded a suitcase from the {} rack.",
                    floor_name(self.load_floor)
                ),
                self.cargo + 1,
            ),
            Action::CollectRescueCase => (
                "I took the marked dispensary suitcase aboard.".into(),
                self.cargo + 1,
            ),
            Action::Unload => (
                format!(
                    "I put a suitcase on the {} rack.",
                    floor_name(self.unload_floor)
                ),
                self.cargo - 1,
            ),
            _ => return None,
        };
        let unit = if count == 1 { "suitcase" } else { "suitcases" };
        Some(format!("{transfer} My lift now carries {count} {unit}."))
    }
}

/// Planar joints shared by the porter's walking samples and the hotel renderer.
#[derive(Deserialize)]
struct CrossingRoutes {
    east: Vec<[f64; 2]>,
    west: Vec<[f64; 2]>,
    shared: Vec<[f64; 2]>,
}

/// Samples each wing's gallery and the common span, advancing at most 0.65 metres per sample.
static CROSSINGS: LazyLock<[Vec<BridgePose>; 2]> = LazyLock::new(|| {
    let routes: CrossingRoutes =
        serde_json::from_str(include_str!("../../client/web/src/scene/bridge-route.json"))
            .expect("the hotel crossing defines two approaches and a common span");
    [routes.east, routes.west].map(|mut route| {
        // Walk the first metre privately, before publishing the porter's position beyond its gate.
        let [x, z] = route[0];
        let dz = route[1][1] - z;
        route[0] = [x, z + dz.signum() * 0.98];
        route.extend_from_slice(&routes.shared);
        let mut poses = Vec::new();
        for segment in route.windows(2) {
            let [x, z] = segment[0];
            let dx = segment[1][0] - x;
            let dz = segment[1][1] - z;
            let steps = (dx.hypot(dz) / 0.65).ceil() as u32;
            for i in 0..steps {
                let fraction = f64::from(i) / f64::from(steps);
                poses.push(BridgePose {
                    x: x + dx * fraction,
                    z: z + dz * fraction,
                    yaw: dx.atan2(dz),
                });
            }
        }
        let [x, z] = *route.last().unwrap();
        poses.push(BridgePose {
            x,
            z,
            yaw: poses.last().unwrap().yaw,
        });
        poses
    })
});

/// Selects the gallery that serves the current porter's side of the hotel.
fn crossing_for(role: PlayerRole) -> &'static [BridgePose] {
    &CROSSINGS[usize::from(role == PlayerRole::B)]
}

/// Time allowed for the browser to present each committed mechanical operation.
struct Pacing {
    ordinary: Duration,
    key_transfer: Duration,
    brake_stroke: Duration,
    walking_step: Duration,
}
impl Pacing {
    const LIVE: Self = Self {
        ordinary: Duration::from_millis(900),
        // The scene takes 1,770 ms to seat the key and 2,250 ms for one floor.
        key_transfer: Duration::from_millis(2000),
        brake_stroke: Duration::from_millis(2500),
        walking_step: Duration::from_millis(250),
    };
    const INSTANT: Self = Self {
        ordinary: Duration::ZERO,
        key_transfer: Duration::ZERO,
        brake_stroke: Duration::ZERO,
        walking_step: Duration::ZERO,
    };
}

/// Per-session porter memory for the bridge entry and the current lift controls.
pub struct Porter {
    role: PlayerRole,
    view: Option<View>,
    entry: Option<Entry>,
    report: EntryReport,
    ask_at_handoff: bool,
    reply: Option<String>,
    announce: bool,
    rejected_entry: Option<Entry>,
    pacing: Pacing,
    last_transfer_motive: Option<String>,
    ignore_pose_response: bool,
    crossing_sample: usize,
    pending_delay: Option<Duration>,
    waiting: bool,
}
impl Porter {
    /// Waits for the first role-specific view before contacting the guest.
    fn new(role: PlayerRole, pacing: Pacing) -> Self {
        Self {
            role,
            view: None,
            entry: None,
            report: EntryReport::default(),
            ask_at_handoff: false,
            reply: None,
            announce: false,
            rejected_entry: None,
            pacing,
            last_transfer_motive: None,
            ignore_pose_response: false,
            crossing_sample: 0,
            pending_delay: None,
            waiting: false,
        }
    }
    /// Replaces the current observation and schedules the first bridge report.
    fn update(&mut self, view: View) {
        if self.view.is_none() {
            self.announce = true;
        }
        if view.handed_over && self.view.as_ref().is_none_or(|old| !old.handed_over) {
            self.ask_at_handoff = true;
        }
        self.view = Some(view);
    }
    /// Reads out the facts recorded for the other wing on the porter’s ledger.
    fn message(view: &View) -> String {
        format!("Your rescue bridge: {}.", view.partner_entry().details())
    }

    /// Explains the single-operator interlock and which wing can power the gates.
    const KEY_REASON: &str = "The lifts share one cable. One key means only one of us can control the brake at a time. East’s socket works the brake. West’s also powers both rescue gate releases.";

    /// Recognizes spoken questions about the reason for an operation.
    fn asks_purpose(text: &str) -> bool {
        text.starts_with("why ")
            || text.starts_with("how come ")
            || text.contains("what for")
            || (text.starts_with("what ") && text.trim_end_matches('?').ends_with(" for"))
    }

    /// Answers requests with ledger entries, observed lift state, or received dispatch details.
    fn fact(&self, text: &str, view: &View) -> Option<String> {
        let mentions = |phrases: &[&str]| phrases.iter().any(|phrase| text.contains(phrase));
        let asks_purpose = Self::asks_purpose(text);
        if asks_purpose && mentions(&["suitcase", "case", "luggage", "baggage", "load", "weight"]) {
            Some(self.last_transfer_motive.clone().unwrap_or_else(|| {
                "The suitcases change the balance. When the brake is released, the heavier lift goes down and the lighter one rises.".into()
            }))
        } else if asks_purpose && text.contains("key") {
            Some(Self::KEY_REASON.into())
        } else if mentions(&["what do you need", "your booking", "your bridge entry"]) {
            Some(self.entry.map_or_else(
                || self.report.request(view.partner_delivery.is_some()),
                Entry::read_back,
            ))
        } else if mentions(&[
            "route sign",
            "route card",
            "inspection sign",
            "rescue bridge route",
            "ledger",
            "my booking",
            "my bridge",
            "my entry",
            "my floor",
            "my seal",
            "my destination",
            "my delivery",
        ]) {
            Some(Self::message(view))
        } else if mentions(&["cases do you have", "cases are you carrying", "your cargo"]) {
            let unit = if view.cargo == 1 {
                "suitcase"
            } else {
                "suitcases"
            };
            Some(format!("My lift carries {} {unit}.", view.cargo))
        } else if mentions(&["who has the key", "who has the brake key"]) {
            Some(if (self.role == PlayerRole::B) == view.handed_over {
                "I have the brake key.".into()
            } else {
                "You have the brake key.".into()
            })
        } else if mentions(&[
            "where are you",
            "your floor",
            "your status",
            "help",
            "stuck",
            "what should i do",
        ]) {
            Some(if view.exited {
                "I’m across my bridge.".into()
            } else if view.gate_open {
                "My bridge gate is open.".into()
            } else {
                format!("My lift is stopped at {}.", floor_name(view.floor))
            })
        } else {
            None
        }
    }

    /// Picks a brake operation or changes the counterweight for the current destination.
    fn travel(&self, v: &View, goal: u8, purpose: &str) -> (Action, Option<String>) {
        if v.can_travel_towards(goal) {
            let wing = match self.role {
                PlayerRole::A => "East",
                PlayerRole::B => "West",
            };
            (
                Action::Release,
                Some(format!(
                    "I pulled {wing}’s brake for one stop on the way to {purpose}. Both lifts move together. The heavier car pulls the lighter one up."
                )),
            )
        } else {
            let (action, weight, direction) = if v.floor > goal {
                (Action::Load, "heavier", "down")
            } else {
                (Action::Unload, "lighter", "up")
            };
            (
                action,
                Some(format!(
                    "I need my lift {weight} than yours so it can go {direction} to {purpose}."
                )),
            )
        }
    }

    /// Plans the next available operation and carries its motive into the committed dialogue.
    fn choose(&self, v: &View) -> Option<(Action, Option<String>)> {
        let (action, motive) = if v.exited {
            return None;
        } else if v.gate_open {
            (Action::ReachRefuge, None)
        } else if v.lock_tripped {
            (Action::ResetLock, None)
        } else if !v.handed_over {
            if v.floor == 2 {
                (Action::Pass, Some("I’m passing the key through the hatch so you can take over the brake. Your socket also powers both rescue gate releases.".into()))
            } else {
                self.travel(v, 2, "the key hatch")
            }
        } else {
            let entry = self.entry?;
            if v.freight_status == Some(FreightStatus::AwaitingPickup) {
                let pickup = v.freight_floor?;
                if v.floor != pickup {
                    let purpose = match self.role {
                        PlayerRole::A => {
                            format!(
                                "{} to collect the marked dispensary suitcase",
                                floor_name(pickup)
                            )
                        }
                        PlayerRole::B => format!(
                            "{} so your lift can reach the supply dock",
                            floor_name(pickup)
                        ),
                    };
                    self.travel(v, pickup, &purpose)
                } else if self.role == PlayerRole::B {
                    return None;
                } else if v.cargo == CABIN_CAPACITY {
                    (
                        Action::Unload,
                        Some("I need room for the marked dispensary suitcase at this dock.".into()),
                    )
                } else {
                    (Action::CollectRescueCase, Some("I need the marked dispensary suitcase in my lift to open the rescue gates.".into()))
                }
            } else if v.floor != entry.floor {
                self.travel(
                    v,
                    entry.floor,
                    &format!("my rescue bridge on {}", floor_name(entry.floor)),
                )
            } else if let Some(count) = entry.cases.filter(|count| *count != v.cargo) {
                let action = if v.cargo < count {
                    Action::Load
                } else {
                    Action::Unload
                };
                let unit = if count == 1 { "suitcase" } else { "suitcases" };
                (
                    action,
                    Some(format!(
                        "My bridge gate requires {count} {unit} in my lift."
                    )),
                )
            } else {
                (Action::Exit { seal: entry.seal }, None)
            }
        };
        v.actions.contains(&action).then_some((action, motive))
    }
}

impl Porter {
    /// Receives the initial view for this guest.
    fn start(&mut self, v: View) -> Result<()> {
        self.update(v);
        Ok(())
    }
    /// Feeds an already decoded observation to the same update path in policy tests.
    #[cfg(test)]
    fn observe_transition(&mut self, actor: PlayerRole, action: Action, v: View) -> Result<()> {
        self.observe_transition_with(actor, action, || Ok(v))
    }

    /// Registers accepted mechanics and decodes views only when the transition uses them.
    fn observe_transition_with(
        &mut self,
        actor: PlayerRole,
        action: Action,
        view: impl FnOnce() -> Result<View>,
    ) -> Result<()> {
        self.ignore_pose_response = false;
        if let Action::BridgePose { pose } = action {
            if actor == self.role
                && pose.is_some()
                && pose.as_ref() == crossing_for(self.role).get(self.crossing_sample)
            {
                self.crossing_sample += 1;
                self.pace_after(self.pacing.walking_step);
            } else {
                // A peer's footsteps must neither wake an idle porter nor stall its own crossing.
                self.ignore_pose_response =
                    !self.view.as_ref().is_some_and(|v| v.gate_open && !v.exited);
            }
            return Ok(());
        }
        let own_transfer = actor == self.role
            && matches!(
                action,
                Action::Load | Action::Unload | Action::CollectRescueCase
            );
        let transfer_motive = self
            .view
            .as_ref()
            .filter(|_| own_transfer)
            .and_then(|previous| self.choose(previous))
            .filter(|(planned, _)| *planned == action)
            .and_then(|(_, motive)| motive);
        self.update(view()?);
        if own_transfer {
            self.last_transfer_motive = transfer_motive;
        }
        let own_gate = actor == self.role && matches!(action, Action::Exit { .. });
        if own_gate && !self.view.as_ref().unwrap().gate_open {
            self.rejected_entry = self.entry.take();
            self.report.seal = None;
            self.reply = Some("That seal tripped my gate lock. I’ll reset it at the brake pedestal. I’m missing the correct seal.".into());
        }
        let delay = match action {
            Action::Pass => self.pacing.key_transfer,
            Action::Release => self.pacing.brake_stroke,
            Action::Exit { .. } => self.pacing.ordinary,
            _ if own_transfer => self.pacing.ordinary,
            _ => Duration::ZERO,
        };
        self.pace_after(delay);
        Ok(())
    }
    /// Requests a wakeup for live presentation on the next decision.
    fn pace_after(&mut self, delay: Duration) {
        if !delay.is_zero() {
            self.pending_delay = Some(delay);
        }
    }
    /// Copies a dispatch entry or answers a request for directly observed facts.
    fn observe_message(&mut self, sender: PlayerRole, text: String) -> Result<()> {
        self.ignore_pose_response = false;
        let Some(v) = &self.view else { return Ok(()) };
        if sender == self.role
            || (text.starts_with("My bridge entry is ") && text.contains("Thank you."))
            || text.starts_with("I’m missing ")
            || text.starts_with("I have my ")
        {
            return Ok(());
        }
        let lower = text.to_lowercase();
        if !text.starts_with("Your rescue bridge:") {
            if let Some(reply) = self
                .fact(&lower, v)
                .filter(|_| lower.contains('?') || Self::asks_purpose(&lower))
            {
                self.reply = Some(reply);
                return Ok(());
            }
        }
        let needs_cases = v.partner_delivery.is_some();
        let dispatch = if text.starts_with("Your rescue bridge:") {
            text.split_once(" My bridge entry is ")
                .map_or(text.as_str(), |(ledger, _)| ledger)
        } else {
            text.as_str()
        };
        // Porter dialogue can place a transfer receipt or handoff explanation after its ledger.
        let dispatch_end = [
            "I loaded a suitcase",
            "I put a suitcase",
            "I took the marked dispensary suitcase",
            "I’m passing the key",
            "I pulled East’s brake",
            "I pulled West’s brake",
            Self::KEY_REASON,
        ]
        .into_iter()
        .filter_map(|phrase| dispatch.find(phrase))
        .min()
        .unwrap_or(dispatch.len());
        let report = EntryReport::parse(&dispatch[..dispatch_end], needs_cases);
        if !report.is_empty() && !v.exited {
            self.reply = Some(
                if report.floor.is_some_and(|floor| floor != v.destination()) {
                    "That floor does not match my bridge entry.".into()
                } else {
                    self.report.merge(report);
                    if let Some(entry) = self.report.complete(needs_cases) {
                        if self
                            .rejected_entry
                            .is_some_and(|rejected| rejected.seal == entry.seal)
                        {
                            self.report.seal = None;
                            "That seal left my gate locked. I’m missing the correct seal.".into()
                        } else {
                            self.entry = Some(entry);
                            format!("{} Thank you.", entry.read_back())
                        }
                    } else {
                        self.report.request(needs_cases)
                    }
                },
            );
        } else if !text.starts_with("Your rescue bridge:") {
            self.reply = self.fact(&lower, v);
        }
        Ok(())
    }
    /// Pairs mechanical receipts with their actions so rejected operations never enter chat.
    fn respond(&mut self, reason: RespondReason) -> Result<Response<Action>> {
        if let Some(delay) = self.pending_delay.take() {
            self.waiting = true;
            return Ok(Response {
                action: None,
                message: self.reply.take(),
                wakeup: Some(WakeupRequest::After(delay)),
            });
        }
        if matches!(reason, RespondReason::WakeupDue) {
            self.waiting = false;
            self.ignore_pose_response = false;
        }
        if self.waiting {
            return Ok(Response {
                message: self.reply.take(),
                ..Response::default()
            });
        }
        if std::mem::take(&mut self.ignore_pose_response) {
            return Ok(Response::default());
        }
        let Some(v) = &self.view else {
            return Ok(Response::default());
        };
        let decision = self.choose(v);
        let decision = if matches!(decision, Some((Action::ReachRefuge, _))) {
            Some((
                crossing_for(self.role)
                    .get(self.crossing_sample)
                    .copied()
                    .map_or(Action::ReachRefuge, |pose| Action::BridgePose {
                        pose: Some(pose),
                    }),
                None,
            ))
        } else {
            decision
        };
        let mut message = self.reply.take();
        if std::mem::take(&mut self.announce) {
            let mut report = Self::message(v);
            if !v.handed_over {
                report.push(' ');
                report.push_str(Self::KEY_REASON);
            }
            message = Some(match message {
                Some(reply) if reply != report => format!("{report} {reply}"),
                _ => report,
            });
        }
        if std::mem::take(&mut self.ask_at_handoff) && self.entry.is_none() {
            let request = self.report.request(v.partner_delivery.is_some());
            message = Some(match message {
                Some(message) if message != request => format!("{message} {request}"),
                _ => request,
            });
        }
        let action = decision.map(|(action, motive)| {
            for line in [v.transfer_report(action), motive].into_iter().flatten() {
                message = Some(match message.take() {
                    Some(message) => format!("{message} {line}"),
                    None => line,
                });
            }
            action
        });
        Ok(Response {
            action,
            message,
            wakeup: None,
        })
    }
}
#[parlando_game_sdk::agent::async_trait]
impl Agent for Porter {
    type Config = PorterConfig;
    type Action = Action;
    type View = LazyView<View>;
    type Completion = Completion;

    /// Declares compatible settings and the stable identity shared by both builds.
    fn definition() -> Definition {
        Definition {
            id: "counterweight_hotel.porter",
            name: "Hotel porter",
            version: "11",
            description: "Shares rescue bridge details and operates one hotel lift.",
            config_fields: r#"[{"key":"pacing","label":"Pacing","help":"Set instant pacing for headless experiments.","type":"choice","choices":[{"value":"live","label":"Live"},{"value":"instant","label":"Instant"}],"required":false,"default_value":"live"}]"#,
        }
    }

    /// Initializes private policy state for this seat.
    async fn init(context: InitContext<Self::Config>) -> Result<Self> {
        let pacing = match context.config.pacing {
            PorterConfigPacing::Live => Pacing::LIVE,
            PorterConfigPacing::Instant => Pacing::INSTANT,
        };
        Ok(Self::new(context.role, pacing))
    }

    /// Receives the initial typed observation.
    async fn start(&mut self, view: Self::View) -> Result<()> {
        Self::start(self, view.decode()?)
    }

    /// Updates the policy from the accepted action first and then any accompanying chat.
    async fn observe(&mut self, observation: Observation<Action, Self::View>) -> Result<()> {
        if let Some(transition) = observation.transition {
            self.observe_transition_with(transition.actor, transition.action, || {
                transition.view.decode()
            })?;
        }
        if let Some(message) = observation.message {
            self.observe_message(message.sender, message.text)?;
        }
        Ok(())
    }

    /// Produces the next typed response for the assigned seat.
    async fn respond(&mut self, reason: RespondReason) -> Result<Response<Action>> {
        Self::respond(self, reason)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Route, State};

    /// A human footstep queued behind every porter observation cannot stall or accelerate the walk.
    #[test]
    fn walks_the_shared_decks_while_its_partner_moves() {
        let mut state = State::deal(17, Route::Escape);
        state.east = state.destination;
        state.passed = true;
        for (seat, role) in [PlayerRole::A, PlayerRole::B].into_iter().enumerate() {
            state = Hotel::apply_action(
                &state,
                &Action::Exit {
                    seal: state.seals[seat],
                },
                role,
            )
            .unwrap();
        }
        let mut porter = Porter::new(PlayerRole::B, Pacing::INSTANT);
        porter.start(Hotel::project(&state, PlayerRole::B)).unwrap();
        let east = crossing_for(PlayerRole::A);
        let west = crossing_for(PlayerRole::B);
        assert_ne!(east.first(), west.first());
        assert_eq!(east.last(), west.last());
        for (sample, peer_sample) in west.iter().copied().zip(east.iter().copied().cycle()) {
            let peer = Action::BridgePose {
                pose: Some(peer_sample),
            };
            state = Hotel::apply_action(&state, &peer, PlayerRole::A).unwrap();
            porter
                .observe_transition(PlayerRole::A, peer, Hotel::project(&state, PlayerRole::B))
                .unwrap();
            let response = porter.respond(RespondReason::AfterObservation).unwrap();
            let action = response
                .action
                .expect("human pose interrupted the crossing");
            assert_eq!(action, Action::BridgePose { pose: Some(sample) });
            state = Hotel::apply_action(&state, &action, PlayerRole::B).unwrap();
            assert_eq!(
                Hotel::observation(&state, PlayerRole::A).partner_bridge_pose,
                Some(sample)
            );
            porter
                .observe_transition(PlayerRole::B, action, Hotel::project(&state, PlayerRole::B))
                .unwrap();
        }
        assert!(matches!(
            porter.respond(RespondReason::AfterObservation).unwrap(),
            Response {
                action: Some(Action::ReachRefuge),
                message: None,
                ..
            }
        ));
    }

    /// The porter shares its ledger, then waits silently for an exploring guest.
    #[test]
    fn waits_for_guest_to_board_before_using_the_cable() {
        let state = State {
            east: 1,
            cargo: [0, 1],
            ..State::deal(17, Route::Escape)
        };
        let step_out = Action::StepOntoLanding;
        let outside = Hotel::apply_action(&state, &step_out, PlayerRole::B).unwrap();
        let mut porter = Porter::new(PlayerRole::A, Pacing::INSTANT);
        porter
            .start(Hotel::project(&outside, PlayerRole::A))
            .unwrap();
        let Response {
            action: None,
            message: Some(reply),
            ..
        } = porter.respond(RespondReason::AfterObservation).unwrap()
        else {
            panic!("expected the initial ledger entry");
        };
        assert_eq!(
            reply,
            format!(
                "{} {}",
                Porter::message(porter.view.as_ref().unwrap()),
                Porter::KEY_REASON
            )
        );
        assert!(porter
            .respond(RespondReason::AfterObservation)
            .is_ok_and(|response| response.action.is_none()
                && response.message.is_none()
                && response.wakeup.is_none()));

        let board = Action::BoardLift;
        let aboard = Hotel::apply_action(&outside, &board, PlayerRole::B).unwrap();
        porter
            .observe_transition(PlayerRole::B, board, Hotel::project(&aboard, PlayerRole::A))
            .unwrap();
        let Response {
            action: Some(Action::Release),
            message: Some(message),
            ..
        } = porter.respond(RespondReason::AfterObservation).unwrap()
        else {
            panic!("expected a brake release with its receipt");
        };
        assert!(message.starts_with("I pulled East’s brake for one stop"));
        assert!(message.contains("the key hatch"));
    }

    /// A committed brake stroke holds the next move while chat remains available.
    #[test]
    fn live_brake_stroke_allows_chat_before_wakeup() {
        let state = State::deal(17, Route::Escape);
        let mut porter = Porter::new(PlayerRole::A, Pacing::LIVE);
        porter.start(Hotel::project(&state, PlayerRole::A)).unwrap();
        porter
            .observe_transition(
                PlayerRole::B,
                Action::Release,
                Hotel::project(&state, PlayerRole::A),
            )
            .unwrap();

        let question = "Why do you need the brake key?";
        porter
            .observe_message(PlayerRole::B, question.into())
            .unwrap();
        let request = porter.respond(RespondReason::AfterObservation).unwrap();
        assert_eq!(
            request.wakeup,
            Some(WakeupRequest::After(Pacing::LIVE.brake_stroke))
        );
        assert!(request.action.is_none());
        assert_eq!(request.message.as_deref(), Some(Porter::KEY_REASON));

        porter
            .observe_message(PlayerRole::B, question.into())
            .unwrap();
        let reply = porter.respond(RespondReason::AfterObservation).unwrap();
        assert_eq!(reply.message.as_deref(), Some(Porter::KEY_REASON));
        assert!(reply.action.is_none() && reply.wakeup.is_none());

        let holding = porter.respond(RespondReason::AfterObservation).unwrap();
        assert!(holding.action.is_none() && holding.message.is_none() && holding.wakeup.is_none());

        let ready = porter.respond(RespondReason::WakeupDue).unwrap();
        assert!(ready.message.unwrap().starts_with("Your rescue bridge:"));
        assert!(ready.wakeup.is_none());
    }

    /// A wrong inscription trips a secured gate. The porter resets the lock and
    /// waits for a corrected entry before opening the gate and crossing.
    #[test]
    fn locked_door_waits_for_a_corrected_ledger_entry() {
        let mut state = State::deal(93, Route::Escape);
        state.east = state.destination;
        state.passed = true;
        state.seals[0] = Seal::Rose;
        state.on_landing[1] = true;
        let mut porter = Porter::new(PlayerRole::A, Pacing::INSTANT);
        porter.start(Hotel::project(&state, PlayerRole::A)).unwrap();
        assert!(matches!(
            porter.respond(RespondReason::AfterObservation).unwrap(),
            Response {
                action: None,
                message: Some(_),
                ..
            }
        ));
        let wrong = format!("Floor {}, stag", state.east);
        porter
            .observe_message(PlayerRole::B, wrong.clone())
            .unwrap();
        let Response {
            action: Some(wrong_exit),
            ..
        } = porter.respond(RespondReason::AfterObservation).unwrap()
        else {
            panic!("expected exit attempt")
        };
        state = Hotel::apply_action(&state, &wrong_exit, PlayerRole::A).unwrap();
        porter
            .observe_transition(
                PlayerRole::A,
                wrong_exit,
                Hotel::project(&state, PlayerRole::A),
            )
            .unwrap();
        let Response {
            action: Some(Action::ResetLock),
            message: Some(message),
            ..
        } = porter.respond(RespondReason::AfterObservation).unwrap()
        else {
            panic!("expected the porter to rearm the lock");
        };
        assert!(message.contains("reset it at the brake pedestal"));
        state = Hotel::apply_action(&state, &Action::ResetLock, PlayerRole::A).unwrap();
        porter
            .observe_transition(
                PlayerRole::A,
                Action::ResetLock,
                Hotel::project(&state, PlayerRole::A),
            )
            .unwrap();
        assert!(porter
            .respond(RespondReason::AfterObservation)
            .is_ok_and(|response| response.action.is_none()
                && response.message.is_none()
                && response.wakeup.is_none()));
        porter.observe_message(PlayerRole::B, wrong).unwrap();
        assert!(matches!(
            porter.respond(RespondReason::AfterObservation).unwrap(),
            Response {
                action: None,
                message: Some(_),
                ..
            }
        ));
        porter
            .observe_message(PlayerRole::B, format!("Floor {}, rose", state.east))
            .unwrap();
        let Response {
            action: Some(retry),
            ..
        } = porter.respond(RespondReason::AfterObservation).unwrap()
        else {
            panic!("expected corrected exit")
        };
        let next = Hotel::apply_action(&state, &retry, PlayerRole::A).unwrap();
        porter
            .observe_transition(PlayerRole::A, retry, Hotel::project(&next, PlayerRole::A))
            .unwrap();
        assert!(matches!(
            porter.respond(RespondReason::AfterObservation).unwrap(),
            Response {
                action: Some(Action::BridgePose { pose: Some(_) }),
                message: None,
                ..
            }
        ));
        let crossed = Hotel::apply_action(&next, &Action::ReachRefuge, PlayerRole::A).unwrap();
        porter
            .observe_transition(
                PlayerRole::A,
                Action::ReachRefuge,
                Hotel::project(&crossed, PlayerRole::A),
            )
            .unwrap();
        assert!(porter
            .respond(RespondReason::AfterObservation)
            .is_ok_and(|response| response.action.is_none()
                && response.message.is_none()
                && response.wakeup.is_none()));
        porter
            .observe_message(PlayerRole::B, "What was my bridge entry?".into())
            .unwrap();
        let Response {
            action: None,
            message: Some(repeated),
            ..
        } = porter.respond(RespondReason::AfterObservation).unwrap()
        else {
            panic!("porter should repeat the remaining guest’s bridge entry after crossing");
        };
        assert_eq!(repeated, Porter::message(porter.view.as_ref().unwrap()));
    }

    /// Ordinary text at the meeting floor receives a receipt even while the guest holds the brake key.
    #[test]
    fn acknowledges_an_entry_while_waiting_for_the_brake_key() {
        let mut state = State::deal(0, Route::Escape);
        state.east = 2;
        state.destination = 4;
        let mut porter = Porter::new(PlayerRole::B, Pacing::INSTANT);
        porter.start(Hotel::project(&state, PlayerRole::B)).unwrap();
        porter.respond(RespondReason::AfterObservation).unwrap();
        for _ in 0..2 {
            porter
                .observe_message(PlayerRole::A, "Ground floor, stag seal".into())
                .unwrap();
            let Response {
                action: None,
                message: Some(reply),
                ..
            } = porter.respond(RespondReason::AfterObservation).unwrap()
            else {
                panic!("the waiting porter must reply");
            };
            assert_eq!(reply, "My bridge entry is Ground, stag seal. Thank you.");
            assert!(porter
                .respond(RespondReason::AfterObservation)
                .is_ok_and(|response| response.action.is_none()
                    && response.message.is_none()
                    && response.wakeup.is_none()));
        }
    }

    /// Accepts the zero-case dispatch on either possible West destination.
    #[test]
    fn reads_zero_case_entries() {
        for (text, floor) in [
            ("Ground floor, stag seal, no suitcases", 0),
            ("Floor 0, stag, 0 cases", 0),
            ("Floor one, stag seal, zero cases", 1),
            ("Rescue Supplies: Floor 1, stag seal, 0 cases", 1),
        ] {
            let entry = EntryReport::parse(text, true).complete(true).expect(text);
            assert_eq!(entry.floor, floor);
            assert_eq!(entry.cases, Some(0));
        }
        assert!(EntryReport::parse("Ground floor, stag seal", true)
            .complete(true)
            .is_none());
    }

    /// A multi-message entry keeps the brake parked until every required fact arrives.
    #[test]
    fn handoff_requires_all_report_fields_before_departure() {
        let mut state = State::deal(0, Route::RescueSupplies);
        state.east = 2;
        state.destination = 3;
        state.cargo = [2, 0];
        state.passed = true;
        let mut porter = Porter::new(PlayerRole::B, Pacing::INSTANT);
        porter.start(Hotel::project(&state, PlayerRole::B)).unwrap();
        assert!(matches!(
            porter.respond(RespondReason::AfterObservation).unwrap(),
            Response { action: None, message: Some(message), .. } if message.contains("I’m missing my bridge floor, seal, and suitcase count.")
        ));
        for (report, missing) in [
            (
                "First floor for your bridge.",
                "I’m missing my seal and suitcase count.",
            ),
            (
                "You have the brake key.",
                "I’m missing my seal and suitcase count.",
            ),
            ("The seal is a key seal.", "I’m missing my suitcase count."),
        ] {
            porter
                .observe_message(PlayerRole::A, report.into())
                .unwrap();
            match porter.respond(RespondReason::AfterObservation).unwrap() {
                Response {
                    action: None,
                    message: Some(message),
                    ..
                } => assert!(message.contains(missing)),
                Response {
                    action: None,
                    message: None,
                    ..
                } if report == "You have the brake key." => {
                    assert!(porter.report.seal.is_none())
                }
                response => panic!("unexpected dispatch response: {response:?}"),
            }
            assert!(porter
                .respond(RespondReason::AfterObservation)
                .is_ok_and(|response| response.action.is_none()
                    && response.message.is_none()
                    && response.wakeup.is_none()));
        }
        porter
            .observe_message(PlayerRole::A, "No cases for your lift.".into())
            .unwrap();
        assert!(matches!(
            porter.respond(RespondReason::AfterObservation).unwrap(),
            Response { action: Some(Action::Release), message: Some(message), .. }
                if message.contains("Floor 1, key seal, 0 suitcases")
        ));
    }

    /// The porter reports ledger facts and the received dispatch without coaching.
    #[test]
    fn chat_at_the_handoff_only_shares_facts() {
        let mut state = State::deal(0, Route::Escape);
        state.east = 2;
        state.destination = 4;
        let mut porter = Porter::new(PlayerRole::B, Pacing::INSTANT);
        porter.start(Hotel::project(&state, PlayerRole::B)).unwrap();
        assert!(matches!(
            porter.respond(RespondReason::AfterObservation).unwrap(),
            Response {
                action: None,
                message: Some(_),
                ..
            }
        ));
        let ledger = Porter::message(porter.view.as_ref().unwrap());
        for (prompt, expected) in [
            ("Help!", "My lift is stopped at Floor 2."),
            ("I'm stuck", "My lift is stopped at Floor 2."),
            ("What should I do now?", "My lift is stopped at Floor 2."),
            ("What do you need?", "I’m missing my bridge floor and seal."),
            ("Where are you?", "My lift is stopped at Floor 2."),
            ("Why do you need the brake key", Porter::KEY_REASON),
            ("Who has the brake key?", "You have the brake key."),
            (
                "How many cases do you have?",
                "My lift carries 0 suitcases.",
            ),
            (
                "How many suitcases are you carrying?",
                "My lift carries 0 suitcases.",
            ),
            ("What is my floor?", ledger.as_str()),
            ("What is my seal?", ledger.as_str()),
            ("What was my booking?", ledger.as_str()),
        ] {
            porter
                .observe_message(PlayerRole::A, prompt.into())
                .unwrap();
            let Response {
                action: None,
                message: Some(reply),
                ..
            } = porter.respond(RespondReason::AfterObservation).unwrap()
            else {
                panic!("expected a factual reply to {prompt}");
            };
            assert_eq!(reply, expected, "{prompt}");
            assert!(porter
                .respond(RespondReason::AfterObservation)
                .is_ok_and(|response| response.action.is_none()
                    && response.message.is_none()
                    && response.wakeup.is_none()));
        }
        porter
            .observe_message(PlayerRole::A, "Floor 1, stag seal".into())
            .unwrap();
        let Response {
            action: None,
            message: Some(reply),
            ..
        } = porter.respond(RespondReason::AfterObservation).unwrap()
        else {
            panic!("expected a receipt for a mismatched entry");
        };
        assert_eq!(reply, "That floor does not match my bridge entry.");
        assert!(porter.entry.is_none());
    }

    /// Reports the selected rack and resulting count on the transfer that commits them.
    #[test]
    fn case_receipts_use_the_service_rack_and_resulting_count() {
        for (east, cargo, local_rack, action, expected) in [
            (
                1,
                0,
                0,
                Action::Load,
                "I loaded a suitcase from the Floor 2 rack. My lift now carries 1 suitcase. I need my lift heavier than yours so it can go down to the key hatch.",
            ),
            (
                3,
                1,
                6,
                Action::Unload,
                "I put a suitcase on the Ground rack. My lift now carries 0 suitcases. I need my lift lighter than yours so it can go up to the key hatch.",
            ),
        ] {
            let role = PlayerRole::B;
            let mut state = State::deal(0, Route::RescueSupplies);
            state.east = east;
            state.cargo = [0, cargo];
            state.landing_loads[1][usize::from(4 - east)] = local_rack;
            let mut porter = Porter::new(role, Pacing::INSTANT);
            let mut view = Hotel::project(&state, role);
            view.actions.clear();
            porter.start(view).unwrap();
            assert!(matches!(
                porter.respond(RespondReason::AfterObservation).unwrap(),
                Response { action: None, message: Some(message), .. } if message.starts_with("Your rescue bridge:")
            ));
            assert!(porter.respond(RespondReason::AfterObservation).is_ok_and(|response| response.action.is_none() && response.message.is_none() && response.wakeup.is_none()));
            porter.view.as_mut().unwrap().actions.push(action);
            assert!(matches!(
                porter.respond(RespondReason::AfterObservation).unwrap(),
                Response { action: Some(actual), message: Some(message), .. }
                    if actual == action && message == expected
            ));
            porter.view.as_mut().unwrap().actions.clear();
            porter
                .observe_message(PlayerRole::A, "Why are you moving a suitcase?".into())
                .unwrap();
            assert!(matches!(
                porter.respond(RespondReason::AfterObservation).unwrap(),
                Response { action: None, message: Some(message), .. } if message.contains("the heavier lift goes down")
            ));
            assert!(porter.report.cases.is_none());

            let after = Hotel::apply_action(&state, &action, role).unwrap();
            // Restore the received view before delivering the confirmed transfer.
            porter.view = Some(Hotel::project(&state, role));
            porter
                .observe_transition(role, action, Hotel::project(&after, role))
                .unwrap();
            porter.view.as_mut().unwrap().actions.clear();
            for prompt in [
                "Why do you keep loading and unloading suitcases?",
                "How come you moved a suitcase",
            ] {
                porter
                    .observe_message(PlayerRole::A, prompt.into())
                    .unwrap();
                assert!(matches!(
                    porter.respond(RespondReason::AfterObservation).unwrap(),
                    Response { action: None, message: Some(message), .. } if expected.ends_with(&message)
                ));
                assert!(porter.report.cases.is_none());
            }
            assert!(porter.respond(RespondReason::AfterObservation).is_ok_and(|response| response.action.is_none() && response.message.is_none() && response.wakeup.is_none()));
        }
    }

    /// The driver stays parked for a full East car, and the East porter
    /// preserves room for the shipment even when West explores its landing.
    #[test]
    fn pickup_waits_for_east_capacity_and_both_guests() {
        let mut state = State {
            east: 1,
            cargo: [CABIN_CAPACITY, 1],
            passed: true,
            ..State::deal(3, Route::RescueSupplies)
        };
        let mut east = Porter::new(PlayerRole::A, Pacing::INSTANT);
        let mut west = Porter::new(PlayerRole::B, Pacing::INSTANT);
        for porter in [&mut east, &mut west] {
            porter.start(Hotel::project(&state, porter.role)).unwrap();
            let other = if porter.role == PlayerRole::A {
                PlayerRole::B
            } else {
                PlayerRole::A
            };
            let view = Hotel::project(&state, other);
            porter
                .observe_message(other, Porter::message(&view))
                .unwrap();
        }

        assert!(state.controls(1).contains(&Action::Release));
        assert_eq!(
            west.choose(west.view.as_ref().unwrap())
                .map(|(action, _)| action),
            None
        );
        assert_eq!(
            east.choose(east.view.as_ref().unwrap())
                .map(|(action, _)| action),
            Some(Action::Unload)
        );
        state = state.step(Action::StepOntoLanding, 1).unwrap();
        state = state.step(Action::Unload, 0).unwrap();
        east.update(Hotel::project(&state, PlayerRole::A));
        assert_eq!(
            east.choose(east.view.as_ref().unwrap())
                .map(|(action, _)| action),
            None
        );
        state = state.step(Action::BoardLift, 1).unwrap();
        east.update(Hotel::project(&state, PlayerRole::A));
        assert_eq!(
            east.choose(east.view.as_ref().unwrap())
                .map(|(action, _)| action),
            Some(Action::CollectRescueCase)
        );
        state = state.step(Action::CollectRescueCase, 0).unwrap();
        east.update(Hotel::project(&state, PlayerRole::A));
        west.update(Hotel::project(&state, PlayerRole::B));
        assert_eq!(
            east.choose(east.view.as_ref().unwrap())
                .map(|(action, _)| action),
            Some(Action::Unload)
        );
        assert_eq!(
            west.choose(west.view.as_ref().unwrap())
                .map(|(action, _)| action),
            Some(Action::Load)
        );

        state = state.step(Action::Unload, 0).unwrap();
        state = state.step(Action::Load, 1).unwrap();
        state = state.step(Action::Unload, 0).unwrap();
        west.update(Hotel::project(&state, PlayerRole::B));
        assert_eq!(
            west.choose(west.view.as_ref().unwrap())
                .map(|(action, _)| action),
            Some(Action::Release)
        );
    }

    /// Two participants finish using the ledger entries and normal actions, even after a late share.
    #[test]
    fn cooperating_porters_finish_both_routes() {
        for route in [Route::Escape, Route::RescueSupplies] {
            for seed in 0..48 {
                let mut state = State::deal(seed, route);
                let roles = [PlayerRole::A, PlayerRole::B];
                let participants = roles.map(|role| Porter::new(role, Pacing::INSTANT));
                let [mut east, mut west] = participants;
                east.start(Hotel::project(&state, PlayerRole::A)).unwrap();
                west.start(Hotel::project(&state, PlayerRole::B)).unwrap();
                let mut deferred_entry = None;
                let mut waited_for_entry = false;
                for turn in 0..100 + 2 * crossing_for(PlayerRole::B).len() {
                    if waited_for_entry {
                        if let Some(message) = deferred_entry.take() {
                            west.observe_message(PlayerRole::A, message).unwrap();
                        }
                    }
                    let (role, actor, peer) = if turn % 2 == 0 {
                        (PlayerRole::A, &mut east, &mut west)
                    } else {
                        (PlayerRole::B, &mut west, &mut east)
                    };
                    let Response {
                        action, message, ..
                    } = actor.respond(RespondReason::AfterObservation).unwrap();
                    if role == PlayerRole::B && state.passed && actor.entry.is_none() {
                        assert!(action.is_none(), "porter drove without a dispatch entry");
                        if let Some(message) = &message {
                            waited_for_entry |= message.contains("I’m missing my bridge floor");
                        }
                    }
                    if action == Some(Action::Release) {
                        let wing = if role == PlayerRole::A {
                            "East"
                        } else {
                            "West"
                        };
                        let receipt = message.as_deref().expect("one receipt per release");
                        assert!(receipt.contains(&format!("I pulled {wing}’s brake for one stop")));
                        assert!(receipt.contains("The heavier car pulls the lighter one up."));
                    }
                    if let Some(message) = message {
                        if turn == 0 && seed % 2 == 0 {
                            deferred_entry = Some(message);
                        } else {
                            peer.observe_message(role, message).unwrap();
                        }
                    }
                    if let Some(action) = action {
                        state = Hotel::apply_action(&state, &action, role).unwrap();
                        actor
                            .observe_transition(role, action, Hotel::project(&state, role))
                            .unwrap();
                        peer.observe_transition(role, action, Hotel::project(&state, peer.role))
                            .unwrap();
                        if Hotel::outcome(&state).is_some() {
                            break;
                        }
                    }
                }
                if seed % 2 == 0 {
                    assert!(
                        waited_for_entry,
                        "seed {seed} reached handoff after the deferral"
                    );
                }
                assert!(Hotel::outcome(&state).is_some(), "{state:?}");
            }
        }
    }
}
