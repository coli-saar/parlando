/**
 * Timeline: turns one `Trace` into a function from simulated time to a drawable
 * world `Frame`, and into the pure motion function `poseAt(t)` on which every
 * motion test stands.
 *
 * Owns: the mapping from whole-hour trace events to continuous "visual times"
 * (events inside one hour are staged one after another, each as long as its motion
 * takes), the legs each engine travels (path over the laid track, with acceleration
 * and braking), the shunting excursions by which engines couple and uncouple cars
 * (planned in `shunting.ts`), where every vehicle is (an address `(track, s)` on the
 * laid track plus position and heading), and the red signals, failures and order
 * marks. The director picks sim-time ranges for shots; the scene draws `evaluate(t)`.
 * Nothing here touches the 3D engine; geometry comes from `Geography`.
 *
 * Rail-bound stock: every engine and car sits on a track at an arc length and heads
 * along its tangent, so nothing ever turns in place or slides sideways. A car has no
 * motor: it moves only coupled to an engine, which drives to the car through the
 * turnouts, couples it, and pulls out (see `shunting.ts`). Engines are bidirectional
 * and never turn: when one leaves a station the way it came it runs tail first, its
 * cars (coupled at its tail) leading the train, and `lead` says which end is in front
 * (its headlight is the lit one).
 *
 * Time. Yard work is physical: a shunting move of `n` seconds takes `n / 3600` sim
 * hours (`SECONDS_PER_HOUR`), and `moves` lists every one of them with its sim window
 * and physical duration, so the director can give each as much film time as it needs.
 * Legs last as long as the trace says; `Leg.minSeconds` is the film time in which a leg
 * can be shown at `LINE_SPEED` with comfortable acceleration.
 * Contract for callers: `evaluate` and `poseAt` are pure functions of `t`, so cutting
 * between shots never produces an inconsistent world; `cuts` lists the instants at which
 * a jump of a vehicle is allowed (always empty: no vehicle ever jumps).
 */
import type { Car, CarKind, Commodity, DisruptionNotice, EventKind, GameEvent, Op, OrderId, StationId, Trace } from "../game";
import { STAND_CENTRE, engineLevel, sidingReach, Geography, type P3, type Path, type Route } from "../scene/geography";
import {
  CAR_LENGTH, CAR_WIDTH, ENGINE_LENGTH, ENGINE_WIDTH, SECONDS_PER_HOUR, Yard, backExtent, layoutYard, offsets, planCouple, planUncouple, runDistance,
  type CarRef, type Excursion, type Future, type OpContext, type Parked
} from "./shunting";

export { CAR_LENGTH, CAR_WIDTH, ENGINE_LENGTH, ENGINE_WIDTH, SECONDS_PER_HOUR };
/** Visual length (in hours) of the animation of an instant op that has no motion of its own, and the shortest window of any event. */
export const ANIM_HOURS = 0.09;
/** Visual spacing (in hours) between events of the same hour, so that each is seen on its own. */
const STAGGER = 0.1;
/** Hours kept between two trains using one junction. */
const HOLD_MARGIN = 0.02;
/** Metres kept free at the ends of a siding's reach. */
const SIDING_MARGIN = 0.3;
/** Gap (in hours) left after a shunting move before the next event of the hour. */
const MOVE_GAP = 0.02;
/** Highest speed (metres per second) and acceleration (metres per second squared) of a train on an edge when a leg is shown at `Leg.minSeconds`. */
export const LINE_SPEED = 25;
export const LINE_ACCEL = 1.2;

/** Where a vehicle's body stands: position, heading, its address on the laid track, and its size. */
export interface BodyPose {
  pos: P3;
  /** Heading of the vehicle's front, radians, `atan2(dx, dz)`. */
  yaw: number;
  /** The track the centre lies on (`Geography.yardTrack` / `Geography.edgeTrack`) and the arc length on it. */
  track: string;
  s: number;
  length: number;
  width: number;
}

/** One engine at an instant: its body and train state. */
export interface EnginePose extends BodyPose {
  id: string;
  index: number;
  /** +1 when the engine's nose is its leading end (the front headlight is lit), -1 when it runs tail first (the rear one is). */
  lead: 1 | -1;
  /** True while the engine is running (on a leg or a shunting run), false when it stands. */
  moving: boolean;
  station: StationId | null;
  /** Progress 0..1 along the active leg, 0 when not on a leg. */
  legProgress: number;
  /** Ids of the cars coupled behind this engine, nearest first. */
  cars: number[];
}

/** One car at an instant: its body and whether (and to which engine) it is coupled. */
export interface CarPose extends BodyPose {
  id: number;
  kind: CarKind;
  /** The engine this car is coupled to, or null while it stands in a yard. */
  coupledTo: string | null;
  /** True while a shunting move is coupling or uncoupling this car. */
  rolling: boolean;
}

