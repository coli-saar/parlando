//! Cooperative hotel lifts sharing one cable and a single brake key for their fixed levers.
pub mod porter;
use anyhow::Result;
use parlando::{
    ActionRejection, Game, GameFactory, GameInitializationContext, GameSessionContext, PlayerRole,
};
use rand::{rngs::StdRng, Rng, SeedableRng};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Engraving required by a landing door. The opposite guest has the porter’s entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Seal {
    Stag,
    Rose,
    Key,
}

const SEALS: [Seal; 3] = [Seal::Stag, Seal::Rose, Seal::Key];
const CABIN_CAPACITY: u8 = 3;
const RACK_CAPACITY: u8 = 6;

/// A guest’s horizontal position and facing on the shared rescue crossing.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgePose {
    /// East–west coordinate in metres in the common crossing frame.
    pub x: f64,
    /// North–south coordinate in metres in the common crossing frame.
    pub z: f64,
    /// Clockwise heading from positive Z, in radians in the interval [−π, π].
    pub yaw: f64,
}

impl BridgePose {
    /// Bounds submitted coordinates to the envelope containing the gallery and refuge.
    fn is_valid(self) -> bool {
        (-8.0..=45.0).contains(&self.x)
            && (-45.0..=12.0).contains(&self.z)
            && (-std::f64::consts::PI..=std::f64::consts::PI).contains(&self.yaw)
    }
}

/// One guest’s physical operation on their car or the shared mechanism.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    /// Take one suitcase aboard from the current landing, or request it from the nearest stocked rack.
    Load,
    /// Request one suitcase from an addressed rack in this wing.
    LoadRack { floor: u8 },
    /// Take the marked dispensary suitcase aboard East at the supply dock after the key exchange.
    CollectRescueCase,
    /// Dispatch one ordinary suitcase to the current landing’s rack, or to the nearest rack with room.
    Unload,
    /// Release the coupled cars for one detent. The brake catches at the next landing.
    Release,
    /// Pass the brake key through the aligned hatches at Floor 2.
    Pass,
    /// Step from the parked car onto the local landing.
    StepOntoLanding,
    /// Return from the landing to the parked car.
    BoardLift,
    /// Try the local rescue bridge gate using the engraving reported by the other guest.
    Exit { seal: Seal },
    /// Rearm this wing’s gate lock after a wrong seal trips it.
    ResetLock,
    /// Share a pose while on the crossing, or clear it on return into the private cabin.
    BridgePose { pose: Option<BridgePose> },
    /// Cross the open local bridge to the refuge.
    ReachRefuge,
}

/// A freight run pairs a fixed stores stop with the range of acceptable seeded loads for each wing.
struct FreightOrder {
    stores_floor_east: u8,
    delivery_ranges: [[u8; 2]; 2],
}

/// The two itineraries use the same cable, exchange grille, brake, and locks.
struct Itinerary {
    title: &'static str,
    start: [u8; 2],
    destination: [u8; 2],
    initial_load: [[u8; 2]; 2],
    freight: Option<FreightOrder>,
}
/// The rescue objective selected when a session is configured.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Route {
    /// Reach the rescue bridges and unlock both doors.
    #[default]
    Escape,
    /// Stage the marked dispensary suitcase from the East supply dock and bring both ledger loads to the bridges.
    RescueSupplies,
}

impl Route {
    /// Supplies the deal parameters and display title for this objective.
    fn itinerary(self) -> Itinerary {
        match self {
            Self::Escape => Itinerary {
                title: "Bridge Escape",
                start: [3, 3],
                destination: [3, 4],
                initial_load: [[0, 0], [0, 0]],
                freight: None,
            },
            Self::RescueSupplies => Itinerary {
                title: "Rescue Supplies",
                start: [3, 4],
                destination: [3, 4],
                initial_load: [[1, 3], [0, 1]],
                freight: Some(FreightOrder {
                    stores_floor_east: 1,
                    delivery_ranges: [[2, 3], [0, 1]],
                }),
            },
        }
    }
}

/// Authoritative replayable state. East and west positions sum to four. Arrays
/// hold east first. Cargo counts exclude the identical tare of each furnished car.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct State {
    route: Route,
    east: u8,
    cargo: [u8; 2],
    /// Suitcases on each wing’s racks, indexed by that wing’s local floor number.
    landing_loads: [[u8; 5]; 2],
    passed: bool,
    /// Guests on their local landings.
    on_landing: [bool; 2],
    gate_open: [bool; 2],
    lock_tripped: [bool; 2],
    exited: [bool; 2],
    /// Latest public crossing pose by actor. Older replay snapshots start with no poses.
    #[serde(default, skip_serializing_if = "poses_unset")]
    bridge_poses: [Option<BridgePose>; 2],
    destination: u8,
    seals: [Seal; 2],
    manifest: Option<[u8; 2]>,
    /// The marked dispensary suitcase in East has moved from its dedicated rack into the cabin.
    rescue_case_aboard: bool,
}

/// Omits an empty presence field from serialized initial and historical game states.
fn poses_unset(poses: &[Option<BridgePose>; 2]) -> bool {
    *poses == [None; 2]
}

