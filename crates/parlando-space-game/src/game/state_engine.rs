use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

use super::level::{
    devices_at_position, door_kind_for_step, is_walkable, room_at_position, room_center,
    room_exits, Position,
};

/// Typed Space Game action accepted by the reusable Parlando server boundary.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type")]
pub enum SpaceAction {
    #[serde(rename = "moveStep")]
    MoveStep { player: String, direction: String },
    #[serde(rename = "move")]
    Move { player: String, room: String },
    #[serde(rename = "reset")]
    Reset { player: Option<String> },
    #[serde(rename = "toggleFuse")]
    ToggleFuse { player: String, color: String },
    #[serde(rename = "toggleBreaker")]
    ToggleBreaker { player: String, breaker: String },
    #[serde(rename = "setValve")]
    SetValve {
        player: String,
        valve: String,
        open: bool,
    },
    #[serde(rename = "holdOverride")]
    HoldOverride { player: String, held: bool },
    #[serde(rename = "togglePlate")]
    TogglePlate { player: String },
    #[serde(rename = "chargeBattery")]
    ChargeBattery { player: String },
    #[serde(rename = "moveBattery")]
    MoveBattery { player: String },
    #[serde(rename = "setRelay")]
    SetRelay { player: String, mode: String },
    #[serde(rename = "cycleRelay")]
    CycleRelay { player: String },
    #[serde(rename = "runDiagnostic")]
    RunDiagnostic { player: String },
    #[serde(rename = "launchBeacon")]
    LaunchBeacon { player: String },
}

impl SpaceAction {
    /// Returns the player encoded in this action, when the action is player-scoped.
    pub fn player(&self) -> Option<&str> {
        match self {
            Self::MoveStep { player, .. }
            | Self::Move { player, .. }
            | Self::ToggleFuse { player, .. }
            | Self::ToggleBreaker { player, .. }
            | Self::SetValve { player, .. }
            | Self::HoldOverride { player, .. }
            | Self::TogglePlate { player }
            | Self::ChargeBattery { player }
            | Self::MoveBattery { player }
            | Self::SetRelay { player, .. }
            | Self::CycleRelay { player }
            | Self::RunDiagnostic { player }
            | Self::LaunchBeacon { player } => Some(player),
            Self::Reset { player } => player.as_deref(),
        }
    }

    /// Returns the client protocol action type string for this typed action.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::MoveStep { .. } => "moveStep",
            Self::Move { .. } => "move",
            Self::Reset { .. } => "reset",
            Self::ToggleFuse { .. } => "toggleFuse",
            Self::ToggleBreaker { .. } => "toggleBreaker",
            Self::SetValve { .. } => "setValve",
            Self::HoldOverride { .. } => "holdOverride",
            Self::TogglePlate { .. } => "togglePlate",
            Self::ChargeBattery { .. } => "chargeBattery",
            Self::MoveBattery { .. } => "moveBattery",
            Self::SetRelay { .. } => "setRelay",
            Self::CycleRelay { .. } => "cycleRelay",
            Self::RunDiagnostic { .. } => "runDiagnostic",
            Self::LaunchBeacon { .. } => "launchBeacon",
        }
    }
}

/// Complete Space Game state as exchanged with the existing TypeScript client.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SpaceGameState {
    pub players: Players,
    pub fuses: Fuses,
    pub breakers: Breakers,
    pub valves: Valves,
    #[serde(rename = "overrideHeld")]
    pub override_held: bool,
    pub battery: Battery,
    pub relay: String,
    #[serde(rename = "pressureDrained")]
    pub pressure_drained: bool,
    #[serde(rename = "oxygenFanTripped")]
    pub oxygen_fan_tripped: bool,
    pub knowledge: Knowledge,
    #[serde(rename = "visualEffects")]
    pub visual_effects: Vec<String>,
    #[serde(rename = "beaconLaunched")]
    pub beacon_launched: bool,
    #[serde(rename = "moveCount")]
    pub move_count: i64,
}

/// Session-local player states keyed by Space Game role.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Players {
    #[serde(rename = "A")]
    pub a: PlayerState,
    #[serde(rename = "B")]
    pub b: PlayerState,
}