/** The positions of every vehicle at an instant: a pure function of the trace and the geography. */
export interface WorldPose { t: number; engines: EnginePose[]; cars: CarPose[] }

/** One engine as drawn at an instant. */
export interface EngineFrame extends EnginePose { failed: boolean }

/** One car as drawn at an instant. */
export interface CarFrame extends CarPose {
  load: Commodity | null;
  /** Load gauge while an op fills or empties the car: the commodity and fill level 0..1. */
  gauge?: { commodity: Commodity; fill: number };
}

/** An op in progress at an instant, for the film's progress labels and the loading/factory effects. */
export interface WorkFrame {
  engine: string;
  station: StationId;
  op: Op;
  /** Progress 0..1 through the op's visual duration. */
  progress: number;
  /** Duration in game hours shown in labels (0 for instant ops). */
  hours: number;
  /** Ids of the cars the op works on. */
  cars: number[];
}

/** A red signal at a station throat (an engine is held there). */
export interface RedSignal { station: StationId; end: 1 | -1; engine: string }

/** Everything the scene needs to draw one instant. */
export interface Frame {
  t: number;
  engines: EngineFrame[];
  cars: CarFrame[];
  reds: RedSignal[];
  disrupted: DisruptionNotice | null;
  met: OrderId[];
  missed: OrderId[];
  /** Ops running at this instant. */
  work: WorkFrame[];
}

/** One op of the run as a span of visual time. */
export interface OpSpan {
  engine: string;
  station: StationId;
  stop: number;
  index: number;
  op: Op;
  /** Visual start and end; instant ops get a short animation window after their event, couplings the whole shunting move. */
  t0: number;
  t1: number;
  /** True when the op has no duration on the wire (only an `op_done`). */
  instant: boolean;
  /** Game hours the op takes (`finish_hour` minus start), 0 when instant. */
  hours: number;
  /** Game hour at which the op started and at which it finishes (equal when instant). */
  hour: number;
  finishHour: number;
  cars: number[];
  /** Physical seconds of the shunting move (couple and uncouple only; `(t1 - t0) * SECONDS_PER_HOUR`). */
  seconds?: number;
}

/**
 * One shunting move: an engine with its consist works the yard to couple or uncouple
 * cars. It takes `seconds` of physical time, i.e. `t1 - t0` sim hours; a shot that
 * shows all of it at speed `k` needs `seconds / k` film seconds.
 */
export interface Move {
  engine: string;
  station: StationId;
  op: "couple" | "uncouple";
  cars: number[];
  t0: number;
  t1: number;
  seconds: number;
  /** Metres the engine runs. */
  distance: number;
}

/** An engine's journey between two stations. */
export interface Leg {
  /** Identifies the departure in the trace (engine and event index). */
  key: string;
  engine: string;
  from: StationId;
  to: StationId;
  /** The consist the engine hauls. */
  consist: CarRef[];
  tDepart: number;
  tArrive: number;
  /** Rails of the leg: departure track, edge and arrival track, and the same as a polyline (see `Geography.legPath`). */
  route: Route;
  path: Path;
  /** Arc length on `path` of the engine's centre when it stands at the departure and at the arrival station. */
  s0: number;
  s1: number;
  dFrom: 1 | -1;
  dTo: 1 | -1;
  /** +1 when the engine's nose leads, -1 when it runs tail first (it left the way it came). */
  sigma: 1 | -1;
  /** Axis direction the engine's nose points at while it stands before and after the leg. */
  noseStart: 1 | -1;
  noseEnd: 1 | -1;
  /** Shortest film time in seconds in which the leg can be shown with speed at most `LINE_SPEED` and acceleration at most `LINE_ACCEL`. */
  minSeconds: number;
}

/** A trace event with its continuous visual time. */
export interface TimedEvent { t: number; hour: number; kind: EventKind }

/** One stretch of an engine's motion: a leg, a shunting run, or a stand during a shunting move. */
interface Seg {
  engine: string;
  t0: number;
  t1: number;
  route: Route;
  sigma: 1 | -1;
  s0: number;
  s1: number;
  consist: CarRef[];
  kind: "leg" | "shunt" | "hold";
  station: StationId | null;
  leg?: Leg;
}

/** Where an engine stands from `t0` until its next stretch: station, nose direction and consist. */
interface Stand { t0: number; station: StationId; nose: 1 | -1; consist: CarRef[]; lead: 1 | -1 }

/** The state of the world that is not motion: loads, failures, the disruption and the marked orders. */
interface Keyframe { t: number; loads: Map<number, Commodity | null>; failed: string[]; disrupted: DisruptionNotice | null; met: OrderId[]; missed: OrderId[] }

/** Speed profile: fraction of distance covered after fraction `u` of the leg time (accelerate, cruise, brake). */
export function ease(u: number): number {
  const a = 0.2, vmax = 1 / (1 - a);
  if (u <= 0) return 0;
  if (u >= 1) return 1;
  if (u < a) return (vmax * u * u) / (2 * a);
  if (u > 1 - a) return 1 - (vmax * (1 - u) * (1 - u)) / (2 * a);
  return vmax * (u - a / 2);
}

