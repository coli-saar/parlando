//! Merge and simulation: the pure function from a world and a plan to a trace.
//!
//! This unit owns three things.
//!
//! * [`merge`] builds the joint plan from the two players' notes. Routing's
//!   stops, without ops, form the route; Freight's ops are attached by visit
//!   key `(engine, station, n)`. Ops whose visit does not exist are reported as
//!   [`Unreached`] and never execute.
//! * [`simulate`] and [`simulate_cut`] advance the world hour by hour with the
//!   fixed step order: arrivals, completions, ops, departures, signals. They
//!   are deterministic: ties go to the lower engine id and the lowest car id.
//! * [`divergence`] compares two traces.
//!
//! The simulator never mutates its inputs. A caller may rely on every
//! [`Trace`] having events in chronological order, `end_hour <= horizon` or
//! the latest deadline, and `orders` describing the state at the last
//! simulated moment. [`simulate_cut`] additionally returns the world and the
//! unexecuted remainder of the plan at the cut, which is the basis of the
//! act-two state in [`crate::disruption`].
//!
//! Semantics fixed here where the design leaves room:
//!
//! * Departure comes last. An engine never leaves a stop while one of its ops
//!   there is in progress, and never while an op there could still start.
//!   Once its depart condition holds it finishes the op in progress, starts
//!   every remaining op that is possible, and only then leaves; ops still
//!   unexecuted at that moment are reported as [`EventKind::OpAbandoned`] with
//!   the [`FailReason`] of each. Before the condition holds, impossible ops
//!   are retried every hour, so hand-overs that wait for another engine work.
//! * Ops do not run in list order. An engine runs at most one op at a time
//!   and, whenever it is free, starts the first op of its list whose
//!   precondition holds now; list order only breaks ties.
//! * An engine that wants to depart but is blocked by a signal or a closed
//!   edge stays in the station and keeps executing ops. Unexecuted ops are
//!   abandoned only when the engine actually enters the edge.
//! * Every missed order carries a [`MissReason`] naming what came closest to
//!   delivering it.
//! * An edge is closed for an engine entering it at hour `t` when a
//!   restriction window covers `t`. Trains already on the edge are not
//!   stopped.
//! * `Hold` is emitted at the first hour of a blockage and again when its
//!   reason changes, not on every retry.
//! * `Deliver` delivers as many matching loaded cars as the order still
//!   needs (at least one is required), so an order can be served by several
//!   engines.

use std::collections::BTreeMap;

use parlando_game_sdk::GameValue;
use serde::{Deserialize, Serialize};

use crate::disruption::DisruptionNotice;
use crate::plan::{Depart, LoadFilter, Op, Plan, Stop};
use crate::world::{
    CarAt, CarId, CarKind, Commodity, EngineId, EngineState, Network, OrderId, OrderState, StationId, World,
    WorldSnapshot,
};

/// Why a departing engine is held in its station.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, GameValue)]
#[serde(rename_all = "snake_case")]
pub enum HoldReason {
    /// The edge is occupied by another train.
    Signal,
    /// The edge is closed by a restriction window.
    Closed,
}

/// A number of cars sharing one load, used to describe what a yard or an
/// engine actually holds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, GameValue)]
pub struct CarCount {
    /// The load of these cars, or `None` for empty cars.
    pub load: Option<Commodity>,
    /// How many cars.
    pub count: u8,
}

/// Why an op could not be started. Every variant is a failed precondition
/// of the op, stated with the numbers a client needs to explain it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, GameValue)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FailReason {
    /// `Couple`: the yard holds fewer matching cars than wanted.
    NoMatchingCarsInYard {
        /// Car kind asked for.
        kind: CarKind,
        /// Load filter asked for.
        load: LoadFilter,
        /// Cars wanted.
        wanted: u8,
        /// Matching cars in the yard.
        found: u8,
        /// The yard's cars of this kind, grouped by load.
        yard: Vec<CarCount>,
    },
    /// `Couple`: the cars exist but would exceed the engine's capacity.
    OverCapacity {
        /// Cars wanted.
        wanted: u8,
        /// Cars already coupled.
        carried: u8,
        /// The engine's capacity.
        capacity: u8,
    },
    /// `Uncouple`: the engine carries fewer matching cars than wanted.
    NoMatchingCarsOnEngine {
        /// Car kind asked for.
        kind: CarKind,
        /// Load filter asked for.
        load: LoadFilter,
        /// Cars wanted.
        wanted: u8,
        /// Matching cars on the engine.
        found: u8,
        /// The engine's cars of this kind, grouped by load.
        coupled: Vec<CarCount>,
    },
    /// `Load`: the station does not stock the commodity.
    NoStockHere {
        /// The commodity asked for.
        commodity: Commodity,
    },
    /// `Load` or `Process`: the engine carries too few empty cars of the kind
    /// the op needs.
    NoEmptyCarsOnEngine {
        /// The kind of empty car needed.
        kind: CarKind,
        /// Cars wanted.
        wanted: u8,
        /// Empty cars of that kind on the engine.
        found: u8,
    },
    /// `Process`: the station has no factory.
    NoFactoryHere,
    /// `Process`: the engine carries too few cars with the factory's input.
    NoInputCars {
        /// The input commodity.
        load: Commodity,
        /// Cars wanted.
        wanted: u8,
        /// Cars with that load on the engine.
        found: u8,
    },
    /// `Deliver`: the order is destined for another station.
    WrongDestination {
        /// The order.
        order: OrderId,
        /// Where the order must be delivered.
        to: StationId,
    },
    /// `Deliver`: the engine carries no car loaded with the order's commodity.
    NoOrderLoadsOnEngine {
        /// The order.
        order: OrderId,
        /// The commodity the order wants.
        commodity: Commodity,
        /// What the engine carries instead, grouped by load.
        coupled: Vec<CarCount>,
    },
    /// `Deliver`: the order is already delivered completely.
    OrderFilled {
        /// The order.
        order: OrderId,
    },
    /// `Deliver`: the order does not exist in this world.
    UnknownOrder {
        /// The order id.
        order: OrderId,
    },
}

