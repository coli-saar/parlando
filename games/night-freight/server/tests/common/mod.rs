//! Shared helpers for the integration tests: a small laboratory world and
//! compact constructors for plans.

#![allow(dead_code)]

use parlando_night_freight::plan::{Depart, LoadFilter, Op, Plan, Stop};
use parlando_night_freight::scenario::Scenario;
use parlando_night_freight::sim::{Event, EventKind, Trace};
use parlando_night_freight::world::{CarKind, Commodity, World};

/// A world with a cycle `a-b-c` and a spur `c-d`:
/// travel times a-b 1, b-c 1, c-a 1, c-d 1.
/// `a` stocks bananas, `c` stocks oranges, `b` has an oranges-to-juice factory
/// (2 hours). Engines E1 at `a` and E2 at `b`, both with capacity 2. Boxcars
/// 1, 2, 4 at `a` and a tanker 3 at `b`; boxcar 5 at `b` holds oranges. One
/// order, O1, asks for one car load of bananas at `c` by hour 10.
pub const LAB: &str = r#"
id: lab
title: Lab
summary: A laboratory.
stations:
  - { id: a, name: A, stock: [bananas] }
  - { id: b, name: B, factory: { input: oranges, output: juice, hours: 2 } }
  - { id: c, name: C, stock: [oranges] }
  - { id: d, name: D }
edges:
  - { a: a, b: b, hours: 1 }
  - { a: b, b: c, hours: 1 }
  - { a: c, b: a, hours: 1 }
  - { a: c, b: d, hours: 1 }
durations: { load: 1, deliver: 0 }
engines:
  - { id: E1, station: a, capacity: 2 }
  - { id: E2, station: b, capacity: 2 }
cars:
  - { id: 1, kind: boxcar, at: a }
  - { id: 2, kind: boxcar, at: a }
  - { id: 3, kind: tanker, at: b }
  - { id: 4, kind: boxcar, at: a }
  - { id: 5, kind: boxcar, at: b, load: oranges }
orders:
  - { id: O1, commodity: bananas, count: 1, to: c, deadline: 10 }
naive_plan: { engines: {} }
reference_plan: { engines: {} }
"#;

/// The laboratory scenario.
pub fn lab() -> Scenario {
    serde_yaml::from_str(LAB).unwrap()
}

/// The laboratory world.
pub fn lab_world() -> World {
    lab().world()
}

/// A stop with a departure condition and ops.
pub fn stop(station: &str, depart: Depart, ops: Vec<Op>) -> Stop {
    Stop {
        station: station.into(),
        depart,
        ops,
        heard_arrival: None,
    }
}

/// A plan from `(engine, stops)` pairs.
pub fn plan(engines: Vec<(&str, Vec<Stop>)>) -> Plan {
    Plan {
        engines: engines
            .into_iter()
            .map(|(id, stops)| (id.to_string(), stops))
            .collect(),
    }
}

/// `Couple` of any boxcars.
pub fn couple_boxcars(count: u8) -> Op {
    Op::Couple {
        kind: CarKind::Boxcar,
        load: LoadFilter::Any,
        count,
    }
}

/// `Load` of bananas.
pub fn load_bananas(count: u8) -> Op {
    Op::Load {
        commodity: Commodity::Bananas,
        count,
    }
}

/// The events of a trace of one kind-predicate, as `(hour, kind)` pairs.
pub fn find(trace: &Trace, pred: impl Fn(&EventKind) -> bool) -> Vec<&Event> {
    trace.events.iter().filter(|e| pred(&e.kind)).collect()
}

/// The hour of the first `Depart` of an engine.
pub fn depart_hour(trace: &Trace, engine: &str) -> Option<u8> {
    find(
        trace,
        |k| matches!(k, EventKind::Depart { engine: e, .. } if e == engine),
    )
    .first()
    .map(|e| e.hour)
}