impl State {
    /// Deals a reproducible itinerary and lock pair. The supply run starts above
    /// the grille, so reaching the final landing requires reversing the imbalance.
    fn deal(seed: u64, route: Route) -> Self {
        let route_seed = match route {
            Route::Escape => seed,
            Route::RescueSupplies => seed ^ 0x9e3779b97f4a7c15,
        };
        let mut rng = StdRng::seed_from_u64(route_seed);
        let itinerary = route.itinerary();
        let mut draw = |[low, high]: [u8; 2]| rng.gen_range(low..=high);
        let destination = draw(itinerary.destination);
        let east = draw(itinerary.start);
        let cargo = itinerary.initial_load.map(&mut draw);
        let manifest = itinerary
            .freight
            .map(|order| order.delivery_ranges.map(&mut draw));
        Self {
            route,
            east,
            cargo,
            landing_loads: [[CABIN_CAPACITY; 5]; 2],
            passed: false,
            on_landing: [false; 2],
            gate_open: [false; 2],
            lock_tripped: [false; 2],
            exited: [false; 2],
            bridge_poses: [None; 2],
            destination,
            manifest,
            rescue_case_aboard: false,
            seals: [SEALS[rng.gen_range(0..3)], SEALS[rng.gen_range(0..3)]],
        }
    }

    /// Reads this wing’s landing from the single shared cable coordinate.
    fn floor(&self, seat: usize) -> u8 {
        if seat == 0 {
            self.east
        } else {
            4 - self.east
        }
    }

    /// Computes the next cable detent from the load imbalance and guide limits.
    fn next_floor(&self) -> Option<u8> {
        use std::cmp::Ordering::*;
        match self.cargo[0].cmp(&self.cargo[1]) {
            Less if self.east < 4 => Some(self.east + 1),
            Greater if self.east > 0 => Some(self.east - 1),
            _ => None,
        }
    }

    /// Uses the current rack when possible. The service porter dispatches a
    /// suitcase to or from the nearest eligible rack otherwise. Equal distances
    /// favor the lower local floor, so both clients and replay can predict it.
    fn transfer_floor(&self, seat: usize, loading: bool) -> usize {
        let current = usize::from(self.floor(seat));
        self.landing_loads[seat]
            .iter()
            .enumerate()
            .filter(|&(_, &cases)| {
                if loading {
                    cases > 0
                } else {
                    cases < RACK_CAPACITY
                }
            })
            .map(|(floor, _)| floor)
            .min_by_key(|&floor| (floor.abs_diff(current), floor))
            .expect("each wing always has both stocked racks and rack space")
    }

    /// Checks freight independently of the dock and gate seals.
    fn freight_ready(&self) -> bool {
        self.manifest
            .is_none_or(|manifest| self.rescue_case_aboard && self.cargo == manifest)
    }

    /// Enumerates accessible controls without revealing which seal opens a gate.
    fn controls(&self, seat: usize) -> Vec<Action> {
        let mut actions = Vec::new();
        if self.exited[seat] {
            return actions;
        }
        if self.gate_open[seat] {
            return vec![Action::ReachRefuge];
        }
        if self.on_landing[seat] {
            return vec![Action::BoardLift];
        }
        actions.push(Action::StepOntoLanding);
        if self.lock_tripped[seat] {
            actions.push(Action::ResetLock);
        } else if self.passed && self.east == self.destination && self.freight_ready() {
            actions.extend(SEALS.map(|seal| Action::Exit { seal }));
        }
        if self.gate_open.contains(&true) {
            return actions;
        }
        // Every wing has 15–18 suitcases. With three in the cabin at most, the
        // 30 rack slots always have both a suitcase and a vacant slot somewhere.
        if self.cargo[seat] < CABIN_CAPACITY {
            actions.push(Action::Load);
            actions.extend(
                self.landing_loads[seat]
                    .iter()
                    .enumerate()
                    .filter(|&(_, &cases)| cases > 0)
                    .map(|(floor, _)| Action::LoadRack { floor: floor as u8 }),
            );
        }
        if seat == 0
            && self.passed
            && self
                .route
                .itinerary()
                .freight
                .is_some_and(|order| self.east == order.stores_floor_east)
            && !self.on_landing.contains(&true)
            && !self.rescue_case_aboard
            && self.cargo[0] < CABIN_CAPACITY
        {
            actions.push(Action::CollectRescueCase);
        }
        if self.cargo[seat] > u8::from(seat == 0 && self.rescue_case_aboard) {
            actions.push(Action::Unload)
        }
        let holds = seat == usize::from(self.passed);
        if holds && !self.on_landing.contains(&true) {
            if self.next_floor().is_some() {
                actions.push(Action::Release)
            }
            if !self.passed && self.east == 2 {
                actions.push(Action::Pass)
            }
        }
        actions
    }

