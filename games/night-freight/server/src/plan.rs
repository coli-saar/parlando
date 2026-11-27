//! Notes: the plan data model, note edits and their validation.
//!
//! Each player owns one [`Plan`]: per engine, a list of [`Stop`]s, each with a
//! [`Depart`] condition (owned by the Routing role), a list of [`Op`]s (owned
//! by the Freight role) and an annotation. This unit owns the shape of the
//! plans, the closed set of edits ([`NoteEdit`]) and the rule that an edit is
//! validated only against facts both roles share, so that a rejection never
//! discloses the partner's private facts.
//!
//! Neighbouring units rely on these guarantees: after any accepted edit
//! `stops[0]` of every engine is unchanged in station, every consecutive pair
//! of stops is adjacent on the map, no engine has more than
//! [`MAX_STOPS`] stops or any stop more than [`MAX_OPS`] ops, and counts and
//! hours are within range. [`apply_edit`] validates completely before it
//! mutates, so a rejected edit leaves the plan untouched.

use std::collections::BTreeMap;

use parlando_game_sdk::GameValue;
use serde::{Deserialize, Serialize};

use crate::world::{CarKind, Commodity, EngineId, Network, OrderId, StationId};

/// Maximum number of stops in one engine's strip.
pub const MAX_STOPS: usize = 12;
/// Maximum number of operations at one stop.
pub const MAX_OPS: usize = 6;
/// Maximum car count in one operation.
pub const MAX_COUNT: u8 = 9;

/// When an engine leaves a stop. Owned by the Routing role.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, GameValue)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Depart {
    /// Leave in the hour of arrival.
    #[default]
    Immediately,
    /// Leave when the clock is at or past `hour`.
    At {
        /// Hour of the day.
        hour: u8,
    },
    /// Leave `plus_hours` after the named engine is first present at this
    /// station at or after this engine's arrival.
    After {
        /// The engine to wait for.
        engine: EngineId,
        /// Hours to wait after it is present.
        plus_hours: u8,
    },
}

/// Which cars of a kind a `Couple` or `Uncouple` op applies to, by load.
///
/// `Any` is the default and matches every car of the kind; the engine takes
/// empty cars before loaded ones, and lowest car ids first within each group.
/// `Empty` and `Loaded` restrict the choice. A `Loaded` filter naming a
/// commodity that the car kind cannot carry never matches, so edits reject it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, GameValue)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LoadFilter {
    /// Cars with any load or none.
    #[default]
    Any,
    /// Only empty cars.
    Empty,
    /// Only cars loaded with this commodity.
    Loaded {
        /// The commodity the cars must carry.
        commodity: Commodity,
    },
}

impl LoadFilter {
    /// Whether a car with this load passes the filter.
    pub fn matches(&self, load: Option<Commodity>) -> bool {
        match self {
            LoadFilter::Any => true,
            LoadFilter::Empty => load.is_none(),
            LoadFilter::Loaded { commodity } => load == Some(*commodity),
        }
    }

    /// Whether this filter can match cars of `kind` at all.
    pub fn fits(&self, kind: CarKind) -> bool {
        match self {
            LoadFilter::Loaded { commodity } => commodity.car_kind() == kind,
            _ => true,
        }
    }
}

/// One cargo-handling step at a stop. Owned by the Freight role.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, GameValue)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Op {
    /// Couple `count` cars of the given kind and load filter from the yard.
    Couple {
        /// Car kind.
        kind: CarKind,
        /// Which cars qualify by load (default: any).
        #[serde(default)]
        load: LoadFilter,
        /// Number of cars.
        count: u8,
    },
    /// Uncouple `count` cars of the given kind and load filter into the yard.
    Uncouple {
        /// Car kind.
        kind: CarKind,
        /// Which cars qualify by load (default: any).
        #[serde(default)]
        load: LoadFilter,
        /// Number of cars.
        count: u8,
    },
    /// Load `count` empty cars on the engine from the station's stock.
    Load {
        /// What to load.
        commodity: Commodity,
        /// Number of cars.
        count: u8,
    },
    /// Run `count` cars through the station's factory.
    Process {
        /// Number of cars processed.
        count: u8,
    },
    /// Deliver loaded cars against an order.
    Deliver {
        /// The order.
        order: OrderId,
    },
}

