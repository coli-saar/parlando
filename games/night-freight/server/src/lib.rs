//! Night Freight: a two-player Parlando dialogue game modelled on the TRAINS
//! corpus.
//!
//! Two dispatchers, *Freight* and *Routing*, plan a day of freight movements on
//! a five-station railway. Each knows half of the facts and keeps private,
//! structured notes ([`plan::Plan`]) on the whole plan. When both press
//! *Ready*, the game merges the notes ([`sim::merge`]), simulates the day
//! ([`sim::simulate`]) and returns three traces: what actually happened and
//! what each player's own notes predicted. The browser plays the actual trace
//! as a film with a "ghost" of the player's own expectation.
//!
//! This module is the [`Game`] implementation. It owns the phase machine
//!
//! ```text
//! Planning --both ready--> Replaying --both finished--> Planning (same act)
//!                                                       Planning (act 2, after a disruption)
//!                                                       ScenarioDone --both continue--> next scenario / Finished
//! ```
//!
//! the validation of actions, the per-role views with their information
//! boundary, scoring, analysis metadata and checkpoint recovery. The rules of
//! the railway live in [`world`], [`plan`], [`sim`] and [`disruption`];
//! scenario data in [`scenario`].
//!
//! The information boundary is a contract of this module: a role's view
//! contains that role's notes and briefing only. The partner's notes and
//! briefing never appear. Facts revealed by the joint run itself (the shared
//! traces in `last_run`) are visible to both roles, as the film shows them.

pub mod disruption;
#[cfg(any(feature = "agent-component", test))]
pub mod partner;
pub mod plan;
pub mod scenario;
pub mod sim;
pub mod world;

use parlando_game_sdk::{Game, GameValue, Rejection, Role};
use serde::{Deserialize, Serialize};

use crate::disruption::{plan_disruption, Announcement, Planned};
use crate::plan::{agreement, apply_edit, EditContext, NoteEdit, Plan};
use crate::scenario::{builtin, Scenario, BUILTIN_IDS};
use crate::sim::{
    divergence, itinerary_to_route, merge, simulate, simulate_cut, Divergence, Event, EventKind, OrderOutcome, Trace,
    Unreached,
};
use crate::world::{Car, Commodity, Edge, EngineId, OrderState, Restriction, StationId, World};

/// Default number of joint runs per scenario.
pub const DEFAULT_MAX_RUNS: u8 = 6;

/// Which of the two players a view or note belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, GameValue)]
#[serde(rename_all = "snake_case")]
pub enum Seat {
    /// Knows orders, cars, stock and handling durations.
    Freight,
    /// Knows travel times, track restrictions, engine capacity and availability.
    Routing,
}

impl Seat {
    /// Array index: Freight is 0, Routing is 1.
    fn index(self) -> usize {
        match self {
            Seat::Freight => 0,
            Seat::Routing => 1,
        }
    }

    /// The other seat.
    fn partner(self) -> Seat {
        match self {
            Seat::Freight => Seat::Routing,
            Seat::Routing => Seat::Freight,
        }
    }
}

/// Experiment parameters. The empty configuration `{}` plays the three
/// built-in scenarios with Freight in seat A.
#[derive(GameValue)]
pub struct Config {
    /// Scenario ids to play in order. Defaults to `tutorial`, `corning-juice`,
    /// `bath-bananas`.
    pub scenarios: Option<Vec<String>>,
    /// `"a"` (default) or `"b"`: the Parlando seat that plays Freight.
    pub freight_seat: Option<String>,
    /// Joint runs allowed per scenario, both acts together. Default 6.
    pub max_runs: Option<u8>,
}

/// One participant action.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, GameValue)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    /// Edit the actor's own notes. Only in `planning`. Withdraws readiness.
    Edit {
        /// The edit.
        edit: NoteEdit,
    },
    /// Declare readiness to run (or withdraw it). Only in `planning`. When
    /// both players are ready the joint run starts.
    SetReady {
        /// The new readiness.
        ready: bool,
    },
    /// The actor has watched the replay. Only in `replaying`, once per role.
    ReplayFinished,
    /// The actor has seen the scenario result. Only in `scenario_done`, once
    /// per role.
    Continue,
}