/** Output commodity of the factory, inferred from the input car (only oranges are processed). */
function processed(load: Commodity | null): Commodity | null { return load === "oranges" ? "juice" : "juice"; }

/** A trace prepared for evaluation at any simulated time. */
export class Timeline {
  events: TimedEvent[] = [];
  legs: Leg[] = [];
  legsByEngine = new Map<string, Leg[]>();
  /** Every shunting move of the run (couplings and uncouplings), in time order. */
  moves: Move[] = [];
  /** Instants at which a vehicle may jump (shot cuts). The timeline never needs one. */
  readonly cuts: number[] = [];
  /** Per game hour, how much of the hour (visual) its events occupy: the clock stands still there. */
  private busy = new Map<number, number>();
  /** Every op of the run with its visual time span. */
  spans: OpSpan[] = [];
  readonly engineIds: string[];
  end = 0;
  private keyframes: Keyframe[] = [];
  private holds: { engine: string; station: StationId; t0: number; t1: number; end: 1 | -1 }[] = [];
  private segs = new Map<string, Seg[]>();
  private stands = new Map<string, Stand[]>();
  private carKind = new Map<number, CarKind>();
  /** Per car, where it stood after each uncoupling (and at the start): from `t0` on, until a train takes it. */
  private parkedAt = new Map<number, { t0: number; station: StationId; car: Parked }[]>();

  constructor(readonly trace: Trace, private readonly geo: Geography, engineOrder: string[]) {
    this.engineIds = engineOrder.length ? engineOrder : trace.initial.engines.map((e) => e.id);
    // Departures are delayed (never arrivals) until no train leaves into or out of a junction that another occupies.
    const earliest = new Map<string, number>();
    for (let pass = 0; pass < 12; pass++) {
      this.reset();
      this.simulate(earliest);
      if (!this.holdDepartures(earliest)) break;
    }
    const last = this.events.length ? this.events[this.events.length - 1].t : trace.initial.hour;
    const lastMotion = Math.max(0, ...[...this.segs.values()].map((l) => (l.length ? l[l.length - 1].t1 : 0)));
    this.end = Math.max(trace.end_hour, last, lastMotion);
  }

  /** Empties everything `simulate` builds. */
  private reset(): void {
    this.events = []; this.legs = []; this.moves = []; this.busy = new Map(); this.spans = []; this.keyframes = []; this.holds = [];
    this.legsByEngine = new Map(); this.segs = new Map(); this.stands = new Map(); this.carKind = new Map(); this.parkedAt = new Map();
    for (const id of this.engineIds) { this.legsByEngine.set(id, []); this.segs.set(id, []); }
  }

  /**
   * Finds departures that run into a junction occupied by another train (an arriving one, or one that
   * left earlier) and records, per departure, the time before which it may not start. Returns whether
   * anything changed. A train only ever waits for others to clear; arrivals keep the hour of the trace.
   */
  private holdDepartures(earliest: Map<string, number>): boolean {
    const sep = (station: StationId, end: 1 | -1) => this.geo.junctionClearance(station, end) + 5;
    const windows = this.legs.flatMap((leg) => {
      const kinds = leg.consist.map((c) => c.kind);
      const toEnd = (-leg.dTo) as 1 | -1;
      const frontExt = leg.sigma > 0 ? ENGINE_LENGTH / 2 : backExtent(kinds);
      const enter = this.timeAtDistance(leg, Math.max(leg.s0, leg.route.pieces[2].arc0 - sep(leg.to, toEnd) - frontExt));
      return [
        { leg, depart: true, station: leg.from, end: leg.dFrom, t0: leg.tDepart, t1: this.clearTime(leg, kinds) },
        { leg, depart: false, station: leg.to, end: toEnd, t0: enter, t1: leg.tArrive }
      ];
    });
    let changed = false;
    for (const x of windows) {
      if (!x.depart) continue;
      for (const y of windows) {
        if (y.leg === x.leg || y.station !== x.station || y.end !== x.end) continue;
        if (y.depart && y.t0 >= x.t0) continue;
        if (y.t0 < x.t1 + HOLD_MARGIN && x.t0 < y.t1 + HOLD_MARGIN) {
          const wait = y.t1 + HOLD_MARGIN;
          if (wait > (earliest.get(x.leg.key) ?? -Infinity) + 1e-9) { earliest.set(x.leg.key, wait); changed = true; }
          break;
        }
      }
    }
    return changed;
  }

  /**
   * The game clock (hours) shown for visual time `t`: it stands still while the
   * events of an hour are staged one after another, then runs on evenly to the
   * next hour, so it is continuous and never jumps.
   */
  trueHour(t: number): number {
    const h = Math.floor(t + 1e-9);
    const f = Math.min(0.95, this.busy.get(h) ?? 0);
    return t - h < f ? h : h + (t - h - f) / (1 - f);
  }

