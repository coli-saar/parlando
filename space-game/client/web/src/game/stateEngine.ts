import { devicesAtPosition, doorForStep, isWalkable, roomAtPosition, roomById } from "./level";
import type { DerivedSystems, GameAction, PlayerId, Position, RelayMode, StationState } from "./types";

const MAX_LOG = 10;

export const initialState: StationState = {
  players: {
    A: { room: "power", position: { x: 2, y: 2 }, plateHeld: false },
    B: { room: "valve", position: { x: 9, y: 6 }, plateHeld: false }
  },
  fuses: {
    blue: false,
    yellow: false,
    red: false
  },
  breakers: {
    main: false,
    aux: false
  },
  valves: {
    A: false,
    C: false,
    floodgate: false
  },
  overrideHeld: false,
  battery: {
    location: "charger",
    charged: false,
    spent: false
  },
  relay: "bypass",
  pressureDrained: false,
  oxygenFanTripped: false,
  knowledge: {
    A: [
      "Blue fuse wakes the bus feed.",
      "Yellow fuse is marked PUMP, but the aux breaker warning is scratched."
    ],
    B: [
      "Valve A is drawn beside cabin pressure.",
      "Valve C is drawn beside charger water and coolant, depending on floodgate state."
    ]
  },
  visualEffects: [],
  log: [
    "Emergency lamps are on. The beacon checklist is dark: power, oxygen, access, signal."
  ],
  beaconLaunched: false,
  moveCount: 0
};

export function deriveSystems(state: StationState): DerivedSystems {
  const pumpPowered = state.fuses.yellow && state.breakers.main && !state.oxygenFanTripped;
  const powerStable = state.fuses.blue && state.breakers.main && !state.battery.spent;
  const chargerFed = powerStable && pumpPowered && state.valves.C && !state.valves.floodgate;
  const coolingRestored = state.valves.floodgate && !state.valves.C;
  const oxygenStable =
    pumpPowered &&
    state.valves.A &&
    !state.pressureDrained &&
    !state.oxygenFanTripped;
  const doorMotorPowered =
    powerStable && (state.battery.location === "charger" || state.overrideHeld);
  const doorAccess =
    doorMotorPowered && oxygenStable && (state.players.A.plateHeld || state.players.B.plateHeld);
  const signalRouted =
    state.relay === "array" &&
    state.battery.location === "signal" &&
    state.battery.charged &&
    !state.battery.spent &&
    coolingRestored;

  return {
    pumpPowered,
    chargerFed,
    coolingRestored,
    oxygenStable,
    powerStable,
    doorAccess,
    signalRouted,
    batteryReady: state.battery.charged && !state.battery.spent,
    readyToLaunch: powerStable && oxygenStable && doorAccess && signalRouted
  };
}

