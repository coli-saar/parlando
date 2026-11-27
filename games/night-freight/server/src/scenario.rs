//! The scenario file format and the built-in scenarios.
//!
//! A scenario is a YAML document compiled into the component with
//! `include_str!`. It defines the map, the engines, the cars, the orders,
//! Routing's track restrictions, the disruption candidates, and two plans
//! that only tests use: `naive_plan` (the plan a pair would try first, which
//! must fail) and `reference_plan` (a plan that meets every order).
//!
//! [`builtin`] returns a validated [`Scenario`]; every built-in is validated
//! by the unit tests, so a caller may rely on stations, edges, engines and
//! cars being consistent, on every plan being walkable on the map, and on no
//! disruption candidate closing a bridge. [`Scenario::world`] and
//! [`Scenario::initial_plan`] turn a scenario into the starting state of its
//! first act.

use serde::Deserialize;

use crate::disruption::{Candidate, Effect, NewOrder};
use crate::plan::Plan;
use crate::world::{
    Car, CarAt, CarId, CarKind, Commodity, Edge, EngineState, Network, OrderState, Restriction,
    Station, StationId, World, WorldSnapshot,
};

/// Handling durations that are scenario data.
#[derive(Clone, Debug, Deserialize)]
pub struct Durations {
    /// Hours a `Load` takes.
    #[serde(default = "one")]
    pub load: u8,
    /// Hours a `Deliver` takes.
    #[serde(default)]
    pub deliver: u8,
}

fn one() -> u8 {
    1
}

/// An engine at the start of the scenario.
#[derive(Clone, Debug, Deserialize)]
pub struct EngineSpec {
    /// Engine id.
    pub id: String,
    /// Starting station.
    pub station: StationId,
    /// Cars it can pull.
    pub capacity: u8,
    /// Hour from which it may operate.
    #[serde(default)]
    pub available_from: u8,
}

/// An empty car standing in a yard at the start.
#[derive(Clone, Debug, Deserialize)]
pub struct CarSpec {
    /// Car id.
    pub id: CarId,
    /// Boxcar or tanker.
    pub kind: CarKind,
    /// The station whose yard it stands in.
    pub at: StationId,
    /// An initial load, if any.
    #[serde(default)]
    pub load: Option<Commodity>,
}

/// A complete scenario definition.
#[derive(Clone, Debug, Deserialize)]
pub struct Scenario {
    /// Scenario id used in configuration.
    pub id: String,
    /// Short title for the browser.
    pub title: String,
    /// One-sentence summary shown to both players.
    pub summary: String,
    /// First simulated hour.
    #[serde(default)]
    pub start_hour: u8,
    /// Stations.
    pub stations: Vec<Station>,
    /// Edges.
    pub edges: Vec<Edge>,
    /// Handling durations.
    pub durations: Durations,
    /// Engines.
    pub engines: Vec<EngineSpec>,
    /// Cars.
    pub cars: Vec<CarSpec>,
    /// Orders.
    pub orders: Vec<NewOrder>,
    /// Routing's track restriction windows.
    #[serde(default)]
    pub restrictions: Vec<Restriction>,
    /// Disruption candidates.
    #[serde(default)]
    pub disruptions: Vec<Candidate>,
    /// The first plan a pair would try; must fail.
    pub naive_plan: Plan,
    /// A plan that meets every order.
    pub reference_plan: Plan,
}

/// Ids of the built-in scenarios in their default session order.
pub const BUILTIN_IDS: [&str; 3] = ["tutorial", "corning-juice", "bath-bananas"];

/// Returns the YAML text of a built-in scenario.
fn source(id: &str) -> Option<&'static str> {
    Some(match id {
        "tutorial" => include_str!("../scenarios/tutorial.yaml"),
        "corning-juice" => include_str!("../scenarios/corning-juice.yaml"),
        "bath-bananas" => include_str!("../scenarios/bath-bananas.yaml"),
        _ => return None,
    })
}

/// Loads and validates a built-in scenario.
pub fn builtin(id: &str) -> Result<Scenario, String> {
    let text = source(id).ok_or_else(|| format!("unknown scenario {id:?}"))?;
    let scenario: Scenario =
        serde_yaml::from_str(text).map_err(|e| format!("scenario {id}: {e}"))?;
    scenario
        .validate()
        .map_err(|e| format!("scenario {id}: {e}"))?;
    Ok(scenario)
}

