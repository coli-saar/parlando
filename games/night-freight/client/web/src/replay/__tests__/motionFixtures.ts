/**
 * Fixtures of the motion tests: every trace the replay can show without a server, plus
 * synthetic traces that exercise what the examples do not.
 *
 * Owns the list of traces (the `last_run` of every generated example under
 * `server/examples/`, both its actual and its expected trace; the two failed tutorial
 * runs of the preview; and the synthetic traces below) and their construction into a
 * `Timeline` with the same inputs the player uses. Pure data, no Babylon scene.
 * Callers rely on `motionFixtures()` returning fresh timelines on every call.
 */
import type { Car, CarKind, EventKind, GameMap, Observation, Op, StationId, Trace } from "../../game";
import { Geography } from "../../scene/geography";
import { Timeline } from "../timeline";

/** One trace with the geography and timeline built from it. */
export interface MotionFixture { name: string; trace: Trace; geo: Geography; tl: Timeline }

/** The generated example views, keyed by their path; Vite inlines them at test time. */
const EXAMPLES = import.meta.glob<Observation>("../../../../../server/examples/*.json", { eager: true, import: "default" });

/** The failed tutorial run of the development preview (`Preview.tsx` builds the same one): the engine leaves with nothing. */
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
  return { ...base, last_run: run };
}

/** Builds a fixture the way the player does. */
function build(name: string, trace: Trace, map: GameMap): MotionFixture {
  const geo = new Geography(map);
  const engines = [...new Set([...map.engines.map((e) => e.id), ...trace.initial.engines.map((e) => e.id)])];
  return { name, trace, geo, tl: new Timeline(trace, geo, engines) };
}

