//! The scripted Night Freight partner (`night_freight.partner`).
//!
//! This policy is the packaged, dependency-free partner. A WASM component
//! cannot call a language model, so it does not plan. It does the parts of the
//! game that are deterministic, and it lets the human use it as a source of
//! the facts that only its role knows:
//!
//! * **Information boundary.** The policy reads its own role view and the
//!   human's chat messages, nothing else. Parlando also delivers the human's
//!   accepted `Edit` actions to the policy (design note 15.1). Their payloads
//!   are never inspected: [`Partner::observe`] looks only at the view that
//!   comes with a transition and at the chat message. The partner's notes are
//!   not part of that view.
//! * **Automatic steps.** `ReplayFinished` after a replay and `Continue` at the
//!   end of a scenario are sent as soon as they are legal.
//! * **Dialogue.** On entering a planning phase the policy introduces itself
//!   and states its private briefing (and, in act two, its private
//!   disruption announcement). It repeats its facts when the human asks. It
//!   presses *Ready* when the human says they are ready and withdraws it when
//!   the human asks it to wait.
//!
//! * **Note commands.** The policy does not plan, but the human can steer its
//!   notes with short typed commands (see [`commands`]). A role owns only some
//!   fields (Routing the stations and departures, Freight the ops), yet a
//!   player may edit every field of their own notes, so the policy accepts
//!   every command and says when the field is not used by its role. Each
//!   command compiles to [`Action::Edit`]s. A policy proposes one action per
//!   decision, so the edits wait in a queue: every accepted edit reaches the
//!   policy as an observation, which prompts the next decision. The
//!   confirmation travels with the last edit and is sent when the game accepts
//!   it; a rejection drops the rest of the queue and is reported in words.
//!
//! The LLM-backed partner in `agent/` (Python, remote gRPC) plans on its own;
//! this one is the fallback for headless checks and for sessions without a
//! model key.

pub mod commands;

use self::commands::{compile, describe_edit, parse_message, reject_words, Item, Vocabulary};
use crate::plan::NoteEdit;
use crate::world::{CarAt, CarKind, Commodity};
use crate::{Action, Briefing, Completion, LastRun, Phase, Seat, View};
use parlando_game_sdk::agent::{
    Agent, Definition, InitContext, Observation, RespondReason, Response, Result,
};
use serde::Deserialize;
use std::collections::VecDeque;

/// When the partner presses *Ready*.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Readiness {
    /// Only after the human says they are ready (the default for play).
    #[default]
    OnRequest,
    /// As soon as the partner has stated its facts, with no cue. Headless
    /// checks use this because no human is there to speak.
    Immediately,
}

/// Dashboard settings of one partner instance.
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PartnerConfig {
    /// When to press *Ready*.
    readiness: Readiness,
}

/// What the human has asked of the partner since its last response.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cues {
    /// The human said they are ready to run.
    pub ready: bool,
    /// The human asked the partner to wait or to withdraw readiness.
    pub hold: bool,
    /// The human asked what the partner knows.
    pub facts: bool,
}

/// Words that signal readiness, matched against lower-cased chat text.
const READY_WORDS: &[&str] = &[
    "ready", "run it", "let's run", "lets run", "go ahead", "let's go", "lets go", "send it",
    "start the run", "do it",
];

/// Words that cancel readiness; they win over [`READY_WORDS`] in one message.
const HOLD_WORDS: &[&str] = &[
    "not ready", "wait", "hold on", "hold off", "don't", "dont", "not yet", "stop", "unready",
];

/// Words that ask the partner for what it knows.
const FACT_WORDS: &[&str] = &[
    "what do you",
    "what are",
    "what's the",
    "tell me",
    "share",
    "your orders",
    "your cars",
    "your times",
    "travel time",
    "how long",
    "do you know",
    "which orders",
    "briefing",
];

/// Reads the human's request from one chat message.
///
/// The test is on lower-cased substrings. A message that both asks to hold and
/// to run counts as a request to hold, because the safe reading of
/// "not ready, wait" is to wait.
pub fn read_cues(text: &str) -> Cues {
    let lower = text.to_lowercase();
    let any = |words: &[&str]| words.iter().any(|w| lower.contains(w));
    let hold = any(HOLD_WORDS);
    Cues {
        ready: !hold && any(READY_WORDS),
        hold,
        facts: any(FACT_WORDS),
    }
}

/// Spoken name of a commodity.
fn commodity(c: Commodity) -> &'static str {
    match c {
        Commodity::Oranges => "oranges",
        Commodity::Bananas => "bananas",
        Commodity::Juice => "juice",
    }
}

/// Spoken name of a car, with its load.
fn car_word(kind: CarKind, load: Option<Commodity>) -> String {
    let kind = match kind {
        CarKind::Boxcar => "boxcar",
        CarKind::Tanker => "tanker",
    };
    match load {
        Some(l) => format!("loaded {kind} ({})", commodity(l)),
        None => format!("empty {kind}"),
    }
}