/// Per-player position and room occupancy.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PlayerState {
    pub room: String,
    pub position: Position,
    #[serde(rename = "plateHeld")]
    pub plate_held: bool,
}

/// Fuse board state in the power room.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Fuses {
    pub blue: bool,
    pub yellow: bool,
    pub red: bool,
}

/// Breaker state in the power room.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Breakers {
    pub main: bool,
    pub aux: bool,
}

/// Valve state in the valve room.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Valves {
    #[serde(rename = "A")]
    pub a: bool,
    #[serde(rename = "C")]
    pub c: bool,
    pub floodgate: bool,
}

/// Battery sled state across charger and signal-array locations.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Battery {
    pub location: String,
    pub charged: bool,
    pub spent: bool,
}

/// Role-private facts discovered while playing the Space Game.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Knowledge {
    #[serde(rename = "A")]
    pub a: Vec<String>,
    #[serde(rename = "B")]
    pub b: Vec<String>,
}

/// Observation knowledge with the other player's private facts filtered out.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FilteredKnowledge {
    #[serde(rename = "A")]
    pub a: Vec<String>,
    #[serde(rename = "B")]
    pub b: Vec<String>,
}

/// Derived system booleans shown by the client and used for launch prerequisites.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Systems {
    pub pump_powered: bool,
    pub charger_fed: bool,
    pub cooling_restored: bool,
    pub oxygen_stable: bool,
    pub power_stable: bool,
    pub door_access: bool,
    pub signal_routed: bool,
    pub battery_ready: bool,
    pub ready_to_launch: bool,
}

/// Player-specific observation sent to one participant.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SpaceObservation {
    pub players: Players,
    pub fuses: Fuses,
    pub breakers: Breakers,
    pub valves: Valves,
    #[serde(rename = "overrideHeld")]
    pub override_held: bool,
    pub battery: Battery,
    pub relay: String,
    #[serde(rename = "pressureDrained")]
    pub pressure_drained: bool,
    #[serde(rename = "oxygenFanTripped")]
    pub oxygen_fan_tripped: bool,
    #[serde(rename = "visualEffects")]
    pub visual_effects: Vec<String>,
    #[serde(rename = "beaconLaunched")]
    pub beacon_launched: bool,
    #[serde(rename = "moveCount")]
    pub move_count: i64,
    pub role: String,
    pub systems: Systems,
    pub knowledge: FilteredKnowledge,
    #[serde(rename = "privateKnowledge")]
    pub private_knowledge: Vec<String>,
    pub log: Vec<String>,
}

/// Human-readable game event emitted for one recipient after an action.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SpaceEvent {
    #[serde(rename = "type")]
    pub event_type: String,
    pub actor: Option<String>,
    pub move_count: i64,
    pub text: String,
}

/// Completion summary persisted and sent when the beacon launches.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SpaceSummary {
    #[serde(rename = "beaconLaunched")]
    pub beacon_launched: bool,
    #[serde(rename = "moveCount")]
    pub move_count: i64,
    pub systems: Systems,
}

/// Builds the initial Space Game state used for every new session.
pub fn initial_state() -> SpaceGameState {
    SpaceGameState {
        players: Players {
            a: PlayerState {
                room: "power".to_string(),
                position: Position { x: 2, y: 2 },
                plate_held: false,
            },
            b: PlayerState {
                room: "valve".to_string(),
                position: Position { x: 9, y: 6 },
                plate_held: false,
            },
        },
        fuses: Fuses {
            blue: false,
            yellow: false,
            red: false,
        },
        breakers: Breakers {
            main: false,
            aux: false,
        },
        valves: Valves {
            a: false,
            c: false,
            floodgate: false,
        },
        override_held: false,
        battery: Battery {
            location: "charger".to_string(),
            charged: false,
            spent: false,
        },
        relay: "bypass".to_string(),
        pressure_drained: false,
        oxygen_fan_tripped: false,
        knowledge: Knowledge {
            a: vec![
                "Blue fuse wakes the bus feed.".to_string(),
                "Yellow fuse is marked PUMP, but the aux breaker warning is scratched.".to_string(),
            ],
            b: vec![
                "Valve A is drawn beside cabin pressure.".to_string(),
                "Valve C is drawn beside charger water and coolant, depending on floodgate state."
                    .to_string(),
            ],
        },
        visual_effects: vec![],
        beacon_launched: false,
        move_count: 0,
    }
}