/// One station visit in an engine's strip.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, GameValue)]
pub struct Stop {
    /// The station.
    pub station: StationId,
    /// Departure condition (route-owned).
    #[serde(default)]
    pub depart: Depart,
    /// Cargo steps (cargo-owned), executed in order.
    #[serde(default)]
    pub ops: Vec<Op>,
    /// An arrival hour the player jotted down. No effect on simulation.
    #[serde(default)]
    pub heard_arrival: Option<u8>,
}

impl Stop {
    /// A stop at the station with default departure and no ops.
    pub fn at(station: &str) -> Self {
        Self {
            station: station.into(),
            depart: Depart::Immediately,
            ops: vec![],
            heard_arrival: None,
        }
    }
}

/// A player's notes: one strip of stops per engine.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, GameValue)]
pub struct Plan {
    /// Stops per engine id. `stops[0]` is the engine's current station.
    pub engines: BTreeMap<String, Vec<Stop>>,
}

impl Plan {
    /// A plan in which every engine consists of only its current station.
    pub fn initial<'a>(engines: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        Self {
            engines: engines
                .into_iter()
                .map(|(id, station)| (id.to_string(), vec![Stop::at(station)]))
                .collect(),
        }
    }

    /// The station sequence of one engine.
    pub fn route(&self, engine: &str) -> Vec<&str> {
        self.engines
            .get(engine)
            .map(|stops| stops.iter().map(|s| s.station.as_str()).collect())
            .unwrap_or_default()
    }
}

/// One edit to a player's own notes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, GameValue)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NoteEdit {
    /// Append a stop. Routing's stop must be at an adjacent station; Freight's
    /// may be any station (its notes are an itinerary, not a route).
    AppendStop {
        /// The engine.
        engine: EngineId,
        /// The station to append.
        station: StationId,
    },
    /// Remove `stops[from..]`. `from` must be at least 1.
    TruncateStops {
        /// The engine.
        engine: EngineId,
        /// First stop index to remove.
        from: usize,
    },
    /// Replace a stop's departure condition.
    SetDepart {
        /// The engine.
        engine: EngineId,
        /// Stop index.
        stop: usize,
        /// The new condition.
        depart: Depart,
    },
    /// Insert an op into a stop's list.
    InsertOp {
        /// The engine.
        engine: EngineId,
        /// Stop index.
        stop: usize,
        /// Position in the op list (`0..=len`).
        index: usize,
        /// The op.
        op: Op,
    },
    /// Remove an op from a stop's list.
    RemoveOp {
        /// The engine.
        engine: EngineId,
        /// Stop index.
        stop: usize,
        /// Position in the op list.
        index: usize,
    },
    /// Set or clear the heard-arrival annotation.
    SetHeardArrival {
        /// The engine.
        engine: EngineId,
        /// Stop index.
        stop: usize,
        /// The jotted hour, or `None` to clear.
        hour: Option<u8>,
    },
}

/// What an edit is validated against: facts both roles share.
pub struct EditContext<'a> {
    /// The map (adjacency, station ids, horizon).
    pub network: &'a Network,
    /// Known engine ids.
    pub engines: Vec<&'a str>,
    /// Order ids to validate `Deliver` ops against, or `None` when the actor
    /// does not know the orders (the Routing role).
    pub orders: Option<Vec<&'a str>>,
    /// Whether `AppendStop` must name a station adjacent to the engine's last
    /// stop. Routing's notes are a route, so it must; Freight's notes are an
    /// itinerary of places, so Freight may append any station (the ghost
    /// inserts the connecting path, see `sim::itinerary_to_route`).
    pub adjacent_only: bool,
}

/// Validates an op's own fields.
fn check_op(op: &Op, ctx: &EditContext) -> Result<(), &'static str> {
    if let Op::Couple { kind, load, .. } | Op::Uncouple { kind, load, .. } = op {
        if !load.fits(*kind) {
            return Err("incompatible_load");
        }
    }
    let count = match op {
        Op::Couple { count, .. }
        | Op::Uncouple { count, .. }
        | Op::Load { count, .. }
        | Op::Process { count } => Some(*count),
        Op::Deliver { order } => {
            if let Some(orders) = &ctx.orders {
                if !orders.contains(&order.as_str()) {
                    return Err("unknown_order");
                }
            }
            None
        }
    };
    match count {
        Some(c) if !(1..=MAX_COUNT).contains(&c) => Err("invalid_count"),
        _ => Ok(()),
    }
}