/// What came closest to delivering an order that was missed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, GameValue)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MissReason {
    /// No car loaded with the commodity ever stood at the destination.
    NoCarReached {
        /// The commodity ordered.
        commodity: Commodity,
        /// The destination.
        to: StationId,
    },
    /// A loaded car first reached the destination after the deadline.
    ReachedAfterDeadline {
        /// The commodity ordered.
        commodity: Commodity,
        /// The destination.
        to: StationId,
        /// The first hour a loaded car stood there.
        reached_at: u8,
        /// The deadline hour.
        deadline: u8,
    },
    /// A loaded car reached the destination in time but the order was never
    /// completed by a `Deliver`.
    ReachedNotDelivered {
        /// The commodity ordered.
        commodity: Commodity,
        /// The destination.
        to: StationId,
        /// The first hour a loaded car stood there.
        reached_at: u8,
        /// Car loads delivered so far.
        delivered: u8,
        /// Car loads ordered.
        count: u8,
    },
    /// The order was completed, but after the deadline.
    DeliveredAfterDeadline {
        /// The commodity ordered.
        commodity: Commodity,
        /// The destination.
        to: StationId,
        /// The hour the last car load was delivered.
        delivered_at: u8,
        /// The deadline hour.
        deadline: u8,
    },
}

/// What happened.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, GameValue)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventKind {
    /// An engine arrives at its next stop.
    Arrive {
        /// The engine.
        engine: EngineId,
        /// The station reached.
        station: StationId,
        /// Index of the stop in the engine's strip.
        stop: usize,
    },
    /// An engine enters an edge.
    Depart {
        /// The engine.
        engine: EngineId,
        /// The station left.
        from: StationId,
        /// The station it is heading to.
        to: StationId,
        /// Index of the stop being left.
        stop: usize,
        /// The hour of arrival at `to`.
        arrive_hour: u8,
    },
    /// An engine that wants to depart is held in its station.
    Hold {
        /// The engine.
        engine: EngineId,
        /// The station.
        station: StationId,
        /// Why.
        reason: HoldReason,
    },
    /// An op with a nonzero duration begins. The op takes effect, with an
    /// `OpDone`, at `finish_hour`.
    OpStarted {
        /// The engine.
        engine: EngineId,
        /// The station.
        station: StationId,
        /// Stop index.
        stop: usize,
        /// Position in the stop's op list.
        index: usize,
        /// The op.
        op: Op,
        /// The hour the op takes effect.
        finish_hour: u8,
    },
    /// An op takes effect.
    OpDone {
        /// The engine.
        engine: EngineId,
        /// The station.
        station: StationId,
        /// Stop index.
        stop: usize,
        /// Position in the stop's op list.
        index: usize,
        /// The op.
        op: Op,
        /// The cars affected. For `Process`, the input cars followed by the
        /// output cars.
        cars: Vec<CarId>,
    },
    /// An op was never executed because its precondition failed until its
    /// engine left the stop (or, at an engine's last stop, until the end of the
    /// run).
    OpAbandoned {
        /// The engine.
        engine: EngineId,
        /// The station.
        station: StationId,
        /// Stop index.
        stop: usize,
        /// Position in the stop's op list.
        index: usize,
        /// The op.
        op: Op,
        /// The failed precondition.
        reason: FailReason,
    },
    /// An op planned for a visit that the route does not contain.
    OpUnreached {
        /// The engine.
        engine: EngineId,
        /// The station of the missing visit.
        station: StationId,
        /// The n-th visit of `station` (1-based).
        visit: usize,
        /// Position in the original op list.
        index: usize,
        /// The op.
        op: Op,
    },
    /// An order was delivered completely before or at its deadline.
    OrderMet {
        /// The order.
        order: OrderId,
    },
    /// An order was still undelivered at its deadline hour.
    OrderMissed {
        /// The order.
        order: OrderId,
        /// What came closest to delivering it.
        why: MissReason,
    },
    /// A disruption interrupts the day. Added only by the joint run.
    Disrupted {
        /// The public part of the disruption.
        what: DisruptionNotice,
    },
}

impl EventKind {
    /// The engine this event is about, if it is an engine event.
    pub fn engine(&self) -> Option<&str> {
        match self {
            EventKind::Arrive { engine, .. }
            | EventKind::Depart { engine, .. }
            | EventKind::Hold { engine, .. }
            | EventKind::OpStarted { engine, .. }
            | EventKind::OpDone { engine, .. }
            | EventKind::OpAbandoned { engine, .. }
            | EventKind::OpUnreached { engine, .. } => Some(engine),
            _ => None,
        }
    }
}

/// An event with the hour it happened.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, GameValue)]
pub struct Event {
    /// Hour of the day.
    pub hour: u8,
    /// What happened.
    pub kind: EventKind,
}

/// The final standing of one order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, GameValue)]
pub struct OrderOutcome {
    /// Order id.
    pub id: OrderId,
    /// Commodity.
    pub commodity: Commodity,
    /// Car loads ordered.
    pub count: u8,
    /// Destination.
    pub to: StationId,
    /// Deadline hour.
    pub deadline: u8,
    /// Car loads delivered.
    pub delivered: u8,
    /// Hour the last car load arrived.
    pub delivered_at: Option<u8>,
    /// Whether the order was delivered completely and on time.
    pub met: bool,
}