    /// Commits an accessible operation. Opening either bridge gate freezes the cars
    /// at the rescue landings so both guests retain access to their refuge.
    fn step(&self, action: Action, seat: usize) -> Result<Self, ActionRejection> {
        if let Action::BridgePose { pose } = action {
            if !self.gate_open[seat] || self.exited[seat] {
                return Err(ActionRejection::new("control_unavailable"));
            }
            if pose.is_some_and(|pose| !pose.is_valid()) {
                return Err(ActionRejection::new("invalid_bridge_pose"));
            }
            let mut next = self.clone();
            next.bridge_poses[seat] = pose;
            return Ok(next);
        }
        if !self.controls(seat).contains(&action) {
            return Err(ActionRejection::new("control_unavailable"));
        }
        let mut next = self.clone();
        match action {
            Action::Load => {
                next.cargo[seat] += 1;
                next.landing_loads[seat][self.transfer_floor(seat, true)] -= 1;
            }
            Action::LoadRack { floor } => {
                next.cargo[seat] += 1;
                next.landing_loads[seat][usize::from(floor)] -= 1;
            }
            Action::CollectRescueCase => {
                next.rescue_case_aboard = true;
                next.cargo[0] += 1;
            }
            Action::Unload => {
                next.cargo[seat] -= 1;
                next.landing_loads[seat][self.transfer_floor(seat, false)] += 1;
            }
            Action::Release => {
                next.east = self
                    .next_floor()
                    .expect("authorized release has a destination");
            }
            Action::Pass => next.passed = true,
            Action::StepOntoLanding => next.on_landing[seat] = true,
            Action::BoardLift => next.on_landing[seat] = false,
            Action::Exit { seal } => {
                if seal == self.seals[seat] {
                    next.gate_open[seat] = true;
                } else {
                    next.lock_tripped[seat] = true;
                }
            }
            Action::ResetLock => next.lock_tripped[seat] = false,
            Action::BridgePose { .. } => unreachable!("pose actions are handled before controls"),
            Action::ReachRefuge => {
                next.exited[seat] = true;
                next.bridge_poses[seat] = None;
            }
        }
        Ok(next)
    }
}

/// Configures the route for one session. Parlando supplies the session seed.
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Escape objective. Omit it to use the introductory bridge escape.
    #[serde(default)]
    pub route: Route,
}
/// Constructs an independent hotel adapter for each session.
pub struct HotelFactory;
/// Implements the Parlando boundary around the two lift itineraries.
pub struct Hotel;
impl GameFactory for HotelFactory {
    type Game = Hotel;
    /// Creates a stateless adapter. All durable mechanics live in State.
    fn create(&self, _: GameSessionContext) -> Result<Hotel> {
        Ok(Hotel)
    }
}
/// Role-visible wing identity used by both the participant and the porter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Wing {
    East,
    West,
}

/// Readiness of the marked suitcase on the supply itinerary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FreightStatus {
    AwaitingPickup,
    Aboard,
    Ready,
}

/// A role's complete view of the coupled lifts. Partner fields contain only
/// information visible from this wing and the entry on its private ledger.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Observation {
    pub wing: Wing,
    pub route: Route,
    pub route_title: std::borrow::Cow<'static, str>,
    pub floor: u8,
    pub partner_floor: u8,
    pub release_floor: Option<u8>,
    pub cargo: u8,
    pub landing_loads: [u8; 5],
    pub partner_landing_loads: [u8; 5],
    pub load_floor: u8,
    pub unload_floor: u8,
    pub partner_cargo: u8,
    pub handle: Wing,
    pub handed_over: bool,
    pub on_landing: bool,
    pub partner_on_landing: bool,
    pub gate_open: bool,
    pub partner_gate_open: bool,
    pub lock_tripped: bool,
    pub partner_lock_tripped: bool,
    pub exited: bool,
    pub partner_exited: bool,
    pub partner_bridge_pose: Option<BridgePose>,
    pub partner_destination: u8,
    pub partner_delivery: Option<u8>,
    pub partner_seal: Seal,
    pub freight_floor: Option<u8>,
    pub freight_status: Option<FreightStatus>,
    pub actions: Vec<Action>,
}

/// Shared escape outcome recorded when both guests reach safety.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Completion {
    pub escaped: bool,
    pub route: Route,
}

impl Game for Hotel {
    /// Replay state can record the latest guest pose for each side of the crossing.
    const STATE_REPRESENTATION_VERSION: &'static str = "6";
    /// Bridge pose actions carry the latest position or clear it on reentry.
    const ACTION_REPRESENTATION_VERSION: &'static str = "4";
    /// Rescue freight is staged at a dedicated dock and a wrong seal trips the local lock.
    const RULES_VERSION: &'static str = "5";

