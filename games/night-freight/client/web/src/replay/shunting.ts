/**
 * Shunting: how an engine with its consist works a station yard.
 *
 * Owns the physical size of the rolling stock, the yard bookkeeping (which siding
 * holds which cars, at which axis coordinates), the planning of every coupling and
 * uncoupling as a sequence of rail-bound runs, and the speed profile of those runs.
 * It consumes a `Geography` (tracks, turnouts, necks) and answers with `Excursion`s:
 * lists of `PlannedSeg`s, each a run of one train over one `Route` or a hold at a
 * standstill, in physical seconds. The timeline places them in simulated time and
 * evaluates them; nothing here knows about events, hours or the 3D scene.
 *
 * The physics it plans (callers may rely on every point):
 * - A car has no motor. A train (the engine and its consist) drives to a car,
 *   couples it at the far end of the consist and pulls it out; an uncoupling is the
 *   reverse. The consist hangs at the engine's tail, so the train always enters a
 *   siding tail first, through the throat its nose points at, with the engine
 *   pushing, and leaves it nose first.
 * - Tracks of a yard merge into their neighbours only at the throats (the ladder in
 *   `geography.ts`), so a train changes track by running out until its last vehicle
 *   is clear of the turnouts and reversing, never by turning.
 * - A siding holds one contiguous cut of cars, coupled at `COUPLER_GAP`. A train
 *   reaches only the car at the end of a cut that faces its throat; cars further in
 *   are reached by first moving the cars in front elsewhere, never by passing
 *   through them.
 * - Cars park only where they keep `FOUL_CLEARANCE` from the neighbouring tracks,
 *   so no train can foul them.
 */
import type { CarKind, StationId } from "../game";
import { Geography, SIDING_LEVELS, STAND_CENTRE, STATION_EXTENT, divergencePoint, type Route, type RoutePiece } from "../scene/geography";

export const ENGINE_LENGTH = 17;
export const ENGINE_WIDTH = 3.2;
export const CAR_LENGTH = { boxcar: 15.2, tanker: 12 } as const;
export const CAR_WIDTH = 3;
/** Distance between the ends of two coupled vehicles, metres. */
export const COUPLER_GAP = 0.9;

/** Simulated hours are real hours for yard work: a run of `n` seconds takes `n / SECONDS_PER_HOUR` hours. */
export const SECONDS_PER_HOUR = 3600;
/** Top speed of a train in the yard, metres per second (about 25 km/h). */
export const YARD_SPEED = 7;
/** Acceleration and braking of a yard run, metres per second squared. */
export const YARD_ACCEL = 1.0;
export const YARD_DECEL = 1.2;
/** Seconds a train stands after reaching a cusp (the points are thrown and the driver changes ends). */
export const CUSP_DWELL = 3;
/** Seconds a train stands while a car is coupled or uncoupled. */
export const COUPLE_DWELL = 4;
/** Metres a train keeps clear of the turnouts beyond the point where its last vehicle is just off them. */
const CLEAR_MARGIN = 1;
/** Metres of track laid behind and ahead of a train beyond its own length, so that routes cover every vehicle. */
const ROUTE_PAD = 12;

/** A car in a consist: its identity and its facing relative to the engine's nose (+1 same way, -1 opposite). */
export interface CarRef { id: number; kind: CarKind; phi: 1 | -1 }

/** A car standing on a siding: arc length `s` of its centre on the siding's yard track (`Geography.yardTrack`) and the axis direction `f` it faces. */
export interface Parked { id: number; kind: CarKind; level: number; s: number; f: 1 | -1 }

/** What the run of events tells the planner about the future of a car, to pick sidings from which it can leave. */
export interface Future {
  /** The next coupling of the car: its rank (order among events) and the throat (`+1`/`-1`) its engine will use. */
  pickup(id: number): { rank: number; end: 1 | -1 } | null;
  /** Rank of the next uncoupling of the car (larger than any rank when there is none). */
  dropRank(id: number): number;
}

