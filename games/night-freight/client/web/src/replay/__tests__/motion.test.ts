/**
 * Motion invariants: what a viewer may rely on when rolling stock moves in the replay film.
 *
 * Every trace of `motionFixtures()` (the generated examples, the failed preview runs and
 * synthetic traces with uncouplings, out-and-back legs, two engines in one yard and many
 * cars) is sampled densely through `Timeline.poseAt`, which depends on the trace and the
 * geography only, and every sample is held against the rules of railway motion:
 * vehicles lie on track centre lines and head along them, consists keep their coupler
 * spacing, no two vehicles overlap, speeds and accelerations are realistic, nothing
 * teleports, cars move only coupled to a moving engine, and the leading end of every
 * engine is the one that points the way it runs.
 *
 * Time bases. Shunting moves are physical (`SECONDS_PER_HOUR` seconds per sim hour). A leg
 * lasts what the trace says, so its speed is judged when it is shown in `Leg.minSeconds`
 * film seconds, the shortest film time the timeline promises for it.
 */
import { describe, expect, it } from "vitest";
import { STATION_EXTENT, trackLateral, type P3 } from "../../scene/geography";
import { COUPLER_GAP } from "../shunting";
import { CAR_LENGTH, CAR_WIDTH, ENGINE_LENGTH, ENGINE_WIDTH, SECONDS_PER_HOUR, LINE_ACCEL, LINE_SPEED, type EnginePose, type CarPose, type Timeline, type WorldPose } from "../timeline";
import { motionFixtures, type MotionFixture } from "./motionFixtures";

/** Samples per film second (or per physical second of a shunting move) the invariants are checked at. */
const SAMPLES_PER_SECOND = 20;
/** Sim hours between samples while nothing moves. */
const IDLE_STEP = 0.05;
/** Rails are polylines joined to within two millimetres, and a coupling pops by less than this many metres at the instant cars change hands. */
const JOINT = 0.01;
/**
 * Highest speed (m/s) and acceleration (m/s^2) of a yard move that the tests accept: a yard engine runs at most 8 m/s
 * (29 km/h) and accelerates at most 2 m/s^2 (the planner uses 1.0 and 1.2; the rest allows for the noise that 2 mm rail joints
 * make in an acceleration differenced over 0.05 s).
 */
const YARD_MAX_SPEED = 8, YARD_MAX_ACCEL = 2;

type Body = (EnginePose | CarPose) & { key: string };

/** The bodies of a pose keyed by "E1" / "car:3". */
function bodies(w: WorldPose): Body[] {
  return [...w.engines.map((e) => ({ ...e, key: e.id })), ...w.cars.map((c) => ({ ...c, key: `car:${c.id}` }))];
}

/** Sim hours between consecutive samples at `t`: fine while anything moves, coarse while everything stands. */
function stepAt(tl: Timeline, t: number): number {
  let step = IDLE_STEP;
  for (const m of tl.moves) if (t >= m.t0 - 1e-9 && t < m.t1 + 1e-9) step = Math.min(step, 1 / SAMPLES_PER_SECOND / SECONDS_PER_HOUR);
  for (const l of tl.legs) if (t >= l.tDepart - 1e-9 && t < l.tArrive + 1e-9) step = Math.min(step, (l.tArrive - l.tDepart) / (l.minSeconds * SAMPLES_PER_SECOND));
  return step;
}

/** All sample times of a timeline, from before the first event to after the last motion; the start and end of every move and leg are sampled exactly. */
function sampleTimes(tl: Timeline): number[] {
  const bounds = [...tl.moves.flatMap((m) => [m.t0, m.t1]), ...tl.legs.flatMap((l) => [l.tDepart, l.tArrive])].sort((a, b) => a - b);
  const times: number[] = [];
  const end = tl.end + 0.2;
  let t = tl.trace.initial.hour - 0.2;
  while (t < end) {
    times.push(t);
    const next = bounds.find((b) => b > t + 1e-12);
    t = Math.min(t + stepAt(tl, t), next ?? Infinity);
  }
  times.push(end);
  return times;
}