/// Computes derived system readiness from the stored game state.
pub fn derive_systems(state: &SpaceGameState) -> Systems {
    let pump_powered = state.fuses.yellow && state.breakers.main && !state.oxygen_fan_tripped;
    let power_stable = state.fuses.blue && state.breakers.main && !state.battery.spent;
    let charger_fed = power_stable && pump_powered && state.valves.c && !state.valves.floodgate;
    let cooling_restored = state.valves.floodgate && !state.valves.c;
    let oxygen_stable =
        pump_powered && state.valves.a && !state.pressure_drained && !state.oxygen_fan_tripped;
    let door_motor_powered =
        power_stable && (state.battery.location == "charger" || state.override_held);
    let door_access = door_motor_powered
        && oxygen_stable
        && (state.players.a.plate_held || state.players.b.plate_held);
    let signal_routed = state.relay == "array"
        && state.battery.location == "signal"
        && state.battery.charged
        && !state.battery.spent
        && cooling_restored;
    Systems {
        pump_powered,
        charger_fed,
        cooling_restored,
        oxygen_stable,
        power_stable,
        door_access,
        signal_routed,
        battery_ready: state.battery.charged && !state.battery.spent,
        ready_to_launch: power_stable && oxygen_stable && door_access && signal_routed,
    }
}

/// Returns typed actions currently available to the given player.
pub fn available_actions(state: &SpaceGameState, player: &str) -> Vec<SpaceAction> {
    let position = player_state(state, player).position;
    let mut actions = vec![];
    for device in devices_at_position(position, &state.battery.location) {
        match device.id {
            "fuse-blue" => actions.push(SpaceAction::ToggleFuse {
                player: player.to_string(),
                color: "blue".to_string(),
            }),
            "fuse-yellow" => actions.push(SpaceAction::ToggleFuse {
                player: player.to_string(),
                color: "yellow".to_string(),
            }),
            "fuse-red" => actions.push(SpaceAction::ToggleFuse {
                player: player.to_string(),
                color: "red".to_string(),
            }),
            "breaker-main" => actions.push(SpaceAction::ToggleBreaker {
                player: player.to_string(),
                breaker: "main".to_string(),
            }),
            "breaker-aux" => actions.push(SpaceAction::ToggleBreaker {
                player: player.to_string(),
                breaker: "aux".to_string(),
            }),
            "bypass" => actions.push(SpaceAction::HoldOverride {
                player: player.to_string(),
                held: !state.override_held,
            }),
            "valve-a" => actions.push(SpaceAction::SetValve {
                player: player.to_string(),
                valve: "A".to_string(),
                open: !state.valves.a,
            }),
            "valve-c" => actions.push(SpaceAction::SetValve {
                player: player.to_string(),
                valve: "C".to_string(),
                open: !state.valves.c,
            }),
            "valve-floodgate" => actions.push(SpaceAction::SetValve {
                player: player.to_string(),
                valve: "floodgate".to_string(),
                open: !state.valves.floodgate,
            }),
            "charger" => actions.push(SpaceAction::ChargeBattery {
                player: player.to_string(),
            }),
            "battery" => actions.push(SpaceAction::MoveBattery {
                player: player.to_string(),
            }),
            "relay" => actions.push(SpaceAction::CycleRelay {
                player: player.to_string(),
            }),
            "beacon" => actions.push(SpaceAction::LaunchBeacon {
                player: player.to_string(),
            }),
            _ => {}
        }
    }
    actions
}

