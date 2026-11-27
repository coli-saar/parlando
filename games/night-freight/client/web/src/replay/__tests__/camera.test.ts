/**
 * Camera invariants of the replay film, checked on every film the replay can show.
 *
 * Each film (the generated examples with a run and the preview's failed runs) is
 * sampled at 30 fps through the pure `cameraAt`, with no Babylon scene. Per frame
 * the tests project the subject vehicles of the shot into a 16:9 frustum and
 * measure distances, terrain clearance and camera motion; per film they check the
 * clock, the length of the film (at most 60 s) and the film time each action receives (at least 2 s). A failing test lists up to a few
 * offending frames (film, film second, shot kind, measured value) so that the
 * director can be fixed from the output. Contract: the tests read only exports of
 * `director.ts` and `timeline.ts` and the timeline's frames; they never depend on
 * how the director builds its rigs.
 */
import { beforeAll, describe, expect, it } from "vitest";
import type { P3 } from "../../scene/geography";
import { cameraAt, type Shot } from "../director";
import { CAR_LENGTH, ENGINE_LENGTH, type Frame } from "../timeline";
import { fixtures, type FilmFixture } from "./cameraFixtures";

const FPS = 30;
/** Generous per-test timeout in milliseconds: building every film and sampling it at 30 fps takes several seconds. */
const SLOW = 60_000;
const ASPECT = 16 / 9;
/** Share of the frame half-extent that counts as "central": the central 80%. */
const CENTRAL = 0.8;
/** Longest film in seconds. */
const MAX_FILM = 60;
/** Shortest shot that contains an op or a failure, in film seconds. */
const MIN_ACTION_SECONDS = 2;
const MAX_ANGULAR_DEG_PER_S = 25;
/**
 * Linear jerk bound in m/s^3 of the camera position and of its target (film time). 1000 m/s^3 is a third difference of
 * 0.037 m per frame: the noise floor of a smooth path sampled at 30 fps. Velocity kinks and cuts inside a shot show up
 * at several thousand.
 */
const MAX_JERK = 1000;

/** A vehicle as the camera sees it: a centre and the extent along its heading. */
interface Vehicle { id: string; centre: P3; yaw: number; length: number }

/** One sampled frame of a film. */
interface Sample {
  film: number; shot: number; kind: Shot["kind"]; k: number; f: number; t: number;
  pos: P3; target: P3; fov: number; frame: Frame; subject: Vehicle[];
}

const sub = (a: P3, b: P3): P3 => ({ x: a.x - b.x, y: a.y - b.y, z: a.z - b.z });
const dot = (a: P3, b: P3) => a.x * b.x + a.y * b.y + a.z * b.z;
const norm = (a: P3) => Math.hypot(a.x, a.y, a.z);
const unit = (a: P3): P3 => { const n = norm(a) || 1; return { x: a.x / n, y: a.y / n, z: a.z / n }; };
const cross = (a: P3, b: P3): P3 => ({ x: a.y * b.z - a.z * b.y, y: a.z * b.x - a.x * b.z, z: a.x * b.y - a.y * b.x });

/** Projects a world point with a look-at camera (no roll, vertical field of view `fov`) to normalised device coordinates. */
function project(p: P3, pos: P3, target: P3, fov: number): { x: number; y: number; depth: number } {
  const fwd = unit(sub(target, pos));
  const right = unit(cross({ x: 0, y: 1, z: 0 }, fwd));
  const up = cross(fwd, right);
  const d = sub(p, pos);
  const depth = dot(d, fwd), tan = Math.tan(fov / 2);
  return { x: dot(d, right) / (depth * tan * ASPECT), y: dot(d, up) / (depth * tan), depth };
}

/** Vehicles the shot is about at one sample: its engine, the cars coupled to it and the cars its running ops work on. */
function subjectOf(frame: Frame, engine: string | null | undefined): Vehicle[] {
  if (!engine) return [];
  const e = frame.engines.find((x) => x.id === engine);
  if (!e) return [];
  const ids = new Set<number>(e.cars);
  for (const w of frame.work) if (w.engine === engine) w.cars.forEach((c) => ids.add(c));
  const out: Vehicle[] = [{ id: engine, centre: { ...e.pos, y: e.pos.y + 2 }, yaw: e.yaw, length: ENGINE_LENGTH }];
  for (const c of frame.cars) if (ids.has(c.id)) out.push({ id: `car${c.id}`, centre: { ...c.pos, y: c.pos.y + 2 }, yaw: c.yaw, length: CAR_LENGTH[c.kind] });
  return out;
}