/// The phase of a session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, GameValue)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// Players talk and edit notes.
    Planning,
    /// The joint run is being played back.
    Replaying,
    /// The scenario is over; its score is final.
    ScenarioDone,
    /// All scenarios are done.
    Finished,
}

/// A readiness flag for the viewer and the partner.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, GameValue)]
pub struct Pair {
    /// The viewer's flag.
    pub me: bool,
    /// The partner's flag.
    pub partner: bool,
}

/// A station on the shared map.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, GameValue)]
pub struct MapStation {
    /// Station id.
    pub id: StationId,
    /// Display name.
    pub name: String,
    /// Whether the station has a factory (what it makes is Freight's fact).
    pub factory: bool,
}

/// An engine on the shared map.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, GameValue)]
pub struct MapEngine {
    /// Engine id.
    pub id: EngineId,
    /// The station it stands in now.
    pub station: StationId,
    /// Whether it has failed (the film showed it).
    pub failed: bool,
    /// The hour from which it can operate. Present only in Routing's view.
    pub available_from: Option<u8>,
}

/// A track connection on the shared map (travel times are Routing's fact).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, GameValue)]
pub struct MapEdge {
    /// One end.
    pub a: StationId,
    /// The other end.
    pub b: StationId,
}

/// The map both players see.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, GameValue)]
pub struct MapView {
    /// Stations.
    pub stations: Vec<MapStation>,
    /// Edges without travel times.
    pub edges: Vec<MapEdge>,
    /// Engines at the current hour.
    pub engines: Vec<MapEngine>,
    /// Last hour of the day.
    pub horizon: u8,
    /// The hour the current act starts (later than the scenario start in act 2).
    pub start_hour: u8,
}

/// Where a commodity is stocked.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, GameValue)]
pub struct Stock {
    /// The station.
    pub station: StationId,
    /// Commodities available there without limit.
    pub commodities: Vec<Commodity>,
}

/// A factory and what it does.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, GameValue)]
pub struct FactoryInfo {
    /// The station.
    pub station: StationId,
    /// Commodity consumed.
    pub input: Commodity,
    /// Commodity produced.
    pub output: Commodity,
    /// Hours per `process`.
    pub hours: u8,
}

/// An engine's capacity, as Routing knows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, GameValue)]
pub struct Capacity {
    /// Engine id.
    pub engine: EngineId,
    /// Cars it can pull.
    pub capacity: u8,
}

/// A role's private facts.
#[derive(Clone, Debug, PartialEq, Serialize, GameValue)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Briefing {
    /// What Freight knows.
    Freight {
        /// Orders with their delivery progress (includes a late order once announced).
        orders: Vec<OrderState>,
        /// Every car with its position (yard or coupled to an engine).
        cars: Vec<Car>,
        /// Stock at each station that has any.
        stock: Vec<Stock>,
        /// Factories and what they make.
        factories: Vec<FactoryInfo>,
        /// Hours a `load` takes.
        load_hours: u8,
        /// Hours a `deliver` takes.
        deliver_hours: u8,
        /// The details of a disruption addressed to Freight (act 2 only).
        announcement: Option<Announcement>,
    },
    /// What Routing knows.
    Routing {
        /// Travel time of each edge in hours.
        travel_times: Vec<Edge>,
        /// Track restriction windows in force.
        restrictions: Vec<Restriction>,
        /// Engine capacities.
        capacities: Vec<Capacity>,
        /// The details of a disruption addressed to Routing (act 2 only).
        announcement: Option<Announcement>,
    },
}

/// The outcome of the latest joint run as one role sees it.
#[derive(Clone, Debug, PartialEq, Serialize, GameValue)]
pub struct LastRun {
    /// 1-based number of the run within the scenario.
    pub number: u8,
    /// The act (1 or 2) the run belonged to.
    pub act: u8,
    /// Whether every order was met in the run as shown. False for an
    /// interrupted run.
    pub success: bool,
    /// The disruption hour when the run was cut short.
    pub interrupted_at: Option<u8>,
    /// What really happened (merged plan), cut at `interrupted_at`.
    pub actual: Trace,
    /// What the viewer's own notes predicted (the ghost), likewise cut.
    pub expected: Trace,
    /// The earliest difference between `actual` and `expected`.
    pub divergence: Option<Divergence>,
    /// Order standing at the end of `actual`.
    pub orders: Vec<OrderOutcome>,
}