impl From<&OrderState> for OrderOutcome {
    fn from(o: &OrderState) -> Self {
        Self {
            id: o.id.clone(),
            commodity: o.commodity,
            count: o.count,
            to: o.to.clone(),
            deadline: o.deadline,
            delivered: o.delivered,
            delivered_at: o.delivered_at,
            met: o.met(),
        }
    }
}

/// The record of one simulated day (or part of it).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, GameValue)]
pub struct Trace {
    /// The world at the start hour. Enough to animate `events`.
    pub initial: WorldSnapshot,
    /// Events in chronological order.
    pub events: Vec<Event>,
    /// The last simulated hour (or the latest order deadline reported).
    pub end_hour: u8,
    /// The standing of every order at the end of the trace.
    pub orders: Vec<OrderOutcome>,
    /// Whether every order was met at the end of the trace.
    pub success: bool,
}

/// The first point at which two traces differ for some engine.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, GameValue)]
pub struct Divergence {
    /// The earlier hour of the two differing events.
    pub hour: u8,
    /// The engine whose events differ.
    pub engine: EngineId,
    /// The event in the actual trace, or `None` if it had no further event.
    pub actual: Option<Event>,
    /// The event in the expected trace, or `None` if it had no further event.
    pub expected: Option<Event>,
}

/// An op whose visit key does not exist in the route.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, GameValue)]
pub struct Unreached {
    /// The engine.
    pub engine: EngineId,
    /// The station of the missing visit.
    pub station: StationId,
    /// The n-th visit of the station (1-based).
    pub visit: usize,
    /// Position in the original op list.
    pub index: usize,
    /// The op.
    pub op: Op,
}

/// The result of [`merge`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Merged {
    /// Routing's route with Freight's ops attached.
    pub plan: Plan,
    /// Freight's ops that found no visit.
    pub unreached: Vec<Unreached>,
}

/// Merges the two notes into the plan that actually runs.
///
/// Routing's stations and departure conditions form the route; its ops and
/// annotations are dropped. Freight's ops are attached to the visit with the
/// same key; ops without a matching visit are returned as [`Unreached`].
pub fn merge(routing: &Plan, freight: &Plan) -> Merged {
    let mut plan = Plan::default();
    for (engine, stops) in &routing.engines {
        plan.engines.insert(
            engine.clone(),
            stops
                .iter()
                .map(|s| Stop {
                    station: s.station.clone(),
                    depart: s.depart.clone(),
                    ops: vec![],
                    heard_arrival: None,
                })
                .collect(),
        );
    }
    let mut unreached = Vec::new();
    for (engine, stops) in &freight.engines {
        let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
        for stop in stops {
            let n = seen.entry(stop.station.as_str()).or_insert(0);
            *n += 1;
            let target = plan.engines.get_mut(engine).and_then(|route| {
                route
                    .iter_mut()
                    .filter(|s| s.station == stop.station)
                    .nth(*n - 1)
            });
            match target {
                Some(slot) => slot.ops.extend(stop.ops.iter().cloned()),
                None => {
                    unreached.extend(stop.ops.iter().enumerate().map(|(index, op)| Unreached {
                        engine: engine.clone(),
                        station: stop.station.clone(),
                        visit: *n,
                        index,
                        op: op.clone(),
                    }))
                }
            }
        }
    }
    Merged { plan, unreached }
}

/// The world and plan remainder at a cut hour.
#[derive(Clone, Debug)]
pub struct Cut {
    /// The world at the cut hour: in-transit engines placed at their
    /// destination (available from their arrival hour), running ops completed
    /// (their engine available from the completion hour).
    pub world: World,
    /// For each engine, its current stop followed by the stops not yet
    /// reached, with the ops not yet executed.
    pub remainder: Plan,
}

/// Normalization "itinerary to route", used only for Freight's expected trace.
///
/// Freight's notes list the places an engine works at, not the track it takes,
/// so consecutive stops need not be adjacent. This returns a plan the simulator
/// can run: between consecutive non-adjacent stops it inserts the stations of
/// the shortest path by travel time (Dijkstra, ties broken by station id), with
/// no ops at the inserted stops, and every stop departs as soon as its ops
/// allow (Freight's depart fields are ignored). Adjacent consecutive stops are
/// kept as they are, and unreachable pairs are left unconnected. The input is
/// not changed, and `merge` keeps using Freight's notes as written, so the
/// inserted stops never shift visit keys.
pub fn itinerary_to_route(network: &Network, itinerary: &Plan) -> Plan {
    let mut route = Plan::default();
    for (engine, stops) in &itinerary.engines {
        let mut out: Vec<Stop> = Vec::new();
        for stop in stops {
            if let Some(prev) = out.last().map(|s| s.station.clone()) {
                if prev != stop.station && network.edge(&prev, &stop.station).is_none() {
                    if let Some(path) = shortest_path(network, &prev, &stop.station) {
                        let inner = path.len().saturating_sub(1);
                        out.extend(path.iter().take(inner).skip(1).map(|s| Stop::at(s)));
                    }
                }
            }
            out.push(Stop {
                station: stop.station.clone(),
                depart: Depart::Immediately,
                ops: stop.ops.clone(),
                heard_arrival: stop.heard_arrival,
            });
        }
        route.engines.insert(engine.clone(), out);
    }
    route
}

/// The stations on the fastest path from `from` to `to`, both included, or
/// `None` if they are not connected.
fn shortest_path(network: &Network, from: &str, to: &str) -> Option<Vec<String>> {
    let mut dist: BTreeMap<&str, u32> = BTreeMap::new();
    let mut prev: BTreeMap<&str, &str> = BTreeMap::new();
    let mut done: Vec<&str> = Vec::new();
    dist.insert(from, 0);
    while let Some((&at, &d)) = dist
        .iter()
        .filter(|(s, _)| !done.contains(s))
        .min_by_key(|(s, d)| (**d, **s))
    {
        if at == to {
            break;
        }
        done.push(at);
        for e in &network.edges {
            let next = if e.a == at {
                e.b.as_str()
            } else if e.b == at {
                e.a.as_str()
            } else {
                continue;
            };
            let nd = d + u32::from(e.hours);
            if dist.get(next).is_none_or(|&old| nd < old) {
                dist.insert(next, nd);
                prev.insert(next, at);
            }
        }
    }
    dist.get(to)?;
    let mut path = vec![to.to_string()];
    let mut cur = to;
    while cur != from {
        cur = prev.get(cur)?;
        path.push(cur.to_string());
    }
    path.reverse();
    Some(path)
}