/** A seeded pseudo random generator (mulberry32), so the stress traces are the same on every run. */
function random(seed: number): () => number {
  let a = seed;
  return () => {
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

/** A synthetic trace over the classic map: initial cars and engines, and events given as `[hour, kind]`. */
function synthetic(cars: Car[], engines: { id: string; station: StationId }[], events: [number, EventKind][], endHour: number): Trace {
  return {
    initial: {
      hour: 0,
      engines: engines.map((e) => ({ id: e.id, station: e.station, capacity: 3, available_from: 0, arrived_at: 0, failed: false })),
      cars, orders: []
    },
    events: events.map(([hour, kind]) => ({ hour, kind })),
    end_hour: endHour, orders: [], success: true
  };
}

const COUPLE: Op = { type: "couple", kind: "boxcar", load: { type: "any" }, count: 1 };
const UNCOUPLE: Op = { type: "uncouple", kind: "boxcar", load: { type: "any" }, count: 1 };

/** Event helpers for the synthetic traces. */
const ev = {
  couple: (engine: string, station: StationId, cars: number[], stop = 0, index = 0): EventKind => ({ type: "op_done", engine, station, stop, index, op: COUPLE, cars }),
  uncouple: (engine: string, station: StationId, cars: number[], stop = 0, index = 0): EventKind => ({ type: "op_done", engine, station, stop, index, op: UNCOUPLE, cars }),
  depart: (engine: string, from: StationId, to: StationId, arrive: number, stop = 0): EventKind => ({ type: "depart", engine, from, to, stop, arrive_hour: arrive }),
  arrive: (engine: string, station: StationId, stop = 1): EventKind => ({ type: "arrive", engine, station, stop })
};

/** Cars of the synthetic traces: id, kind and where it stands (a station) or the engine it hangs on. */
function carList(spec: [number, CarKind, string][], engines: string[]): Car[] {
  return spec.map(([id, kind, at]) => ({ id, kind, load: null, at: engines.includes(at) ? { type: "coupled", engine: at } : { type: "yard", station: at } }));
}

/**
 * Synthetic traces with what the examples lack: several cars per yard of both kinds,
 * an uncoupling of a car that is not at the far end of the consist, out-and-back legs
 * (every second one tail first), two engines working one yard in turn, and an engine
 * that starts with a consist.
 */
function shuttle(map: GameMap): Trace {
  const engines = ["E1", "E2", "E3"];
  const cars = carList([[1, "boxcar", "corning"], [2, "boxcar", "corning"], [3, "tanker", "corning"], [4, "boxcar", "corning"], [5, "boxcar", "corning"], [6, "tanker", "corning"],
    [7, "boxcar", "dansville"], [8, "boxcar", "dansville"], [9, "boxcar", "E3"]], engines);
  return synthetic(cars, [{ id: "E1", station: "corning" }, { id: "E2", station: "elmira" }, { id: "E3", station: "elmira" }], [
    [0, ev.couple("E1", "corning", [1, 4, 5])],
    [0, ev.uncouple("E1", "corning", [4], 0, 1)],
    [1, ev.depart("E1", "corning", "dansville", 2)],
    [1, ev.uncouple("E3", "elmira", [9])],
    [1, ev.depart("E3", "elmira", "corning", 3)],
    [1, ev.depart("E2", "elmira", "corning", 3)],
    [2, ev.arrive("E1", "dansville")],
    [2, ev.uncouple("E1", "dansville", [1], 1, 0)],
    [2, ev.couple("E1", "dansville", [7, 8], 1, 1)],
    [2, ev.depart("E1", "dansville", "corning", 4, 1)],
    [3, ev.arrive("E3", "corning", 1)],
    [3, ev.arrive("E2", "corning", 1)],
    [3, ev.couple("E3", "corning", [2, 3], 1, 0)],
    [3, ev.couple("E2", "corning", [6], 1, 0)],
    [4, ev.arrive("E1", "corning", 2)],
    [4, ev.uncouple("E1", "corning", [8], 2, 0)],
    [4, ev.couple("E1", "corning", [4], 2, 1)],
    [4, ev.depart("E1", "corning", "dansville", 5, 2)],
    [4, ev.depart("E3", "corning", "elmira", 6, 1)],
    [5, ev.arrive("E1", "dansville", 3)],
    [5, ev.uncouple("E1", "dansville", [7, 4], 3, 0)],
    [5, ev.depart("E1", "dansville", "corning", 7, 3)],
    [6, ev.arrive("E3", "elmira", 2)],
    [6, ev.uncouple("E3", "elmira", [3], 2, 0)],
    [7, ev.arrive("E1", "corning", 4)]
  ], 8);
}

/**
 * Many couplings and uncouplings of random cars by two engines in one yard, to cover
 * blocked cars (a wanted car behind others) and yards with many cars. Engines work
 * alternately and always one op per hour.
 */
function stress(map: GameMap, seed: number, carsAtYard: number): Trace {
  const rnd = random(seed);
  const station = "corning";
  const kinds: CarKind[] = ["boxcar", "tanker"];
  const cars = carList(Array.from({ length: carsAtYard }, (_, i) => [i + 1, kinds[Math.floor(rnd() * 2)], station] as [number, CarKind, string]), ["E1", "E2"]);
  const onEngine: Record<string, number[]> = { E1: [], E2: [] };
  const inYard = new Set(cars.map((c) => c.id));
  const events: [number, EventKind][] = [];
  for (let hour = 0; hour < 14; hour++) {
    const engine = hour % 2 === 0 ? "E1" : "E2";
    const mine = onEngine[engine];
    const couple = mine.length < 3 && (inYard.size > 0 && (mine.length === 0 || rnd() < 0.6));
    if (couple) {
      const pool = [...inYard];
      const n = Math.min(pool.length, 1 + Math.floor(rnd() * Math.min(2, 3 - mine.length)));
      const picked: number[] = [];
      while (picked.length < n) { const id = pool.splice(Math.floor(rnd() * pool.length), 1)[0]; picked.push(id); }
      picked.forEach((id) => inYard.delete(id));
      mine.push(...picked);
      events.push([hour, ev.couple(engine, station, picked, 0, hour)]);
    } else if (mine.length) {
      const n = 1 + Math.floor(rnd() * mine.length);
      const picked: number[] = [];
      const pool = [...mine];
      while (picked.length < n) picked.push(pool.splice(Math.floor(rnd() * pool.length), 1)[0]);
      picked.forEach((id) => { mine.splice(mine.indexOf(id), 1); inYard.add(id); });
      events.push([hour, ev.uncouple(engine, station, picked, 0, hour)]);
    }
  }
  return synthetic(cars, [{ id: "E1", station }, { id: "E2", station }], events, 15);
}

/** Every trace under test. */
export function motionFixtures(): MotionFixture[] {
  const out: MotionFixture[] = [];
  const examples = Object.entries(EXAMPLES).filter(([, o]) => o.last_run).sort(([a], [b]) => a.localeCompare(b));
  for (const [path, o] of examples) {
    const name = path.replace(/^.*\/([^/]*)\.json$/, "$1");
    out.push(build(`${name} actual`, structuredClone(o).last_run!.actual, o.map));
    out.push(build(`${name} expected`, structuredClone(o).last_run!.expected, o.map));
  }
  const after = examples.find(([p]) => p.endsWith("view-freight-after-run.json"))![1];
  out.push(build("freight-failed", failedRun(structuredClone(after)).last_run!.actual, after.map));
  out.push(build("routing-failed", failedRun(structuredClone(examples.find(([p]) => p.endsWith("view-routing-after-run.json"))![1])).last_run!.actual, after.map));
  out.push(build("synthetic shuttle", shuttle(after.map), after.map));
  for (const [seed, n] of [[1, 6], [2, 8], [3, 10], [4, 12], [5, 7]] as const) out.push(build(`synthetic stress ${seed}`, stress(after.map, seed, n), after.map));
  return out;
}