/// The result of one finished scenario.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, GameValue)]
pub struct ScenarioScore {
    /// Scenario id.
    pub scenario: String,
    /// Orders met in the last run.
    pub orders_met: u8,
    /// Orders in the last run.
    pub orders_total: u8,
    /// Joint runs used, both acts together.
    pub runs_used: u8,
    /// Whether a run succeeded after a disruption.
    pub replanned: bool,
}

/// Everything one role perceives.
#[derive(Clone, Debug, PartialEq, Serialize, GameValue)]
pub struct View {
    /// The viewer's role.
    pub role: Seat,
    /// The phase.
    pub phase: Phase,
    /// The act of the current scenario: 1, or 2 after a disruption.
    pub act: u8,
    /// 0-based index of the current scenario.
    pub scenario_index: usize,
    /// Number of scenarios in the session.
    pub scenario_count: usize,
    /// Current scenario id.
    pub scenario_id: String,
    /// Current scenario title.
    pub scenario_title: String,
    /// Current scenario one-line summary.
    pub scenario_summary: String,
    /// Joint runs used in the current scenario.
    pub runs_used: u8,
    /// Joint runs allowed per scenario.
    pub max_runs: u8,
    /// The shared map.
    pub map: MapView,
    /// The viewer's private facts.
    pub briefing: Briefing,
    /// The viewer's own notes.
    pub notes: Plan,
    /// Readiness to run.
    pub ready: Pair,
    /// Whether each role has finished watching the replay.
    pub replay_finished: Pair,
    /// Whether each role has pressed continue after the scenario ended.
    #[serde(rename = "continue")]
    pub r#continue: Pair,
    /// The latest joint run, present from the first run on.
    pub last_run: Option<LastRun>,
    /// Scores of finished scenarios.
    pub scores: Vec<ScenarioScore>,
}

/// The shared completion: the scores of all scenarios.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, GameValue)]
pub struct Completion {
    /// One entry per scenario.
    pub scores: Vec<ScenarioScore>,
}

/// A condensed result of one trace, for analysis.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, GameValue)]
pub struct TraceOutcome {
    /// Whether every order was met.
    pub success: bool,
    /// Orders met.
    pub orders_met: u8,
    /// Orders in the world.
    pub orders_total: u8,
    /// Last simulated hour.
    pub end_hour: u8,
}

impl TraceOutcome {
    /// Summarizes a trace.
    fn of(trace: &Trace) -> Self {
        Self {
            success: trace.success,
            orders_met: trace.orders.iter().filter(|o| o.met).count() as u8,
            orders_total: trace.orders.len() as u8,
            end_hour: trace.end_hour,
        }
    }
}

/// Transition metadata for researchers: never shown to participants.
///
/// The `Run` variant is large by design (it carries both full notes); the
/// value is built once per run, so boxing it would only complicate the wire
/// schema.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Serialize, GameValue)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Analysis {
    /// An accepted note edit and how far the two notes now agree (0 to 1,
    /// fraction of stops and ops identical by visit key).
    Edit {
        /// Who edited.
        actor: Seat,
        /// The agreement measure after the edit.
        agreement: f64,
    },
    /// A joint run with the full notes of both roles.
    Run {
        /// 1-based run number in the scenario.
        number: u8,
        /// The act.
        act: u8,
        /// Freight's notes at the run.
        freight_notes: Plan,
        /// Routing's notes at the run.
        routing_notes: Plan,
        /// The merged plan that ran.
        merged: Plan,
        /// Freight ops without a visit in Routing's route.
        unreached: Vec<Unreached>,
        /// Outcome of the actual (full, uncut) run.
        actual: TraceOutcome,
        /// Outcome of Freight's expectation (full, uncut).
        expected_freight: TraceOutcome,
        /// Outcome of Routing's expectation (full, uncut).
        expected_routing: TraceOutcome,
        /// Where actual and Freight's expectation first differ.
        divergence_freight: Option<Divergence>,
        /// Where actual and Routing's expectation first differ.
        divergence_routing: Option<Divergence>,
        /// Agreement of the two notes at the time of the run.
        agreement: f64,
        /// The disruption hour, if the run was cut.
        interrupted_at: Option<u8>,
        /// The chosen disruption candidate id, if any.
        disruption: Option<String>,
    },
}