/// Runs the plan to its end and returns the trace.
pub fn simulate(world: &World, plan: &Plan, unreached: &[Unreached]) -> Trace {
    Sim::new(world, plan, unreached).run(None).0
}

/// Runs the plan until the cut hour (after that hour's arrivals and
/// completions) or to its end, whichever comes first.
///
/// The cut is `None` when the simulation ended before reaching the hour.
pub fn simulate_cut(
    world: &World,
    plan: &Plan,
    unreached: &[Unreached],
    hour: u8,
) -> (Trace, Option<Cut>) {
    Sim::new(world, plan, unreached).run(Some(hour))
}

/// Compares two traces engine by engine and reports the earliest difference.
pub fn divergence(actual: &Trace, expected: &Trace) -> Option<Divergence> {
    let per_engine = |trace: &Trace| {
        let mut map: BTreeMap<String, Vec<Event>> = BTreeMap::new();
        for e in &trace.events {
            if let Some(engine) = e.kind.engine() {
                map.entry(engine.to_string()).or_default().push(e.clone());
            }
        }
        map
    };
    let a = per_engine(actual);
    let e = per_engine(expected);
    let mut best: Option<Divergence> = None;
    let engines: std::collections::BTreeSet<&String> = a.keys().chain(e.keys()).collect();
    for engine in engines {
        let empty = Vec::new();
        let xs = a.get(engine).unwrap_or(&empty);
        let ys = e.get(engine).unwrap_or(&empty);
        for i in 0..xs.len().max(ys.len()) {
            let (x, y) = (xs.get(i), ys.get(i));
            if x != y {
                let hour = match (x, y) {
                    (Some(x), Some(y)) => x.hour.min(y.hour),
                    (Some(x), None) => x.hour,
                    (None, Some(y)) => y.hour,
                    (None, None) => unreachable!(),
                };
                if best.as_ref().is_none_or(|b| hour < b.hour) {
                    best = Some(Divergence {
                        hour,
                        engine: engine.clone(),
                        actual: x.cloned(),
                        expected: y.cloned(),
                    });
                }
                break;
            }
        }
    }
    best
}

/// An engine moving along an edge.
struct Motion {
    from: StationId,
    to: StationId,
    arrive: u8,
}

/// An op with a duration that has started.
struct Running {
    /// The hour the op takes effect.
    finish: u8,
    /// Position of the op in the stop's op list.
    index: usize,
    /// Cars the op acts on. For `Process`, inputs then outputs.
    cars: Vec<CarId>,
}

/// An engine's runtime state.
struct Run {
    id: EngineId,
    stops: Vec<Stop>,
    capacity: u8,
    station: StationId,
    stop: usize,
    arrived_at: u8,
    available_from: u8,
    failed: bool,
    motion: Option<Motion>,
    /// Positions, in list order, of the current stop's ops that were not yet
    /// started.
    pending: Vec<usize>,
    running: Option<Running>,
    hold: Option<HoldReason>,
}

/// An interval in which an engine stood in a station.
struct Visit {
    engine: EngineId,
    station: StationId,
    from: u8,
    to: Option<u8>,
}

/// The cars and duration of an op whose precondition holds.
struct Start {
    cars: Vec<CarId>,
    hours: u8,
}

/// The simulator state.
struct Sim<'a> {
    world: &'a World,
    cars: BTreeMap<CarId, crate::world::Car>,
    orders: Vec<OrderState>,
    engines: Vec<Run>,
    visits: Vec<Visit>,
    events: Vec<Event>,
    /// For each order, the first hour a car loaded with its commodity stood
    /// at its destination. Feeds [`MissReason`].
    reached: BTreeMap<OrderId, u8>,
}

/// Engine ids sorted for the deterministic processing order.
fn sorted_engines(world: &World) -> Vec<&EngineState> {
    let mut engines: Vec<&EngineState> = world.state.engines.iter().collect();
    engines.sort_by(|a, b| a.id.cmp(&b.id));
    engines
}

/// The positions of all ops of a stop, in list order.
fn all_ops(stop: &Stop) -> Vec<usize> {
    (0..stop.ops.len()).collect()
}

impl<'a> Sim<'a> {
    /// Builds the starting state and records unreached ops.
    fn new(world: &'a World, plan: &Plan, unreached: &[Unreached]) -> Self {
        let mut engines = Vec::new();
        let mut visits = Vec::new();
        for e in sorted_engines(world) {
            let stops = match plan.engines.get(&e.id) {
                Some(stops) if stops.first().is_some_and(|s| s.station == e.station) => {
                    stops.clone()
                }
                _ => vec![Stop::at(&e.station)],
            };
            visits.push(Visit {
                engine: e.id.clone(),
                station: e.station.clone(),
                from: e.arrived_at,
                to: None,
            });
            engines.push(Run {
                id: e.id.clone(),
                capacity: e.capacity,
                station: e.station.clone(),
                stop: 0,
                arrived_at: e.arrived_at,
                available_from: e.available_from,
                failed: e.failed,
                motion: None,
                pending: all_ops(&stops[0]),
                stops,
                running: None,
                hold: None,
            });
        }
        let start = world.state.hour;
        let events = unreached
            .iter()
            .map(|u| Event {
                hour: start,
                kind: EventKind::OpUnreached {
                    engine: u.engine.clone(),
                    station: u.station.clone(),
                    visit: u.visit,
                    index: u.index,
                    op: u.op.clone(),
                },
            })
            .collect();
        Self {
            world,
            cars: world.state.cars.iter().map(|c| (c.id, c.clone())).collect(),
            orders: world.state.orders.clone(),
            engines,
            visits,
            events,
            reached: BTreeMap::new(),
        }
    }