impl Scenario {
    /// The starting world of the first act.
    pub fn world(&self) -> World {
        World {
            network: Network {
                stations: self.stations.clone(),
                edges: self.edges.clone(),
                horizon: 24,
                load_hours: self.durations.load,
                deliver_hours: self.durations.deliver,
            },
            state: WorldSnapshot {
                hour: self.start_hour,
                engines: self
                    .engines
                    .iter()
                    .map(|e| EngineState {
                        id: e.id.clone(),
                        station: e.station.clone(),
                        capacity: e.capacity,
                        available_from: e.available_from.max(self.start_hour),
                        arrived_at: self.start_hour,
                        failed: false,
                    })
                    .collect(),
                cars: {
                    let mut cars: Vec<Car> = self
                        .cars
                        .iter()
                        .map(|c| Car {
                            id: c.id,
                            kind: c.kind,
                            load: c.load,
                            at: CarAt::Yard {
                                station: c.at.clone(),
                            },
                        })
                        .collect();
                    cars.sort_by_key(|c| c.id);
                    cars
                },
                orders: self
                    .orders
                    .iter()
                    .map(|o| OrderState {
                        id: o.id.clone(),
                        commodity: o.commodity,
                        count: o.count,
                        to: o.to.clone(),
                        deadline: o.deadline,
                        delivered: 0,
                        delivered_at: None,
                    })
                    .collect(),
            },
            restrictions: self.restrictions.clone(),
        }
    }

    /// The notes both players start with: every engine at its station.
    pub fn initial_plan(&self) -> Plan {
        Plan::initial(
            self.engines
                .iter()
                .map(|e| (e.id.as_str(), e.station.as_str())),
        )
    }

    /// Checks internal consistency.
    pub fn validate(&self) -> Result<(), String> {
        let net = self.world().network;
        let unique = |ids: Vec<String>, what: &str| {
            let mut sorted = ids.clone();
            sorted.sort();
            sorted.dedup();
            if sorted.len() == ids.len() {
                Ok(())
            } else {
                Err(format!("duplicate {what} id"))
            }
        };
        unique(
            self.stations.iter().map(|s| s.id.clone()).collect(),
            "station",
        )?;
        unique(
            self.engines.iter().map(|s| s.id.clone()).collect(),
            "engine",
        )?;
        unique(self.cars.iter().map(|c| c.id.to_string()).collect(), "car")?;
        unique(self.orders.iter().map(|o| o.id.clone()).collect(), "order")?;
        let station = |id: &str| {
            net.station(id)
                .map(|_| ())
                .ok_or_else(|| format!("unknown station {id:?}"))
        };
        for e in &self.edges {
            station(&e.a)?;
            station(&e.b)?;
            if e.hours == 0 {
                return Err("edges take at least one hour".into());
            }
        }
        for e in &self.engines {
            station(&e.station)?;
        }
        for c in &self.cars {
            station(&c.at)?;
        }
        for o in &self.orders {
            station(&o.to)?;
            if o.deadline > net.horizon {
                return Err(format!("order {} deadline beyond the horizon", o.id));
            }
        }
        for r in &self.restrictions {
            net.edge(&r.a, &r.b)
                .ok_or_else(|| "restriction on an unknown edge".to_string())?;
        }
        for (name, plan) in [
            ("naive_plan", &self.naive_plan),
            ("reference_plan", &self.reference_plan),
        ]
        .into_iter()
        .chain(
            self.disruptions
                .iter()
                .filter_map(|d| d.recovery.as_ref().map(|p| ("recovery", p))),
        ) {
            self.check_plan(&net, name, plan)?;
        }
        for d in &self.disruptions {
            if let Effect::TrackClosure {
                a: Some(a),
                b: Some(b),
                ..
            } = &d.effect
            {
                if net.edge(a, b).is_none() || net.is_bridge(a, b) {
                    return Err(format!(
                        "candidate {} closes a bridge or unknown edge",
                        d.id
                    ));
                }
            }
        }
        Ok(())
    }

    /// Checks that a plan names known engines and walks adjacent stations.
    fn check_plan(&self, net: &Network, name: &str, plan: &Plan) -> Result<(), String> {
        for (engine, stops) in &plan.engines {
            if !self.engines.iter().any(|e| &e.id == engine) {
                return Err(format!("{name}: unknown engine {engine}"));
            }
            for pair in stops.windows(2) {
                if net.edge(&pair[0].station, &pair[1].station).is_none() {
                    return Err(format!(
                        "{name}: {engine}: {} and {} are not adjacent",
                        pair[0].station, pair[1].station
                    ));
                }
            }
        }
        Ok(())
    }
}
