//! Disruption candidates, selection, and the world after a disruption.
//!
//! The first joint run in a scenario that would succeed is interrupted. This
//! unit owns how a scenario's [`Candidate`]s are resolved against the pair's
//! merged plan, which of them qualify, how one is picked from the session
//! seed, and what the world and notes look like once it has happened.
//!
//! Neighbouring units rely on the following. [`plan_disruption`] returns a
//! [`Planned`] only for a candidate whose re-simulation from the disruption
//! hour (starting from the cut world and the unexecuted remainder of the
//! merged plan) fails, so the replan is always necessary. The `world` in a
//! `Planned` starts at the disruption hour. `remainder` is the plan both
//! players' notes are replaced by. The public [`DisruptionNotice`] is safe to
//! put in a trace that both players watch; the private [`Announcement`]s
//! go only to the role named for the kind of disruption (Routing for engine
//! failures and track closures, Freight for late orders).

use parlando_game_sdk::GameValue;
use serde::{Deserialize, Serialize};

use crate::plan::Plan;
use crate::sim::{simulate, simulate_cut, Cut, Merged};
use crate::world::{
    CarAt, Commodity, EngineId, OrderId, OrderState, Restriction, StationId, World,
};

/// How a candidate names its target when it does not name it directly.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Selector {
    /// The engine pulling the most cars at the disruption hour (lowest id
    /// breaks ties; none qualifies when no engine pulls a car).
    MostLoadedEngine,
    /// The edge, excluding bridges, with the most legs still to be started in
    /// the merged plan at the disruption hour (lexicographically smallest edge
    /// breaks ties).
    BusiestRemainingEdge,
}

/// A new order announced by a late-order disruption.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, GameValue)]
pub struct NewOrder {
    /// Order id.
    pub id: OrderId,
    /// Commodity.
    pub commodity: Commodity,
    /// Car loads.
    pub count: u8,
    /// Destination.
    pub to: StationId,
    /// Deadline hour.
    pub deadline: u8,
}

/// A changed deadline of an existing order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, GameValue)]
pub struct DeadlineChange {
    /// The order.
    pub order: OrderId,
    /// Its new deadline hour.
    pub deadline: u8,
}

/// What a candidate does, with a direct target or a selector.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Effect {
    /// An engine fails at the hour.
    EngineFailure {
        /// The engine, or `None` to use `selector`.
        #[serde(default)]
        engine: Option<EngineId>,
        /// Selector used when `engine` is absent.
        #[serde(default)]
        selector: Option<Selector>,
    },
    /// An edge closes from the hour to the end of the day.
    TrackClosure {
        /// One end of the edge, or `None` to use `selector`.
        #[serde(default)]
        a: Option<StationId>,
        /// The other end.
        #[serde(default)]
        b: Option<StationId>,
        /// Selector used when the edge is not named.
        #[serde(default)]
        selector: Option<Selector>,
    },
    /// A new order is announced, optionally with deadline changes.
    LateOrder {
        /// The new order.
        order: NewOrder,
        /// Changed deadlines of existing orders.
        #[serde(default)]
        deadline_changes: Vec<DeadlineChange>,
    },
}

/// A disruption a scenario may use.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    /// Name used in tests and analysis.
    pub id: String,
    /// The disruption hour.
    pub hour: u8,
    /// What happens.
    pub effect: Effect,
    /// A recorded recovery used only by tests: the notes both players could
    /// write after this disruption when the reference plan was running.
    #[serde(default)]
    pub recovery: Option<Plan>,
}

/// What both players see of a disruption: the film shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, GameValue)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DisruptionNotice {
    /// An engine breaks down where it stands.
    EngineFailure {
        /// The engine.
        engine: EngineId,
        /// The station it stands in (or rolled to).
        station: StationId,
    },
    /// Signals on a line go red.
    TrackClosure {
        /// One end of the line.
        a: StationId,
        /// The other end.
        b: StationId,
    },
    /// A telegram arrives at the depot. Its content is private to Freight.
    LateOrder {},
}