export function runStateEngine(state: StationState, action: GameAction): StationState {
  const before = deriveSystems(state);
  let next: StationState = cloneState(state);
  let message = "";
  let effects: string[] = [];

  switch (action.type) {
    case "moveStep": {
      const current = next.players[action.player].position;
      const target = step(current, action.direction);
      const targetRoom = roomAtPosition(target);
      if (!targetRoom || !isWalkable(target)) {
        message = `${name(action.player)} bumps into a sealed wall panel.`;
        break;
      }
      const currentRoom = next.players[action.player].room;
      if (currentRoom !== targetRoom) {
        const door = doorForStep(current, target);
        if (!door) {
          message = `${name(action.player)} reaches a bulkhead seam, but there is no door here.`;
          break;
        }
        if (door.kind === "pressure" && !before.doorAccess) {
          reveal(next, action.player, "The airlock hatch needs oxygen pressure, door power, and the floor plate.");
          message = `${name(action.player)} tries the ${door.label}. It needs pressure, power, and a latch signal.`;
          break;
        }
      }
      if (targetRoom === "airlock" && currentRoom !== "airlock" && !before.doorAccess) {
        reveal(next, action.player, "The airlock hatch needs oxygen pressure, door power, and the floor plate.");
        message = `${name(action.player)} tries the airlock hatch. It stays sealed.`;
        break;
      }
      next.players[action.player].position = target;
      next.players[action.player].room = targetRoom;
      next.players[action.player].plateHeld = target.x === 7 && target.y === 2;
      message = `${name(action.player)} walks to ${roomById[targetRoom].name}.`;
      break;
    }
    case "toggleFuse": {
      next.fuses[action.color] = !next.fuses[action.color];
      effects = [`device:fuse-${action.color}`, "room:power"];
      message = `${name(action.player)} ${next.fuses[action.color] ? "inserts" : "removes"} the ${action.color} fuse.`;
      if (action.color === "yellow" && next.fuses.yellow && next.breakers.aux) {
        next.oxygenFanTripped = true;
        reveal(next, "A", "Yellow pump surge trips the oxygen fan when AUX is already live.");
        reveal(next, "B", "The fan trip is electrical, not a valve failure.");
        message += " The oxygen fan snaps offline as AUX backfeeds the pump leg.";
      }
      if (action.color === "red" && next.fuses.red) {
        reveal(next, action.player, "The red fuse only feeds the reserve lamps in this prototype.");
      }
      break;
    }
    case "toggleBreaker": {
      next.breakers[action.breaker] = !next.breakers[action.breaker];
      effects = [`device:breaker-${action.breaker}`, "room:power"];
      message = `${name(action.player)} turns ${next.breakers[action.breaker] ? "on" : "off"} the ${action.breaker.toUpperCase()} breaker.`;
      if (action.breaker === "aux" && next.breakers.aux && next.fuses.yellow) {
        next.oxygenFanTripped = true;
        reveal(next, "A", "AUX and the yellow pump fuse together trip the fan relay.");
        reveal(next, "B", "If oxygen dies while valves look right, ask about the aux breaker.");
        message += " A pump surge trips the oxygen fan; the pressure light falls.";
      }
      if (action.breaker === "main" && !next.breakers.main) {
        next.battery.charged = next.battery.location === "signal" ? next.battery.charged : false;
        message += " The charger lamp goes dark.";
      }
      break;
    }
    case "setValve": {
      next.valves[action.valve] = action.open;
      effects = [`device:valve-${action.valve.toLowerCase()}`, "room:valve"];
      if (action.valve === "floodgate" && action.open) {
        if (before.oxygenStable) {
          next.pressureDrained = true;
          reveal(next, "B", "Opening the floodgate restores coolant but drains cabin pressure first.");
        }
        message = `${name(action.player)} opens the floodgate. Coolant rushes through; cabin pressure bleeds down.`;
      } else if (action.valve === "A" && action.open) {
        next.pressureDrained = false;
        next.oxygenFanTripped = false;
        message = `${name(action.player)} opens valve A. The oxygen cabinet begins refilling pressure.`;
      } else if (action.valve === "C" && !action.open) {
        message = `${name(action.player)} diverts valve C away from charger water toward the coolant loop.`;
        reveal(next, "A", "Valve C closed starves the charger but helps cooling after floodgate opens.");
      } else {
        message = `${name(action.player)} ${action.open ? "opens" : "closes"} valve ${action.valve}.`;
      }
      break;
    }
    case "holdOverride": {
      next.overrideHeld = action.held;
      effects = ["device:bypass", "room:junction"];
      message = `${name(action.player)} ${action.held ? "holds" : "releases"} the manual bypass lever.`;
      if (action.held) {
        reveal(next, action.player, "The bypass keeps the door motor powered during battery transfer.");
      }
      break;
    }
    case "togglePlate": {
      message = `${name(action.player)} has to stand on the floor plate to hold it down.`;
      break;
    }
    case "chargeBattery": {
      if (before.chargerFed && next.battery.location === "charger") {
        next.battery.charged = true;
        next.battery.spent = false;
        effects = ["device:charger", "device:battery", "room:charger"];
        message = `${name(action.player)} starts the charger. The battery reaches full charge.`;
        reveal(next, "A", "The charger needs blue bus, yellow pump, main breaker, and valve C water.");
        reveal(next, "B", "Floodgate open steals the water return from the charger.");
      } else {
        effects = ["device:charger", "room:charger"];
        message = `${name(action.player)} starts the charger, but it refuses the cycle.`;
        reveal(next, action.player, diagnosticFor(next, action.player));
      }
      break;
    }
    case "moveBattery": {
      if (next.battery.location === "charger") {
        if (!next.battery.charged) {
          message = `${name(action.player)} tugs the battery sled, but the pack is too depleted to transfer.`;
          reveal(next, action.player, "Charge the battery before moving it to the signal array.");
          break;
        }
        next.battery.location = "signal";
        effects = ["device:battery", "room:charger", "room:signal"];
        message = `${name(action.player)} rolls the charged battery to the signal array.`;
        if (!next.overrideHeld) {
          reveal(next, "B", "Battery transfer removes the door motor supply unless the bypass is held.");
          message += " The airlock motor browns out because the bypass was not held.";
        }
      } else {
        next.battery.location = "charger";
        next.battery.spent = false;
        effects = ["device:battery", "room:signal", "room:charger"];
        message = `${name(action.player)} returns the battery to the charger dock.`;
      }
      break;
    }
    case "setRelay": {
      next.relay = action.mode;
      effects = ["device:relay", "room:signal"];
      message = `${name(action.player)} sets the relay to ${action.mode.toUpperCase()}.`;
      if (action.mode === "array" && !before.signalRouted) {
        reveal(next, "A", "ARRAY mode should wait until charged battery and coolant are both ready.");
        reveal(next, "B", "A loop fault means relay mode was changed before the physical path was ready.");
      }
      break;
    }
    case "cycleRelay": {
      next.relay = nextRelay(next.relay);
      effects = ["device:relay", "room:signal"];
      message = `${name(action.player)} rotates the signal relay to ${next.relay.toUpperCase()}.`;
      if (next.relay === "array" && !before.signalRouted) {
        reveal(next, "A", "ARRAY mode should wait until charged battery and coolant are both ready.");
        reveal(next, "B", "A loop fault means relay mode was changed before the physical path was ready.");
      }
      break;
    }
    case "runDiagnostic": {
      const report = diagnosticFor(next, action.player);
      reveal(next, action.player, report);
      message = `${name(action.player)} runs diagnostics: ${report}`;
      break;
    }
    case "launchBeacon": {
      if (before.readyToLaunch) {
        next.beaconLaunched = true;
        effects = ["device:beacon", "room:airlock", "room:signal"];
        message = `${name(action.player)} launches the evacuation beacon. The station answers with a clean carrier lock.`;
      } else {
        next.battery.spent = next.battery.location === "signal" && next.battery.charged;
        effects = ["device:beacon", "device:battery", "room:airlock", "room:signal"];
        reveal(next, action.player, "Early launch attempts consume the signal battery pulse.");
        message = `${name(action.player)} fires the beacon early. The signal coughs once and drains the battery pulse.`;
      }
      break;
    }
    default:
      return next;
  }

  return finalize(next, before, message, effects);
}