/// States the viewer's private briefing in plain sentences.
///
/// Receives the view and returns text containing exactly the facts of the
/// viewer's own role: nothing else is available to it.
pub fn describe_briefing(view: &View) -> String {
    match &view.briefing {
        Briefing::Freight {
            orders,
            cars,
            stock,
            factories,
            load_hours,
            deliver_hours,
            ..
        } => {
            let mut parts = Vec::new();
            for o in orders {
                parts.push(format!(
                    "order {}: {} car load of {} to {} by hour {} ({} delivered)",
                    o.id,
                    o.count,
                    commodity(o.commodity),
                    o.to,
                    o.deadline,
                    o.delivered
                ));
            }
            for c in cars {
                let at = match &c.at {
                    CarAt::Yard { station } => format!("in the {station} yard"),
                    CarAt::Coupled { engine } => format!("coupled to {engine}"),
                };
                parts.push(format!("car {}: {} {at}", c.id, car_word(c.kind, c.load)));
            }
            for s in stock {
                let items: Vec<_> = s.commodities.iter().map(|c| commodity(*c)).collect();
                parts.push(format!("{} stocks {}", s.station, items.join(", ")));
            }
            for f in factories {
                parts.push(format!(
                    "the {} factory turns {} into {} in {}h",
                    f.station,
                    commodity(f.input),
                    commodity(f.output),
                    f.hours
                ));
            }
            parts.push(format!(
                "loading takes {load_hours}h, delivering {deliver_hours}h"
            ));
            format!("My freight facts: {}.", parts.join("; "))
        }
        Briefing::Routing {
            travel_times,
            restrictions,
            capacities,
            ..
        } => {
            let mut parts = Vec::new();
            for e in travel_times {
                parts.push(format!("{} to {} takes {}h", e.a, e.b, e.hours));
            }
            for r in restrictions {
                parts.push(format!(
                    "{} to {} is closed to entry from hour {} until hour {}",
                    r.a, r.b, r.from_hour, r.until_hour
                ));
            }
            for c in capacities {
                parts.push(format!("{} pulls at most {} cars", c.engine, c.capacity));
            }
            for e in &view.map.engines {
                if let Some(h) = e.available_from {
                    parts.push(format!("{} can start from hour {h}", e.id));
                }
            }
            format!("My routing facts: {}.", parts.join("; "))
        }
    }
}

/// The private disruption announcement in the viewer's briefing, if any.
fn announcement(view: &View) -> Option<&str> {
    use crate::disruption::Announcement::{EngineFailure, LateOrder, TrackClosure};
    match &view.briefing {
        Briefing::Freight { announcement, .. } | Briefing::Routing { announcement, .. } => {
            announcement.as_ref().map(|a| match a {
                EngineFailure { message, .. }
                | TrackClosure { message, .. }
                | LateOrder { message, .. } => message.as_str(),
            })
        }
    }
}

/// Summarizes the outcome of a joint run for the human.
pub fn describe_run(run: &LastRun) -> String {
    let orders: Vec<_> = run
        .orders
        .iter()
        .map(|o| {
            format!(
                "{} {}",
                o.id,
                if o.met { "met" } else { "not met" }
            )
        })
        .collect();
    let mut text = format!("Run {}: {}.", run.number, orders.join(", "));
    if let Some(h) = run.interrupted_at {
        text.push_str(&format!(" The day was interrupted at hour {h}."));
    }
    if let Some(d) = &run.divergence {
        text.push_str(&format!(
            " My notes first differed from the film at hour {} on {}.",
            d.hour, d.engine
        ));
    }
    text
}

/// Identifies one planning phase: scenario, act and number of runs so far.
type PlanningKey = (usize, u8, u8);

/// Per-session memory of the partner.
pub struct Partner {
    /// The latest role view, the only game state the policy keeps.
    view: Option<View>,
    /// Requests from the human not yet answered.
    cues: Cues,
    /// The planning phase the partner last spoke in.
    introduced: Option<PlanningKey>,
    /// When the partner presses *Ready*.
    readiness: Readiness,
    /// Commands and replies heard from the human and not yet carried out.
    queue: VecDeque<Item>,
    /// The edits of the command being carried out, one per decision.
    batch: VecDeque<NoteEdit>,
    /// The confirmation sent with the last edit of the batch.
    batch_say: Option<String>,
    /// The edit proposed and not yet known to be accepted.
    in_flight: Option<NoteEdit>,
    /// Replies waiting for the next message.
    outbox: Vec<String>,
}