/// Validates a depart condition against shared facts.
fn check_depart(depart: &Depart, ctx: &EditContext) -> Result<(), &'static str> {
    match depart {
        Depart::Immediately => Ok(()),
        Depart::At { hour } if *hour > ctx.network.horizon => Err("invalid_hour"),
        Depart::At { .. } => Ok(()),
        Depart::After { engine, plus_hours } => {
            if !ctx.engines.contains(&engine.as_str()) {
                Err("unknown_engine")
            } else if *plus_hours > ctx.network.horizon {
                Err("invalid_hour")
            } else {
                Ok(())
            }
        }
    }
}

/// Applies one edit to a plan, or returns the stable rejection code.
///
/// All validation happens before any mutation. Codes: `unknown_engine`,
/// `unknown_station`, `unknown_order`, `not_adjacent`, `index_out_of_range`,
/// `fixed_stop`, `too_many_stops`, `too_many_ops`, `invalid_count`,
/// `incompatible_load`, `invalid_hour`, `no_change`.
pub fn apply_edit(plan: &mut Plan, edit: &NoteEdit, ctx: &EditContext) -> Result<(), &'static str> {
    let engine = match edit {
        NoteEdit::AppendStop { engine, .. }
        | NoteEdit::TruncateStops { engine, .. }
        | NoteEdit::SetDepart { engine, .. }
        | NoteEdit::InsertOp { engine, .. }
        | NoteEdit::RemoveOp { engine, .. }
        | NoteEdit::SetHeardArrival { engine, .. } => engine,
    };
    let stops = plan.engines.get_mut(engine).ok_or("unknown_engine")?;
    match edit {
        NoteEdit::AppendStop { station, .. } => {
            ctx.network.station(station).ok_or("unknown_station")?;
            let last = stops.last().ok_or("index_out_of_range")?;
            if ctx.adjacent_only && ctx.network.edge(&last.station, station).is_none() {
                return Err("not_adjacent");
            }
            if stops.len() >= MAX_STOPS {
                return Err("too_many_stops");
            }
            stops.push(Stop::at(station));
        }
        NoteEdit::TruncateStops { from, .. } => {
            if *from == 0 {
                return Err("fixed_stop");
            }
            if *from >= stops.len() {
                return Err("index_out_of_range");
            }
            stops.truncate(*from);
        }
        NoteEdit::SetDepart { stop, depart, .. } => {
            check_depart(depart, ctx)?;
            let target = stops.get_mut(*stop).ok_or("index_out_of_range")?;
            if &target.depart == depart {
                return Err("no_change");
            }
            target.depart = depart.clone();
        }
        NoteEdit::InsertOp {
            stop, index, op, ..
        } => {
            check_op(op, ctx)?;
            let target = stops.get_mut(*stop).ok_or("index_out_of_range")?;
            if *index > target.ops.len() {
                return Err("index_out_of_range");
            }
            if target.ops.len() >= MAX_OPS {
                return Err("too_many_ops");
            }
            target.ops.insert(*index, op.clone());
        }
        NoteEdit::RemoveOp { stop, index, .. } => {
            let target = stops.get_mut(*stop).ok_or("index_out_of_range")?;
            if *index >= target.ops.len() {
                return Err("index_out_of_range");
            }
            target.ops.remove(*index);
        }
        NoteEdit::SetHeardArrival { stop, hour, .. } => {
            if hour.is_some_and(|h| h > ctx.network.horizon) {
                return Err("invalid_hour");
            }
            let target = stops.get_mut(*stop).ok_or("index_out_of_range")?;
            if target.heard_arrival == *hour {
                return Err("no_change");
            }
            target.heard_arrival = *hour;
        }
    }
    Ok(())
}

/// The visit key of a stop: `(engine, station, n)` for the n-th (1-based)
/// occurrence of the station in the engine's list.
pub type VisitKey = (EngineId, StationId, usize);

/// Lists `(visit key, stop)` for every stop of a plan.
pub fn visits(plan: &Plan) -> Vec<(VisitKey, &Stop)> {
    let mut out = Vec::new();
    for (engine, stops) in &plan.engines {
        let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
        for stop in stops {
            let n = seen.entry(stop.station.as_str()).or_insert(0);
            *n += 1;
            out.push(((engine.clone(), stop.station.clone(), *n), stop));
        }
    }
    out
}