/** Screen-space bounds of vehicles: their ends and centres projected. Null when something is behind the camera. */
function screenBox(vs: Vehicle[], s: Pick<Sample, "pos" | "target" | "fov">) {
  let x0 = Infinity, x1 = -Infinity, y0 = Infinity, y1 = -Infinity;
  for (const v of vs) {
    const f = { x: Math.sin(v.yaw) * v.length / 2, z: Math.cos(v.yaw) * v.length / 2 };
    for (const k of [-1, 0, 1]) {
      const q = project({ x: v.centre.x + f.x * k, y: v.centre.y, z: v.centre.z + f.z * k }, s.pos, s.target, s.fov);
      if (q.depth <= 0.1) return null;
      x0 = Math.min(x0, q.x); x1 = Math.max(x1, q.x); y0 = Math.min(y0, q.y); y1 = Math.max(y1, q.y);
    }
  }
  return { x0, x1, y0, y1 };
}

const cache = new Map<string, Sample[]>();
let all: FilmFixture[] = [];

/** All frames of one film at 30 fps (computed once). */
function samples(fx: FilmFixture): Sample[] {
  const hit = cache.get(fx.name);
  if (hit) return hit;
  const out: Sample[] = [];
  const n = Math.floor(fx.film.duration * FPS);
  for (let k = 0; k <= n; k++) {
    const film = Math.min(k / FPS, fx.film.duration);
    const cam = cameraAt(fx.film, film);
    const shot = fx.film.shots[cam.shot];
    const frame = fx.tl.evaluate(cam.t);
    let start = 0;
    for (let i = 0; i < cam.shot; i++) start += fx.film.shots[i].duration;
    out.push({ film, shot: cam.shot, kind: shot.kind, k, f: film - start, t: cam.t, pos: cam.pos, target: cam.target, fov: cam.fov, frame, subject: subjectOf(frame, shot.engine) });
  }
  cache.set(fx.name, out);
  return out;
}

/** Groups samples by shot index. */
function byShot(ss: Sample[]): Sample[][] {
  const out: Sample[][] = [];
  for (const s of ss) (out[s.shot] ??= []).push(s);
  return out.filter(Boolean);
}

/** Formats the first few violations of a check so a failure names the offending frames. */
function report(fx: FilmFixture, what: string, bad: { s: Sample; v: string }[]): string[] {
  return bad.slice(0, 6).map(({ s, v }) => `${fx.name} ${what} film ${s.film.toFixed(2)}s shot ${s.shot} (${s.kind}): ${v}`).concat(bad.length > 6 ? [`... ${bad.length} violations in all`] : []);
}

beforeAll(() => { all = fixtures(); });

