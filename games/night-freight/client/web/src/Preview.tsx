/**
 * Development preview: renders the game from the generated example views in
 * `server/examples/` without a server (`?preview=freight-planning` and so on).
 *
 * `*-failed` fixtures are the tutorial's first plan failing (abandoned steps and a missed order).
 * Only for `vite dev`; production builds never load it. Actions are applied
 * minimally: `replay_finished` leaves the replay, `set_ready` toggles the flag;
 * edits are logged because the example views are static documents.
 */
import { useState } from "react";
import { GameView } from "./GameView";
import type { Action, Observation } from "./game";
import freightPlanning from "../../../server/examples/view-freight-planning.json";
import routingPlanning from "../../../server/examples/view-routing-planning.json";
import freightAfterRun from "../../../server/examples/view-freight-after-run.json";
import routingAfterRun from "../../../server/examples/view-routing-after-run.json";
import freightInterrupted from "../../../server/examples/view-freight-interrupted-replay.json";
import routingInterrupted from "../../../server/examples/view-routing-interrupted-replay.json";
import freightAct2 from "../../../server/examples/view-freight-act2.json";
import routingAct2 from "../../../server/examples/view-routing-act2.json";
import freightDone from "../../../server/examples/view-freight-scenario-done.json";

/**
 * A failed tutorial run built from the successful example: the first plan (couple a boxcar of oranges, then
 * load) cannot work because the yard boxcar is empty, so the engine leaves with nothing and O1 is missed.
 */
function failedRun(base: Observation): Observation {
  const run = structuredClone(base.last_run!);
  const op = (o: object) => o as never;
  const ev = (hour: number, kind: object) => ({ hour, kind } as never);
  const couple = { type: "couple", kind: "boxcar", load: { type: "loaded", commodity: "oranges" }, count: 1 };
  const load = { type: "load", commodity: "oranges", count: 1 };
  const deliver = { type: "deliver", order: "O1" };
  const common = { engine: "E1", stop: 0 };
  run.actual.events = [
    ev(0, { type: "op_abandoned", ...common, station: "corning", index: 0, op: op(couple), reason: { type: "no_matching_cars_in_yard", kind: "boxcar", load: { type: "loaded", commodity: "oranges" }, wanted: 1, found: 0, yard: [{ load: null, count: 1 }] } }),
    ev(0, { type: "op_abandoned", ...common, station: "corning", index: 1, op: op(load), reason: { type: "no_empty_cars_on_engine", kind: "boxcar", wanted: 1, found: 0 } }),
    ev(0, { type: "depart", engine: "E1", from: "corning", to: "dansville", stop: 0, arrive_hour: 2 }),
    ev(2, { type: "arrive", engine: "E1", station: "dansville", stop: 1 }),
    ev(2, { type: "op_abandoned", engine: "E1", stop: 1, station: "dansville", index: 0, op: op(deliver), reason: { type: "no_order_loads_on_engine", order: "O1", commodity: "oranges", coupled: [] } }),
    ev(5, { type: "order_missed", order: "O1", why: { type: "no_car_reached", commodity: "oranges", to: "dansville" } })
  ];
  run.actual.end_hour = 5; run.actual.success = false;
  run.success = false;
  run.orders = run.orders.map((o) => ({ ...o, delivered: 0, delivered_at: null, met: false }));
  run.actual.orders = run.orders;
  run.divergence = { hour: 0, engine: "E1", actual: run.actual.events[0], expected: base.last_run!.actual.events[0] };
  return { ...base, last_run: run };
}

const fixtures: Record<string, unknown> = {
  "freight-planning": freightPlanning, "routing-planning": routingPlanning,
  "freight-after-run": freightAfterRun, "routing-after-run": routingAfterRun,
  "freight-interrupted": freightInterrupted, "routing-interrupted": routingInterrupted,
  "freight-failed": failedRun(freightAfterRun as unknown as Observation), "routing-failed": failedRun(routingAfterRun as unknown as Observation),
  "freight-act2": freightAct2, "routing-act2": routingAct2, "freight-done": freightDone
};

/** Hosts one example view inside the real GameView. */
export default function Preview() {
  const name = new URLSearchParams(window.location.search).get("preview") ?? "freight-planning";
  const [observation, setObservation] = useState(() => (fixtures[name] ?? freightPlanning) as unknown as Observation);
  const act = (action: Action) => {
    console.info("preview action", action);
    if (action.type === "replay_finished") setObservation((o) => ({ ...o, phase: "planning", replay_finished: { me: false, partner: false } }));
    if (action.type === "set_ready") setObservation((o) => ({ ...o, ready: { ...o.ready, me: action.ready } }));
  };
  return <GameView snapshot={{ observation, interactionEnabled: true, act }} />;
}