/// The stored result of the latest run.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct RunRecord {
    number: u8,
    act: u8,
    success: bool,
    interrupted_at: Option<u8>,
    actual: Trace,
    expected: [Trace; 2],
    divergence: [Option<Divergence>; 2],
}

/// The act-two state prepared when a run is interrupted.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Pending {
    world: World,
    remainder: Plan,
    announcements: [Option<Announcement>; 2],
}

/// The authoritative, replayable state of a session.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct State {
    scenario_index: usize,
    phase: Phase,
    act: u8,
    world: World,
    notes: [Plan; 2],
    ready: [bool; 2],
    replay_finished: [bool; 2],
    continued: [bool; 2],
    runs_used: u8,
    last_run: Option<RunRecord>,
    pending: Option<Pending>,
    announcements: [Option<Announcement>; 2],
    scores: Vec<ScenarioScore>,
}

/// What a checkpoint stores: the session setup, seed and state.
#[derive(Serialize, Deserialize)]
struct Saved {
    scenarios: Vec<String>,
    freight_is_a: bool,
    max_runs: u8,
    seed: u64,
    state: State,
}

/// One session of Night Freight.
pub struct NightFreight {
    scenario_ids: Vec<String>,
    scenarios: Vec<Scenario>,
    freight_is_a: bool,
    max_runs: u8,
    seed: u64,
    state: State,
    analysis: Option<Analysis>,
}

/// Builds the state at the start of a scenario.
fn fresh_state(index: usize, scenario: &Scenario, scores: Vec<ScenarioScore>) -> State {
    let plan = scenario.initial_plan();
    State {
        scenario_index: index,
        phase: Phase::Planning,
        act: 1,
        world: scenario.world(),
        notes: [plan.clone(), plan],
        ready: [false; 2],
        replay_finished: [false; 2],
        continued: [false; 2],
        runs_used: 0,
        last_run: None,
        pending: None,
        announcements: [None, None],
        scores,
    }
}

impl NightFreight {
    /// The seat an actor plays.
    fn seat(&self, actor: Role) -> Seat {
        if (actor == Role::A) == self.freight_is_a {
            Seat::Freight
        } else {
            Seat::Routing
        }
    }

    /// The current scenario.
    fn scenario(&self) -> &Scenario {
        &self.scenarios[self.state.scenario_index]
    }

    /// Computes the joint run, chooses a disruption if one is due, and enters
    /// the replay.
    fn start_run(&mut self) {
        let scenario = self.scenario().clone();
        let world = self.state.world.clone();
        let freight = self.state.notes[Seat::Freight.index()].clone();
        let routing = self.state.notes[Seat::Routing.index()].clone();
        let merged = merge(&routing, &freight);
        let full = simulate(&world, &merged.plan, &merged.unreached);
        let freight_route = itinerary_to_route(&world.network, &freight);
        let full_freight = simulate(&world, &freight_route, &[]);
        let full_routing = simulate(&world, &routing, &[]);
        self.state.runs_used += 1;
        let number = self.state.runs_used;
        let planned: Option<Planned> =
            if self.state.act == 1 && full.success && number < self.max_runs {
                plan_disruption(
                    &world,
                    &merged,
                    full.end_hour,
                    &scenario.disruptions,
                    self.seed,
                    self.state.scenario_index as u64,
                )
            } else {
                None
            };
        let (actual, expected_freight, expected_routing) = match &planned {
            Some(p) => {
                let mut actual = simulate_cut(&world, &merged.plan, &merged.unreached, p.hour).0;
                actual.events.push(Event {
                    hour: p.hour,
                    kind: EventKind::Disrupted {
                        what: p.notice.clone(),
                    },
                });
                actual.success = false;
                (
                    actual,
                    simulate_cut(&world, &freight_route, &[], p.hour).0,
                    simulate_cut(&world, &routing, &[], p.hour).0,
                )
            }
            None => (full.clone(), full_freight.clone(), full_routing.clone()),
        };
        let shown = [
            divergence(&actual, &expected_freight),
            divergence(&actual, &expected_routing),
        ];
        self.analysis = Some(Analysis::Run {
            number,
            act: self.state.act,
            freight_notes: freight.clone(),
            routing_notes: routing.clone(),
            merged: merged.plan.clone(),
            unreached: merged.unreached.clone(),
            actual: TraceOutcome::of(&full),
            expected_freight: TraceOutcome::of(&full_freight),
            expected_routing: TraceOutcome::of(&full_routing),
            divergence_freight: divergence(&full, &full_freight),
            divergence_routing: divergence(&full, &full_routing),
            agreement: agreement(&freight, &routing),
            interrupted_at: planned.as_ref().map(|p| p.hour),
            disruption: planned.as_ref().map(|p| p.candidate.clone()),
        });
        let interrupted_at = planned.as_ref().map(|p| p.hour);
        self.state.pending = planned.map(|p| Pending {
            world: p.world,
            remainder: p.remainder,
            announcements: [p.freight, p.routing],
        });
        self.state.last_run = Some(RunRecord {
            number,
            act: self.state.act,
            success: interrupted_at.is_none() && actual.success,
            interrupted_at,
            actual,
            expected: [expected_freight, expected_routing],
            divergence: shown,
        });
        self.state.phase = Phase::Replaying;
        self.state.replay_finished = [false; 2];
    }