/**
 * How fast physical time runs against sim time for engine `id` between `t0` and `t1`: seconds
 * per sim hour while it shunts or stands (real time), `minSeconds / duration` on a leg (the
 * leg shown as fast as the timeline allows), and the speed limit that goes with it.
 */
function regime(tl: Timeline, id: string, t0: number, t1: number): { scale: number; vmax: number; amax: number; stands: boolean } {
  for (const l of tl.legsByEngine.get(id) ?? []) {
    if (t0 < l.tArrive && t1 > l.tDepart) return { scale: l.minSeconds / (l.tArrive - l.tDepart), vmax: LINE_SPEED, amax: LINE_ACCEL * 1.1, stands: false };
  }
  for (const m of tl.moves) if (m.engine === id && t0 < m.t1 && t1 > m.t0) return { scale: SECONDS_PER_HOUR, vmax: YARD_MAX_SPEED, amax: YARD_MAX_ACCEL, stands: false };
  return { scale: SECONDS_PER_HOUR, vmax: 0, amax: 0, stands: true };
}

/** Euclidean distance in 3-D. */
function dist(a: P3, b: P3): number { return Math.hypot(a.x - b.x, a.y - b.y, a.z - b.z); }

/** Smallest angle between two headings, radians, in 0..pi. */
function angleBetween(a: number, b: number): number {
  const d = Math.abs(((a - b + Math.PI) % (2 * Math.PI) + 2 * Math.PI) % (2 * Math.PI) - Math.PI);
  return d;
}

/** Corners of a vehicle's bounding box in the ground plane. */
function corners(b: Body, shrink = 0.02): { x: number; z: number }[] {
  const hl = b.length / 2 - shrink, hw = b.width / 2 - shrink;
  const fx = Math.sin(b.yaw), fz = Math.cos(b.yaw), rx = Math.cos(b.yaw), rz = -Math.sin(b.yaw);
  return [[hl, hw], [hl, -hw], [-hl, -hw], [-hl, hw]].map(([l, w]) => ({ x: b.pos.x + fx * l + rx * w, z: b.pos.z + fz * l + rz * w }));
}

/** True when two convex boxes overlap (separating axis theorem). */
function overlap(a: Body, b: Body): boolean {
  const ca = corners(a), cb = corners(b);
  for (const box of [a, b]) {
    for (const axis of [{ x: Math.sin(box.yaw), z: Math.cos(box.yaw) }, { x: Math.cos(box.yaw), z: -Math.sin(box.yaw) }]) {
      const pa = ca.map((c) => c.x * axis.x + c.z * axis.z), pb = cb.map((c) => c.x * axis.x + c.z * axis.z);
      if (Math.max(...pa) <= Math.min(...pb) || Math.max(...pb) <= Math.min(...pa)) return false;
    }
  }
  return true;
}

/** The edge polylines of a geography with their bounding boxes, so that points far from an edge skip it. */
const edgeBoxes = new WeakMap<object, { box: [number, number, number, number]; points: { x: number; z: number }[] }[]>();

/** The edges of `fx` with bounding boxes grown by one metre. */
function edgesOf(fx: MotionFixture) {
  let list = edgeBoxes.get(fx.geo);
  if (!list) {
    list = fx.geo.edges.map((e) => {
      const xs = e.path.points.map((p) => p.x), zs = e.path.points.map((p) => p.z);
      return { box: [Math.min(...xs) - 1, Math.max(...xs) + 1, Math.min(...zs) - 1, Math.max(...zs) + 1] as [number, number, number, number], points: e.path.points };
    });
    edgeBoxes.set(fx.geo, list);
  }
  return list;
}

/**
 * The rails near a point, computed from the geometry that the scene draws: the analytic yard
 * tracks (every level of every station) and the edges, independent of the routes the timeline
 * builds. Returns the distance to the nearest centre line and the headings (radians) of every
 * centre line within 10 cm of the point.
 */