impl Partner {
    /// Creates the policy for one session.
    pub fn new() -> Self {
        Self {
            view: None,
            cues: Cues::default(),
            introduced: None,
            readiness: Readiness::OnRequest,
            queue: VecDeque::new(),
            batch: VecDeque::new(),
            batch_say: None,
            in_flight: None,
            outbox: Vec::new(),
        }
    }

    /// Stores a fresh view. Transition payloads (actor and action) are not
    /// passed here on purpose.
    pub fn update(&mut self, view: View) {
        self.view = Some(view);
    }

    /// Records the human's requests from a chat message.
    ///
    /// Note commands join the queue in the order written; the rest of the
    /// text is read for the readiness and fact cues.
    pub fn hear(&mut self, text: &str) {
        let vocab = match &self.view {
            Some(v) => Vocabulary::new(v.map.stations.iter().map(|s| (s.id.as_str(), s.name.as_str()))),
            None => Vocabulary::default(),
        };
        let parsed = parse_message(text, &vocab);
        self.queue.extend(parsed.items);
        let c = read_cues(&parsed.rest);
        self.cues.ready |= c.ready;
        self.cues.hold |= c.hold;
        self.cues.facts |= c.facts;
        // A later hold cancels an earlier ready cue and vice versa.
        if c.hold {
            self.cues.ready = false;
        } else if c.ready {
            self.cues.hold = false;
        }
    }

    /// Handles the game's rejection of the edit (or other action) just sent.
    ///
    /// Drops the unfinished command and everything queued behind it, because
    /// later commands may have relied on it, and explains the rejection.
    pub fn rejected(&mut self, code: &str) {
        let attempt = self.in_flight.take().map(|e| describe_edit(&e));
        let dropped = !self.batch.is_empty() || !self.queue.is_empty();
        self.batch.clear();
        self.batch_say = None;
        self.queue.clear();
        let what = attempt.map_or(String::new(), |a| format!(" ({a})"));
        let mut text = format!("I could not do that{what}: {}.", reject_words(code));
        if dropped {
            text.push_str(" I dropped the rest of your commands.");
        }
        self.outbox.push(text);
    }

    /// Takes the next edit of the queued commands.
    ///
    /// Returns the edit and, for the last edit of a command, the confirmation
    /// to send with it. Commands without edits (help, show notes, problems)
    /// only add to the outbox. Commands compile against the notes as they are
    /// when their turn comes, so a command may rely on the one before it.
    fn next_edit(&mut self, view: &View) -> Option<(NoteEdit, Option<String>)> {
        loop {
            if let Some(edit) = self.batch.pop_front() {
                let say = if self.batch.is_empty() {
                    self.batch_say.take()
                } else {
                    None
                };
                return Some((edit, say));
            }
            match self.queue.pop_front()? {
                Item::Problem(text) => self.outbox.push(text),
                Item::Command(command) => match compile(&command, &view.notes, view.role) {
                    Err(text) => self.outbox.push(text),
                    Ok(c) if c.edits.is_empty() => self.outbox.push(c.say),
                    Ok(c) => {
                        self.batch = c.edits.into();
                        self.batch_say = Some(c.say);
                    }
                },
            }
        }
    }

    /// Chooses the partner's response to the current view and requests.
    ///
    /// Replay and scenario-end steps are automatic. In planning the partner
    /// carries out note commands one edit per decision, speaks at most once
    /// per phase unprompted, answers fact requests, and follows the human's
    /// readiness cues once its edits are done. Outside planning, queued note
    /// commands are dropped with a notice. Sending it twice with no new
    /// observation in between yields no further output.
    pub fn decide(&mut self) -> Response<Action> {
        let Some(view) = self.view.clone() else {
            return Response::default();
        };
        if view.phase != Phase::Planning
            && !(self.queue.is_empty() && self.batch.is_empty())
        {
            self.queue.clear();
            self.batch.clear();
            self.batch_say = None;
            self.outbox
                .push("I can only edit notes while planning, so I dropped your commands.".into());
        }
        let mut response = self.decide_phase(&view);
        if view.phase != Phase::Planning && !self.outbox.is_empty() {
            let notice = self.outbox.drain(..).collect::<Vec<_>>().join("\n");
            response.message = Some(match response.message.take() {
                Some(m) => format!("{m}\n{notice}"),
                None => notice,
            });
        }
        response
    }