/// What one role is told about a disruption, with all details.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, GameValue)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Announcement {
    /// Announced to Routing.
    EngineFailure {
        /// The engine.
        engine: EngineId,
        /// The station it stands in.
        station: StationId,
        /// The disruption hour.
        hour: u8,
        /// Plain-language text.
        message: String,
    },
    /// Announced to Routing.
    TrackClosure {
        /// One end of the closed edge.
        a: StationId,
        /// The other end.
        b: StationId,
        /// First closed hour.
        from_hour: u8,
        /// Plain-language text.
        message: String,
    },
    /// Announced to Freight.
    LateOrder {
        /// The new order.
        order: NewOrder,
        /// Changed deadlines of existing orders.
        deadline_changes: Vec<DeadlineChange>,
        /// The disruption hour.
        hour: u8,
        /// Plain-language text.
        message: String,
    },
}

/// A disruption chosen for the pair's run and everything that follows from it.
#[derive(Clone, Debug)]
pub struct Planned {
    /// The id of the chosen candidate.
    pub candidate: String,
    /// The disruption hour.
    pub hour: u8,
    /// Public description for the film.
    pub notice: DisruptionNotice,
    /// The act-two world, starting at `hour`.
    pub world: World,
    /// The plan both players' notes are replaced by.
    pub remainder: Plan,
    /// Details for Freight, if it is the addressee.
    pub freight: Option<Announcement>,
    /// Details for Routing, if it is the addressee.
    pub routing: Option<Announcement>,
}

/// A candidate resolved against a cut.
enum Resolved {
    Engine(EngineId),
    Edge(StationId, StationId),
    Late(NewOrder, Vec<DeadlineChange>),
}

/// Resolves a candidate's target against the cut world and plan.
fn resolve(cut: &Cut, effect: &Effect) -> Option<Resolved> {
    match effect {
        Effect::EngineFailure {
            engine: Some(id), ..
        } => cut.world.engine(id).map(|_| Resolved::Engine(id.clone())),
        Effect::EngineFailure {
            selector: Some(Selector::MostLoadedEngine),
            ..
        } => {
            let mut best: Option<(usize, &str)> = None;
            for e in &cut.world.state.engines {
                let load = cut
                    .world
                    .state
                    .cars
                    .iter()
                    .filter(|c| matches!(&c.at, CarAt::Coupled { engine } if engine == &e.id))
                    .count();
                let better = match best {
                    None => load > 0,
                    Some((n, id)) => load > n || (load == n && e.id.as_str() < id),
                };
                if better {
                    best = Some((load, e.id.as_str()));
                }
            }
            best.map(|(_, id)| Resolved::Engine(id.to_string()))
        }
        Effect::TrackClosure {
            a: Some(a),
            b: Some(b),
            ..
        } => cut
            .world
            .network
            .edge(a, b)
            .filter(|_| !cut.world.network.is_bridge(a, b))
            .map(|_| Resolved::Edge(a.clone(), b.clone())),
        Effect::TrackClosure {
            selector: Some(Selector::BusiestRemainingEdge),
            ..
        } => {
            let mut counts: std::collections::BTreeMap<(String, String), usize> =
                Default::default();
            for stops in cut.remainder.engines.values() {
                for pair in stops.windows(2) {
                    let (x, y) = (&pair[0].station, &pair[1].station);
                    if cut.world.network.edge(x, y).is_some() && !cut.world.network.is_bridge(x, y)
                    {
                        let key = if x <= y {
                            (x.clone(), y.clone())
                        } else {
                            (y.clone(), x.clone())
                        };
                        *counts.entry(key).or_default() += 1;
                    }
                }
            }
            let max = counts.values().copied().max()?;
            counts
                .into_iter()
                .find(|(_, n)| *n == max)
                .map(|((a, b), _)| Resolved::Edge(a, b))
        }
        Effect::LateOrder {
            order,
            deadline_changes,
        } => Some(Resolved::Late(order.clone(), deadline_changes.clone())),
        _ => None,
    }
}

/// Display name of a station for announcement text.
fn name(world: &World, id: &str) -> String {
    world
        .network
        .station(id)
        .map(|s| s.name.clone())
        .unwrap_or_else(|| id.to_string())
}