/** Centre-to-centre distances from an engine's centre to each car at its tail, for the kinds of the consist. */
export function offsets(kinds: CarKind[]): number[] {
  const out: number[] = [];
  let prevHalf = ENGINE_LENGTH / 2, at = 0;
  for (const kind of kinds) {
    const half = CAR_LENGTH[kind] / 2;
    at += prevHalf + COUPLER_GAP + half;
    out.push(at);
    prevHalf = half;
  }
  return out;
}

/** Distance from the engine's centre to the far end of its last vehicle (the engine's own half length when it has no cars). */
export function backExtent(kinds: CarKind[]): number {
  if (!kinds.length) return ENGINE_LENGTH / 2;
  return offsets(kinds)[kinds.length - 1] + CAR_LENGTH[kinds[kinds.length - 1]] / 2;
}

/** Seconds a run of `d` metres takes from rest to rest: accelerate, cruise at `YARD_SPEED`, brake. */
export function runSeconds(d: number): number {
  const up = (YARD_SPEED * YARD_SPEED) / (2 * YARD_ACCEL), down = (YARD_SPEED * YARD_SPEED) / (2 * YARD_DECEL);
  if (d >= up + down) return YARD_SPEED / YARD_ACCEL + YARD_SPEED / YARD_DECEL + (d - up - down) / YARD_SPEED;
  const peak = Math.sqrt((2 * d) / (1 / YARD_ACCEL + 1 / YARD_DECEL));
  return peak / YARD_ACCEL + peak / YARD_DECEL;
}

/** Metres covered `tau` seconds into a run of `d` metres whose duration is `runSeconds(d)`. */
export function runDistance(d: number, tau: number): number {
  const total = runSeconds(d);
  if (tau <= 0) return 0;
  if (tau >= total) return d;
  const up = (YARD_SPEED * YARD_SPEED) / (2 * YARD_ACCEL), down = (YARD_SPEED * YARD_SPEED) / (2 * YARD_DECEL);
  const peak = d >= up + down ? YARD_SPEED : Math.sqrt((2 * d) / (1 / YARD_ACCEL + 1 / YARD_DECEL));
  const tUp = peak / YARD_ACCEL, tDown = peak / YARD_DECEL;
  if (tau < tUp) return 0.5 * YARD_ACCEL * tau * tau;
  if (tau > total - tDown) { const r = total - tau; return d - 0.5 * YARD_DECEL * r * r; }
  return (peak * peak) / (2 * YARD_ACCEL) + peak * (tau - tUp);
}

/**
 * The cars parked on the sidings of one station. Each siding holds one cut of cars,
 * kept in ascending arc length. Cars never shift once parked: they are added to
 * and removed from the ends of a cut only. `limits` gives, per siding, the arc lengths
 * between which cars may stand (the siding's reach, see `sidingReach`).
 */
export class Yard {
  readonly cuts = new Map<number, Parked[]>(SIDING_LEVELS.map((l) => [l, []]));
  constructor(private readonly limits: (level: number) => { lo: number; hi: number }) {}

  /** The parked record of a car, or undefined when it is not parked here. */
  find(id: number): Parked | undefined {
    for (const cut of this.cuts.values()) { const p = cut.find((c) => c.id === id); if (p) return p; }
    return undefined;
  }

  /** The cars at the `nu` end of siding `level` that are all in `wanted`, nearest to that end first. */
  block(level: number, nu: 1 | -1, wanted: Set<number>): Parked[] {
    const cut = this.cuts.get(level)!;
    const ordered = nu > 0 ? [...cut].reverse() : [...cut];
    const out: Parked[] = [];
    for (const p of ordered) { if (!wanted.has(p.id)) break; out.push(p); }
    return out;
  }

  /** Records cars as parked. */
  add(cars: Parked[]): void {
    for (const p of cars) {
      const cut = this.cuts.get(p.level)!;
      cut.push(p);
      cut.sort((a, b) => a.s - b.s);
    }
  }

  /** Removes cars from the yard. */
  remove(ids: number[]): void {
    for (const [level, cut] of this.cuts) this.cuts.set(level, cut.filter((p) => !ids.includes(p.id)));
  }

