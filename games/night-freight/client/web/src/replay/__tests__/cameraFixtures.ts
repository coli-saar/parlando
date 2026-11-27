/**
 * Fixtures of the camera tests: every film the replay can show without a server.
 *
 * Owns the list of runs (the generated examples under `server/examples/` that
 * carry a `last_run`, plus the two failed tutorial runs of the preview) and the
 * construction of a film for each with the same inputs the player uses
 * (`Timeline`, `Geography`, station names). Pure data; no Babylon scene.
 * Callers rely on `fixtures()` returning a fresh film per call.
 */
import type { LastRun, Observation } from "../../game";
import { Geography } from "../../scene/geography";
import { direct, type Film } from "../director";
import { Timeline } from "../timeline";

/** One film with everything a test needs to evaluate the world next to the camera. */
export interface FilmFixture { name: string; film: Film; tl: Timeline; geo: Geography; run: LastRun; map: Observation["map"] }

/** The generated example views, keyed by their path; Vite inlines them at test time, so no Node typings are needed. */
const EXAMPLES = import.meta.glob<Observation>("../../../../../server/examples/view-*.json", { eager: true, import: "default" });

/** The example view of `name` (for example `freight-after-run`). */
function example(name: string): Observation {
  const hit = Object.entries(EXAMPLES).find(([path]) => path.endsWith(`/view-${name}.json`));
  return structuredClone(hit![1]);
}

/**
 * The failed tutorial run of the development preview (`Preview.tsx` builds the same one): the first plan cannot
 * couple a boxcar of oranges, the engine leaves with nothing and order O1 is missed.
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

/** Builds the film of an observation's last run exactly as the player does. */
function build(name: string, o: Observation): FilmFixture {
  const run = o.last_run!;
  const geo = new Geography(o.map);
  const tl = new Timeline(run.actual, geo, o.map.engines.map((e) => e.id));
  const names = (id: string) => o.map.stations.find((s) => s.id === id)?.name ?? id;
  return { name, film: direct(run, tl, geo, names), tl, geo, run, map: o.map };
}

/** Every film under test: the examples that have a run, and the preview's failed runs. */
export function fixtures(): FilmFixture[] {
  const out: FilmFixture[] = [];
  for (const path of Object.keys(EXAMPLES).sort()) {
    const o = structuredClone(EXAMPLES[path]);
    if (o.last_run) out.push(build(path.replace(/^.*\/(view-[^/]*)\.json$/, "$1"), o));
  }
  for (const role of ["freight", "routing"]) {
    out.push(build(`${role}-failed`, failedRun(example(`${role}-after-run`))));
  }
  return out;
}