    /// Appends an event.
    fn emit(&mut self, hour: u8, kind: EventKind) {
        self.events.push(Event { hour, kind });
    }

    /// Ids of cars in a station's yard matching a kind and load filter, in
    /// the order they are taken: empty cars first, then lowest id.
    fn yard_cars(&self, station: &str, kind: CarKind, load: &LoadFilter) -> Vec<CarId> {
        let mut ids: Vec<CarId> = self
            .cars
            .values()
            .filter(|c| {
                c.kind == kind
                    && load.matches(c.load)
                    && matches!(&c.at, CarAt::Yard { station: s } if s == station)
            })
            .map(|c| c.id)
            .collect();
        ids.sort_by_key(|id| self.cars[id].load.is_some());
        ids
    }

    /// Ids of cars coupled to an engine satisfying a predicate, lowest first.
    fn coupled(&self, engine: &str, pred: impl Fn(&crate::world::Car) -> bool) -> Vec<CarId> {
        self.cars
            .values()
            .filter(|c| matches!(&c.at, CarAt::Coupled { engine: e } if e == engine) && pred(c))
            .map(|c| c.id)
            .collect()
    }

    /// Groups the cars satisfying a predicate by load, empty cars first.
    fn tally(&self, pred: impl Fn(&crate::world::Car) -> bool) -> Vec<CarCount> {
        let mut groups: BTreeMap<Option<Commodity>, u8> = BTreeMap::new();
        for c in self.cars.values().filter(|c| pred(c)) {
            *groups.entry(c.load).or_insert(0) += 1;
        }
        groups
            .into_iter()
            .map(|(load, count)| CarCount { load, count })
            .collect()
    }

    /// Applies an op's effect to the cars and orders, and emits `OrderMet`
    /// when an order becomes complete in time.
    fn apply(&mut self, ei: usize, op: &Op, cars: &[CarId], hour: u8, emit: bool) {
        let engine = self.engines[ei].id.clone();
        let station = self.engines[ei].station.clone();
        match op {
            Op::Couple { .. } => {
                for id in cars {
                    self.cars.get_mut(id).unwrap().at = CarAt::Coupled {
                        engine: engine.clone(),
                    };
                }
            }
            Op::Uncouple { .. } => {
                for id in cars {
                    self.cars.get_mut(id).unwrap().at = CarAt::Yard {
                        station: station.clone(),
                    };
                }
            }
            Op::Load { commodity, .. } => {
                for id in cars {
                    self.cars.get_mut(id).unwrap().load = Some(*commodity);
                }
            }
            Op::Process { count } => {
                let factory = self.world.network.station(&station).and_then(|s| s.factory);
                if let Some(f) = factory {
                    let n = *count as usize;
                    for id in &cars[..n] {
                        self.cars.get_mut(id).unwrap().load = None;
                    }
                    for id in &cars[n..] {
                        self.cars.get_mut(id).unwrap().load = Some(f.output);
                    }
                }
            }
            Op::Deliver { order } => {
                for id in cars {
                    self.cars.get_mut(id).unwrap().load = None;
                }
                self.reached.entry(order.clone()).or_insert(hour);
                if let Some(o) = self.orders.iter_mut().find(|o| &o.id == order) {
                    o.delivered += cars.len() as u8;
                    if o.delivered >= o.count {
                        o.delivered_at = Some(hour);
                        if emit && hour <= o.deadline {
                            let id = o.id.clone();
                            self.emit(hour, EventKind::OrderMet { order: id });
                        }
                    }
                }
            }
        }
    }

    /// Completes the running op of an engine at `hour`.
    fn complete(&mut self, ei: usize, hour: u8, emit: bool) {
        let running = self.engines[ei].running.take().unwrap();
        let index = running.index;
        let op = self.engines[ei].stops[self.engines[ei].stop].ops[index].clone();
        self.apply(ei, &op, &running.cars, hour, emit);
        if emit {
            let e = &self.engines[ei];
            let (engine, station, stop) = (e.id.clone(), e.station.clone(), e.stop);
            self.emit(
                hour,
                EventKind::OpDone {
                    engine,
                    station,
                    stop,
                    index,
                    op,
                    cars: running.cars,
                },
            );
        }
    }