  /**
   * Arc lengths at which a block of cars (nearest to the engine first, so the last one
   * is pushed furthest in) stands once pushed in from the `nu` end of siding `level` up to the
   * cut, or null when it does not fit within the siding's reach. On an empty siding the block
   * stands at its far end so that the near end stays free for more cars.
   */
  place(level: number, nu: 1 | -1, cars: { kind: CarKind }[]): number[] | null {
    const { lo, hi } = this.limits(level);
    const cut = this.cuts.get(level)!;
    const end = cut.length ? (nu > 0 ? cut[cut.length - 1] : cut[0]) : null;
    const half = (k: CarKind) => CAR_LENGTH[k] / 2;
    const m = cars.length;
    const at = new Array<number>(m);
    at[m - 1] = end ? end.s + nu * (half(end.kind) + COUPLER_GAP + half(cars[m - 1].kind)) : nu > 0 ? lo + half(cars[m - 1].kind) : hi - half(cars[m - 1].kind);
    for (let j = m - 2; j >= 0; j--) at[j] = at[j + 1] + nu * (half(cars[j + 1].kind) + COUPLER_GAP + half(cars[j].kind));
    const outer = at[0] + nu * half(cars[0].kind);
    return nu > 0 ? (outer <= hi + 1e-6 ? at : null) : outer >= lo - 1e-6 ? at : null;
  }
}

/** How many cars of a cut (ascending axis coordinate) stand behind a car that leaves later through the same end. */
function blocked(order: { id: number }[], fut: Future): number {
  let n = 0;
  order.forEach((x, i) => {
    const f = fut.pickup(x.id);
    if (!f) return;
    const between = f.end > 0 ? order.slice(i + 1) : order.slice(0, i);
    if (between.some((y) => (fut.pickup(y.id)?.rank ?? Infinity) > f.rank)) n++;
  });
  return n;
}

/**
 * The siding on which a block of cars, entering from throat `nu`, stands best: where it
 * (and the cars already there) block the fewest future pickups, preferably beside cars
 * that the same coupling will take, on the side of the engine's own track and close to it.
 * Returns the level and the arc lengths of the cars, or null when nothing fits.
 */
export function chooseSiding(yard: Yard, nu: 1 | -1, cars: { id: number; kind: CarKind }[], fut: Future, engineLevel: number, excluded: Set<number>): { level: number; us: number[] } | null {
  let best: { level: number; us: number[]; score: number[] } | undefined;
  /** Lexicographic comparison of two scores, smaller first. */
  const before = (a: number[], b: number[]) => { for (let i = 0; i < a.length; i++) if (a[i] !== b[i]) return a[i] < b[i]; return false; };
  SIDING_LEVELS.forEach((level, order) => {
    if (excluded.has(level)) return;
    const us = yard.place(level, nu, cars);
    if (!us) return;
    const cut = yard.cuts.get(level)!;
    const added = cars.map((c, i) => ({ id: c.id, s: us[i] })).sort((a, b) => a.s - b.s);
    const arranged = (nu > 0 ? [...cut, ...added] : [...added, ...cut]).map((c) => ({ id: c.id }));
    const rank = fut.pickup(cars[0].id)?.rank;
    const together = rank !== undefined && cut.some((c) => fut.pickup(c.id)?.rank === rank) ? 0 : 1;
    const side = engineLevel !== 0 && Math.sign(level) !== Math.sign(engineLevel) ? 1 : 0;
    const score = [blocked(arranged, fut), together, side, cut.length ? 0 : 1, Math.abs(level), order];
    if (!best || before(score, best.score)) best = { level, us, score };
  });
  const found = best as { level: number; us: number[] } | undefined;
  return found ? { level: found.level, us: found.us } : null;
}

/**
 * Lays the cars of a station out on its sidings for the start of a run: the cars that
 * leave last stand deepest, cars of one coupling stand together with the earliest to be
 * uncoupled furthest in, and every car is parked at the end of its siding that the engine
 * which takes it will approach, facing the way that engine's nose points.
 */