/// Validates a typed action against the current state and player role.
pub fn validate_action(state: &SpaceGameState, action: &SpaceAction, player: &str) -> Result<()> {
    if action.player() != Some(player) {
        bail!(
            "Cannot submit an action for Player {} as Player {player}.",
            action.player().unwrap_or("?")
        );
    }
    if matches!(
        action,
        SpaceAction::MoveStep { .. } | SpaceAction::Move { .. } | SpaceAction::Reset { .. }
    ) {
        return Ok(());
    }
    if !available_actions(state, player).contains(action) {
        bail!("Action is not available at the player's current position.");
    }
    Ok(())
}

/// Applies one typed action and returns the next immutable game state.
pub fn apply_action(state: &SpaceGameState, action: &SpaceAction) -> Result<SpaceGameState> {
    if matches!(action, SpaceAction::Reset { .. }) {
        return Ok(initial_state());
    }
    let before = derive_systems(state);
    let mut next = state.clone();
    let mut effects = vec![];
    match action {
        SpaceAction::MoveStep { player, direction } => {
            apply_move_step(&mut next, &before, player, direction)
        }
        SpaceAction::Move { player, room } => apply_move(&mut next, &before, player, room),
        SpaceAction::ToggleFuse { player, color } => {
            toggle_fuse(&mut next, player, color, &mut effects)
        }
        SpaceAction::ToggleBreaker { breaker, .. } => {
            toggle_breaker(&mut next, breaker, &mut effects)
        }
        SpaceAction::SetValve { .. } => set_valve(&mut next, &before, action, &mut effects),
        SpaceAction::HoldOverride { player, held } => {
            next.override_held = *held;
            effects.extend(["device:bypass".to_string(), "room:junction".to_string()]);
            if next.override_held {
                reveal(
                    &mut next,
                    player,
                    "The bypass keeps the door motor powered during battery transfer.",
                );
            }
        }
        SpaceAction::ChargeBattery { player } => charge_battery(&mut next, &before, player),
        SpaceAction::MoveBattery { player } => move_battery(&mut next, player),
        SpaceAction::SetRelay { mode, .. } => set_relay(&mut next, &before, mode, &mut effects),
        SpaceAction::CycleRelay { player: _ } => {
            let mode = next_relay(&next.relay).to_string();
            set_relay(&mut next, &before, &mode, &mut effects);
        }
        SpaceAction::RunDiagnostic { player } => {
            let diagnostic = diagnostic_for(&next, player);
            reveal(&mut next, player, diagnostic);
        }
        SpaceAction::LaunchBeacon { player } => launch_beacon(&mut next, &before, player),
        SpaceAction::TogglePlate { .. } | SpaceAction::Reset { .. } => {}
    }
    Ok(finalize(next, before, effects))
}

// Applies one grid step, including door and pressure-gate handling.
fn apply_move_step(state: &mut SpaceGameState, before: &Systems, player: &str, direction: &str) {
    let current = player_state(state, player).position;
    let target = step(current, direction);
    let Some(target_room) = room_at_position(target) else {
        return;
    };
    let current_room = player_state(state, player).room.clone();
    if current_room != target_room && door_kind_for_step(current, target).is_none() {
        return;
    }
    if target_room == "airlock" && current_room != "airlock" && !before.door_access {
        reveal(
            state,
            player,
            "The airlock hatch needs oxygen pressure, door power, and the floor plate.",
        );
    } else if is_walkable(target) {
        let player_state = player_state_mut(state, player);
        player_state.position = target;
        player_state.room = target_room.to_string();
        player_state.plate_held = target.x == 7 && target.y == 2;
    }
}

// Moves directly between adjacent named rooms for legacy/client compatibility.
fn apply_move(state: &mut SpaceGameState, before: &Systems, player: &str, room: &str) {
    let current_room = player_state(state, player).room.clone();
    if room_exits(&current_room).contains(&room) && (room != "airlock" || before.door_access) {
        let player_state = player_state_mut(state, player);
        player_state.room = room.to_string();
        player_state.position = room_center(room);
    } else if room == "airlock" {
        reveal(
            state,
            player,
            "The airlock hatch needs oxygen pressure, door power, and the floor plate.",
        );
    }
}