    /// Leaves the replay: replans after a disruption, ends the scenario, or
    /// returns to planning.
    fn finish_replay(&mut self) {
        self.state.ready = [false; 2];
        self.state.replay_finished = [false; 2];
        let run = self.state.last_run.clone().expect("a replay has a run");
        if run.interrupted_at.is_some() {
            let pending = self
                .state
                .pending
                .take()
                .expect("an interrupted run has act two");
            self.state.world = pending.world;
            self.state.notes = [pending.remainder.clone(), pending.remainder];
            self.state.announcements = pending.announcements;
            self.state.act = 2;
            self.state.phase = Phase::Planning;
        } else if run.success || self.state.runs_used >= self.max_runs {
            let met = run.actual.orders.iter().filter(|o| o.met).count() as u8;
            self.state.scores.push(ScenarioScore {
                scenario: self.scenario().id.clone(),
                orders_met: met,
                orders_total: run.actual.orders.len() as u8,
                runs_used: self.state.runs_used,
                replanned: run.success && run.act == 2,
            });
            self.state.phase = Phase::ScenarioDone;
            self.state.continued = [false; 2];
        } else {
            self.state.phase = Phase::Planning;
        }
    }

    /// Starts the next scenario or finishes the session.
    fn advance_scenario(&mut self) {
        let next = self.state.scenario_index + 1;
        if next < self.scenarios.len() {
            let scores = std::mem::take(&mut self.state.scores);
            self.state = fresh_state(next, &self.scenarios[next], scores);
        } else {
            self.state.phase = Phase::Finished;
        }
    }