export function layoutYard(yard: Yard, cars: { id: number; kind: CarKind }[], fut: Future): Parked[] {
  const order = [...cars].sort((a, b) => (fut.pickup(b.id)?.rank ?? Infinity) - (fut.pickup(a.id)?.rank ?? Infinity) || fut.dropRank(a.id) - fut.dropRank(b.id) || a.id - b.id);
  const out: Parked[] = [];
  for (const c of order) {
    const end = fut.pickup(c.id)?.end ?? 1;
    const at = chooseSiding(yard, end, [c], fut, 0, new Set());
    if (!at) throw new Error(`no room on the sidings for car ${c.id}`);
    const parked: Parked = { id: c.id, kind: c.kind, level: at.level, s: at.us[0], f: end };
    yard.add([parked]);
    out.push(parked);
  }
  return out;
}

/** One stretch of an excursion: a run of the train over a route (`hold` false) or a stand (`hold` true, `s0 === s1`). */
export interface PlannedSeg {
  route: Route;
  /** +1 when the engine's nose leads along the route, -1 when it runs tail first. */
  sigma: 1 | -1;
  /** Arc lengths of the engine's centre on the route at the start and at the end. */
  s0: number;
  s1: number;
  /** The vehicles of the train, in force for the whole stretch. */
  consist: CarRef[];
  hold: boolean;
  seconds: number;
}

/** Everything one coupling or uncoupling does: the runs of the train and the cars that stand down at the end of a stretch. */
export interface Excursion {
  segs: PlannedSeg[];
  /** Cars that have been uncoupled and stand on a siding from the start of stretch `seg` on. */
  parked: { seg: number; cars: Parked[] }[];
  seconds: number;
  /** Metres the engine runs in all. */
  distance: number;
}

/** Everything the planner needs to know about the engine and the station. */
export interface OpContext {
  geo: Geography;
  station: StationId;
  yard: Yard;
  engineLevel: number;
  nose: 1 | -1;
  /** The consist of the engine; the planner leaves it in its final state. */
  consist: CarRef[];
  fut: Future;
}

/** One step of an excursion: couple a block of parked cars, or leave a block of the consist's far end on a siding. */
type Step =
  | { type: "take"; level: number; cars: Parked[] }
  | { type: "drop"; level: number; cars: CarRef[]; us: number[] };

/**
 * Builds the runs of one excursion step by step, starting and ending at the engine's stand.
 * Positions are arc lengths on the yard tracks (`Geography.yardTrack`, which grow with the axis
 * coordinate), so that every distance between vehicles is a distance along the rails.
 */
class Shunt {
  readonly segs: PlannedSeg[] = [];
  readonly parked: { seg: number; cars: Parked[] }[] = [];
  /** Where the engine stands: its track and the arc length of its centre. */
  private cur: { level: number; s: number };
  /** Index of the stand after a flip that still waits for the route of the run that follows it. */
  private pending: number | null = null;

  constructor(private readonly ctx: OpContext) {
    this.cur = { level: ctx.engineLevel, s: this.lane(ctx.engineLevel).arcAt(ctx.nose * STAND_CENTRE) };
  }

  private lane(level: number) { return this.ctx.geo.lane(this.ctx.station, level, 1); }

  private piece(level: number, s0: number, s1: number): RoutePiece {
    return { track: Geography.yardTrack(this.ctx.station, level), from: s0, to: s1 };
  }

  /** Adds a run (or hold) and gives a waiting post-flip stand its route. */
  private push(seg: PlannedSeg): void {
    if (this.pending !== null) {
      const waiting = this.segs[this.pending];
      waiting.route = seg.route; waiting.sigma = seg.sigma; waiting.s0 = waiting.s1 = seg.s0;
      this.pending = null;
    }
    this.segs.push(seg);
  }