  /** Index of an engine for track assignment. */
  engineIndex(id: string): number { return Math.max(0, this.engineIds.indexOf(id)); }

  /**
   * Walks the events in order and builds everything: visual times, legs, the coupling
   * and uncoupling excursions (which decide how long each event takes), the stands of
   * the engines, the parked cars and the keyframes of the non-motion state. Single pass,
   * because the time of each event depends on the motion of the ones before it.
   */
  private simulate(earliest: Map<string, number>): void {
    const { trace, geo } = this;
    // Wire order, except that within an hour an order is marked met after the delivery that met it.
    const ordered = [...trace.events].sort((a, b) => a.hour - b.hour || Number(a.kind.type === "order_met") - Number(b.kind.type === "order_met"));
    const noses = this.planNoses(ordered);
    const noseAt = (engine: string, index: number): 1 | -1 => noses.at(engine, index);
    // Future of every car, for choosing sidings from which cars can leave.
    const pickups = new Map<number, { idx: number; engine: string }[]>(), drops = new Map<number, number[]>();
    ordered.forEach((e, idx) => {
      const k = e.kind;
      if (k.type !== "op_done") return;
      const table = k.op.type === "couple" ? pickups : k.op.type === "uncouple" ? drops : null;
      if (!table) return;
      for (const id of k.cars) {
        if (table === pickups) (pickups.get(id) ?? pickups.set(id, []).get(id)!).push({ idx, engine: k.engine });
        else (drops.get(id) ?? drops.set(id, []).get(id)!).push(idx);
      }
    });
    let now = -1;
    const fut: Future = {
      pickup: (id) => {
        const hit = (pickups.get(id) ?? []).find((p) => p.idx > now);
        return hit ? { rank: hit.idx, end: noseAt(hit.engine, hit.idx) } : null;
      },
      dropRank: (id) => (drops.get(id) ?? []).find((d) => d > now) ?? Infinity
    };

    // The standing world: engines at their stands with the cars coupled to them, the other cars on the sidings.
    const loads = new Map<number, Commodity | null>();
    const yards = new Map<StationId, Yard>();
    const yardOf = (station: StationId) => yards.get(station) ?? yards.set(station, new Yard((level) => {
      // A siding's reach in arc length: the cars may stand between these points of its track.
      const lane = geo.lane(station, level, 1), reach = sidingReach(level) - SIDING_MARGIN;
      return { lo: lane.arcAt(-reach), hi: lane.arcAt(reach) };
    })).get(station)!;
    const consists = new Map<string, CarRef[]>();
    const initialStation = new Map<string, StationId>();
    for (const e of trace.initial.engines) initialStation.set(e.id, e.station);
    const cars = [...(trace.initial.cars as Car[])].sort((a, b) => a.id - b.id);
    const byStation = new Map<StationId, { id: number; kind: CarKind }[]>();
    for (const c of cars) {
      this.carKind.set(c.id, c.kind);
      loads.set(c.id, c.load);
      if (c.at.type === "coupled") (consists.get(c.at.engine) ?? consists.set(c.at.engine, []).get(c.at.engine)!).push({ id: c.id, kind: c.kind, phi: 1 });
      else (byStation.get(c.at.station) ?? byStation.set(c.at.station, []).get(c.at.station)!).push({ id: c.id, kind: c.kind });
    }
    for (const [station, list] of byStation) {
      if (!geo.stations.has(station)) continue;
      for (const p of layoutYard(yardOf(station), list, fut)) this.parkedAt.set(p.id, [{ t0: -Infinity, station, car: p }]);
    }
    const refs = (engine: string) => consists.get(engine) ?? consists.set(engine, []).get(engine)!;
    for (const id of this.engineIds) {
      const station = initialStation.get(id) ?? geo.stations.keys().next().value!;
      this.stands.set(id, [{ t0: -Infinity, station, nose: noseAt(id, -1), consist: refs(id).map((c) => ({ ...c })), lead: 1 }]);
    }

    const failed: string[] = [];
    let disrupted: DisruptionNotice | null = null;
    const met: OrderId[] = [], missed: OrderId[] = [];
    const snap = (t: number) => this.keyframes.push({ t, loads: new Map(loads), failed: [...failed], disrupted, met: [...met], missed: [...missed] });
    for (const e of trace.initial.engines) if (e.failed) failed.push(e.id);
    snap(-Infinity);

    // Per hour, whether an arrival opens it (its other events then start a little later) and where its next event goes.
    const arrives = new Set<number>();
    for (const e of ordered) if (e.kind.type === "arrive") arrives.add(e.hour);
    const nextOffset = new Map<number, number>();
    const ready = new Map<string, number>();          // when an engine has arrived and may work or depart
    const stationBusy = new Map<StationId, number>(); // until when a departing train still occupies a station's tracks
    const nose = new Map<string, 1 | -1>();
    let cursor = -Infinity;

    ordered.forEach((e, idx) => {
      now = idx;
      const k = e.kind;
      let t = e.hour;
      let window = 0;
      if (k.type !== "arrive") {
        t = Math.max(e.hour + (nextOffset.get(e.hour) ?? (arrives.has(e.hour) ? STAGGER : 0)), cursor);
        if (k.type === "depart" || (k.type === "op_done" && (k.op.type === "couple" || k.op.type === "uncouple"))) t = Math.max(t, ready.get(k.engine) ?? -Infinity);
        // A train waits for the one that left before it to clear the junction.
        if (k.type === "depart") t = Math.max(t, stationBusy.get(k.from) ?? -Infinity, earliest.get(`${k.engine}#${idx}`) ?? -Infinity);
      }
      if (k.type === "depart") {
        const leg = this.makeLeg(`${k.engine}#${idx}`, k.engine, k.from, k.to, k.arrive_hour, t, nose.get(k.engine) ?? noseAt(k.engine, idx), refs(k.engine).map((c) => ({ ...c })));
        if (leg) {
          const consist = refs(k.engine).map((c) => ({ ...c }));
          this.segs.get(k.engine)?.push({ engine: k.engine, t0: leg.tDepart, t1: leg.tArrive, route: leg.route, sigma: leg.sigma, s0: leg.s0, s1: leg.s1, consist, kind: "leg", station: null, leg });
          this.stands.get(k.engine)?.push({ t0: leg.tArrive, station: k.to, nose: leg.noseEnd, consist: consist.map((c) => ({ ...c })), lead: leg.sigma });
          nose.set(k.engine, leg.noseEnd);
          ready.set(k.engine, leg.tArrive);
          stationBusy.set(k.from, Math.max(stationBusy.get(k.from) ?? -Infinity, this.clearTime(leg, consist.map((c) => c.kind))));
          this.legs.push(leg);
          this.legsByEngine.get(k.engine)?.push(leg);
        }
      } else if (k.type === "op_done" && (k.op.type === "couple" || k.op.type === "uncouple")) {
        const move = this.shunt(k, idx, t, nose.get(k.engine) ?? noseAt(k.engine, idx), yardOf(k.station), refs(k.engine), fut, stationBusy.get(k.station) ?? -Infinity);
        if (move) {
          t = move.t0;
          window = move.t1 - move.t0;
          ready.set(k.engine, move.t1);
        }
      } else if (k.type === "op_done") {
        const op = k.op;
        if (op.type === "load") for (const id of k.cars) loads.set(id, op.commodity);
        else if (op.type === "process") {
          const n = Math.floor(k.cars.length / 2);
          k.cars.forEach((id, i) => loads.set(id, i < n ? null : processed(loads.get(id) ?? null)));
        } else if (op.type === "deliver") for (const id of k.cars) loads.set(id, null);
      } else if (k.type === "disrupted") {
        disrupted = k.what;
        if (k.what.type === "engine_failure") failed.push(k.what.engine);
      } else if (k.type === "order_met") met.push(k.order);
      else if (k.type === "order_missed") missed.push(k.order);
      this.events.push({ t, hour: e.hour, kind: k });
      this.busy.set(e.hour, Math.max(this.busy.get(e.hour) ?? 0, t - e.hour + Math.max(ANIM_HOURS, window)));
      if (k.type !== "arrive") {
        nextOffset.set(e.hour, t - e.hour + Math.max(STAGGER, window + MOVE_GAP));
        cursor = Math.max(cursor, t + window);
      }
      if (["op_done", "disrupted", "order_met", "order_missed"].includes(k.type)) snap(t);
    });
    this.events.sort((a, b) => a.t - b.t);
    this.buildSpans();
    // Signal holds last until the engine's next departure.
    for (const ev of this.events) {
      if (ev.kind.type !== "hold") continue;
      const hold = ev.kind;
      const next = (this.legsByEngine.get(hold.engine) ?? []).find((l) => l.tDepart >= ev.t && l.from === hold.station);
      this.holds.push({ engine: hold.engine, station: hold.station, t0: ev.t, t1: next ? next.tDepart : ev.t + 3, end: next ? next.dFrom : 1 });
    }
  }