    /// The edit validation context for an actor.
    fn edit_context(&self, seat: Seat) -> EditContext<'_> {
        EditContext {
            adjacent_only: seat == Seat::Routing,
            network: &self.state.world.network,
            engines: self
                .state
                .world
                .state
                .engines
                .iter()
                .map(|e| e.id.as_str())
                .collect(),
            orders: (seat == Seat::Freight).then(|| {
                self.state
                    .world
                    .state
                    .orders
                    .iter()
                    .map(|o| o.id.as_str())
                    .collect()
            }),
        }
    }

    /// Projects the state for one seat.
    fn view(&self, seat: Seat) -> View {
        let s = &self.state;
        let world = &s.world;
        let scenario = self.scenario();
        let me = seat.index();
        let other = seat.partner().index();
        let pair = |flags: &[bool; 2]| Pair {
            me: flags[me],
            partner: flags[other],
        };
        let map = MapView {
            stations: world
                .network
                .stations
                .iter()
                .map(|st| MapStation {
                    id: st.id.clone(),
                    name: st.name.clone(),
                    factory: st.factory.is_some(),
                })
                .collect(),
            edges: world
                .network
                .edges
                .iter()
                .map(|e| MapEdge {
                    a: e.a.clone(),
                    b: e.b.clone(),
                })
                .collect(),
            engines: world
                .state
                .engines
                .iter()
                .map(|e| MapEngine {
                    id: e.id.clone(),
                    station: e.station.clone(),
                    failed: e.failed,
                    available_from: (seat == Seat::Routing).then_some(e.available_from),
                })
                .collect(),
            horizon: world.network.horizon,
            start_hour: world.state.hour,
        };
        let briefing = match seat {
            Seat::Freight => Briefing::Freight {
                orders: world.state.orders.clone(),
                cars: world.state.cars.clone(),
                stock: world
                    .network
                    .stations
                    .iter()
                    .filter(|st| !st.stock.is_empty())
                    .map(|st| Stock {
                        station: st.id.clone(),
                        commodities: st.stock.clone(),
                    })
                    .collect(),
                factories: world
                    .network
                    .stations
                    .iter()
                    .filter_map(|st| {
                        st.factory.map(|f| FactoryInfo {
                            station: st.id.clone(),
                            input: f.input,
                            output: f.output,
                            hours: f.hours,
                        })
                    })
                    .collect(),
                load_hours: world.network.load_hours,
                deliver_hours: world.network.deliver_hours,
                announcement: s.announcements[0].clone(),
            },
            Seat::Routing => Briefing::Routing {
                travel_times: world.network.edges.clone(),
                restrictions: world.restrictions.clone(),
                capacities: world
                    .state
                    .engines
                    .iter()
                    .map(|e| Capacity {
                        engine: e.id.clone(),
                        capacity: e.capacity,
                    })
                    .collect(),
                announcement: s.announcements[1].clone(),
            },
        };
        View {
            role: seat,
            phase: s.phase,
            act: s.act,
            scenario_index: s.scenario_index,
            scenario_count: self.scenarios.len(),
            scenario_id: scenario.id.clone(),
            scenario_title: scenario.title.clone(),
            scenario_summary: scenario.summary.clone(),
            runs_used: s.runs_used,
            max_runs: self.max_runs,
            map,
            briefing,
            notes: s.notes[me].clone(),
            ready: pair(&s.ready),
            replay_finished: pair(&s.replay_finished),
            r#continue: pair(&s.continued),
            last_run: s.last_run.as_ref().map(|r| LastRun {
                number: r.number,
                act: r.act,
                success: r.success,
                interrupted_at: r.interrupted_at,
                actual: r.actual.clone(),
                expected: r.expected[me].clone(),
                divergence: r.divergence[me].clone(),
                orders: r.actual.orders.clone(),
            }),
            scores: s.scores.clone(),
        }
    }
}

impl Game for NightFreight {
    type Config = Config;
    type Action = Action;
    type ViewA<'a> = View;
    type ViewB<'a> = View;
    type Completion<'a> = Completion;
    type Metadata<'a> = &'a Analysis;

    /// Loads the configured scenarios and starts the first one in `planning`.
    fn init(config: Config, seed: u64) -> Result<Self, String> {
        let ids = config
            .scenarios
            .unwrap_or_else(|| BUILTIN_IDS.iter().map(|s| s.to_string()).collect());
        if ids.is_empty() {
            return Err("scenarios must name at least one scenario".into());
        }
        let freight_is_a = match config.freight_seat.as_deref().map(str::to_lowercase) {
            None => true,
            Some(s) if s == "a" => true,
            Some(s) if s == "b" => false,
            Some(other) => {
                return Err(format!(
                    "freight_seat must be \"a\" or \"b\", not {other:?}"
                ))
            }
        };
        let max_runs = config.max_runs.unwrap_or(DEFAULT_MAX_RUNS);
        if max_runs == 0 {
            return Err("max_runs must be at least 1".into());
        }
        let scenarios = ids
            .iter()
            .map(|id| builtin(id))
            .collect::<Result<Vec<_>, _>>()?;
        let state = fresh_state(0, &scenarios[0], vec![]);
        Ok(Self {
            scenario_ids: ids,
            scenarios,
            freight_is_a,
            max_runs,
            seed,
            state,
            analysis: None,
        })
    }