export function availableActions(state: StationState, player: PlayerId): GameAction[] {
  const room = state.players[player].room;
  const devices = devicesAtPlayerPosition(state, player);
  const actions: GameAction[] = [];

  for (const device of devices) {
    if (device.id === "fuse-blue") actions.push({ type: "toggleFuse", player, color: "blue" });
    if (device.id === "fuse-yellow") actions.push({ type: "toggleFuse", player, color: "yellow" });
    if (device.id === "fuse-red") actions.push({ type: "toggleFuse", player, color: "red" });
    if (device.id === "breaker-main") actions.push({ type: "toggleBreaker", player, breaker: "main" });
    if (device.id === "breaker-aux") actions.push({ type: "toggleBreaker", player, breaker: "aux" });
    if (device.id === "bypass") actions.push({ type: "holdOverride", player, held: !state.overrideHeld });
    if (device.id === "valve-a") actions.push({ type: "setValve", player, valve: "A", open: !state.valves.A });
    if (device.id === "valve-c") actions.push({ type: "setValve", player, valve: "C", open: !state.valves.C });
    if (device.id === "valve-floodgate") {
      actions.push({ type: "setValve", player, valve: "floodgate", open: !state.valves.floodgate });
    }
    if (device.id === "charger") actions.push({ type: "chargeBattery", player });
    if (device.id === "battery") actions.push({ type: "moveBattery", player });
    if (device.id === "relay") {
      actions.push({ type: "cycleRelay", player });
    }
    if (device.id === "beacon") actions.push({ type: "launchBeacon", player });
  }

  return actions;
}

export function describeAction(action: GameAction, state?: StationState): string {
  switch (action.type) {
    case "moveStep":
      return `Walk ${action.direction}`;
    case "toggleFuse":
      return `${state?.fuses[action.color] ? "Remove" : "Insert"} ${action.color} fuse`;
    case "toggleBreaker":
      return `Turn ${action.breaker.toUpperCase()} ${state?.breakers[action.breaker] ? "off" : "on"}`;
    case "setValve":
      return `${action.open ? "Open" : "Close"} valve ${action.valve}`;
    case "holdOverride":
      return `${action.held ? "Hold" : "Release"} bypass`;
    case "togglePlate":
      return "Stand on floor plate";
    case "chargeBattery":
      return "Start charger";
    case "moveBattery":
      return state?.battery.location === "charger"
        ? "Roll battery to Signal Array"
        : "Return battery to Charger";
    case "setRelay":
      return `Point relay to ${action.mode.toUpperCase()}`;
    case "cycleRelay":
      return `Rotate relay to ${nextRelay(state?.relay ?? "bypass").toUpperCase()}`;
    case "runDiagnostic":
      return "Run diagnostics";
    case "launchBeacon":
      return "Launch beacon";
  }
}

function cloneState(state: StationState): StationState {
  return {
    ...state,
    players: {
      A: { ...state.players.A, position: { ...state.players.A.position } },
      B: { ...state.players.B, position: { ...state.players.B.position } }
    },
    fuses: { ...state.fuses },
    breakers: { ...state.breakers },
    valves: { ...state.valves },
    battery: { ...state.battery },
    knowledge: {
      A: [...state.knowledge.A],
      B: [...state.knowledge.B]
    },
    visualEffects: [...state.visualEffects],
    log: [...state.log]
  };
}

