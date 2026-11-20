import { useState } from "react";
import { HotelEnding } from "./App";
import { GameView } from "./GameView";
import type { Action, Observation } from "./game";

const params = new URLSearchParams(window.location.search);
const west = params.get("wing") === "west";
const supplies = params.get("route") === "rescue_supplies";
const atBridge = params.get("at") === "bridge";

const eastBridge = 4;
const eastFloor = atBridge ? eastBridge : 3;
const initial: Observation = {
  wing: west ? "west" : "east",
  route: supplies ? "rescue_supplies" : "escape",
  route_title: supplies ? "Rescue Supplies" : "Bridge Escape",
  floor: west ? 4 - eastFloor : eastFloor,
  partner_floor: west ? eastFloor : 4 - eastFloor,
  release_floor: null,
  cargo: supplies ? (west ? 1 : 2) : 0,
  partner_cargo: supplies ? (west ? 2 : 1) : 0,
  freight_floor: supplies ? (west ? 3 : 1) : null,
  freight_status: supplies ? (atBridge ? "ready" : "awaiting_pickup") : null,
  landing_loads: [3, 3, 3, 3, 3],
  partner_landing_loads: [3, 3, 3, 3, 3],
  load_floor: west ? 4 - eastFloor : eastFloor,
  unload_floor: west ? 4 - eastFloor : eastFloor,
  gate_open: false,
  partner_gate_open: false,
  lock_tripped: false,
  partner_lock_tripped: false,
  on_landing: false,
  partner_on_landing: false,
  handle: atBridge ? "west" : "east",
  handed_over: atBridge,
  exited: false,
  partner_exited: false,
  partner_destination: west ? eastBridge : 4 - eastBridge,
  partner_seal: west ? "rose" : "stag",
  partner_delivery: supplies ? (west ? 2 : 1) : null,
  actions: []
};

/** Applies visual changes to one fixture so instrument panels can be exercised without a server. */
function adjust(before: Observation, action: Action): Observation {
  switch (action.type) {
    case "bridge_pose": return before;
    case "step_onto_landing": return { ...before, on_landing: true };
    case "board_lift": return { ...before, on_landing: false };
    case "load_rack":
    case "load":
    case "unload": {
      const loading = action.type !== "unload";
      if (loading ? before.cargo === 3 : before.cargo === 0) return before;
      const rack = action.type === "load_rack" ? action.floor : loading ? before.load_floor : before.unload_floor;
      const change = loading ? 1 : -1;
      return { ...before, cargo: before.cargo + change,
        landing_loads: before.landing_loads.map((n, floor) => floor === rack ? n - change : n) };
    }
    case "release": {
      const floor = before.release_floor;
      if (floor === null) return before;
      return { ...before, floor, partner_floor: 4 - floor };
    }
    case "reset_lock": return { ...before, lock_tripped: false };
    case "collect_rescue_case": return { ...before, cargo: before.cargo + 1, freight_status: "aboard" };
    case "reach_refuge": return { ...before, exited: true };
    case "pass": return { ...before, handed_over: true, handle: "west" };
    case "exit": {
      const seal = west ? "stag" : "rose";
      if (before.freight_status && before.freight_status !== "ready") return before;
      return action.seal === seal ? { ...before, gate_open: true } : { ...before, lock_tripped: true };
    }
  }
}

/** Supplies development-only controls to inspect the two lift scenes independently. */
export default function Preview() {
  const [observation, setObservation] = useState(initial);
  if (observation.exited) return <HotelEnding completion={{ escaped: true, route: observation.route }} />;
  const actions: Action[] = observation.gate_open ? [{ type: "reach_refuge" }]
    : observation.on_landing ? [{ type: "board_lift" }] : [
      ...(observation.freight_status === "awaiting_pickup" && !west && observation.handed_over && observation.floor === observation.freight_floor && observation.cargo < 3 ? [{ type: "collect_rescue_case" } as const] : []),
      { type: "step_onto_landing" }, { type: "load" }, { type: "unload" },
      ...observation.landing_loads.flatMap((count, floor): Action[] => count && observation.cargo < 3 ? [{ type: "load_rack", floor }] : []),
      { type: "release" }, { type: "pass" },
      ...(observation.lock_tripped ? [{ type: "reset_lock" } as const] :
        (["stag", "rose", "key"] as const).map(seal => ({ type: "exit" as const, seal }))),
    ];
  // The live server supplies these two rack addresses. Recompute them for the local fixture.
  const nearest = (stockOk: (count: number) => boolean) => observation.landing_loads
    .map((count, floor) => ({ count, floor }))
    .filter(({ count }) => stockOk(count))
    .sort((a, b) => Math.abs(a.floor - observation.floor) - Math.abs(b.floor - observation.floor))[0].floor;
  const direction = Math.sign(observation.partner_cargo - observation.cargo);
  const next = observation.floor + direction;
  const release_floor = direction !== 0 && next >= 0 && next <= 4 ? next : null;
  const manifestsMatch = observation.cargo === (west ? 1 : 2) && observation.partner_cargo === observation.partner_delivery;
  const freightStatus = observation.freight_status === "ready" || observation.freight_status === "aboard"
    ? (manifestsMatch ? "ready" as const : "aboard" as const) : observation.freight_status;
  const view = { ...observation, freight_status: freightStatus, release_floor, load_floor: nearest(n => n > 0), unload_floor: nearest(n => n < 6) };
  return <GameView snapshot={{
    observation: { ...view, actions: actions.filter(action =>
      (action.type !== "pass" || (view.wing === "east" && !view.handed_over && view.floor === 2)) &&
      (action.type !== "release" || (view.handle === view.wing && release_floor !== null)) &&
      (action.type !== "exit" || (view.handed_over && view.partner_floor === view.partner_destination && (!supplies || view.freight_status === "ready"))) &&
      (action.type !== "load" || view.cargo < 3) && (action.type !== "unload" || view.cargo > Number(!west && (view.freight_status === "aboard" || view.freight_status === "ready")))) },
    interactionEnabled: !observation.exited,
    act: (action) => setObservation(adjust(view, action))
  }} />;
}