// Toggles one fuse and records the system consequences of unsafe ordering.
fn toggle_fuse(state: &mut SpaceGameState, player: &str, color: &str, effects: &mut Vec<String>) {
    match color {
        "yellow" => state.fuses.yellow = !state.fuses.yellow,
        "red" => state.fuses.red = !state.fuses.red,
        _ => state.fuses.blue = !state.fuses.blue,
    }
    effects.extend([format!("device:fuse-{color}"), "room:power".to_string()]);
    if color == "yellow" && state.fuses.yellow && state.breakers.aux {
        state.oxygen_fan_tripped = true;
        reveal(
            state,
            "A",
            "Yellow pump surge trips the oxygen fan when AUX is already live.",
        );
        reveal(
            state,
            "B",
            "The fan trip is electrical, not a valve failure.",
        );
    }
    if color == "red" && state.fuses.red {
        reveal(
            state,
            player,
            "The red fuse only feeds the reserve lamps in this prototype.",
        );
    }
}

// Toggles one breaker and records battery/fan side effects.
fn toggle_breaker(state: &mut SpaceGameState, breaker: &str, effects: &mut Vec<String>) {
    if breaker == "aux" {
        state.breakers.aux = !state.breakers.aux;
    } else {
        state.breakers.main = !state.breakers.main;
    }
    effects.extend([
        format!("device:breaker-{breaker}"),
        "room:power".to_string(),
    ]);
    if breaker == "aux" && state.breakers.aux && state.fuses.yellow {
        state.oxygen_fan_tripped = true;
        reveal(
            state,
            "A",
            "AUX and the yellow pump fuse together trip the fan relay.",
        );
        reveal(
            state,
            "B",
            "If oxygen dies while valves look right, ask about the aux breaker.",
        );
    }
    if breaker == "main" && !state.breakers.main && state.battery.location != "signal" {
        state.battery.charged = false;
    }
}

// Applies a valve change and records pressure/cooling side effects.
fn set_valve(
    state: &mut SpaceGameState,
    before: &Systems,
    action: &SpaceAction,
    effects: &mut Vec<String>,
) {
    let SpaceAction::SetValve { valve, open, .. } = action else {
        return;
    };
    match valve.as_str() {
        "C" => state.valves.c = *open,
        "floodgate" => state.valves.floodgate = *open,
        _ => state.valves.a = *open,
    }
    effects.extend([
        format!("device:valve-{}", valve.to_lowercase()),
        "room:valve".to_string(),
    ]);
    if valve == "floodgate" && *open && before.oxygen_stable {
        state.pressure_drained = true;
        reveal(
            state,
            "B",
            "Opening the floodgate restores coolant but drains cabin pressure first.",
        );
    } else if valve == "A" && *open {
        state.pressure_drained = false;
        state.oxygen_fan_tripped = false;
    } else if valve == "C" && !*open {
        reveal(
            state,
            "A",
            "Valve C closed starves the charger but helps cooling after floodgate opens.",
        );
    }
}

// Attempts to charge the battery, revealing diagnostics when prerequisites are missing.
fn charge_battery(state: &mut SpaceGameState, before: &Systems, player: &str) {
    if before.charger_fed && state.battery.location == "charger" {
        state.battery.charged = true;
        state.battery.spent = false;
        reveal(
            state,
            "A",
            "The charger needs blue bus, yellow pump, main breaker, and valve C water.",
        );
        reveal(
            state,
            "B",
            "Floodgate open steals the water return from the charger.",
        );
    } else {
        let diagnostic = diagnostic_for(state, player);
        reveal(state, player, diagnostic);
    }
}

// Transfers the battery between the charger and the signal array.
fn move_battery(state: &mut SpaceGameState, player: &str) {
    if state.battery.location == "charger" {
        if state.battery.charged {
            state.battery.location = "signal".to_string();
            if !state.override_held {
                reveal(
                    state,
                    "B",
                    "Battery transfer removes the door motor supply unless the bypass is held.",
                );
            }
        } else {
            reveal(
                state,
                player,
                "Charge the battery before moving it to the signal array.",
            );
        }
    } else {
        state.battery.location = "charger".to_string();
        state.battery.spent = false;
    }
}