    /// Checks an op's precondition for an engine in the current state: the
    /// cars and duration if it can start now, or the reason it cannot.
    fn check(&self, ei: usize, op: &Op) -> Result<Start, FailReason> {
        let e = &self.engines[ei];
        let (engine, station) = (e.id.as_str(), e.station.as_str());
        let net = &self.world.network;
        match op {
            Op::Couple { kind, load, count } => {
                let yard = self.yard_cars(station, *kind, load);
                let n = *count as usize;
                if yard.len() < n {
                    return Err(FailReason::NoMatchingCarsInYard {
                        kind: *kind,
                        load: *load,
                        wanted: *count,
                        found: yard.len() as u8,
                        yard: self.tally(|c| {
                            c.kind == *kind
                                && matches!(&c.at, CarAt::Yard { station: s } if s == station)
                        }),
                    });
                }
                let carried = self.coupled(engine, |_| true).len();
                if carried + n > e.capacity as usize {
                    return Err(FailReason::OverCapacity {
                        wanted: *count,
                        carried: carried as u8,
                        capacity: e.capacity,
                    });
                }
                Ok(Start {
                    cars: yard[..n].to_vec(),
                    hours: 0,
                })
            }
            Op::Uncouple { kind, load, count } => {
                let mut on = self.coupled(engine, |c| c.kind == *kind && load.matches(c.load));
                on.sort_by_key(|id| self.cars[id].load.is_some());
                let n = *count as usize;
                if on.len() < n {
                    return Err(FailReason::NoMatchingCarsOnEngine {
                        kind: *kind,
                        load: *load,
                        wanted: *count,
                        found: on.len() as u8,
                        coupled: self.tally(|c| {
                            c.kind == *kind
                                && matches!(&c.at, CarAt::Coupled { engine: x } if x == engine)
                        }),
                    });
                }
                Ok(Start {
                    cars: on[..n].to_vec(),
                    hours: 0,
                })
            }
            Op::Load { commodity, count } => {
                if !net
                    .station(station)
                    .is_some_and(|s| s.stock.contains(commodity))
                {
                    return Err(FailReason::NoStockHere {
                        commodity: *commodity,
                    });
                }
                let kind = commodity.car_kind();
                let empty = self.coupled(engine, |c| c.load.is_none() && c.kind == kind);
                let n = *count as usize;
                if empty.len() < n {
                    return Err(FailReason::NoEmptyCarsOnEngine {
                        kind,
                        wanted: *count,
                        found: empty.len() as u8,
                    });
                }
                Ok(Start {
                    cars: empty[..n].to_vec(),
                    hours: net.load_hours,
                })
            }
            Op::Process { count } => {
                let Some(f) = net.station(station).and_then(|s| s.factory) else {
                    return Err(FailReason::NoFactoryHere);
                };
                let n = *count as usize;
                let input = self.coupled(engine, |c| c.load == Some(f.input));
                if input.len() < n {
                    return Err(FailReason::NoInputCars {
                        load: f.input,
                        wanted: *count,
                        found: input.len() as u8,
                    });
                }
                let kind = f.output.car_kind();
                let output = self.coupled(engine, |c| c.load.is_none() && c.kind == kind);
                if output.len() < n {
                    return Err(FailReason::NoEmptyCarsOnEngine {
                        kind,
                        wanted: *count,
                        found: output.len() as u8,
                    });
                }
                let mut cars = input[..n].to_vec();
                cars.extend_from_slice(&output[..n]);
                Ok(Start { cars, hours: f.hours })
            }
            Op::Deliver { order } => {
                let Some(o) = self.orders.iter().find(|o| &o.id == order) else {
                    return Err(FailReason::UnknownOrder {
                        order: order.clone(),
                    });
                };
                if o.to != station {
                    return Err(FailReason::WrongDestination {
                        order: order.clone(),
                        to: o.to.clone(),
                    });
                }
                if o.delivered >= o.count {
                    return Err(FailReason::OrderFilled {
                        order: order.clone(),
                    });
                }
                let on = self.coupled(engine, |c| c.load == Some(o.commodity));
                if on.is_empty() {
                    return Err(FailReason::NoOrderLoadsOnEngine {
                        order: order.clone(),
                        commodity: o.commodity,
                        coupled: self.tally(
                            |c| matches!(&c.at, CarAt::Coupled { engine: x } if x == engine),
                        ),
                    });
                }
                let n = ((o.count - o.delivered) as usize).min(on.len());
                Ok(Start {
                    cars: on[..n].to_vec(),
                    hours: net.deliver_hours,
                })
            }
        }
    }

    /// Starts or executes the first pending op, in list order, whose
    /// precondition holds. Returns whether the engine made progress.
    fn try_op(&mut self, ei: usize, t: u8) -> bool {
        let (engine, station, stop) = {
            let e = &self.engines[ei];
            (e.id.clone(), e.station.clone(), e.stop)
        };
        let pending = self.engines[ei].pending.clone();
        for (slot, index) in pending.into_iter().enumerate() {
            let op = self.engines[ei].stops[stop].ops[index].clone();
            let Ok(Start { cars, hours }) = self.check(ei, &op) else {
                continue;
            };
            self.engines[ei].pending.remove(slot);
            if hours == 0 {
                self.apply(ei, &op, &cars, t, true);
                self.emit(
                    t,
                    EventKind::OpDone {
                        engine,
                        station,
                        stop,
                        index,
                        op,
                        cars,
                    },
                );
            } else {
                self.emit(
                    t,
                    EventKind::OpStarted {
                        engine,
                        station,
                        stop,
                        index,
                        op,
                        finish_hour: t + hours,
                    },
                );
                self.engines[ei].running = Some(Running {
                    finish: t + hours,
                    index,
                    cars,
                });
            }
            return true;
        }
        false
    }

    /// Whether an engine has a pending op that could start now.
    fn can_start_any(&self, ei: usize) -> bool {
        let e = &self.engines[ei];
        e.pending
            .iter()
            .any(|&i| self.check(ei, &e.stops[e.stop].ops[i]).is_ok())
    }

    /// Emits `OpAbandoned` with its reason for every pending op of an engine
    /// that cannot start, in list order, and clears them.
    fn abandon_pending(&mut self, ei: usize, t: u8) {
        let (engine, station, stop) = {
            let e = &self.engines[ei];
            (e.id.clone(), e.station.clone(), e.stop)
        };
        let pending = std::mem::take(&mut self.engines[ei].pending);
        for index in pending {
            let op = self.engines[ei].stops[stop].ops[index].clone();
            if let Err(reason) = self.check(ei, &op) {
                self.emit(
                    t,
                    EventKind::OpAbandoned {
                        engine: engine.clone(),
                        station: station.clone(),
                        stop,
                        index,
                        op,
                        reason,
                    },
                );
            }
        }
    }