  /**
   * The nose direction of every engine while it stands, derived from its departures: the
   * first leg of an engine starts nose first; each later leg leaves from the nose direction
   * in which the previous one ended (see `Geography.legPath`). Returns a lookup by event index.
   */
  private planNoses(ordered: GameEvent[]): { at(engine: string, index: number): 1 | -1 } {
    const changes = new Map<string, { idx: number; start: 1 | -1; end: 1 | -1 }[]>();
    const current = new Map<string, 1 | -1>();
    ordered.forEach((e, idx) => {
      const k = e.kind;
      if (k.type !== "depart") return;
      const throat = this.geo.throatToward(k.from, k.to);
      const start = current.get(k.engine) ?? throat;
      const made = this.geo.legPath(k.from, k.to, this.engineIndex(k.engine), start);
      if (!made) return;
      current.set(k.engine, made.noseEnd);
      (changes.get(k.engine) ?? changes.set(k.engine, []).get(k.engine)!).push({ idx, start, end: made.noseEnd });
    });
    return {
      at: (engine, index) => {
        const list = changes.get(engine) ?? [];
        let nose: 1 | -1 = list[0]?.start ?? 1;
        for (const c of list) if (c.idx < index) nose = c.end;
        return nose;
      }
    };
  }

  /** Builds the leg of an engine that departs at `t` (see `Leg`); null when the stations are not joined by an edge. */
  private makeLeg(key: string, engine: string, from: StationId, to: StationId, arriveHour: number, t: number, noseStart: 1 | -1, consist: CarRef[]): Leg | null {
    const made = this.geo.legPath(from, to, this.engineIndex(engine), noseStart);
    if (!made) return null;
    const dist = Math.abs(made.s1 - made.s0);
    return {
      key, consist, engine, from, to, tDepart: t, tArrive: Math.max(arriveHour, t + 0.3), route: made.route, path: made.path, s0: made.s0, s1: made.s1,
      dFrom: made.dFrom, dTo: made.dTo, sigma: made.sigma, noseStart, noseEnd: made.noseEnd,
      minSeconds: Math.max((1.25 * dist) / LINE_SPEED, Math.sqrt((6.25 * dist) / LINE_ACCEL))
    };
  }