describe("replay camera invariants", () => {
  it("has films to test", () => {
    expect(all.length).toBeGreaterThanOrEqual(9);
    for (const fx of all) expect(fx.film.shots.length, fx.name).toBeGreaterThan(0);
  }, SLOW);

  it("keeps the subject in the central 80% of the frame in at least 90% of every shot, and never fully out of frame", () => {
    const problems: string[] = [];
    for (const fx of all) for (const shot of byShot(samples(fx))) {
      if (!shot[0].subject.length) continue;
      const bad: { s: Sample; v: string }[] = [];
      let out = 0;
      for (const s of shot) {
        const inside = s.subject.every((v) => {
          const q = project(v.centre, s.pos, s.target, s.fov);
          return q.depth > 0 && Math.abs(q.x) <= CENTRAL && Math.abs(q.y) <= CENTRAL;
        });
        if (!inside) { out++; bad.push({ s, v: `${s.subject.map((v) => v.id).join("+")} outside the central 80%` }); }
        const any = s.subject.some((v) => { const q = project(v.centre, s.pos, s.target, s.fov); return q.depth > 0 && Math.abs(q.x) <= 1 && Math.abs(q.y) <= 1; });
        if (!any) problems.push(...report(fx, "subject fully out of frame", [{ s, v: s.subject.map((v) => v.id).join("+") }]).slice(0, 1));
      }
      if (out / shot.length > 0.1) problems.push(...report(fx, `subject outside central 80% in ${(100 * out / shot.length).toFixed(0)}% of the shot`, bad).slice(0, 2));
    }
    expect(problems).toEqual([]);
  }, SLOW);

  it("shows the subject between 8% and 70% of the frame in at least 90% of every shot", () => {
    const problems: string[] = [];
    for (const fx of all) for (const shot of byShot(samples(fx))) {
      const bad: { s: Sample; v: string }[] = [];
      for (const s of shot) {
        if (!s.subject.length) continue;
        const b = screenBox(s.subject, s);
        if (!b) { bad.push({ s, v: "subject behind the camera" }); continue; }
        // Size is the larger of the bounding box's share of the frame width and of the frame height.
        const size = Math.max((b.x1 - b.x0) / 2, (b.y1 - b.y0) / 2);
        if (size < 0.08 || size > 0.7) bad.push({ s, v: `size ${(100 * size).toFixed(1)}%` });
      }
      // While a car joins the subject at the start of a move the camera pulls back over about a second, so a few frames may be off.
      if (bad.length / shot.length > 0.1) problems.push(...report(fx, `subject size off in ${(100 * bad.length / shot.length).toFixed(0)}% of the shot`, bad).slice(0, 3));
    }
    expect(problems).toEqual([]);
  }, SLOW);

  it("keeps the camera at least 20 m from every vehicle", () => {
    const problems: string[] = [];
    for (const fx of all) {
      const bad: { s: Sample; v: string }[] = [];
      for (const s of samples(fx)) {
        let best = Infinity;
        for (const v of [...s.frame.engines.map((e) => e.pos), ...s.frame.cars.map((c) => c.pos)]) best = Math.min(best, norm(sub({ ...v, y: v.y + 2 }, s.pos)));
        if (best < 20) bad.push({ s, v: `nearest vehicle ${best.toFixed(1)} m` });
      }
      problems.push(...report(fx, "distance", bad));
    }
    expect(problems).toEqual([]);
  }, SLOW);

  it("keeps the camera 3 m above the terrain and the line of sight to the subject free of terrain", () => {
    const problems: string[] = [];
    for (const fx of all) {
      const bad: { s: Sample; v: string }[] = [];
      for (const s of samples(fx)) {
        const clearance = s.pos.y - fx.geo.groundHeight(s.pos.x, s.pos.z);
        if (clearance < 3) { bad.push({ s, v: `${clearance.toFixed(1)} m above the terrain` }); continue; }
        if (!s.subject.length) continue;
        const c = s.subject.reduce((a, v) => ({ x: a.x + v.centre.x / s.subject.length, y: a.y + v.centre.y / s.subject.length, z: a.z + v.centre.z / s.subject.length }), { x: 0, y: 0, z: 0 });
        const d = sub(c, s.pos), len = norm(d);
        for (let m = 2; m < len - 2; m += 2) {
          const p = { x: s.pos.x + (d.x * m) / len, y: s.pos.y + (d.y * m) / len, z: s.pos.z + (d.z * m) / len };
          if (fx.geo.groundHeight(p.x, p.z) > p.y) { bad.push({ s, v: `terrain blocks the view at ${m} m` }); break; }
        }
      }
      problems.push(...report(fx, "terrain", bad));
    }
    expect(problems).toEqual([]);
  }, SLOW);

  it("moves the camera smoothly inside every shot", () => {
    const problems: string[] = [];
    const dt = 1 / FPS;
    for (const fx of all) for (const shot of byShot(samples(fx))) {
      const bad: { s: Sample; v: string }[] = [];
      const dir = shot.map((s) => unit(sub(s.target, s.pos)));
      for (let i = 1; i < shot.length; i++) {
        const ang = (Math.acos(Math.min(1, Math.max(-1, dot(dir[i], dir[i - 1])))) * 180) / Math.PI / dt;
        if (ang > MAX_ANGULAR_DEG_PER_S) bad.push({ s: shot[i], v: `view turns ${ang.toFixed(0)} deg/s` });
      }
      for (const [name, pick] of [["position", (s: Sample) => s.pos], ["target", (s: Sample) => s.target]] as const) {
        for (let i = 3; i < shot.length; i++) {
          const [a, b, c, d] = [pick(shot[i - 3]), pick(shot[i - 2]), pick(shot[i - 1]), pick(shot[i])];
          const j = { x: d.x - 3 * c.x + 3 * b.x - a.x, y: d.y - 3 * c.y + 3 * b.y - a.y, z: d.z - 3 * c.z + 3 * b.z - a.z };
          const jerk = norm(j) / dt ** 3;
          if (jerk > MAX_JERK) bad.push({ s: shot[i], v: `${name} jerk ${jerk.toFixed(0)} m/s^3` });
        }
      }
      problems.push(...report(fx, "smoothness", bad));
    }
    expect(problems).toEqual([]);
  }, SLOW);

  it("runs the clock forward and joins shots without a jump", () => {
    const problems: string[] = [];
    for (const fx of all) {
      const ss = samples(fx);
      for (let i = 1; i < ss.length; i++) if (ss[i].t < ss[i - 1].t - 1e-9) problems.push(`${fx.name} clock runs backwards at film ${ss[i].film.toFixed(2)}s`);
      fx.film.shots.forEach((s, i) => { if (i > 0 && Math.abs(fx.film.shots[i - 1].t1 - s.t0) > 1e-6) problems.push(`${fx.name} shot ${i} starts at ${s.t0} but shot ${i - 1} ends at ${fx.film.shots[i - 1].t1}`); });
      fx.film.shots.forEach((s, i) => { if (!(s.duration > 0)) problems.push(`${fx.name} shot ${i} has no duration`); });
    }
    expect(problems).toEqual([]);
  }, SLOW);

  it("lasts at most 60 s, gives every op and failure shot at least 2 s and shows each of them in a caption", () => {
    const problems: string[] = [];
    for (const fx of all) {
      const { shots, duration } = fx.film;
      if (duration > MAX_FILM + 1e-6) problems.push(`${fx.name} lasts ${duration.toFixed(1)} s`);
      /** The shot whose sim window contains `t` (the last one at its end). */
      const shotAt = (t: number) => shots.find((s) => t >= s.t0 - 1e-9 && t < s.t1 - 1e-9) ?? shots[shots.length - 1];
      /** Whether some caption of a shot of `engine` (any engine when null) covers part of the sim window. */
      const captioned = (t0: number, t1: number, engine: string | null) =>
        shots.some((s) => (engine === null || s.engine === engine) && s.captions.some((c) => c.t0 < t1 && c.t1 > t0));
      for (const sp of fx.tl.spans) {
        const shot = shotAt(sp.t0 + 1e-6);
        if (shot.duration < MIN_ACTION_SECONDS - 1e-6) problems.push(`${fx.name} ${sp.engine} ${sp.op.type} at ${sp.station} is in a shot of only ${shot.duration.toFixed(2)} s`);
        if (shot.engine !== sp.engine) problems.push(`${fx.name} ${sp.engine} ${sp.op.type} at ${sp.station} is filmed in a shot about ${shot.engine}`);
        if (!captioned(sp.t0, sp.t1, sp.engine)) problems.push(`${fx.name} ${sp.engine} ${sp.op.type} at ${sp.station} has no caption`);
      }
      for (const e of fx.tl.events) {
        const k = e.kind;
        if (k.type !== "op_abandoned" && k.type !== "hold" && k.type !== "op_unreached" && k.type !== "order_missed" && k.type !== "disrupted") continue;
        const shot = shotAt(e.t + 1e-6);
        if (shot.duration < MIN_ACTION_SECONDS - 1e-6) problems.push(`${fx.name} ${k.type} at hour ${e.hour} is in a shot of only ${shot.duration.toFixed(2)} s`);
        const engine = "engine" in k ? (k as { engine: string }).engine : null;
        if (!captioned(e.t, e.t + 0.09, engine)) problems.push(`${fx.name} ${k.type} at hour ${e.hour} has no caption`);
        if (engine && shot.engine !== engine) problems.push(`${fx.name} ${k.type} at hour ${e.hour} is filmed in a shot about ${shot.engine}`);
      }
      // No sliver shots: a shot is long enough to be read.
      shots.forEach((s, i) => { if (s.duration < 1.5 && i < shots.length - 1) problems.push(`${fx.name} shot ${i} (${s.kind}) lasts only ${s.duration.toFixed(2)} s`); });
    }
    expect(problems).toEqual([]);
  }, SLOW);

  it("frames the network view of missed orders so that every station is readable", () => {
    const problems: string[] = [];
    for (const fx of all) for (const shot of byShot(samples(fx))) {
      if (shot[0].kind !== "orders") continue;
      const stations = [...fx.geo.stations.values()];
      let out = 0;
      for (const s of shot) {
        const ok = stations.every((st) => { const q = project({ x: st.x, y: st.y, z: st.z }, s.pos, s.target, s.fov); return q.depth > 0 && Math.abs(q.x) <= CENTRAL && Math.abs(q.y) <= CENTRAL; });
        if (!ok) out++;
      }
      if (out / shot.length > 0.1) problems.push(`${fx.name} network view loses a station in ${(100 * out / shot.length).toFixed(0)}% of its frames`);
    }
    expect(problems).toEqual([]);
  }, SLOW);
});