// Points the signal relay and reveals early ARRAY-mode diagnostics.
fn set_relay(state: &mut SpaceGameState, before: &Systems, mode: &str, effects: &mut Vec<String>) {
    state.relay = mode.to_string();
    effects.extend(["device:relay".to_string(), "room:signal".to_string()]);
    if state.relay == "array" && !before.signal_routed {
        reveal(
            state,
            "A",
            "ARRAY mode should wait until charged battery and coolant are both ready.",
        );
        reveal(
            state,
            "B",
            "A loop fault means relay mode was changed before the physical path was ready.",
        );
    }
}

// Attempts the final beacon launch and handles early-launch battery drain.
fn launch_beacon(state: &mut SpaceGameState, before: &Systems, player: &str) {
    if before.ready_to_launch {
        state.beacon_launched = true;
    } else if state.battery.location == "signal" && state.battery.charged {
        state.battery.spent = true;
        reveal(
            state,
            player,
            "Early launch attempts consume the signal battery pulse.",
        );
    }
}

// Applies shared post-action bookkeeping such as visual effects and move count.
fn finalize(
    mut state: SpaceGameState,
    before: Systems,
    mut effects: Vec<String>,
) -> SpaceGameState {
    let after = derive_systems(&state);
    for (changed, effect) in [
        (before.power_stable != after.power_stable, "room:power"),
        (before.oxygen_stable != after.oxygen_stable, "room:oxygen"),
        (before.door_access != after.door_access, "room:airlock"),
        (before.signal_routed != after.signal_routed, "room:signal"),
        (
            before.cooling_restored != after.cooling_restored,
            "room:valve",
        ),
    ] {
        if changed {
            effects.push(effect.to_string());
        }
    }
    state.move_count += 1;
    state.visual_effects = effects;
    state
}

// Adds a role-private knowledge item once.
fn reveal(state: &mut SpaceGameState, player: &str, text: &str) {
    let items = if player == "B" {
        &mut state.knowledge.b
    } else {
        &mut state.knowledge.a
    };
    if !items.iter().any(|item| item == text) {
        items.push(text.to_string());
    }
}

// Chooses the current diagnostic report for a player from derived system state.
fn diagnostic_for(state: &SpaceGameState, player: &str) -> &'static str {
    let systems = derive_systems(state);
    if !systems.power_stable {
        if player == "A" {
            "Bus feed dark: blue fuse and MAIN breaker must both be live."
        } else {
            "Node Lantern has no stable feed; ask for the blue bus and main line."
        }
    } else if state.oxygen_fan_tripped {
        if player == "A" {
            "Fan relay tripped by AUX backfeed through yellow pump leg."
        } else {
            "Oxygen fault is electrical; valve positions are not the first problem."
        }
    } else if !systems.pump_powered {
        if player == "A" {
            "Pump leg idle: yellow fuse needs MAIN, but not AUX surge."
        } else {
            "Cabin pressure cannot refill until the pump leg is live."
        }
    } else if !systems.charger_fed && state.battery.location == "charger" && !state.battery.charged
    {
        if player == "A" {
            "Charger starved: check yellow pump and valve C water return."
        } else {
            "Water return is missing; floodgate and valve C fight over the charger."
        }
    } else if !systems.oxygen_stable {
        if player == "A" {
            "Pressure is not holding; valve A must refill after any floodgate drain."
        } else {
            "Cabin pressure drained through the flood path; use valve A to refill."
        }
    } else if !systems.door_access {
        if player == "A" {
            "Door motor wants bypass during battery transfer and a latch plate signal."
        } else {
            "The hatch needs oxygen pressure, motor feed, and someone on the plate."
        }
    } else if !systems.signal_routed {
        if player == "A" {
            "Signal path missing: battery at array, relay ARRAY, and coolant restored."
        } else {
            "Relay map is not locked: coolant plus charged array battery required."
        }
    } else {
        "All launch prerequisites are green at this instant."
    }
}