    /// Whether the engine's depart condition holds at hour `t`.
    fn depart_ready(&self, ei: usize, t: u8) -> bool {
        let e = &self.engines[ei];
        match &e.stops[e.stop].depart {
            Depart::Immediately => true,
            Depart::At { hour } => t >= *hour,
            Depart::After { engine, plus_hours } => {
                if engine == &e.id {
                    return false;
                }
                self.visits
                    .iter()
                    .filter(|v| {
                        &v.engine == engine
                            && v.station == e.station
                            && v.to.is_none_or(|to| to >= e.arrived_at)
                    })
                    .map(|v| v.from.max(e.arrived_at))
                    .min()
                    .is_some_and(|t0| t as u16 >= t0 as u16 + *plus_hours as u16)
            }
        }
    }

    /// Whether an engine has nothing more to do.
    fn done(&self, e: &Run) -> bool {
        e.motion.is_none()
            && e.running.is_none()
            && (e.failed || e.stop + 1 >= e.stops.len())
            && e.pending.is_empty()
    }

    /// Records, for each open order, the first hour a car loaded with its
    /// commodity stands at its destination, in the yard or on a standing
    /// engine.
    fn note_reached(&mut self, t: u8) {
        for o in &self.orders {
            if self.reached.contains_key(&o.id) {
                continue;
            }
            let there = self.cars.values().any(|c| {
                c.load == Some(o.commodity)
                    && match &c.at {
                        CarAt::Yard { station } => station == &o.to,
                        CarAt::Coupled { engine } => self
                            .engines
                            .iter()
                            .any(|e| &e.id == engine && e.motion.is_none() && e.station == o.to),
                    }
            });
            if there {
                self.reached.insert(o.id.clone(), t);
            }
        }
    }

    /// What came closest to delivering an order, from what has happened so far.
    fn explain(&self, o: &OrderState) -> MissReason {
        let (commodity, to) = (o.commodity, o.to.clone());
        match (self.reached.get(&o.id), o.delivered_at) {
            (_, Some(delivered_at)) if o.delivered >= o.count => {
                MissReason::DeliveredAfterDeadline {
                    commodity,
                    to,
                    delivered_at,
                    deadline: o.deadline,
                }
            }
            (None, _) => MissReason::NoCarReached { commodity, to },
            (Some(&reached_at), _) if reached_at > o.deadline => {
                MissReason::ReachedAfterDeadline {
                    commodity,
                    to,
                    reached_at,
                    deadline: o.deadline,
                }
            }
            (Some(&reached_at), _) => MissReason::ReachedNotDelivered {
                commodity,
                to,
                reached_at,
                delivered: o.delivered,
                count: o.count,
            },
        }
    }

    /// Emits `OrderMissed` for an order with its current explanation.
    fn emit_missed(&mut self, hour: u8, order: &str) {
        let Some(o) = self.orders.iter().find(|o| o.id == order) else {
            return;
        };
        let why = self.explain(o);
        self.emit(
            hour,
            EventKind::OrderMissed {
                order: order.to_string(),
                why,
            },
        );
    }

    /// Replaces the explanation of every `OrderMissed` event with one based on
    /// the complete trace, so that a car arriving after the deadline is named.
    fn finalize_explanations(&mut self) {
        let fresh: Vec<(usize, MissReason)> = self
            .events
            .iter()
            .enumerate()
            .filter_map(|(i, e)| match &e.kind {
                EventKind::OrderMissed { order, .. } => self
                    .orders
                    .iter()
                    .find(|o| &o.id == order)
                    .map(|o| (i, self.explain(o))),
                _ => None,
            })
            .collect();
        for (i, new) in fresh {
            if let EventKind::OrderMissed { why, .. } = &mut self.events[i].kind {
                *why = new;
            }
        }
    }

    /// The world snapshot at an hour.
    fn snapshot(&self, hour: u8) -> WorldSnapshot {
        WorldSnapshot {
            hour,
            engines: self
                .engines
                .iter()
                .map(|e| EngineState {
                    id: e.id.clone(),
                    station: e.station.clone(),
                    capacity: e.capacity,
                    available_from: e.available_from,
                    arrived_at: e.arrived_at,
                    failed: e.failed,
                })
                .collect(),
            cars: self.cars.values().cloned().collect(),
            orders: self.orders.clone(),
        }
    }

    /// Settles in-flight work and captures the world and plan remainder.
    fn cut(&mut self, t: u8) -> Cut {
        for ei in 0..self.engines.len() {
            if let Some(m) = self.engines[ei].motion.take() {
                let e = &mut self.engines[ei];
                e.station = m.to;
                e.stop += 1;
                e.arrived_at = m.arrive;
                e.available_from = e.available_from.max(m.arrive);
                e.pending = all_ops(&e.stops[e.stop]);
                e.hold = None;
            }
            if let Some(finish) = self.engines[ei].running.as_ref().map(|r| r.finish) {
                self.complete(ei, finish, false);
                let e = &mut self.engines[ei];
                e.available_from = e.available_from.max(finish);
            }
        }
        let mut remainder = Plan::default();
        for e in &self.engines {
            let mut stops: Vec<Stop> = e.stops[e.stop..].to_vec();
            stops[0].ops = e.pending.iter().map(|&i| e.stops[e.stop].ops[i].clone()).collect();
            remainder.engines.insert(e.id.clone(), stops);
        }
        Cut {
            world: World {
                network: self.world.network.clone(),
                state: self.snapshot(t),
                restrictions: self.world.restrictions.clone(),
            },
            remainder,
        }
    }