    /// Validates the action for the current phase, then applies it. Rejected
    /// actions change nothing.
    fn step(&mut self, actor: Role, action: Action) -> Result<(), Rejection> {
        let seat = self.seat(actor);
        let me = seat.index();
        let wrong_phase = || Rejection::new("wrong_phase");
        match action {
            Action::Edit { edit } => {
                if self.state.phase != Phase::Planning {
                    return Err(wrong_phase());
                }
                let mut notes = self.state.notes[me].clone();
                apply_edit(&mut notes, &edit, &self.edit_context(seat)).map_err(Rejection::new)?;
                self.state.notes[me] = notes;
                self.state.ready[me] = false;
                self.analysis = Some(Analysis::Edit {
                    actor: seat,
                    agreement: agreement(&self.state.notes[0], &self.state.notes[1]),
                });
            }
            Action::SetReady { ready } => {
                if self.state.phase != Phase::Planning {
                    return Err(wrong_phase());
                }
                if self.state.ready[me] == ready {
                    return Err(Rejection::new("no_change"));
                }
                self.state.ready[me] = ready;
                self.analysis = None;
                if self.state.ready == [true; 2] {
                    self.start_run();
                }
            }
            Action::ReplayFinished => {
                if self.state.phase != Phase::Replaying {
                    return Err(wrong_phase());
                }
                if self.state.replay_finished[me] {
                    return Err(Rejection::new("no_change"));
                }
                self.state.replay_finished[me] = true;
                self.analysis = None;
                if self.state.replay_finished == [true; 2] {
                    self.finish_replay();
                }
            }
            Action::Continue => {
                if self.state.phase != Phase::ScenarioDone {
                    return Err(wrong_phase());
                }
                if self.state.continued[me] {
                    return Err(Rejection::new("no_change"));
                }
                self.state.continued[me] = true;
                self.analysis = None;
                if self.state.continued == [true; 2] {
                    self.advance_scenario();
                }
            }
        }
        Ok(())
    }

    /// Projects Freight's or Routing's view for seat A.
    fn view_a(&self) -> View {
        self.view(self.seat(Role::A))
    }

    /// Projects Freight's or Routing's view for seat B.
    fn view_b(&self) -> View {
        self.view(self.seat(Role::B))
    }

    /// The scores of all scenarios once the last one has been continued past.
    fn completion(&self) -> Option<Completion> {
        (self.state.phase == Phase::Finished).then(|| Completion {
            scores: self.state.scores.clone(),
        })
    }

    /// Edit agreement or run analysis of the latest accepted action.
    fn metadata(&self) -> Option<&Analysis> {
        self.analysis.as_ref()
    }

    /// Serializes the complete state, including the seed that selects disruptions.
    fn checkpoint(&self) -> Option<Vec<u8>> {
        let saved = Saved {
            scenarios: self.scenario_ids.clone(),
            freight_is_a: self.freight_is_a,
            max_runs: self.max_runs,
            seed: self.seed,
            state: self.state.clone(),
        };
        Some(serde_json::to_vec(&saved).expect("checkpoint serializes"))
    }

    /// Rebuilds a session from a checkpoint. Metadata restarts empty.
    fn restore(bytes: &[u8]) -> Result<Self, String> {
        let saved: Saved = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        let scenarios = saved
            .scenarios
            .iter()
            .map(|id| builtin(id))
            .collect::<Result<Vec<_>, _>>()?;
        if saved.state.scenario_index >= scenarios.len() {
            return Err("checkpoint scenario index out of range".into());
        }
        Ok(Self {
            scenario_ids: saved.scenarios,
            scenarios,
            freight_is_a: saved.freight_is_a,
            max_runs: saved.max_runs,
            seed: saved.seed,
            state: saved.state,
            analysis: None,
        })
    }
}

// One crate builds two components: the game by default, and the scripted
// partner policy with the `agent-component` feature (see `../agent.yaml`).
#[cfg(not(all(feature = "agent-component", target_arch = "wasm32")))]
parlando_game_sdk::export!(NightFreight);

#[cfg(all(feature = "agent-component", target_arch = "wasm32"))]
parlando_game_sdk::agent::export_agent!(partner::Partner);
