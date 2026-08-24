import type { GameAction, StationObservation } from "./types";

/** Returns the participant-facing label for one server-advertised action. */
export function describeAction(action: GameAction, state?: StationObservation): string {
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
    case "chargeBattery":
      return "Start charger";
    case "moveBattery":
      return state?.battery.location === "charger"
        ? "Roll battery to Signal Array"
        : "Return battery to Charger";
    case "cycleRelay":
      return `Rotate relay to ${nextRelay(state?.relay ?? "bypass").toUpperCase()}`;
    case "launchBeacon":
      return "Launch beacon";
  }
}

/** Returns the relay position reached by the server's cycle action. */
function nextRelay(relay: StationObservation["relay"]): StationObservation["relay"] {
  if (relay === "bypass") return "loop";
  if (relay === "loop") return "array";
  return "bypass";
}