  /**
   * Runs from where the train stands out along its track to the cusp beyond the
   * turnouts, stands there, and runs back (tail first) over the switches onto siding
   * `level` as far as arc length `sT`, where the engine's centre is to stand.
   */
  private approach(level: number, sT: number): void {
    const { ctx } = this, nu = ctx.nose;
    const consist = ctx.consist.map((c) => ({ ...c }));
    const ext = backExtent(consist.map((c) => c.kind));
    const from = this.lane(this.cur.level), to = this.lane(level);
    const sw = divergencePoint(this.cur.level, level);
    // The cusp: far enough out that the last vehicle (the far end of the consist) is clear of the point where the two tracks part.
    const uHere = from.uAt(this.cur.s);
    const uc = nu * Math.min(STATION_EXTENT - ENGINE_LENGTH / 2 - 0.5, Math.max(nu * uHere + 1, sw + CLEAR_MARGIN + ext));
    const sStart = this.cur.s - nu * (ext + ROUTE_PAD), sc = from.arcAt(uc);
    const out = ctx.geo.route([this.piece(this.cur.level, sStart, sc)]);
    const s0 = Math.abs(this.cur.s - sStart), s1 = Math.abs(sc - sStart);
    this.push({ route: out, sigma: 1, s0, s1, consist, hold: false, seconds: runSeconds(s1 - s0) });
    // Back in: over the common stretch, then onto the siding.
    const sEnd = sT - nu * (ext + ROUTE_PAD);
    const swFrom = from.arcAt(nu * sw), swTo = to.arcAt(nu * sw);
    const pieces: RoutePiece[] = sw > 0 ? [this.piece(this.cur.level, sc, swFrom), this.piece(level, swTo, sEnd)] : [this.piece(level, sc, sEnd)];
    const back = ctx.geo.route(pieces);
    const toTarget = sw > 0 ? Math.abs(sc - swFrom) + Math.abs(swTo - sT) : Math.abs(sc - sT);
    this.push({ route: back, sigma: -1, s0: 0, s1: 0, consist, hold: true, seconds: CUSP_DWELL });
    this.push({ route: back, sigma: -1, s0: 0, s1: toTarget, consist, hold: false, seconds: runSeconds(toTarget) });
  }

  /** Performs one step: runs to the cars or to the siding, couples or uncouples, and leaves the train standing there. */
  go(step: Step): void {
    const { ctx } = this, nu = ctx.nose;
    const kinds = ctx.consist.map((c) => c.kind);
    let sT: number;
    if (step.type === "take") {
      const c1 = step.cars[0];
      const off = offsets([...kinds, c1.kind]);
      sT = c1.s + nu * off[off.length - 1];
    } else {
      const index = ctx.consist.length - step.cars.length;
      sT = step.us[0] + nu * offsets(kinds)[index];
    }
    this.approach(step.level, sT);
    // Contact: the cars change hands while the train stands.
    if (step.type === "take") {
      for (const p of step.cars) ctx.consist.push({ id: p.id, kind: p.kind, phi: (p.f * nu) as 1 | -1 });
      ctx.yard.remove(step.cars.map((p) => p.id));
    } else {
      const left = ctx.consist.splice(ctx.consist.length - step.cars.length, step.cars.length);
      const cars = left.map((c, i) => ({ id: c.id, kind: c.kind, level: step.level, s: step.us[i], f: (c.phi * nu) as 1 | -1 }));
      ctx.yard.add(cars);
      this.parked.push({ seg: this.segs.length, cars });
    }
    const last = this.segs[this.segs.length - 1];
    this.segs.push({ route: last.route, sigma: 1, s0: 0, s1: 0, consist: ctx.consist.map((c) => ({ ...c })), hold: true, seconds: COUPLE_DWELL });
    this.pending = this.segs.length - 1;
    this.cur = { level: step.level, s: sT };
  }

  /** Finishes the excursion: back to the engine's own track and the stand where it started. */
  finish(): Excursion {
    const { ctx } = this, nu = ctx.nose;
    this.approach(ctx.engineLevel, this.lane(ctx.engineLevel).arcAt(nu * STAND_CENTRE));
    const seconds = this.segs.reduce((a, s) => a + s.seconds, 0);
    const distance = this.segs.reduce((a, s) => a + Math.abs(s.s1 - s.s0), 0);
    return { segs: this.segs, parked: this.parked, seconds, distance };
  }
}

/**
 * Couples the cars `wanted` that stand in this station's yard to the engine: the train
 * takes every block of wanted cars that is at the end of a siding facing its throat; when
 * a wanted car lies behind other cars, it first takes those in front, leaves them on another
 * siding, and then takes the wanted car.
 */