    /// Runs hour by hour.
    fn run(mut self, cut_hour: Option<u8>) -> (Trace, Option<Cut>) {
        let initial = self.world.state.clone();
        let horizon = self.world.network.horizon;
        let start = self.world.state.hour;
        let mut end_hour = start;
        let mut cut = None;
        for t in start..=horizon {
            end_hour = t;
            self.arrivals(t);
            self.completions(t);
            if cut_hour == Some(t) {
                cut = Some(self.cut(t));
                break;
            }
            self.note_reached(t);
            self.ops(t);
            self.note_reached(t);
            self.departures(t);
            self.deadlines(t);
            if self.engines.iter().all(|e| self.done(e)) {
                break;
            }
        }
        if cut.is_none() {
            // Engines still standing with ops that never became possible.
            for ei in 0..self.engines.len() {
                if self.engines[ei].motion.is_none() {
                    self.abandon_pending(ei, end_hour);
                }
            }
            let mut late: Vec<&OrderState> = self
                .orders
                .iter()
                .filter(|o| o.delivered < o.count && o.deadline > end_hour)
                .collect();
            late.sort_by_key(|o| o.deadline);
            let missed: Vec<(u8, OrderId)> =
                late.iter().map(|o| (o.deadline, o.id.clone())).collect();
            for (hour, order) in missed {
                end_hour = end_hour.max(hour);
                self.emit_missed(hour, &order);
            }
        }
        self.finalize_explanations();
        let orders: Vec<OrderOutcome> = self.orders.iter().map(OrderOutcome::from).collect();
        let success = self.orders.iter().all(|o| o.met());
        (
            Trace {
                initial,
                events: self.events,
                end_hour,
                orders,
                success,
            },
            cut,
        )
    }

    /// Step 1: engines whose leg ends now arrive.
    fn arrivals(&mut self, t: u8) {
        for ei in 0..self.engines.len() {
            if self.engines[ei]
                .motion
                .as_ref()
                .is_some_and(|m| m.arrive == t)
            {
                let m = self.engines[ei].motion.take().unwrap();
                let e = &mut self.engines[ei];
                e.station = m.to.clone();
                e.stop += 1;
                e.arrived_at = t;
                e.pending = all_ops(&e.stops[e.stop]);
                e.hold = None;
                let (engine, stop) = (e.id.clone(), e.stop);
                self.visits.push(Visit {
                    engine: engine.clone(),
                    station: m.to.clone(),
                    from: t,
                    to: None,
                });
                self.emit(
                    t,
                    EventKind::Arrive {
                        engine,
                        station: m.to,
                        stop,
                    },
                );
            }
        }
    }

    /// Step 2: ops whose duration ends now take effect.
    fn completions(&mut self, t: u8) {
        for ei in 0..self.engines.len() {
            if self.engines[ei]
                .running
                .as_ref()
                .is_some_and(|r| r.finish == t)
            {
                self.complete(ei, t, true);
            }
        }
    }

    /// Step 3: every free engine starts its first possible pending op, until
    /// no engine can progress.
    fn ops(&mut self, t: u8) {
        loop {
            let mut progress = false;
            for ei in 0..self.engines.len() {
                loop {
                    let e = &self.engines[ei];
                    let ready = e.motion.is_none()
                        && t >= e.available_from
                        && e.running.is_none()
                        && !e.pending.is_empty();
                    if ready && self.try_op(ei, t) {
                        progress = true;
                    } else {
                        break;
                    }
                }
            }
            if !progress {
                break;
            }
        }
    }

    /// Steps 4 and 5: engines whose depart condition holds, with no op in
    /// progress and none that could start, try to enter their edge in
    /// engine-id order.
    fn departures(&mut self, t: u8) {
        for ei in 0..self.engines.len() {
            let e = &self.engines[ei];
            let wants = e.motion.is_none()
                && !e.failed
                && t >= e.available_from
                && e.stop + 1 < e.stops.len()
                && e.running.is_none()
                && self.depart_ready(ei, t)
                && !self.can_start_any(ei);
            if !wants {
                continue;
            }
            let from = e.station.clone();
            let to = e.stops[e.stop + 1].station.clone();
            let Some(edge) = self.world.network.edge(&from, &to) else {
                continue;
            };
            let hours = edge.hours;
            let reason = if self.world.closed_at(&from, &to, t) {
                Some(HoldReason::Closed)
            } else if self.engines.iter().any(|o| {
                o.motion.as_ref().is_some_and(|m| {
                    (m.from == from && m.to == to) || (m.from == to && m.to == from)
                })
            }) {
                Some(HoldReason::Signal)
            } else {
                None
            };
            if let Some(reason) = reason {
                if self.engines[ei].hold != Some(reason) {
                    self.engines[ei].hold = Some(reason);
                    let engine = self.engines[ei].id.clone();
                    self.emit(
                        t,
                        EventKind::Hold {
                            engine,
                            station: from,
                            reason,
                        },
                    );
                }
                continue;
            }
            // Enter the edge: report the ops that never became possible and
            // close the visit.
            self.abandon_pending(ei, t);
            let (engine, stop) = {
                let e = &self.engines[ei];
                (e.id.clone(), e.stop)
            };
            if let Some(v) = self
                .visits
                .iter_mut()
                .rev()
                .find(|v| v.engine == engine && v.to.is_none())
            {
                v.to = Some(t);
            }
            let arrive = t + hours;
            let e = &mut self.engines[ei];
            e.hold = None;
            e.motion = Some(Motion {
                from: from.clone(),
                to: to.clone(),
                arrive,
            });
            self.emit(
                t,
                EventKind::Depart {
                    engine,
                    from,
                    to,
                    stop,
                    arrive_hour: arrive,
                },
            );
        }
    }

    /// End of hour: orders whose deadline is now and are still open are missed.
    fn deadlines(&mut self, t: u8) {
        let missed: Vec<OrderId> = self
            .orders
            .iter()
            .filter(|o| o.deadline == t && o.delivered < o.count)
            .map(|o| o.id.clone())
            .collect();
        for order in missed {
            self.emit_missed(t, &order);
        }
    }
}