/// The fraction of stops and ops, compared by visit key, that two plans share.
///
/// A stop counts as shared when both plans contain its visit key with the same
/// departure condition. Ops at a visit key are compared as multisets. Two
/// plans without any stop or op agree completely (1.0).
pub fn agreement(a: &Plan, b: &Plan) -> f64 {
    let va: BTreeMap<VisitKey, &Stop> = visits(a).into_iter().collect();
    let vb: BTreeMap<VisitKey, &Stop> = visits(b).into_iter().collect();
    let mut total = 0usize;
    let mut same = 0usize;
    let keys: std::collections::BTreeSet<&VisitKey> = va.keys().chain(vb.keys()).collect();
    for key in keys {
        total += 1;
        let ops_a: &[Op] = va.get(key).map(|s| s.ops.as_slice()).unwrap_or(&[]);
        let ops_b: &[Op] = vb.get(key).map(|s| s.ops.as_slice()).unwrap_or(&[]);
        if let (Some(x), Some(y)) = (va.get(key), vb.get(key)) {
            if x.depart == y.depart {
                same += 1;
            }
        }
        let mut rest: Vec<&Op> = ops_b.iter().collect();
        let mut inter = 0usize;
        for op in ops_a {
            if let Some(pos) = rest.iter().position(|o| *o == op) {
                rest.remove(pos);
                inter += 1;
            }
        }
        total += ops_a.len() + ops_b.len() - inter;
        same += inter;
    }
    if total == 0 {
        1.0
    } else {
        same as f64 / total as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::{Edge, Station};

    fn net() -> Network {
        let st = |id: &str| Station {
            id: id.into(),
            name: id.into(),
            stock: vec![],
            factory: None,
        };
        Network {
            stations: vec![st("a"), st("b"), st("c")],
            edges: vec![Edge {
                a: "a".into(),
                b: "b".into(),
                hours: 1,
            }],
            horizon: 24,
            load_hours: 1,
            deliver_hours: 0,
        }
    }

    fn ctx(net: &Network) -> EditContext<'_> {
        EditContext {
            network: net,
            engines: vec!["E1", "E2"],
            orders: Some(vec!["O1"]),
            adjacent_only: true,
        }
    }

    fn base() -> Plan {
        Plan::initial([("E1", "a"), ("E2", "c")])
    }

    /// Every rejection code is reachable and leaves the plan untouched.
    #[test]
    fn rejections_leave_the_plan_unchanged() {
        let net = net();
        let ctx = ctx(&net);
        let mut plan = base();
        let before = plan.clone();
        let bad: Vec<(NoteEdit, &str)> = vec![
            (
                NoteEdit::AppendStop {
                    engine: "E9".into(),
                    station: "b".into(),
                },
                "unknown_engine",
            ),
            (
                NoteEdit::AppendStop {
                    engine: "E1".into(),
                    station: "z".into(),
                },
                "unknown_station",
            ),
            (
                NoteEdit::AppendStop {
                    engine: "E2".into(),
                    station: "a".into(),
                },
                "not_adjacent",
            ),
            (
                NoteEdit::TruncateStops {
                    engine: "E1".into(),
                    from: 0,
                },
                "fixed_stop",
            ),
            (
                NoteEdit::TruncateStops {
                    engine: "E1".into(),
                    from: 1,
                },
                "index_out_of_range",
            ),
            (
                NoteEdit::SetDepart {
                    engine: "E1".into(),
                    stop: 0,
                    depart: Depart::At { hour: 25 },
                },
                "invalid_hour",
            ),
            (
                NoteEdit::SetDepart {
                    engine: "E1".into(),
                    stop: 0,
                    depart: Depart::After {
                        engine: "E7".into(),
                        plus_hours: 1,
                    },
                },
                "unknown_engine",
            ),
            (
                NoteEdit::SetDepart {
                    engine: "E1".into(),
                    stop: 0,
                    depart: Depart::Immediately,
                },
                "no_change",
            ),
            (
                NoteEdit::SetDepart {
                    engine: "E1".into(),
                    stop: 3,
                    depart: Depart::At { hour: 1 },
                },
                "index_out_of_range",
            ),
            (
                NoteEdit::InsertOp {
                    engine: "E1".into(),
                    stop: 0,
                    index: 0,
                    op: Op::Process { count: 0 },
                },
                "invalid_count",
            ),
            (
                NoteEdit::InsertOp {
                    engine: "E1".into(),
                    stop: 0,
                    index: 0,
                    op: Op::Deliver { order: "O9".into() },
                },
                "unknown_order",
            ),
            (
                NoteEdit::InsertOp {
                    engine: "E1".into(),
                    stop: 0,
                    index: 4,
                    op: Op::Process { count: 1 },
                },
                "index_out_of_range",
            ),
            (
                NoteEdit::RemoveOp {
                    engine: "E1".into(),
                    stop: 0,
                    index: 0,
                },
                "index_out_of_range",
            ),
            (
                NoteEdit::SetHeardArrival {
                    engine: "E1".into(),
                    stop: 0,
                    hour: Some(30),
                },
                "invalid_hour",
            ),
            (
                NoteEdit::SetHeardArrival {
                    engine: "E1".into(),
                    stop: 0,
                    hour: None,
                },
                "no_change",
            ),
        ];
        for (edit, code) in bad {
            assert_eq!(apply_edit(&mut plan, &edit, &ctx), Err(code), "{edit:?}");
            assert_eq!(plan, before);
        }
    }

    /// Limits on stops and ops are enforced, and Routing may name unknown orders.
    #[test]
    fn limits_and_unknown_orders() {
        let net = net();
        let mut ctx = ctx(&net);
        let mut plan = base();
        for i in 0..MAX_STOPS - 1 {
            let station = if i % 2 == 0 { "b" } else { "a" };
            let edit = NoteEdit::AppendStop {
                engine: "E1".into(),
                station: station.into(),
            };
            apply_edit(&mut plan, &edit, &ctx).unwrap();
        }
        let more = NoteEdit::AppendStop {
            engine: "E1".into(),
            station: "a".into(),
        };
        assert_eq!(apply_edit(&mut plan, &more, &ctx), Err("too_many_stops"));
        for _ in 0..MAX_OPS {
            let op = NoteEdit::InsertOp {
                engine: "E1".into(),
                stop: 0,
                index: 0,
                op: Op::Process { count: 1 },
            };
            apply_edit(&mut plan, &op, &ctx).unwrap();
        }
        let op = NoteEdit::InsertOp {
            engine: "E1".into(),
            stop: 0,
            index: 0,
            op: Op::Process { count: 1 },
        };
        assert_eq!(apply_edit(&mut plan, &op, &ctx), Err("too_many_ops"));
        ctx.orders = None;
        let deliver = NoteEdit::InsertOp {
            engine: "E2".into(),
            stop: 0,
            index: 0,
            op: Op::Deliver {
                order: "anything".into(),
            },
        };
        assert_eq!(apply_edit(&mut plan, &deliver, &ctx), Ok(()));
    }

    /// Agreement is 1.0 for equal plans and drops with each difference.
    #[test]
    fn agreement_measures_overlap() {
        let net = net();
        let ctx = ctx(&net);
        let mut a = base();
        let mut b = base();
        assert_eq!(agreement(&a, &b), 1.0);
        let append = NoteEdit::AppendStop {
            engine: "E1".into(),
            station: "b".into(),
        };
        apply_edit(&mut a, &append, &ctx).unwrap();
        assert!(agreement(&a, &b) < 1.0);
        apply_edit(&mut b, &append, &ctx).unwrap();
        assert_eq!(agreement(&a, &b), 1.0);
        let op = NoteEdit::InsertOp {
            engine: "E1".into(),
            stop: 1,
            index: 0,
            op: Op::Deliver { order: "O1".into() },
        };
        apply_edit(&mut a, &op, &ctx).unwrap();
        let half = agreement(&a, &b);
        assert!(half < 1.0 && half > 0.0);
    }

    /// Visit keys count repeated stations separately.
    #[test]
    fn visit_keys_count_occurrences() {
        let mut plan = base();
        plan.engines
            .get_mut("E1")
            .unwrap()
            .extend([Stop::at("b"), Stop::at("a")]);
        let keys: Vec<VisitKey> = visits(&plan)
            .into_iter()
            .map(|(k, _)| k)
            .filter(|k| k.0 == "E1")
            .collect();
        assert_eq!(
            keys,
            vec![
                ("E1".into(), "a".into(), 1),
                ("E1".into(), "b".into(), 1),
                ("E1".into(), "a".into(), 2)
            ]
        );
    }
}
