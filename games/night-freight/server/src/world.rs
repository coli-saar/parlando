//! The railway world: stations, track, rolling stock, cargo and orders.
//!
//! This module owns the vocabulary shared by every other unit of the game:
//! identifiers, the static [`Network`] (stations, single-track edges, factory,
//! handling durations) and the dynamic [`WorldSnapshot`] (where each engine and
//! car is, and how far each order is delivered). A [`World`] pairs the two with
//! the track [`Restriction`] windows in force.
//!
//! The simulator ([`crate::sim`]) reads a `World` and never mutates it;
//! [`crate::disruption`] builds the next act's `World` from a simulator
//! cut. Neighbouring units may rely on these invariants: identifiers are
//! unique within their list, every edge joins two known stations, every edge
//! takes at least one hour, and car ids are unique.

use parlando_game_sdk::GameValue;
use serde::{Deserialize, Serialize};

/// Identifier of an engine, such as `E1`.
pub type EngineId = String;
/// Identifier of a station, a lower-case slug such as `corning`.
pub type StationId = String;
/// Identifier of an order, such as `O1`.
pub type OrderId = String;
/// Stable numeric identifier of a car. Lower ids are chosen first.
pub type CarId = u16;

/// The kind of a freight car.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, GameValue,
)]
#[serde(rename_all = "snake_case")]
pub enum CarKind {
    /// A ribbed boxcar, which carries oranges and bananas.
    Boxcar,
    /// A tank car, which carries juice.
    Tanker,
}

/// A cargo commodity.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, GameValue,
)]
#[serde(rename_all = "snake_case")]
pub enum Commodity {
    /// Travels in boxcars.
    Oranges,
    /// Travels in boxcars.
    Bananas,
    /// Travels in tankers.
    Juice,
}

impl Commodity {
    /// The kind of car that can carry this commodity.
    pub fn car_kind(self) -> CarKind {
        match self {
            Commodity::Oranges | Commodity::Bananas => CarKind::Boxcar,
            Commodity::Juice => CarKind::Tanker,
        }
    }
}

/// A factory that turns one commodity into another.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, GameValue)]
pub struct Factory {
    /// The commodity consumed.
    pub input: Commodity,
    /// The commodity produced.
    pub output: Commodity,
    /// Hours one `Process` operation takes.
    pub hours: u8,
}

/// A station. Stations have unlimited yard capacity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Station {
    /// Station identifier.
    pub id: StationId,
    /// Display name.
    pub name: String,
    /// Commodities available in unlimited supply for `Load`.
    #[serde(default)]
    pub stock: Vec<Commodity>,
    /// The factory at this station, if any.
    #[serde(default)]
    pub factory: Option<Factory>,
}

/// An undirected single-track connection between two stations.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, GameValue)]
pub struct Edge {
    /// One end.
    pub a: StationId,
    /// The other end.
    pub b: StationId,
    /// Travel time in hours (at least one).
    pub hours: u8,
}

impl Edge {
    /// Whether this edge joins the two stations, in either order.
    pub fn joins(&self, x: &str, y: &str) -> bool {
        (self.a == x && self.b == y) || (self.a == y && self.b == x)
    }
}

/// The static part of a world: map and handling times.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Network {
    /// All stations.
    pub stations: Vec<Station>,
    /// All edges.
    pub edges: Vec<Edge>,
    /// Last simulated hour of the day (24).
    #[serde(default = "default_horizon")]
    pub horizon: u8,
    /// Hours a `Load` operation takes.
    #[serde(default = "default_load_hours")]
    pub load_hours: u8,
    /// Hours a `Deliver` operation takes.
    #[serde(default)]
    pub deliver_hours: u8,
}

fn default_horizon() -> u8 {
    24
}

fn default_load_hours() -> u8 {
    1
}

impl Network {
    /// Looks up a station by id.
    pub fn station(&self, id: &str) -> Option<&Station> {
        self.stations.iter().find(|s| s.id == id)
    }

    /// Looks up the edge joining two stations.
    pub fn edge(&self, x: &str, y: &str) -> Option<&Edge> {
        self.edges.iter().find(|e| e.joins(x, y))
    }

    /// Whether removing the edge would disconnect the map.
    pub fn is_bridge(&self, x: &str, y: &str) -> bool {
        let mut seen = vec![x.to_string()];
        let mut frontier = vec![x.to_string()];
        while let Some(at) = frontier.pop() {
            for edge in &self.edges {
                if edge.joins(x, y) {
                    continue;
                }
                let next = if edge.a == at {
                    &edge.b
                } else if edge.b == at {
                    &edge.a
                } else {
                    continue;
                };
                if !seen.contains(next) {
                    seen.push(next.clone());
                    frontier.push(next.clone());
                }
            }
        }
        !seen.iter().any(|s| s == y)
    }
}

/// Where a car is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, GameValue)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CarAt {
    /// In the yard of a station.
    Yard {
        /// The station.
        station: StationId,
    },
    /// Coupled to an engine.
    Coupled {
        /// The engine.
        engine: EngineId,
    },
}