  /** Sim time at which the last vehicle of a train on `leg` has left the necks of its departure station. */
  private clearTime(leg: Leg, kinds: CarKind[]): number {
    const edgeStart = leg.route.pieces[1].arc0;
    const rear = leg.sigma > 0 ? backExtent(kinds) : ENGINE_LENGTH / 2;
    return this.timeAtDistance(leg, Math.min(leg.s1, edgeStart + rear + this.geo.junctionClearance(leg.from, leg.dFrom) + 5));
  }

  /**
   * Plans and records the excursion that couples or uncouples the cars of an `op_done`
   * (see `shunting.ts`): its stretches become segments of the engine's motion starting at
   * `t`, the cars that end up on a siding are parked from the time they are left there, and
   * the engine stands with its new consist when it is back. Returns the move, or null when
   * no car of the op is where the op needs it.
   */
  private shunt(k: Extract<EventKind, { type: "op_done" }>, idx: number, t: number, nose: 1 | -1, yard: Yard, consist: CarRef[], fut: Future, busyUntil: number): Move | null {
    if (!this.geo.stations.has(k.station)) return null;
    const ctx: OpContext = { geo: this.geo, station: k.station, yard, engineLevel: engineLevel(this.engineIndex(k.engine)), nose, consist, fut };
    const coupling = k.op.type === "couple";
    const plan: Excursion | null = coupling ? planCouple(ctx, k.cars) : planUncouple(ctx, k.cars);
    if (!plan) return null;
    let at = Math.max(t, busyUntil);
    const t0 = at;
    const starts: number[] = [];
    for (const seg of plan.segs) {
      const t1 = at + seg.seconds / SECONDS_PER_HOUR;
      starts.push(at);
      this.segs.get(k.engine)?.push({
        engine: k.engine, t0: at, t1, route: seg.route, sigma: seg.sigma, s0: seg.s0, s1: seg.s1, consist: seg.consist,
        kind: seg.hold ? "hold" : "shunt", station: k.station
      });
      at = t1;
    }
    for (const group of plan.parked) {
      for (const car of group.cars) (this.parkedAt.get(car.id) ?? this.parkedAt.set(car.id, []).get(car.id)!).push({ t0: starts[group.seg], station: k.station, car });
    }
    const last = plan.segs[plan.segs.length - 1];
    this.stands.get(k.engine)?.push({ t0: at, station: k.station, nose, consist: consist.map((c) => ({ ...c })), lead: last.sigma });
    const move: Move = { engine: k.engine, station: k.station, op: coupling ? "couple" : "uncouple", cars: k.cars, t0, t1: at, seconds: plan.seconds, distance: plan.distance };
    this.moves.push(move);
    return move;
  }

  /** Pairs `op_started` with `op_done` and gives every op its visual span (couplings span their whole shunting move). */
  private buildSpans(): void {
    const started = new Map<string, TimedEvent>();
    const id = (k: { engine: string; stop: number; index: number }) => `${k.engine}#${k.stop}#${k.index}`;
    for (const ev of this.events) {
      const k = ev.kind;
      if (k.type === "op_started") started.set(id(k), ev);
      else if (k.type === "op_done") {
        const s = started.get(id(k));
        const sk = s?.kind.type === "op_started" ? s.kind : null;
        const move = this.moves.find((m) => m.engine === k.engine && Math.abs(m.t0 - ev.t) < 1e-9 && m.op === k.op.type);
        const t0 = s ? s.t : ev.t;
        this.spans.push({
          engine: k.engine, station: k.station, stop: k.stop, index: k.index, op: k.op, cars: k.cars,
          t0, t1: move ? move.t1 : s ? Math.max(ev.t, t0 + 0.05) : ev.t + ANIM_HOURS, instant: !s,
          hours: sk ? sk.finish_hour - s!.hour : 0, hour: s ? s.hour : ev.hour, finishHour: sk ? sk.finish_hour : ev.hour,
          seconds: move?.seconds
        });
      }
    }
  }