/// Applies a resolved disruption to a cut, producing the act-two world, the
/// notice and the announcements.
fn apply(cut: Cut, resolved: Resolved, hour: u8) -> Planned {
    let Cut {
        mut world,
        remainder,
    } = cut;
    let (notice, freight, routing) = match resolved {
        Resolved::Engine(id) => {
            let station = world
                .engine(&id)
                .map(|e| e.station.clone())
                .unwrap_or_default();
            if let Some(e) = world.state.engines.iter_mut().find(|e| e.id == id) {
                e.failed = true;
            }
            let message = format!(
                "Engine {id} failed at {} at hour {hour}. It cannot leave again, but its cars can still be uncoupled there.",
                name(&world, &station)
            );
            (
                DisruptionNotice::EngineFailure {
                    engine: id.clone(),
                    station: station.clone(),
                },
                None,
                Some(Announcement::EngineFailure {
                    engine: id,
                    station,
                    hour,
                    message,
                }),
            )
        }
        Resolved::Edge(a, b) => {
            world.restrictions.push(Restriction {
                a: a.clone(),
                b: b.clone(),
                from_hour: hour,
                until_hour: world.network.horizon.saturating_add(1),
            });
            let message = format!(
                "The track between {} and {} is closed from hour {hour} to the end of the day.",
                name(&world, &a),
                name(&world, &b)
            );
            (
                DisruptionNotice::TrackClosure {
                    a: a.clone(),
                    b: b.clone(),
                },
                None,
                Some(Announcement::TrackClosure {
                    a,
                    b,
                    from_hour: hour,
                    message,
                }),
            )
        }
        Resolved::Late(order, changes) => {
            world.state.orders.push(OrderState {
                id: order.id.clone(),
                commodity: order.commodity,
                count: order.count,
                to: order.to.clone(),
                deadline: order.deadline,
                delivered: 0,
                delivered_at: None,
            });
            for change in &changes {
                if let Some(o) = world.state.orders.iter_mut().find(|o| o.id == change.order) {
                    o.deadline = change.deadline;
                }
            }
            let what = match order.commodity {
                Commodity::Oranges => "oranges",
                Commodity::Bananas => "bananas",
                Commodity::Juice => "juice",
            };
            let message = format!(
                "A late order arrives at hour {hour}: {} car load(s) of {what} to {} by hour {}.",
                order.count,
                name(&world, &order.to),
                order.deadline
            );
            (
                DisruptionNotice::LateOrder {},
                Some(Announcement::LateOrder {
                    order,
                    deadline_changes: changes,
                    hour,
                    message,
                }),
                None,
            )
        }
    };
    Planned {
        candidate: String::new(),
        hour,
        notice,
        world,
        remainder,
        freight,
        routing,
    }
}

/// A well-mixed value derived from the seed and scenario position.
fn mix(seed: u64, salt: u64) -> u64 {
    let mut z = seed ^ salt.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Resolves one candidate against a world and merged plan.
///
/// Returns the planned disruption together with whether it qualifies
/// (re-simulating from the disruption hour fails), or `None` when the
/// candidate does not apply (hour outside the run, unresolvable target).
fn evaluate(
    world: &World,
    merged: &Merged,
    run_end: u8,
    candidate: &Candidate,
) -> Option<(Planned, bool)> {
    if candidate.hour < world.start_hour() || candidate.hour > run_end {
        return None;
    }
    let (_, cut) = simulate_cut(world, &merged.plan, &merged.unreached, candidate.hour);
    let cut = cut?;
    let resolved = resolve(&cut, &candidate.effect)?;
    let mut planned = apply(cut, resolved, candidate.hour);
    planned.candidate = candidate.id.clone();
    let rerun = simulate(&planned.world, &planned.remainder, &[]);
    Some((planned, !rerun.success))
}

/// Selects the disruption for a successful run of the merged plan.
///
/// `run_end` is the end hour of the full successful trace. Each candidate is
/// resolved against the merged plan; those that break it (by re-simulation
/// from their hour) are kept and one is chosen by the seed. Returns `None`
/// when no candidate qualifies.
pub fn plan_disruption(
    world: &World,
    merged: &Merged,
    run_end: u8,
    candidates: &[Candidate],
    seed: u64,
    salt: u64,
) -> Option<Planned> {
    let mut kept: Vec<Planned> = candidates
        .iter()
        .filter_map(|c| evaluate(world, merged, run_end, c))
        .filter(|(_, breaks)| *breaks)
        .map(|(p, _)| p)
        .collect();
    if kept.is_empty() {
        return None;
    }
    let pick = (mix(seed, salt) % kept.len() as u64) as usize;
    Some(kept.swap_remove(pick))
}

/// Resolves and applies one candidate without checking that it breaks the
/// plan. Used by scenario tests to build the post-disruption world.
pub fn plan_candidate(
    world: &World,
    merged: &Merged,
    run_end: u8,
    candidate: &Candidate,
) -> Option<(Planned, bool)> {
    evaluate(world, merged, run_end, candidate)
}