/// A freight car with at most one load.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, GameValue)]
pub struct Car {
    /// Stable id.
    pub id: CarId,
    /// Boxcar or tanker.
    pub kind: CarKind,
    /// The commodity inside, or `None` when empty.
    pub load: Option<Commodity>,
    /// Current position.
    pub at: CarAt,
}

/// An engine's state at an instant.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, GameValue)]
pub struct EngineState {
    /// Engine id.
    pub id: EngineId,
    /// The station it stands in (snapshots never place an engine on an edge).
    pub station: StationId,
    /// Number of cars it can pull.
    pub capacity: u8,
    /// The engine can neither start operations nor depart before this hour.
    pub available_from: u8,
    /// The hour it arrived at `station`.
    pub arrived_at: u8,
    /// A failed engine cannot depart again but can still couple and uncouple.
    pub failed: bool,
}

/// An order and how far it has been delivered.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, GameValue)]
pub struct OrderState {
    /// Order id.
    pub id: OrderId,
    /// Commodity to deliver.
    pub commodity: Commodity,
    /// Number of car loads.
    pub count: u8,
    /// Destination station.
    pub to: StationId,
    /// Last hour at which delivery still counts.
    pub deadline: u8,
    /// Car loads delivered so far.
    #[serde(default)]
    pub delivered: u8,
    /// The hour the final car load was delivered.
    #[serde(default)]
    pub delivered_at: Option<u8>,
}

impl OrderState {
    /// Whether the order has been delivered completely and on time.
    pub fn met(&self) -> bool {
        self.delivered >= self.count && self.delivered_at.is_some_and(|h| h <= self.deadline)
    }
}

/// The dynamic state of the world at one hour: all engines, cars and orders.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, GameValue)]
pub struct WorldSnapshot {
    /// The hour this snapshot describes.
    pub hour: u8,
    /// All engines in id order.
    pub engines: Vec<EngineState>,
    /// All cars in id order.
    pub cars: Vec<Car>,
    /// All orders.
    pub orders: Vec<OrderState>,
}

/// A window during which an edge cannot be entered.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, GameValue)]
pub struct Restriction {
    /// One end of the closed edge.
    pub a: StationId,
    /// The other end.
    pub b: StationId,
    /// First hour at which the edge is closed.
    pub from_hour: u8,
    /// First hour at which the edge is open again (exclusive end).
    pub until_hour: u8,
}

/// Everything the simulator needs: map, state and restrictions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct World {
    /// The static map.
    pub network: Network,
    /// The state at `state.hour`, which is also the simulation start hour.
    pub state: WorldSnapshot,
    /// Track restriction windows in force.
    pub restrictions: Vec<Restriction>,
}

impl World {
    /// Whether the edge between two stations is closed at an hour.
    pub fn closed_at(&self, x: &str, y: &str, hour: u8) -> bool {
        self.restrictions.iter().any(|r| {
            let same = (r.a == x && r.b == y) || (r.a == y && r.b == x);
            same && r.from_hour <= hour && hour < r.until_hour
        })
    }

    /// The hour at which simulation starts.
    pub fn start_hour(&self) -> u8 {
        self.state.hour
    }

    /// Looks up an engine's state.
    pub fn engine(&self, id: &str) -> Option<&EngineState> {
        self.state.engines.iter().find(|e| e.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line() -> Network {
        let station = |id: &str| Station {
            id: id.into(),
            name: id.into(),
            stock: vec![],
            factory: None,
        };
        let edge = |a: &str, b: &str| Edge {
            a: a.into(),
            b: b.into(),
            hours: 1,
        };
        Network {
            stations: vec![station("a"), station("b"), station("c"), station("d")],
            edges: vec![
                edge("a", "b"),
                edge("b", "c"),
                edge("c", "a"),
                edge("c", "d"),
            ],
            horizon: 24,
            load_hours: 1,
            deliver_hours: 0,
        }
    }

    /// A cycle edge is not a bridge; the spur to a leaf is.
    #[test]
    fn bridges_are_detected() {
        let net = line();
        assert!(!net.is_bridge("a", "b"));
        assert!(!net.is_bridge("c", "a"));
        assert!(net.is_bridge("c", "d"));
    }

    /// Restriction windows are half open and apply in both directions.
    #[test]
    fn restriction_windows_are_half_open() {
        let world = World {
            network: line(),
            state: WorldSnapshot {
                hour: 0,
                engines: vec![],
                cars: vec![],
                orders: vec![],
            },
            restrictions: vec![Restriction {
                a: "a".into(),
                b: "b".into(),
                from_hour: 4,
                until_hour: 8,
            }],
        };
        assert!(!world.closed_at("b", "a", 3));
        assert!(world.closed_at("b", "a", 4));
        assert!(world.closed_at("a", "b", 7));
        assert!(!world.closed_at("a", "b", 8));
    }
}