    /// The response for one phase, before outbox replies of non-planning
    /// phases are added.
    fn decide_phase(&mut self, view: &View) -> Response<Action> {
        match view.phase {
            Phase::Replaying if !view.replay_finished.me => {
                let text = view
                    .last_run
                    .as_ref()
                    .map(describe_run)
                    .unwrap_or_else(|| "Replay watched.".into());
                Response::action_and_message(Action::ReplayFinished, text)
            }
            Phase::ScenarioDone if !view.r#continue.me => {
                let text = view
                    .scores
                    .last()
                    .map(|s| {
                        format!(
                            "Scenario {} over: {} of {} orders met in {} runs.",
                            s.scenario, s.orders_met, s.orders_total, s.runs_used
                        )
                    })
                    .unwrap_or_else(|| "Scenario over.".into());
                Response::action_and_message(Action::Continue, text)
            }
            Phase::Planning => self.plan(view),
            _ => Response::default(),
        }
    }

    /// The planning-phase response: introduction, facts, note edits and
    /// readiness.
    fn plan(&mut self, view: &View) -> Response<Action> {
        let mut texts: Vec<String> = Vec::new();
        let key = (view.scenario_index, view.act, view.runs_used);
        let first_in_phase = self.introduced != Some(key);
        if first_in_phase {
            self.introduced = Some(key);
            let who = match view.role {
                Seat::Freight => "Freight",
                Seat::Routing => "Routing",
            };
            if view.act == 1 && view.runs_used == 0 {
                texts.push(format!(
                    "I'm {who} on {}. {} Type help for my note commands.",
                    view.scenario_title,
                    describe_briefing(view)
                ));
            } else if let Some(a) = announcement(view) {
                texts.push(format!("New for me: {a}"));
            }
        } else if self.cues.facts {
            texts.push(describe_briefing(view));
        }
        self.cues.facts = false;
        if let Some((edit, say)) = self.next_edit(view) {
            // Readiness cues wait until the edits are done: edits withdraw it.
            if let Some(say) = say {
                texts.append(&mut self.outbox);
                texts.push(say);
            }
            self.in_flight = Some(edit.clone());
            return Self::respond_with(Some(Action::Edit { edit }), texts);
        }
        texts.append(&mut self.outbox);
        let mut action = None;
        if self.cues.hold && view.ready.me {
            action = Some(Action::SetReady { ready: false });
            texts.push("Holding off.".into());
        } else if (self.cues.ready || (first_in_phase && self.readiness == Readiness::Immediately))
            && !view.ready.me
        {
            action = Some(Action::SetReady { ready: true });
            texts.push("Ready on my side.".into());
        }
        self.cues.ready = false;
        self.cues.hold = false;
        Self::respond_with(action, texts)
    }

    /// Builds a response from an optional action and the texts to send.
    fn respond_with(action: Option<Action>, texts: Vec<String>) -> Response<Action> {
        let text = texts.join("\n");
        match (action, text.is_empty()) {
            (Some(a), false) => Response::action_and_message(a, text),
            (Some(a), true) => Response::action(a),
            (None, false) => Response::message(text),
            (None, true) => Response::default(),
        }
    }
}

impl Default for Partner {
    /// Creates the policy for one session.
    fn default() -> Self {
        Self::new()
    }
}

#[parlando_game_sdk::agent::async_trait]
impl Agent for Partner {
    type Config = PartnerConfig;
    type Action = Action;
    type View = View;
    type Completion = Completion;

    /// Identifies the policy; the id and name match `agent.yaml`.
    fn definition() -> Definition {
        Definition {
            id: "night_freight.partner",
            name: "Night Freight scripted partner",
            version: "1.0.0",
            description: "Plays either role without a language model: states its private \
                facts, presses ready when you do, watches replays and continues \
                automatically, and edits its own notes when you type short commands \
                such as \"E1 to Dansville\" (type help in the chat).",
            config_fields: r#"[{"key":"readiness","label":"Readiness","help":"Press Ready only on request, or at once (for headless checks).","type":"choice","choices":[{"value":"on_request","label":"On request"},{"value":"immediately","label":"Immediately"}],"required":false,"default_value":"on_request"}]"#,
        }
    }

    /// Creates a fresh policy; the seat comes from the first view's role.
    async fn init(context: InitContext<PartnerConfig>) -> Result<Self> {
        Ok(Self {
            readiness: context.config.readiness,
            ..Self::new()
        })
    }

    /// Stores the opening view.
    async fn start(&mut self, view: View) -> Result<()> {
        self.update(view);
        Ok(())
    }

    /// Handles the view of an accepted transition, then a chat message.
    ///
    /// The transition's actor and action are deliberately not read: they may
    /// be the human's `Edit`, which belongs to the human's notes.
    async fn observe(&mut self, event: Observation<Action, View>) -> Result<()> {
        if let Some(t) = event.transition {
            self.update(t.view);
        }
        if let Some(m) = event.message {
            self.hear(&m.text);
        }
        Ok(())
    }

    /// Takes the one decision of this opportunity.
    ///
    /// A rejection reason first reports the rejected edit; any other reason
    /// means the previous edit was accepted.
    async fn respond(&mut self, reason: RespondReason) -> Result<Response<Action>> {
        match reason {
            RespondReason::ActionRejected(code) => self.rejected(&code),
            _ => self.in_flight = None,
        }
        Ok(self.decide())
    }
}

#[cfg(test)]
mod tests;