    type Config = Config;
    type State = State;
    type Action = Action;
    type Observation = Observation;
    type Completion = Completion;
    /// Starts the configured itinerary from the runtime’s replay seed.
    fn initial_state(&self, c: GameInitializationContext<'_, Config>) -> Result<State> {
        Ok(State::deal(c.seed, c.config.route))
    }
    /// Applies precisely one operation on the actor’s car.
    fn apply_action(&self, s: &State, a: &Action, r: PlayerRole) -> Result<State, ActionRejection> {
        s.step(*a, usize::from(r == PlayerRole::B))
    }
    /// Reports the visible mechanism and the porter’s routing entry for the other guest.
    /// Inventory and transfer floors use the viewer’s local floor numbers. Transfer
    /// floors describe rack selection independently of whether the action is available.
    /// Freight readiness tracks the manifest independently of bridge docking and lock state.
    fn observation(&self, s: &State, r: PlayerRole) -> Observation {
        let seat = usize::from(r == PlayerRole::B);
        let peer = 1 - seat;
        let wings = [Wing::East, Wing::West];
        let freight_floor = s.route.itinerary().freight.map(|order| {
            if seat == 0 {
                order.stores_floor_east
            } else {
                4 - order.stores_floor_east
            }
        });
        let freight_status = s.manifest.map(|_| {
            if s.freight_ready() {
                FreightStatus::Ready
            } else if s.rescue_case_aboard {
                FreightStatus::Aboard
            } else {
                FreightStatus::AwaitingPickup
            }
        });
        Observation {
            wing: wings[seat],
            route: s.route,
            route_title: s.route.itinerary().title.into(),
            floor: s.floor(seat),
            partner_floor: s.floor(peer),
            release_floor: s
                .next_floor()
                .map(|east| if seat == 0 { east } else { 4 - east }),
            cargo: s.cargo[seat],
            landing_loads: s.landing_loads[seat],
            partner_landing_loads: s.landing_loads[peer],
            load_floor: s.transfer_floor(seat, true) as u8,
            unload_floor: s.transfer_floor(seat, false) as u8,
            partner_cargo: s.cargo[peer],
            handle: wings[usize::from(s.passed)],
            handed_over: s.passed,
            on_landing: s.on_landing[seat],
            partner_on_landing: s.on_landing[peer],
            gate_open: s.gate_open[seat],
            partner_gate_open: s.gate_open[peer],
            lock_tripped: s.lock_tripped[seat],
            partner_lock_tripped: s.lock_tripped[peer],
            exited: s.exited[seat],
            partner_exited: s.exited[peer],
            partner_bridge_pose: s.bridge_poses[peer],
            partner_destination: if peer == 0 {
                s.destination
            } else {
                4 - s.destination
            },
            partner_delivery: s.manifest.map(|manifest| manifest[peer]),
            partner_seal: s.seals[peer],
            freight_floor,
            freight_status,
            actions: s.controls(seat),
        }
    }
    /// Provides the same physical control choices as the participant view.
    fn available_actions(&self, s: &State, r: PlayerRole) -> Option<Vec<Action>> {
        Some(s.controls(usize::from(r == PlayerRole::B)))
    }
    /// Records the public cable position and key ownership after the operation.
    fn transition_metadata(
        &self,
        _: &State,
        s: &State,
        action: &Action,
        _: PlayerRole,
    ) -> Option<Value> {
        if matches!(action, Action::BridgePose { .. }) {
            return None;
        }
        Some(json!({"route":s.route,"east_floor":s.east,"handed_over":s.passed}))
    }
    /// Both guests must reach safety to finish the hotel session.
    fn completion(&self, s: &State) -> Option<Completion> {
        (s.exited == [true; 2]).then_some(Completion {
            escaped: true,
            route: s.route,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pins both role records to the opening views produced before typed delivery.
    #[test]
    fn typed_opening_views_preserve_the_participant_and_record_formats() {
        let previous: [Value; 2] = serde_json::from_str(include_str!(
            "../tests/fixtures/rescue-supplies-seed-7.json"
        ))
        .unwrap();
        let state = State::deal(7, Route::RescueSupplies);
        let views = [PlayerRole::A, PlayerRole::B].map(|role| wire_observation(&state, role));
        assert_eq!(views, previous);
    }

    /// Projects through the same serializer as the participant and stored record.
    fn wire_observation(state: &State, role: PlayerRole) -> Value {
        serde_json::to_value(Hotel.observation(state, role)).unwrap()
    }

    /// Crossing reports affect only the authenticated role’s pose and leave the mechanics intact.
    #[test]
    fn crossing_pose_is_scoped_and_clears_on_arrival() {
        let pose = BridgePose {
            x: 10.8,
            z: -9.0,
            yaw: 0.25,
        };
        let report = Action::BridgePose { pose: Some(pose) };
        let mut hotel = State::deal(0, Route::Escape);
        assert!(Hotel.apply_action(&hotel, &report, PlayerRole::A).is_err());
        hotel.gate_open[0] = true;
        assert!(Hotel.apply_action(&hotel, &report, PlayerRole::B).is_err());

        let sample = Hotel.apply_action(&hotel, &report, PlayerRole::A).unwrap();
        assert_eq!(sample.bridge_poses, [Some(pose), None]);
        assert_eq!(
            wire_observation(&sample, PlayerRole::B)["partner_bridge_pose"],
            json!(pose)
        );
        assert!(wire_observation(&sample, PlayerRole::A)["partner_bridge_pose"].is_null());
        assert_eq!(sample.controls(0), vec![Action::ReachRefuge]);
        assert_eq!(sample.controls(1), hotel.controls(1));

        let invalid = Action::BridgePose {
            pose: Some(BridgePose {
                x: f64::NAN,
                ..pose
            }),
        };
        assert!(Hotel
            .apply_action(&sample, &invalid, PlayerRole::A)
            .is_err());
        let cleared = Hotel
            .apply_action(&sample, &Action::BridgePose { pose: None }, PlayerRole::A)
            .unwrap();
        assert_eq!(cleared, hotel);
        let arrived = Hotel
            .apply_action(&sample, &Action::ReachRefuge, PlayerRole::A)
            .unwrap();
        assert_eq!(arrived.bridge_poses, [None; 2]);
        assert!(Hotel
            .apply_action(&arrived, &report, PlayerRole::A)
            .is_err());
    }

    /// Drives with public load and brake operations and records an exact replay.
    fn drive(s: &mut State, destination: u8, log: &mut Vec<(usize, Action)>) {
        while s.east != destination {
            let heavy = usize::from(s.east < destination);
            while s.cargo[heavy] <= s.cargo[1 - heavy] {
                let (seat, a) = if s.cargo[heavy] < 3 {
                    (heavy, Action::Load)
                } else {
                    (1 - heavy, Action::Unload)
                };
                perform(s, seat, a, log);
            }
            let holder = usize::from(s.passed);
            perform(s, holder, Action::Release, log);
        }
    }

    /// Uses the public adapter and checks that offered controls include the action.
    fn perform(s: &mut State, seat: usize, a: Action, log: &mut Vec<(usize, Action)>) {
        let r = if seat == 0 {
            PlayerRole::A
        } else {
            PlayerRole::B
        };
        assert!(Hotel.available_actions(s, r).unwrap().contains(&a));
        let next = Hotel.apply_action(s, &a, r).unwrap();
        assert_eq!(next.route, s.route);
        for wing in 0..2 {
            let total = |state: &State| {
                let freight_on_rack =
                    wing == 0 && state.manifest.is_some() && !state.rescue_case_aboard;
                state.cargo[wing]
                    + state.landing_loads[wing].iter().sum::<u8>()
                    + u8::from(freight_on_rack)
            };
            assert_eq!(total(&next), total(s));
        }
        assert_eq!(
            Hotel.transition_metadata(s, &next, &a, r).unwrap()["east_floor"],
            next.east
        );
        *s = next;
        log.push((seat, a));
    }

    /// Solves each itinerary using exchanged entries, then replays every accepted
    /// action from the same seed and route. Both bridge positions must be reachable.
    #[test]
    fn solves_and_replays_seeded_itineraries() {
        for (config, route_name, requires_supplies) in [
            (json!({}), "escape", false),
            (json!({"route": "rescue_supplies"}), "rescue_supplies", true),
        ] {
            let config: Config = serde_json::from_value(config).unwrap();
            let secrets = Default::default();
            let mut destinations = std::collections::HashSet::new();
            for seed in 0..128 {
                let mut s = Hotel
                    .initial_state(GameInitializationContext {
                        config: &config,
                        seed,
                        secrets: &secrets,
                    })
                    .unwrap();
                let mut log = Vec::new();
                assert!(Hotel.completion(&s).is_none());
                let east = wire_observation(&s, PlayerRole::A);
                let west = wire_observation(&s, PlayerRole::B);
                assert_eq!(east["route"], route_name);
                assert_eq!(east["partner_delivery"].is_number(), requires_supplies);
                assert_eq!(west["partner_delivery"].is_number(), requires_supplies);
                let destination = west["partner_destination"].as_u64().unwrap() as u8;
                let own: Seal = serde_json::from_value(west["partner_seal"].clone()).unwrap();
                let peer: Seal = serde_json::from_value(east["partner_seal"].clone()).unwrap();
                destinations.insert(destination);
                drive(&mut s, 2, &mut log);
                perform(&mut s, 0, Action::Pass, &mut log);
                if requires_supplies {
                    drive(
                        &mut s,
                        east["freight_floor"].as_u64().unwrap() as u8,
                        &mut log,
                    );
                    if s.cargo[0] == CABIN_CAPACITY {
                        perform(&mut s, 0, Action::Unload, &mut log);
                    }
                    perform(&mut s, 0, Action::CollectRescueCase, &mut log);
                }
                drive(&mut s, destination, &mut log);
                for (seat, order) in [&west, &east].into_iter().enumerate() {
                    if let Some(n) = order["partner_delivery"].as_u64() {
                        while u64::from(s.cargo[seat]) != n {
                            let a = if u64::from(s.cargo[seat]) < n {
                                Action::Load
                            } else {
                                Action::Unload
                            };
                            perform(&mut s, seat, a, &mut log);
                        }
                    }
                }
                perform(&mut s, 1, Action::Exit { seal: peer }, &mut log);
                assert_eq!(s.controls(1), [Action::ReachRefuge]);
                assert!(!s.exited[1]);
                assert!(Hotel.completion(&s).is_none());
                perform(&mut s, 1, Action::ReachRefuge, &mut log);
                assert!(Hotel.completion(&s).is_none());
                perform(&mut s, 0, Action::Exit { seal: own }, &mut log);
                assert!(Hotel.completion(&s).is_none());
                perform(&mut s, 0, Action::ReachRefuge, &mut log);
                assert_eq!(
                    Hotel.completion(&s),
                    Some(Completion {
                        escaped: true,
                        route: config.route
                    })
                );
                assert!(s.controls(0).is_empty() && s.controls(1).is_empty());
                let mut replay = State::deal(seed, config.route);
                for (seat, a) in log {
                    replay = replay.step(a, seat).unwrap();
                }
                assert_eq!(s, replay);
            }
            assert_eq!(destinations, [3, 4].into_iter().collect());
        }
    }

    /// Checks that every legal stroke advances exactly one detent and preserves the load.
    #[test]
    fn only_an_unbalanced_release_can_move_the_cars() {
        for east in 0..=4 {
            for a in 0..=CABIN_CAPACITY {
                for b in 0..=CABIN_CAPACITY {
                    for passed in [false, true] {
                        let state = State {
                            east,
                            cargo: [a, b],
                            passed,
                            ..State::deal(7, Route::Escape)
                        };
                        let delta = (i16::from(b) - i16::from(a)).signum();
                        let target = i16::from(east) + delta;
                        let expected = (delta != 0 && (0..=4).contains(&target)).then_some(target);
                        for (seat, role) in [PlayerRole::A, PlayerRole::B].into_iter().enumerate() {
                            let local = expected.map(|n| if seat == 0 { n } else { 4 - n });
                            assert_eq!(
                                wire_observation(&state, role)["release_floor"],
                                json!(local)
                            );
                            let release = state.step(Action::Release, seat);
                            assert_eq!(
                                release.is_ok(),
                                expected.is_some() && seat == usize::from(passed)
                            );
                            if let Ok(settled) = release {
                                assert_eq!(
                                    settled,
                                    State {
                                        east: target as u8,
                                        ..state.clone()
                                    }
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    /// Depleting a rack persists between visits. Depositing a suitcase makes it
    /// available again to that guest alone.
    #[test]
    fn baggage_stays_on_its_own_landing_between_visits() {
        let mut s = State {
            east: 2,
            cargo: [0, 0],
            ..State::deal(93, Route::Escape)
        };
        let mut log = Vec::new();
        for _ in 0..CABIN_CAPACITY {
            perform(&mut s, 0, Action::Load, &mut log);
        }
        perform(&mut s, 0, Action::Release, &mut log);
        perform(&mut s, 0, Action::Unload, &mut log);
        for _ in 0..CABIN_CAPACITY {
            perform(&mut s, 1, Action::Load, &mut log);
        }
        perform(&mut s, 0, Action::Release, &mut log);
        assert_eq!(s.east, 2);
        assert_eq!(
            wire_observation(&s, PlayerRole::A)["landing_loads"],
            json!([3, 4, 0, 3, 3])
        );
        assert_eq!(
            wire_observation(&s, PlayerRole::B)["landing_loads"],
            json!([3, 3, 3, 0, 3])
        );
        assert!(s.step(Action::LoadRack { floor: 2 }, 0).is_err());
        assert!(s.step(Action::LoadRack { floor: 5 }, 0).is_err());
        perform(&mut s, 0, Action::LoadRack { floor: 4 }, &mut log);
        assert_eq!(s.landing_loads[0], [3, 4, 0, 3, 2]);
        assert_eq!(s.landing_loads[1], [3, 3, 3, 0, 3]);
        perform(&mut s, 0, Action::Unload, &mut log);
        assert_eq!(s.landing_loads[0][2], 1);
        perform(&mut s, 0, Action::Load, &mut log);
        assert_eq!(s.landing_loads[0][2], 0);
    }

    /// A full rack sends a suitcase to the closest rack with space, counting local floors.
    #[test]
    fn full_rack_dispatches_to_nearest_available_floor() {
        let mut s = State {
            east: 2,
            cargo: [1, 1],
            ..State::deal(93, Route::Escape)
        };
        s.landing_loads[1] = [0, 3, RACK_CAPACITY, 3, 3];
        let mut log = Vec::new();
        perform(&mut s, 1, Action::Unload, &mut log);
        assert_eq!(s.landing_loads[1], [0, 4, 6, 3, 3]);
        assert_eq!(s.landing_loads[0], [3; 5]);
        assert_eq!(s.cargo, [1, 0]);
    }

    /// Dispatch lets an empty end stop regain motion and a full destination rack
    /// accept a suitcase so both guests can meet their rescue supply manifest.
    #[test]
    fn dispatch_recovers_end_stop_and_delivery() {
        let mut s = State {
            east: 4,
            cargo: [2, 2],
            landing_loads: [[4, 4, 3, 3, 0], [0, 6, 3, 3, 2]],
            passed: true,
            destination: 3,
            manifest: Some([3, 1]),
            rescue_case_aboard: true,
            ..State::deal(93, Route::RescueSupplies)
        };
        let mut log = Vec::new();
        assert!(!s.controls(1).contains(&Action::Release));
        assert_eq!(wire_observation(&s, PlayerRole::A)["load_floor"], 3);
        perform(&mut s, 0, Action::Load, &mut log);
        perform(&mut s, 1, Action::Release, &mut log);
        assert_eq!(wire_observation(&s, PlayerRole::B)["load_floor"], 1);
        assert_eq!(wire_observation(&s, PlayerRole::B)["unload_floor"], 0);
        perform(&mut s, 1, Action::Unload, &mut log);
        assert_eq!(s.landing_loads[1][1], RACK_CAPACITY);
        assert_eq!(s.landing_loads[1][0], 1);
        for seat in 0..2 {
            let seal = s.seals[seat];
            perform(&mut s, seat, Action::Exit { seal }, &mut log);
            perform(&mut s, seat, Action::ReachRefuge, &mut log);
        }
        assert!(Hotel.completion(&s).is_some());
    }

    /// The marked shipment occupies a cabin slot and gates require freight plus docking.
    #[test]
    fn marked_case_is_collected_once_at_its_own_dock() {
        let mut state = State {
            east: 1,
            cargo: [1, 0],
            manifest: Some([2, 0]),
            ..State::deal(93, Route::RescueSupplies)
        };
        let blocked = |s: &State| !s.controls(0).contains(&Action::CollectRescueCase);
        assert!(blocked(&state));
        state.passed = true;
        assert!(!state.controls(1).contains(&Action::CollectRescueCase));
        let mut undocked = state.clone();
        undocked.east = 2;
        assert!(blocked(&undocked));
        let mut full = state.clone();
        full.cargo[0] = CABIN_CAPACITY;
        assert!(blocked(&full));
        let explorer = state.step(Action::StepOntoLanding, 1).unwrap();
        assert!(blocked(&explorer));
        assert!(!state.controls(0).contains(&Action::Exit {
            seal: state.seals[0]
        }));
        assert_eq!(
            wire_observation(&state, PlayerRole::A)["freight_status"],
            "awaiting_pickup"
        );
        assert_eq!(wire_observation(&state, PlayerRole::B)["freight_floor"], 3);
        let mut freight_incomplete = state.clone();
        freight_incomplete.east = state.destination;
        freight_incomplete.cargo[0] = 2;
        assert!(!freight_incomplete.controls(0).contains(&Action::Exit {
            seal: state.seals[0]
        }));

        let loaded = state.step(Action::CollectRescueCase, 0).unwrap();
        assert!(blocked(&loaded));
        assert_eq!(loaded.landing_loads, state.landing_loads);
        assert_eq!(loaded.cargo, [2, 0]);
        assert_eq!(
            wire_observation(&loaded, PlayerRole::B)["freight_status"],
            "ready"
        );
        assert!(!loaded.controls(0).contains(&Action::Exit {
            seal: state.seals[0]
        }));
        let lightened = loaded.step(Action::Unload, 0).unwrap();
        assert_eq!(lightened.cargo, [1, 0]);
        assert_eq!(
            wire_observation(&lightened, PlayerRole::A)["freight_status"],
            "aboard"
        );
        assert!(!lightened.controls(0).contains(&Action::Unload));
    }

    /// Exercises cable ownership, end stops, capacity, secret filtering, and the gate lock.
    #[test]
    fn interlocks_and_private_entries() {
        let mut s = State::deal(17, Route::Escape);
        s.east = 0;
        s.cargo = [3, 3];
        s.seals = [Seal::Stag, Seal::Rose];
        assert!(s.step(Action::Release, 0).is_err());
        assert!(s.step(Action::Load, 0).is_err());
        assert!(s.step(Action::Pass, 0).is_err());
        s.cargo = [3, 0];
        assert!(s.step(Action::Release, 0).is_err());
        assert!(s.step(Action::Unload, 1).is_err());
        s.cargo = [0, 1];
        assert!(s.step(Action::Release, 1).is_err());
        let settled = s.step(Action::Release, 0).unwrap();
        assert_eq!(settled.east, 1);
        assert!(settled.controls(0).contains(&Action::Release));
        assert!(settled.controls(1).contains(&Action::StepOntoLanding));
        s.east = 2;
        assert!(s.step(Action::Pass, 1).is_err());
        s = s.step(Action::Pass, 0).unwrap();
        assert!(s.step(Action::Release, 0).is_err());
        assert!(s.step(Action::Pass, 1).is_err());
        assert!(s.step(Action::Exit { seal: Seal::Stag }, 0).is_err());
        s.east = s.destination;
        let returned = s.step(Action::Exit { seal: Seal::Rose }, 0).unwrap();
        assert_eq!(returned.east, s.east);
        assert!(returned.lock_tripped[0]);
        assert_eq!(returned.cargo, s.cargo);
        assert_eq!(returned.landing_loads, s.landing_loads);
        assert!(!returned.gate_open[0]);

        let outside = s.step(Action::StepOntoLanding, 1).unwrap();
        let tripped = outside.step(Action::Exit { seal: Seal::Rose }, 0).unwrap();
        assert_eq!(tripped.east, outside.east);
        assert!(tripped.on_landing[1]);
        assert_eq!(tripped.cargo, s.cargo);
        assert_eq!(
            wire_observation(&tripped, PlayerRole::A)["lock_tripped"],
            true
        );
        assert_eq!(
            wire_observation(&tripped, PlayerRole::B)["lock_tripped"],
            false
        );
        assert!(tripped.step(Action::Exit { seal: Seal::Stag }, 0).is_err());
        assert!(tripped.step(Action::ReachRefuge, 0).is_err());
        assert_eq!(tripped.step(Action::ResetLock, 0).unwrap(), outside);
        let observed = wire_observation(&s, PlayerRole::A);
        assert_eq!(observed["partner_seal"], "rose");
        s.seals[0] = Seal::Key;
        assert_eq!(wire_observation(&s, PlayerRole::A), observed);
        assert_eq!(wire_observation(&s, PlayerRole::B)["partner_seal"], "key");
    }

    /// Parks both cars until each explorer has returned, regardless of key ownership.
    #[test]
    fn both_guests_must_board_before_using_the_cable_or_hatch() {
        for passed in [false, true] {
            let state = State {
                east: 2,
                cargo: [0, 1],
                passed,
                ..State::deal(17, Route::Escape)
            };
            let holder = usize::from(passed);
            let peer = 1 - holder;
            assert!(state.step(Action::BoardLift, holder).is_err());
            let outside = state.step(Action::StepOntoLanding, peer).unwrap();
            assert!(outside.step(Action::Release, holder).is_err());
            assert!(outside.step(Action::Pass, holder).is_err());
            assert_eq!(outside.controls(peer), [Action::BoardLift]);
            let both_outside = outside.step(Action::StepOntoLanding, holder).unwrap();
            assert_eq!(both_outside.controls(holder), [Action::BoardLift]);
            let aboard = both_outside.step(Action::BoardLift, peer).unwrap();
            assert!(aboard.step(Action::Release, holder).is_err());
            let aboard = aboard.step(Action::BoardLift, holder).unwrap();
            assert_eq!(aboard, state);
            if !passed {
                assert!(aboard.step(Action::Pass, holder).is_ok());
            }
            let settled = aboard.step(Action::Release, holder).unwrap();
            assert!(settled.step(Action::StepOntoLanding, holder).is_ok());
            assert!(settled.step(Action::StepOntoLanding, peer).is_ok());
        }
    }

    /// Landing exploration is independent of the two gate crossings and their completion.
    #[test]
    fn explorer_can_reboard_after_the_other_guest_crosses() {
        let mut state = State::deal(17, Route::Escape);
        state.east = state.destination;
        state.passed = true;
        let [east_seal, west_seal] = state.seals;
        let state = state.step(Action::StepOntoLanding, 1).unwrap();
        let east_view = wire_observation(&state, PlayerRole::A);
        let west_view = wire_observation(&state, PlayerRole::B);
        assert_eq!(east_view["on_landing"], false);
        assert_eq!(east_view["partner_on_landing"], true);
        assert_eq!(west_view["on_landing"], true);
        assert_eq!(west_view["partner_on_landing"], false);
        let west_exit = Action::Exit { seal: west_seal };
        assert!(state.step(west_exit, 1).is_err());
        let open = state.step(Action::Exit { seal: east_seal }, 0).unwrap();
        assert!(Hotel.completion(&open).is_none());
        assert_eq!(
            wire_observation(&open, PlayerRole::B)["partner_gate_open"],
            true
        );
        let crossed = open.step(Action::ReachRefuge, 0).unwrap();
        assert!(Hotel.completion(&crossed).is_none());
        assert!(crossed.controls(0).is_empty());
        let aboard = crossed.step(Action::BoardLift, 1).unwrap();
        assert!(aboard.step(Action::Release, 1).is_err());
        let wrong_seal = SEALS.into_iter().find(|seal| *seal != west_seal).unwrap();
        let tripped = aboard.step(Action::Exit { seal: wrong_seal }, 1).unwrap();
        assert_eq!(tripped.east, aboard.east);
        assert!(tripped.exited[0]);
        assert!(tripped.step(west_exit, 1).is_err());
        let finished = tripped
            .step(Action::ResetLock, 1)
            .unwrap()
            .step(west_exit, 1)
            .unwrap()
            .step(Action::ReachRefuge, 1)
            .unwrap();
        assert!(Hotel.completion(&finished).is_some());
    }

    /// Keeps the wire names aligned with the controls used by the client.
    #[test]
    fn action_wire_names() {
        for (action, name) in [
            (Action::StepOntoLanding, "step_onto_landing"),
            (Action::BoardLift, "board_lift"),
            (Action::CollectRescueCase, "collect_rescue_case"),
            (Action::ResetLock, "reset_lock"),
            (Action::ReachRefuge, "reach_refuge"),
        ] {
            assert_eq!(serde_json::to_value(action).unwrap(), json!({"type": name}));
        }
        assert_eq!(
            serde_json::to_value(Action::LoadRack { floor: 2 }).unwrap(),
            json!({"type": "load_rack", "floor": 2})
        );
    }
}