function devicesAtPlayerPosition(state: StationState, player: PlayerId) {
  const position = state.players[player].position;
  const devices = devicesAtPosition(position).filter(
    (device) => device.id !== "battery" || state.battery.location === "charger"
  );
  if (state.battery.location === "signal" && position.x === 8 && position.y === 8) {
    devices.push({
      id: "battery",
      kind: "battery",
      label: "Battery sled",
      room: "signal",
      position: { x: 8, y: 8 }
    });
  }
  return devices;
}

function nextRelay(relay: RelayMode): RelayMode {
  if (relay === "bypass") return "loop";
  if (relay === "loop") return "array";
  return "bypass";
}

function step(position: Position, direction: "up" | "down" | "left" | "right"): Position {
  const deltas = {
    up: { x: 0, y: -1 },
    down: { x: 0, y: 1 },
    left: { x: -1, y: 0 },
    right: { x: 1, y: 0 }
  };
  const delta = deltas[direction];
  return { x: position.x + delta.x, y: position.y + delta.y };
}

function finalize(
  state: StationState,
  before: DerivedSystems,
  message: string,
  effects: string[] = []
): StationState {
  const after = deriveSystems(state);
  const changes = [
    statusChange("Power", before.powerStable, after.powerStable),
    statusChange("Oxygen", before.oxygenStable, after.oxygenStable),
    statusChange("Door access", before.doorAccess, after.doorAccess),
    statusChange("Signal", before.signalRouted, after.signalRouted),
    statusChange("Cooling", before.coolingRestored, after.coolingRestored)
  ].filter(Boolean);
  const visualEffects = [
    ...effects,
    ...systemEffect("Power", before.powerStable, after.powerStable, "room:power"),
    ...systemEffect("Oxygen", before.oxygenStable, after.oxygenStable, "room:oxygen"),
    ...systemEffect("Door access", before.doorAccess, after.doorAccess, "room:airlock"),
    ...systemEffect("Signal", before.signalRouted, after.signalRouted, "room:signal"),
    ...systemEffect("Cooling", before.coolingRestored, after.coolingRestored, "room:valve")
  ];

  return {
    ...state,
    moveCount: state.moveCount + 1,
    visualEffects,
    log: [message, ...changes, ...state.log].filter(Boolean).slice(0, MAX_LOG)
  };
}

function statusChange(label: string, before: boolean, after: boolean): string {
  if (before === after) return "";
  return `${label} ${after ? "comes online" : "drops offline"}.`;
}

function systemEffect(_label: string, before: boolean, after: boolean, effect: string): string[] {
  return before === after ? [] : [effect];
}

function reveal(state: StationState, player: PlayerId, text: string): void {
  if (!state.knowledge[player].includes(text)) {
    state.knowledge[player].push(text);
  }
}

function name(player: PlayerId): string {
  return `Player ${player}`;
}

function diagnosticFor(state: StationState, player: PlayerId): string {
  const systems = deriveSystems(state);
  if (!systems.powerStable) {
    return player === "A"
      ? "Bus feed dark: blue fuse and MAIN breaker must both be live."
      : "Node Lantern has no stable feed; ask for the blue bus and main line.";
  }
  if (state.oxygenFanTripped) {
    return player === "A"
      ? "Fan relay tripped by AUX backfeed through yellow pump leg."
      : "Oxygen fault is electrical; valve positions are not the first problem.";
  }
  if (!systems.pumpPowered) {
    return player === "A"
      ? "Pump leg idle: yellow fuse needs MAIN, but not AUX surge."
      : "Cabin pressure cannot refill until the pump leg is live.";
  }
  if (!systems.chargerFed && state.battery.location === "charger" && !state.battery.charged) {
    return player === "A"
      ? "Charger starved: check yellow pump and valve C water return."
      : "Water return is missing; floodgate and valve C fight over the charger.";
  }
  if (!systems.oxygenStable) {
    return player === "A"
      ? "Pressure is not holding; valve A must refill after any floodgate drain."
      : "Cabin pressure drained through the flood path; use valve A to refill.";
  }
  if (!systems.doorAccess) {
    return player === "A"
      ? "Door motor wants bypass during battery transfer and a latch plate signal."
      : "The hatch needs oxygen pressure, motor feed, and someone on the plate.";
  }
  if (!systems.signalRouted) {
    return player === "A"
      ? "Signal path missing: battery at array, relay ARRAY, and coolant restored."
      : "Relay map is not locked: coolant plus charged array battery required.";
  }
  return "All launch prerequisites are green at this instant.";
}