function takeAll(shunt: Shunt, ctx: OpContext, wanted: Set<number>): void {
  const nu = ctx.nose;
  for (let guard = 0; wanted.size && guard < 60; guard++) {
    const blocks = SIDING_LEVELS.map((level) => ({ level, cars: ctx.yard.block(level, nu, wanted) })).filter((b) => b.cars.length);
    if (blocks.length) {
      // The block holding a car that will be uncoupled soonest is taken last, so that it ends up at the far end.
      const key = (b: { cars: Parked[] }) => Math.min(...b.cars.map((c) => ctx.fut.dropRank(c.id)));
      blocks.sort((a, b) => key(b) - key(a) || Math.abs(a.level) - Math.abs(b.level));
      const pick = blocks[0];
      shunt.go({ type: "take", level: pick.level, cars: pick.cars });
      pick.cars.forEach((c) => wanted.delete(c.id));
      continue;
    }
    // Every wanted car stands behind other cars: move the cars in front of the one with the fewest.
    let best: { level: number; ahead: Parked[] } | null = null;
    for (const [level, cut] of ctx.yard.cuts) {
      const ordered = nu > 0 ? [...cut].reverse() : [...cut];
      const at = ordered.findIndex((p) => wanted.has(p.id));
      if (at > 0 && (!best || at < best.ahead.length)) best = { level, ahead: ordered.slice(0, at) };
    }
    if (!best) break;
    shunt.go({ type: "take", level: best.level, cars: best.ahead });
    const block = ctx.consist.slice(ctx.consist.length - best.ahead.length);
    const at = chooseSiding(ctx.yard, nu, block, ctx.fut, ctx.engineLevel, new Set([best.level]));
    if (!at) throw new Error("no room on the sidings to move cars out of the way");
    shunt.go({ type: "drop", level: at.level, cars: block, us: at.us });
  }
}

/**
 * Plans the coupling of cars `ids` by the engine described in `ctx`: the engine, with its
 * consist, drives through the turnouts onto the sidings that hold the cars, couples them at
 * the far end of the consist and pulls out, then returns to its stand. Cars not in this
 * yard are ignored. Updates `ctx.consist` and `ctx.yard`; returns null when nothing is to be coupled.
 */
export function planCouple(ctx: OpContext, ids: number[]): Excursion | null {
  const wanted = new Set(ids.filter((id) => ctx.yard.find(id)));
  if (!wanted.size) return null;
  const shunt = new Shunt(ctx);
  takeAll(shunt, ctx, wanted);
  return shunt.finish();
}

/**
 * Plans the uncoupling of cars `ids` from the engine's consist: the train pushes the cars
 * at the far end of the consist onto a siding, leaves them there and pulls out. A car that
 * is not at the far end is reached by leaving every car behind it on a siding of its own and
 * taking back those that stay. Updates `ctx.consist` and `ctx.yard`; returns null when
 * none of the cars is coupled.
 */
export function planUncouple(ctx: OpContext, ids: number[]): Excursion | null {
  const leaving = new Set(ids.filter((id) => ctx.consist.some((c) => c.id === id)));
  if (!leaving.size) return null;
  const shunt = new Shunt(ctx);
  const nu = ctx.nose;
  const first = ctx.consist.findIndex((c) => leaving.has(c.id));
  // Runs of the far part of the consist, from the far end back, alternating between leaving and staying cars.
  const runs: { cars: CarRef[]; leave: boolean }[] = [];
  for (let i = ctx.consist.length - 1; i >= first; i--) {
    const c = ctx.consist[i], leave = leaving.has(c.id);
    const run = runs[runs.length - 1];
    if (run && run.leave === leave) run.cars.unshift(c); else runs.push({ cars: [c], leave });
  }
  const used = new Set<number>();
  const stay = new Set<number>();
  for (const run of runs) {
    const at = chooseSiding(ctx.yard, nu, run.cars, ctx.fut, ctx.engineLevel, used) ?? chooseSiding(ctx.yard, nu, run.cars, ctx.fut, ctx.engineLevel, new Set());
    if (!at) throw new Error("no room on the sidings to leave cars");
    used.add(at.level);
    shunt.go({ type: "drop", level: at.level, cars: run.cars, us: at.us });
    if (!run.leave) run.cars.forEach((c) => stay.add(c.id));
  }
  takeAll(shunt, ctx, stay);
  return shunt.finish();
}