  /** Latest keyframe at or before `t`. */
  private keyframe(t: number): Keyframe {
    let lo = 0, hi = this.keyframes.length - 1;
    while (lo < hi) {
      const mid = (lo + hi + 1) >> 1;
      if (this.keyframes[mid].t <= t) lo = mid; else hi = mid - 1;
    }
    return this.keyframes[lo];
  }

  /** The stretch of motion of an engine that covers `t`, or null while it stands. */
  private segAt(id: string, t: number): Seg | null {
    const list = this.segs.get(id) ?? [];
    let lo = 0, hi = list.length - 1, found = -1;
    while (lo <= hi) {
      const mid = (lo + hi) >> 1;
      if (list[mid].t0 <= t) { found = mid; lo = mid + 1; } else hi = mid - 1;
    }
    return found >= 0 && t < list[found].t1 ? list[found] : null;
  }

  /** The stand of an engine at `t` (the latest one that began by then). */
  private standAt(id: string, t: number): Stand {
    const list = this.stands.get(id)!;
    let found = 0;
    for (let i = 0; i < list.length; i++) if (list[i].t0 <= t) found = i;
    return list[found];
  }

  /** The station an engine stands in at `t` (null while on a leg); during a shunting move it is the station being worked. */
  engineStation(id: string, t: number): StationId | null {
    const seg = this.segAt(id, t);
    if (seg) return seg.station;
    return this.standAt(id, t).station;
  }

  /** Arc length of the engine's centre on the route of `seg` at `t`: the leg's speed profile, or a braked yard run. */
  private arcOf(seg: Seg, t: number): number {
    const d = seg.s1 - seg.s0;
    if (seg.kind === "hold" || d === 0) return seg.s0;
    if (seg.kind === "leg") return seg.s0 + ease((t - seg.t0) / (seg.t1 - seg.t0)) * d;
    return seg.s0 + Math.sign(d) * runDistance(Math.abs(d), (t - seg.t0) * SECONDS_PER_HOUR);
  }

  /**
   * Where every vehicle is at simulated time `t`: engines and the cars coupled to them
   * follow the rails of their current stretch at fixed spacing; all other cars stand where
   * they were last left. A pure function of the trace and the geography: nothing depends on
   * a scene, on earlier calls or on the order of calls.
   */
  poseAt(t: number): WorldPose {
    const engines: EnginePose[] = [];
    const cars = new Map<number, CarPose>();
    const rolling = new Set<number>();
    for (const m of this.moves) if (t >= m.t0 && t < m.t1) m.cars.forEach((c) => rolling.add(c));
    for (const id of this.engineIds) {
      const seg = this.segAt(id, t);
      const body = (kind: CarKind | null) => ({ length: kind ? CAR_LENGTH[kind] : ENGINE_LENGTH, width: kind ? CAR_WIDTH : ENGINE_WIDTH });
      let consist: CarRef[], place: (arc: number) => { p: P3; yaw: number; track: string; s: number }, centre: number, sigma: 1 | -1, lead: 1 | -1, station: StationId | null, moving = false, progress = 0;
      if (seg) {
        const route = seg.route;
        centre = this.arcOf(seg, t); sigma = seg.sigma; lead = seg.sigma; consist = seg.consist; station = seg.station;
        place = (arc) => { const q = route.path.at(arc); return { p: q.p, yaw: q.yaw, ...route.locate(arc) }; };
        moving = seg.kind !== "hold" && seg.s0 !== seg.s1;
        if (seg.leg) progress = Math.min(1, Math.max(0, (t - seg.t0) / (seg.t1 - seg.t0)));
      } else {
        const stand = this.standAt(id, t);
        const level = engineLevel(this.engineIndex(id));
        const lane = this.geo.lane(stand.station, level, 1);
        const track = Geography.yardTrack(stand.station, level);
        centre = lane.arcAt(stand.nose * STAND_CENTRE); sigma = stand.nose; lead = stand.lead; consist = stand.consist; station = stand.station;
        place = (arc) => { const q = lane.path.at(arc); return { p: q.p, yaw: q.yaw, track, s: arc }; };
      }
      // On a route the train runs along its arc direction (`sigma` is its nose along it); standing on a yard track the nose points along the axis (`sigma` = nose).
      const forward = seg ? seg.sigma : 1;
      const offs = offsets(consist.map((c) => c.kind));
      const e = place(centre);
      engines.push({
        id, index: this.engineIndex(id), pos: e.p, yaw: e.yaw + (seg ? (seg.sigma < 0 ? Math.PI : 0) : sigma < 0 ? Math.PI : 0), track: e.track, s: e.s,
        ...body(null), lead, moving, station, legProgress: progress, cars: consist.map((c) => c.id)
      });
      consist.forEach((c, i) => {
        // Cars hang at the engine's tail: behind it on the rails, or ahead of it when it runs tail first.
        const q = place(centre - (seg ? forward : sigma) * offs[i]);
        const flip = (seg ? (seg.sigma < 0 ? Math.PI : 0) : sigma < 0 ? Math.PI : 0) + (c.phi < 0 ? Math.PI : 0);
        cars.set(c.id, { id: c.id, kind: c.kind, pos: q.p, yaw: q.yaw + flip, track: q.track, s: q.s, ...body(c.kind), coupledTo: id, rolling: rolling.has(c.id) });
      });
    }
    for (const [id, hist] of this.parkedAt) {
      if (cars.has(id)) continue;
      let at = hist[0];
      for (const h of hist) if (h.t0 <= t) at = h;
      if (at.t0 > t) continue;
      const lane = this.geo.lane(at.station, at.car.level, 1);
      const s = at.car.s, q = lane.path.at(s);
      cars.set(id, {
        id, kind: at.car.kind, pos: q.p, yaw: q.yaw + (at.car.f < 0 ? Math.PI : 0), track: Geography.yardTrack(at.station, at.car.level), s,
        length: CAR_LENGTH[at.car.kind], width: CAR_WIDTH, coupledTo: null, rolling: rolling.has(id)
      });
    }
    return { t, engines, cars: [...cars.values()].sort((a, b) => a.id - b.id) };
  }