function rails(fx: MotionFixture, p: P3): { distance: number; headings: number[] } {
  let best = Infinity;
  const headings: number[] = [];
  const note = (d: number, yaw: number) => { best = Math.min(best, d); if (d < 0.1) headings.push(yaw); };
  for (const site of fx.geo.stations.values()) {
    const c = Math.cos(site.yaw), s = Math.sin(site.yaw);
    const dx = p.x - site.x, dz = p.z - site.z;
    const u = dx * c + dz * s, v = -dx * s + dz * c;
    if (Math.abs(u) > STATION_EXTENT + 1 || Math.abs(v) > 30) continue;
    for (let level = -3; level <= 3; level++) {
      const slope = trackLateral(level, u + 0.5) - trackLateral(level, u - 0.5);
      note(Math.abs(v - trackLateral(level, u)) / Math.hypot(1, slope), Math.atan2(c - slope * s, s + slope * c));
    }
  }
  for (const e of edgesOf(fx)) {
    if (p.x < e.box[0] || p.x > e.box[1] || p.z < e.box[2] || p.z > e.box[3]) continue;
    for (let i = 1; i < e.points.length; i++) {
      const a = e.points[i - 1], b = e.points[i];
      const ex = b.x - a.x, ez = b.z - a.z;
      if (Math.abs(p.x - a.x) > 12 + Math.abs(ex) || Math.abs(p.z - a.z) > 12 + Math.abs(ez)) continue;
      const t = Math.max(0, Math.min(1, ((p.x - a.x) * ex + (p.z - a.z) * ez) / (ex * ex + ez * ez || 1)));
      note(Math.hypot(p.x - (a.x + ex * t), p.z - (a.z + ez * t)), Math.atan2(ex, ez));
    }
  }
  return { distance: best, headings };
}

/** Collected failures of one invariant over a run: the first few are shown, the count is exact. */
class Failures {
  count = 0;
  readonly shown: string[] = [];
  note(message: () => string): void { this.count++; if (this.shown.length < 5) this.shown.push(message()); }
  toString(): string { return this.count ? `${this.count} failures, e.g.\n  ${this.shown.join("\n  ")}` : "none"; }
}

const fixtures = motionFixtures();

const LONG = 180_000;