// Computes a one-cell grid step for movement actions.
fn step(position: Position, direction: &str) -> Position {
    let (dx, dy) = match direction {
        "down" => (0, 1),
        "left" => (-1, 0),
        "right" => (1, 0),
        _ => (0, -1),
    };
    Position {
        x: position.x + dx,
        y: position.y + dy,
    }
}

// Advances the relay through its client-visible cycle.
fn next_relay(relay: &str) -> &'static str {
    match relay {
        "bypass" => "loop",
        "loop" => "array",
        _ => "bypass",
    }
}

// Returns the immutable player state for a role, defaulting to A for legacy input.
fn player_state<'a>(state: &'a SpaceGameState, player: &str) -> &'a PlayerState {
    if player == "B" {
        &state.players.b
    } else {
        &state.players.a
    }
}

// Returns the mutable player state for a role, defaulting to A for legacy input.
fn player_state_mut<'a>(state: &'a mut SpaceGameState, player: &str) -> &'a mut PlayerState {
    if player == "B" {
        &mut state.players.b
    } else {
        &mut state.players.a
    }
}

#[cfg(test)]
mod tests {
    use parlando_server::{GameAdapter, PlayerRole};
    use serde_json::json;

    use crate::game::adapter::SpaceGameAdapter;

    use super::*;

    fn apply(actions: Vec<SpaceAction>) -> SpaceGameState {
        let mut state = initial_state();
        for action in actions {
            state = apply_action(&state, &action).unwrap();
        }
        state
    }

    fn holding_plate(mut state: SpaceGameState) -> SpaceGameState {
        state.players.b.position = Position { x: 7, y: 2 };
        state.players.b.room = "junction".to_string();
        state.players.b.plate_held = true;
        state
    }

    #[test]
    fn requires_all_final_systems_to_launch() {
        let ready = apply(vec![
            SpaceAction::ToggleFuse {
                player: "A".to_string(),
                color: "blue".to_string(),
            },
            SpaceAction::ToggleBreaker {
                player: "A".to_string(),
                breaker: "main".to_string(),
            },
            SpaceAction::ToggleFuse {
                player: "A".to_string(),
                color: "yellow".to_string(),
            },
            SpaceAction::SetValve {
                player: "B".to_string(),
                valve: "C".to_string(),
                open: true,
            },
            SpaceAction::SetValve {
                player: "B".to_string(),
                valve: "A".to_string(),
                open: true,
            },
            SpaceAction::ChargeBattery {
                player: "A".to_string(),
            },
            SpaceAction::HoldOverride {
                player: "A".to_string(),
                held: true,
            },
            SpaceAction::MoveBattery {
                player: "A".to_string(),
            },
            SpaceAction::SetValve {
                player: "B".to_string(),
                valve: "C".to_string(),
                open: false,
            },
            SpaceAction::SetValve {
                player: "B".to_string(),
                valve: "floodgate".to_string(),
                open: true,
            },
            SpaceAction::SetValve {
                player: "B".to_string(),
                valve: "A".to_string(),
                open: true,
            },
            SpaceAction::SetRelay {
                player: "B".to_string(),
                mode: "array".to_string(),
            },
        ]);
        let ready = holding_plate(ready);
        assert!(derive_systems(&ready).ready_to_launch);
        let launched = apply_action(
            &ready,
            &SpaceAction::LaunchBeacon {
                player: "B".to_string(),
            },
        )
        .unwrap();
        assert!(launched.beacon_launched);
    }

    #[test]
    fn aux_and_yellow_fuse_order_trips_oxygen_fan() {
        let state = apply(vec![
            SpaceAction::ToggleBreaker {
                player: "A".to_string(),
                breaker: "aux".to_string(),
            },
            SpaceAction::ToggleFuse {
                player: "A".to_string(),
                color: "yellow".to_string(),
            },
            SpaceAction::ToggleFuse {
                player: "A".to_string(),
                color: "blue".to_string(),
            },
            SpaceAction::ToggleBreaker {
                player: "A".to_string(),
                breaker: "main".to_string(),
            },
        ]);
        assert!(state.oxygen_fan_tripped);
        assert!(!derive_systems(&state).oxygen_stable);
    }