  /** Renders the world at simulated time `t`. */
  evaluate(t: number): Frame {
    const kf = this.keyframe(t);
    const pose = this.poseAt(t);
    const placed = new Map<number, CarFrame>(pose.cars.map((c) => [c.id, { ...c, load: kf.loads.get(c.id) ?? null }]));
    const engines: EngineFrame[] = pose.engines.map((e) => ({ ...e, failed: kf.failed.includes(e.id) }));
    const work: WorkFrame[] = [];
    for (const sp of this.spans) {
      if (t < sp.t0 || t >= sp.t1) continue;
      const p = (t - sp.t0) / (sp.t1 - sp.t0);
      work.push({ engine: sp.engine, station: sp.station, op: sp.op, progress: p, hours: sp.hours, cars: sp.cars });
      // The state flips at the event for instant ops (animate after it) and at the end for timed ops (animate before it).
      const before = (sp.instant ? this.keyframe(sp.t0 - 1e-6) : kf).loads;
      if (sp.op.type === "load") {
        const commodity = sp.op.commodity;
        for (const id of sp.cars) { const c = placed.get(id); if (c) placed.set(id, { ...c, gauge: { commodity, fill: p } }); }
      } else if (sp.op.type === "deliver") {
        for (const id of sp.cars) {
          const c = placed.get(id), was = before.get(id);
          if (c && was) placed.set(id, { ...c, load: was, gauge: { commodity: was, fill: 1 - p } });
        }
      } else if (sp.op.type === "process") {
        const n = Math.floor(sp.cars.length / 2);
        sp.cars.forEach((id, i) => {
          const c = placed.get(id), was = before.get(id);
          if (!c) return;
          if (i < n && was) placed.set(id, { ...c, load: was, gauge: { commodity: was, fill: 1 - p } });
          else if (i >= n) placed.set(id, { ...c, load: null, gauge: { commodity: "juice", fill: p } });
        });
      }
    }
    const reds = this.holds.filter((h) => t >= h.t0 && t < h.t1).map((h) => ({ station: h.station, end: h.end, engine: h.engine }));
    return { t, engines, cars: [...placed.values()], reds, disrupted: kf.disrupted, met: kf.met, missed: kf.missed, work };
  }

  /** Visual time of the first event satisfying the predicate, or null. */
  firstTime(pred: (e: TimedEvent) => boolean): number | null {
    const found = this.events.find(pred);
    return found ? found.t : null;
  }

  /**
   * Sim time at which the engine of `leg` is at arc length `s` of the leg's path
   * (inverse of the speed profile), used to frame trackside passes.
   */
  timeAtDistance(leg: Leg, s: number): number {
    const f = Math.min(1, Math.max(0, (s - leg.s0) / (leg.s1 - leg.s0)));
    let lo = 0, hi = 1;
    for (let i = 0; i < 24; i++) { const m = (lo + hi) / 2; if (ease(m) < f) lo = m; else hi = m; }
    return leg.tDepart + lo * (leg.tArrive - leg.tDepart);
  }
}

/** Narrow helper for callers that only have trace events. */
export function eventsOf(trace: Trace): GameEvent[] { return trace.events; }