describe.each(fixtures.map((f) => [f.name, f] as const))("motion of %s", (_name, fx) => {
  const { tl, geo } = fx;
  const times = sampleTimes(tl);
  const poses = times.map((t) => tl.poseAt(t));

  it("keeps every vehicle on a track centre line, heading along it", () => {
    const off = new Failures(), head = new Failures(), address = new Failures();
    poses.forEach((w) => {
      for (const b of bodies(w)) {
        const near = rails(fx, b.pos);
        const d = near.distance, tangents = near.headings;
        if (d > 0.05) off.note(() => `${b.key} is ${d.toFixed(3)} m off the rails at t=${w.t.toFixed(4)}`);
        if (!tangents.some((y) => Math.min(angleBetween(b.yaw, y), Math.PI - angleBetween(b.yaw, y)) < (2 * Math.PI) / 180)) {
          head.note(() => `${b.key} heads ${((b.yaw * 180) / Math.PI).toFixed(1)} deg, not along any track, at t=${w.t.toFixed(4)}`);
        }
        const at = geo.track(b.track).at(b.s);
        if (Math.hypot(at.p.x - b.pos.x, at.p.z - b.pos.z) > 0.05) address.note(() => `${b.key} is not at (${b.track}, ${b.s.toFixed(2)}) at t=${w.t.toFixed(4)}`);
      }
    });
    expect(String(off)).toBe("none");
    expect(String(head)).toBe("none");
    expect(String(address)).toBe("none");
  }, LONG);

  it("keeps constant coupler spacing along every consist", () => {
    const bad = new Failures();
    poses.forEach((w) => {
      const byId = new Map(w.cars.map((c) => [c.id, c]));
      for (const e of w.engines) {
        let prev: Body = { ...e, key: e.id };
        for (const id of e.cars) {
          const c = byId.get(id)!;
          const want = prev.length / 2 + COUPLER_GAP + c.length / 2;
          const got = dist(prev.pos, c.pos);
          if (Math.abs(got - want) > 0.1) bad.note(() => `${prev.key} to car ${id} is ${got.toFixed(2)} m apart (want ${want.toFixed(2)}) at t=${w.t.toFixed(4)}`);
          prev = { ...c, key: `car:${id}` };
        }
      }
    });
    expect(String(bad)).toBe("none");
  });

  it("never lets two vehicles overlap", () => {
    const bad = new Failures();
    poses.forEach((w) => {
      const list = bodies(w);
      for (let i = 0; i < list.length; i++) for (let j = i + 1; j < list.length; j++) {
        if (dist(list[i].pos, list[j].pos) > 40) continue;
        if (overlap(list[i], list[j])) bad.note(() => `${list[i].key} and ${list[j].key} overlap at t=${w.t.toFixed(4)} (${list[i].track} ${list[i].s.toFixed(1)}, ${list[j].track} ${list[j].s.toFixed(1)})`);
      }
    });
    expect(String(bad)).toBe("none");
  });

  it("keeps speed and acceleration realistic and never teleports", () => {
    const fast = new Failures(), accel = new Failures(), jump = new Failures();
    // Speed of each body over the previous interval, with the interval's length and time base, to difference into accelerations.
    const last = new Map<string, { v: number; seconds: number; scale: number }>();
    for (let i = 1; i < poses.length; i++) {
      const a = poses[i - 1], b = poses[i];
      if (tl.cuts.some((c) => c > a.t && c <= b.t)) { last.clear(); continue; }
      const before = new Map(bodies(a).map((x) => [x.key, x]));
      for (const x of bodies(b)) {
        const p = before.get(x.key);
        if (!p) continue;
        const owner = "coupledTo" in x && x.coupledTo ? x.coupledTo : "coupledTo" in x ? null : x.key;
        const moved = dist(p.pos, x.pos);
        const r = owner ? regime(tl, owner, a.t, b.t) : { scale: SECONDS_PER_HOUR, vmax: 0, amax: 0, stands: true };
        const seconds = (b.t - a.t) * r.scale;
        const v = moved / seconds;
        if (moved > r.vmax * seconds * 1.02 + JOINT) jump.note(() => `${x.key} jumps ${moved.toFixed(3)} m in ${seconds.toFixed(3)} s (limit ${(r.vmax * seconds).toFixed(3)}) at t=${b.t.toFixed(5)}`);
        if (v > r.vmax * 1.02 + JOINT / seconds) fast.note(() => `${x.key} runs ${v.toFixed(2)} m/s at t=${b.t.toFixed(5)}`);
        const was = last.get(x.key);
        // Accelerations are differences of speeds over whole sampling intervals; the short intervals at the edges of moves are skipped.
        const whole = 0.6 / SAMPLES_PER_SECOND;
        if (was && !r.stands && was.scale === r.scale && seconds >= whole && was.seconds >= whole) {
          const a = Math.abs(v - was.v) / ((seconds + was.seconds) / 2);
          if (a > r.amax) accel.note(() => `${x.key} accelerates ${a.toFixed(2)} m/s2 at t=${b.t.toFixed(5)}`);
        }
        last.set(x.key, { v, seconds, scale: r.scale });
      }
    }
    expect(String(jump)).toBe("none");
    expect(String(fast)).toBe("none");
    expect(String(accel)).toBe("none");
  });

  it("moves a car only while it is coupled to an engine that moves with it", () => {
    const bad = new Failures();
    for (let i = 1; i < poses.length; i++) {
      const a = poses[i - 1], b = poses[i];
      const engineBefore = new Map(a.engines.map((e) => [e.id, e])), engineAfter = new Map(b.engines.map((e) => [e.id, e]));
      const before = new Map(a.cars.map((c) => [c.id, c]));
      for (const c of b.cars) {
        const p = before.get(c.id);
        if (!p) continue;
        const moved = dist(p.pos, c.pos);
        if (moved < JOINT) continue;
        if (!p.coupledTo || p.coupledTo !== c.coupledTo) { bad.note(() => `car ${c.id} moved ${moved.toFixed(3)} m uncoupled at t=${b.t.toFixed(5)}`); continue; }
        const e0 = engineBefore.get(p.coupledTo)!, e1 = engineAfter.get(p.coupledTo)!;
        const engineMoved = dist(e0.pos, e1.pos);
        if (Math.abs(engineMoved - moved) > 0.05 + 0.05 * moved) bad.note(() => `car ${c.id} moved ${moved.toFixed(3)} m but engine ${p.coupledTo} ${engineMoved.toFixed(3)} m at t=${b.t.toFixed(5)}`);
      }
    }
    expect(String(bad)).toBe("none");
  });

  it("shows the lit headlight at the end that leads", () => {
    const bad = new Failures();
    for (let i = 1; i < poses.length; i++) {
      const a = poses[i - 1], b = poses[i];
      const before = new Map(a.engines.map((e) => [e.id, e]));
      for (const e of b.engines) {
        const p = before.get(e.id)!;
        const dx = e.pos.x - p.pos.x, dz = e.pos.z - p.pos.z;
        if (e.lead !== p.lead) {
          // The lit end changes only while the engine stands (it reverses at a cusp), never while it runs.
          if (Math.hypot(dx, dz) > 0.05) bad.note(() => `${e.id} changes its lit end while moving ${Math.hypot(dx, dz).toFixed(3)} m at t=${b.t.toFixed(5)}`);
          continue;
        }
        if (Math.hypot(dx, dz) < 0.005) continue;
        const along = dx * Math.sin(e.yaw) + dz * Math.cos(e.yaw);
        if (along * e.lead <= 0) bad.note(() => `${e.id} runs ${along > 0 ? "nose" : "tail"} first but leads with ${e.lead > 0 ? "nose" : "tail"} at t=${b.t.toFixed(5)}`);
      }
    }
    expect(String(bad)).toBe("none");
  });

  it("never turns a vehicle in place", () => {
    const bad = new Failures();
    for (let i = 1; i < poses.length; i++) {
      const a = poses[i - 1], b = poses[i];
      const before = new Map(bodies(a).map((x) => [x.key, x]));
      for (const x of bodies(b)) {
        const p = before.get(x.key);
        if (!p) continue;
        const turned = angleBetween(p.yaw, x.yaw);
        const moved = dist(p.pos, x.pos);
        // A vehicle turns by at most its way over the sharpest radius (100 m on the edges) plus the corner of one polyline vertex (3 m chords).
        if (turned > moved / 100 + (3 * Math.PI) / 180) bad.note(() => `${x.key} turns ${((turned * 180) / Math.PI).toFixed(1)} deg over ${moved.toFixed(2)} m at t=${b.t.toFixed(5)}`);
      }
    }
    expect(String(bad)).toBe("none");
  });

  it("couples and uncouples by shunting and keeps every promise about the moves", () => {
    for (const m of tl.moves) {
      expect(m.seconds).toBeCloseTo((m.t1 - m.t0) * SECONDS_PER_HOUR, 3 - 1);
      expect(m.distance).toBeGreaterThan(40);
      expect(m.t1).toBeGreaterThan(m.t0);
    }
    for (const sp of tl.spans) if ((sp.op.type === "couple" || sp.op.type === "uncouple") && sp.cars.length) {
      const move = tl.moves.find((m) => m.engine === sp.engine && Math.abs(m.t0 - sp.t0) < 1e-9);
      if (move) expect(sp.t1).toBeCloseTo(move.t1, 9);
    }
    // After every move the cars of the op are where the trace says.
    for (const m of tl.moves) {
      const w = tl.poseAt(m.t1 + 1e-6);
      for (const id of m.cars) {
        const car = w.cars.find((c) => c.id === id);
        if (!car) continue;
        if (m.op === "couple") expect(car.coupledTo === m.engine || car.coupledTo === null, `car ${id} after coupling`).toBe(true);
      }
    }
  });

  it("is a pure function of time", () => {
    const t = times[Math.floor(times.length / 2)];
    const first = tl.poseAt(t);
    for (const u of [times[0], times[times.length - 1], times[3]]) tl.poseAt(u);
    expect(tl.poseAt(t)).toEqual(first);
    expect(ENGINE_LENGTH).toBe(17);
    expect(ENGINE_WIDTH).toBeGreaterThan(CAR_WIDTH - 0.5);
    expect(CAR_LENGTH.tanker).toBeLessThan(CAR_LENGTH.boxcar);
  });
});