    #[test]
    fn observation_filters_other_player_knowledge() {
        let adapter = SpaceGameAdapter::new();
        let state = initial_state();
        let observation_a = adapter.observe_state(&state, PlayerRole::A);
        let observation_b = adapter.observe_state(&state, PlayerRole::B);
        assert_eq!(observation_a.private_knowledge, state.knowledge.a);
        assert!(observation_a.knowledge.b.is_empty());
        assert_eq!(observation_b.private_knowledge, state.knowledge.b);
        assert!(observation_b.knowledge.a.is_empty());
    }

    #[test]
    fn adapter_exposes_typed_available_actions_with_client_json_shape() {
        let adapter = SpaceGameAdapter::new();
        let mut state = initial_state();
        state.players.a.position = Position { x: 1, y: 1 };
        let actions = adapter.available_actions(&state, PlayerRole::A);
        let expected = SpaceAction::ToggleFuse {
            player: "A".to_string(),
            color: "blue".to_string(),
        };
        assert!(actions.contains(&expected));
        assert_eq!(
            serde_json::to_value(expected).unwrap(),
            json!({"type": "toggleFuse", "player": "A", "color": "blue"})
        );
    }

    #[test]
    fn state_serializes_with_client_compatible_field_names() {
        let state = initial_state();
        let value = serde_json::to_value(state).unwrap();

        assert_eq!(value["players"]["A"]["room"], "power");
        assert_eq!(value["players"]["B"]["position"], json!({"x": 9, "y": 6}));
        assert_eq!(value["overrideHeld"], false);
        assert_eq!(value["oxygenFanTripped"], false);
        assert_eq!(value["beaconLaunched"], false);
        assert_eq!(value["moveCount"], 0);
        assert!(value.get("override_held").is_none());
        assert!(value.get("move_count").is_none());
    }

    #[test]
    fn adapter_events_include_action_and_recipient_knowledge_delta() {
        let adapter = SpaceGameAdapter::new();
        let before = initial_state();
        let after = apply_action(
            &before,
            &SpaceAction::ChargeBattery {
                player: "A".to_string(),
            },
        )
        .unwrap();

        let events_a = adapter.events_for_action(
            &before,
            &after,
            &SpaceAction::ChargeBattery {
                player: "A".to_string(),
            },
            PlayerRole::A,
        );
        let events_b = adapter.events_for_action(
            &before,
            &after,
            &SpaceAction::ChargeBattery {
                player: "A".to_string(),
            },
            PlayerRole::B,
        );

        assert!(events_a
            .iter()
            .any(|event| event.event_type == "action" && event.text.starts_with("You ")));
        assert!(events_b
            .iter()
            .any(|event| event.event_type == "action" && event.text.starts_with("Player A ")));
        assert!(events_a.iter().any(|event| event.event_type == "knowledge"));
        assert!(!events_b.iter().any(|event| event.event_type == "knowledge"));
    }

    #[test]
    fn early_array_relay_reveals_python_compatible_diagnostics() {
        let state = apply(vec![SpaceAction::SetRelay {
            player: "B".to_string(),
            mode: "array".to_string(),
        }]);

        assert!(state.knowledge.a.contains(
            &"ARRAY mode should wait until charged battery and coolant are both ready.".to_string()
        ));
        assert!(state.knowledge.b.contains(
            &"A loop fault means relay mode was changed before the physical path was ready."
                .to_string()
        ));
        assert_eq!(state.visual_effects, vec!["device:relay", "room:signal"]);
    }

    #[test]
    fn completion_summary_serializes_with_client_shape() {
        let adapter = SpaceGameAdapter::new();
        let state = initial_state();
        let summary = adapter.completion_summary(&state);
        let value = serde_json::to_value(summary).unwrap();

        assert_eq!(value["beaconLaunched"], false);
        assert_eq!(value["moveCount"], 0);
        assert_eq!(value["systems"]["readyToLaunch"], false);
        assert!(value.get("beacon_launched").is_none());
    }
}
