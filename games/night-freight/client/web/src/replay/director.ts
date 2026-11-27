/**
 * Director: turns one joint-run `LastRun` into a film, a list of shots.
 *
 * The film is a continuous walk through simulated time. Every moment belongs to
 * exactly one shot, shots follow in chronological order and share their cut
 * times, so the clock never jumps backwards. Every happening of the run (an
 * arrival, a departure, each op, each abandoned op with its reason, each order
 * met or missed, a disruption, each stretch of travel) is an "activity" with a
 * sim-time window and a number of film seconds; where activities of different
 * engines overlap, the shortest one keeps the window and the longer one resumes
 * after it. The whole film lasts at most `MAX_FILM` (60) seconds. Sim time stays
 * physical (the timeline moves every vehicle correctly); only the mapping of sim
 * time to film time is compressed, per shot: an op or failure gets about 3 film
 * seconds however long its move takes (a shunting move of 100-300 sim seconds then
 * plays at 30-100x and the "fast-forward" indicator is shown), an arrival and a
 * departure get 1.5 s to cross the station, travel is time-lapsed. A film that
 * would still be longer is composed again with the next `LEVELS` entry: consecutive
 * ops of one engine at one station merge into one shot, trackside passes are
 * dropped, ops shrink to 2 s, and finally everything else is scaled down.
 *
 Every shot owns a smooth time warp (film second to sim time), timed captions
 * and a camera that is a pure function of the film second inside the shot. The
 * camera follows the real subject (an engine, its cars and the cars its running
 * op works on, read from the timeline), is sampled in film time, smoothed in
 * film time and kept above the terrain with a free line of sight. The invariants
 * (subject framed at a readable size, 20 m clearance, smooth motion, a film of at most
 * 60 s with at least 2 s for each op and failure) are enforced by `__tests__/camera.test.ts` through `cameraAt`.
 * Contract: shots are pure data plus functions of film time; the player owns
 * the film clock and only reads `at` and `camera`.
 */
import type { LastRun } from "../game";
import { clock12 } from "../game";
import { STATION_HALF, stationPoint, type Geography, type P3, type StationSite } from "../scene/geography";
import type { CameraPose } from "../scene/world";
import { describeAbandon, describeMiss, finishedText, narrate } from "./narrate";
import { ANIM_HOURS, CAR_LENGTH, ENGINE_LENGTH, SECONDS_PER_HOUR, type Frame, type Leg, type OpSpan, type Timeline } from "./timeline";

export type ShotKind = "crane" | "yard" | "lapse" | "pass" | "orders" | "disruption" | "chase";

/** Tone of a caption: failures are drawn in warning colours, met orders in success colours. */
export type Tone = "info" | "fail" | "good";

/** A caption valid for a sim-time window of a shot. */
export interface Caption { t0: number; t1: number; text: string; note?: string; tone: Tone }

/** A stretch of a shot with one constant speed: film seconds `f0..f1` play sim time `t0..t1`. */
export interface Piece { f0: number; f1: number; t0: number; t1: number; fast: boolean }

/** One shot: a sim-time range with a time warp, captions and a camera. */
export interface Shot {
  kind: ShotKind;
  /** Film seconds. */
  duration: number;
  /** Simulated time at the start and end of the shot. */
  t0: number;
  t1: number;
  /** Time warp: sim time and fast-forward flag at film second `f` of the shot (`0 <= f <= duration`). */
  at(f: number): { t: number; fast: boolean };
  /** Camera at film second `f` of the shot. */
  camera(f: number): CameraPose;
  captions: Caption[];
  /** Engine this shot is about, or null. */
  engine?: string | null;
}

/** The finished film. */
export interface Film {
  shots: Shot[];
  duration: number;
  /** True when the film ends in an outcome that needs "Back to planning". */
  needsAcknowledge: boolean;
}

/** Where the film stands at one film second: the camera, the shot it belongs to and the sim time shown. */
export interface FilmCamera extends CameraPose { shot: number; t: number; fast: boolean }

/**
 * Pure camera of a film: the pose at film second `filmTime` (clamped to the film), without any scene.
 * It evaluates the shot's time warp and then the shot's camera function, which reads only the timeline poses and the
 * terrain-height function it was built with. The camera tests sample it at 30 fps.
 */
export function cameraAt(film: Film, filmTime: number): FilmCamera {
  let acc = 0;
  let index = film.shots.length - 1, f = film.shots[index]?.duration ?? 0;
  for (let i = 0; i < film.shots.length; i++) {
    if (filmTime < acc + film.shots[i].duration) { index = i; f = Math.max(0, filmTime - acc); break; }
    acc += film.shots[i].duration;
  }
  const shot = film.shots[index];
  const { t, fast } = shot.at(f);
  return { ...shot.camera(f), shot: index, t, fast };
}

const FOV = 0.7;

/** Station display name lookup. */
export type Names = (id: string) => string;

/** What the camera functions read: the terrain, the world poses and the station names. */
interface Ctx { geo: Geography; tl: Timeline; names: Names }

const lerp = (a: P3, b: P3, t: number): P3 => ({ x: a.x + (b.x - a.x) * t, y: a.y + (b.y - a.y) * t, z: a.z + (b.z - a.z) * t });
const smooth = (u: number) => u * u * (3 - 2 * u);
const clamp = (x: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, x));

/** Distance in metres in 3-D a camera keeps from the centre of every vehicle that is not its subject (the tests demand 20 m). */
const VEHICLE_CLEARANCE = 29;
/** Lowest height of a camera above the ground below it, in metres (the tests demand 3 m). */
const GROUND_CLEARANCE = 6;
/** Height of a camera above the centre of its subject, at least, in metres. */
const MIN_ABOVE_SUBJECT = 5;
/** Camera path samples per film second. */
const SAMPLE_RATE = 15;
/** Largest share of the half frame the centre of a subject (and half its radius) may be away from the middle of the picture before the camera pulls back: the tests demand 80%. */
const CENTRAL_SHARE = 0.75;

/**
 * What a shot films at one instant: the bounding circle of a set of vehicles (or of a fixed set
 * piece). `centre` is the point the camera looks at (at eye height of the stock), `R` the
 * radius that contains every filmed vehicle end, `heading` the yaw of the engine (forward = (sin, cos)).
 */
interface Sub { centre: P3; R: number; heading: number }

/** Gives the subject in a world frame. */
type Sampler = (frame: Frame) => Sub;

/**
 * Subject made of one engine, the cars coupled behind it and the cars its shunting move couples or uncouples. A car
 * of a move joins the subject smoothly: its contribution glides from the engine to its real place over the first and last 6% of the move, so the subject's centre and radius
 * never jump when a yard car far away starts to matter. Cars of ops that do not move them (loading, delivering) are
 * always coupled and need no such treatment.
 */
function consistSampler(tl: Timeline, id: string): Sampler {
  const moves = tl.spans.filter((sp) => sp.engine === id && sp.cars.length);
  /** Weight 0..1 of a car at time `t`: 1 inside a move that handles it, fading in and out over 6% of the move at its ends. */
  const weight = (car: number, t: number): number => {
    let w = 0;
    for (const sp of moves) {
      if (!sp.cars.includes(car)) continue;
      const lead = Math.max(0.06 * (sp.t1 - sp.t0), 1e-6);
      w = Math.max(w, smooth(clamp(Math.min((t - sp.t0) / lead, (sp.t1 - t) / lead), 0, 1)));
    }
    return w;
  };
  return (frame) => {
    const e = frame.engines.find((x) => x.id === id);
    if (!e) return { centre: { x: 0, y: 2, z: 0 }, R: ENGINE_LENGTH / 2, heading: 0 };
    const pts: P3[] = [];
    const ends = (pos: P3, yaw: number, len: number, w = 1) => {
      const fx = (Math.sin(yaw) * len) / 2, fz = (Math.cos(yaw) * len) / 2;
      const at = (x: number, z: number) => ({ x: e.pos.x + (x - e.pos.x) * w, y: pos.y + 2, z: e.pos.z + (z - e.pos.z) * w });
      pts.push(at(pos.x + fx, pos.z + fz), at(pos.x - fx, pos.z - fz));
    };
    ends(e.pos, e.yaw, ENGINE_LENGTH);
    const coupled = new Set<number>(e.cars);
    for (const car of frame.cars) {
      const w = coupled.has(car.id) ? 1 : weight(car.id, frame.t);
      if (w > 0) ends(car.pos, car.yaw, CAR_LENGTH[car.kind], w);
    }
    const x0 = Math.min(...pts.map((p) => p.x)), x1 = Math.max(...pts.map((p) => p.x));
    const z0 = Math.min(...pts.map((p) => p.z)), z1 = Math.max(...pts.map((p) => p.z));
    const centre = { x: (x0 + x1) / 2, y: e.pos.y + 2, z: (z0 + z1) / 2 };
    return { centre, R: Math.max(ENGINE_LENGTH / 2, ...pts.map((p) => Math.hypot(p.x - centre.x, p.z - centre.z))), heading: e.yaw };
  };
}

/** Subject that never moves: a point of interest with a rough radius, seen along `heading`. */
function fixedSampler(centre: P3, R: number, heading = 0): Sampler {
  return () => ({ centre, R, heading });
}

/** Moving average with clamped edges, applied `passes` times (three box passes give a piecewise-cubic, C2 result) (zero lag, so smoothed paths stay on the subject). */
function blur(a: number[], radius: number, passes = 3): number[] {
  let cur = a;
  for (let p = 0; p < passes; p++) {
    const out = new Array<number>(cur.length);
    for (let i = 0; i < cur.length; i++) {
      let sum = 0;
      for (let j = -radius; j <= radius; j++) sum += cur[clamp(i + j, 0, cur.length - 1)];
      out[i] = sum / (2 * radius + 1);
    }
    cur = out;
  }
  return cur;
}

/** Smooth maximum: never below `max(a, b)` and at most 2.8 m above it, without the kink of `Math.max`. */
function softMax(a: number, b: number): number {
  const k = 4, hi = Math.max(a, b), lo = Math.min(a, b);
  return hi + k * Math.log1p(Math.exp((lo - hi) / k));
}

/** Unit vector of (x, y, z). */
function norm3(x: number, y: number, z: number): P3 { const l = Math.hypot(x, y, z) || 1; return { x: x / l, y: y / l, z: z / l }; }

/** Catmull-Rom interpolation (C1) of an evenly spaced series at fractional index `g`. */
function spline(a: number[], g: number): number {
  const n = a.length - 1;
  const i = clamp(Math.floor(g), 0, n - 1), k = clamp(g - i, 0, 1);
  const p0 = a[Math.max(0, i - 1)], p1 = a[i], p2 = a[i + 1], p3 = a[Math.min(n, i + 2)];
  return 0.5 * (2 * p1 + (-p0 + p2) * k + (2 * p0 - 5 * p1 + 4 * p2 - p3) * k * k + (-p0 + 3 * p1 - 3 * p2 + p3) * k * k * k);
}

/** The film clock of one shot: film second to sim time, as the rigs and the shot itself read it (defined a few seconds beyond both ends too). */
interface Clip { duration: number; at(f: number): { t: number; fast: boolean } }

/**
 * Builds the time warp of a shot from its constant-speed pieces. The speed profile is smoothed over about a second so
 * that the world never lurches where a slow activity meets a fast stretch; the warp still starts at the first piece's
 * `t0`, ends at the last piece's `t1` and is strictly increasing. `fast` is read from the unsmoothed piece.
 */
function makeClip(pieces: Piece[]): Clip {
  const duration = pieces[pieces.length - 1].f1;
  const rate = 60, m = Math.max(2, Math.ceil(duration * rate));
  const at = (f: number) => pieces.find((p) => f < p.f1) ?? pieces[pieces.length - 1];
  const speed = Array.from({ length: m }, (_, j) => { const p = at(((j + 0.5) * duration) / m); return (p.t1 - p.t0) / Math.max(p.f1 - p.f0, 1e-9); });
  const sm = blur(speed, Math.round(0.4 * rate), 2);
  const cum = [0];
  for (let j = 0; j < m; j++) cum.push(cum[j] + sm[j]);
  const t0 = pieces[0].t0, span = pieces[pieces.length - 1].t1 - t0, span0 = span;
  // Outside the shot the warp continues at the speed it has at its ends, so that smoothing filters can look past a cut.
  const slope = (j: number) => (sm[j] * span0 * m) / (cum[m] * duration);
  return {
    duration,
    at: (f) => {
      if (f < 0) return { t: t0 + f * slope(0), fast: pieces[0].fast };
      if (f > duration) return { t: t0 + span0 + (f - duration) * slope(m - 1), fast: pieces[pieces.length - 1].fast };
      const g = clamp(f / duration, 0, 1) * m, i = Math.min(m - 1, Math.floor(g));
      const c = cum[i] + (cum[i + 1] - cum[i]) * (g - i);
      return { t: t0 + (span * c) / cum[m], fast: at(f).fast };
    }
  };
}

/** What a placer sees at one sample of a shot. */
interface RigInput {
  /** Progress through the shot, 0..1. */
  u: number;
  /** Smoothed radius of the subject. */
  R: number;
  /** Framing distance for this subject: 2.6 radii, at least `dMin`, times the stretch that keeps a fast subject framed. */
  D: number;
  /** Factor (at least 1) the framing distance has been stretched by so that the subject, which strays from the smoothed look-at point where it moves fast, stays in the central part of the frame. */
  stretch: number;
  /** Smoothed subject centre the camera is rigidly anchored to (the same series as `look`). */
  anchor: P3;
  /** Lightly smoothed subject centre: what the camera looks at. */
  look: P3;
  /** Smoothed unit direction of motion of the subject (x, z); the heading of the engine while it stands. */
  fwd: { x: number; z: number };
  /** Heading unit vector of the engine at the end of the shot, for choosing a side that stays the same all shot. */
  endFwd: { x: number; z: number };
}

/** Chooses a camera position (and optionally target and field of view) for one sample. */
type Placer = (r: RigInput) => { pos: P3; target?: P3; fov?: number };

/** Tuning of `makeRig`. */
interface RigOptions {
  /** Camera is bolted down: it never moves, it is only lifted as far as the ground and the line of sight need. */
  fixed?: boolean;
  /** Smallest framing distance in metres (default 44). */
  dMin?: number;
  /** Largest share of the half frame the subject may be off the middle before the camera pulls back (default `CENTRAL_SHARE`). */
  share?: number;
}

/**
 * Turns a subject and a placement rule into a camera path for one shot.
 * The subject is sampled in film time through the shot's clock, so the camera follows what really
 * happens. The look-at point (which is also the anchor of the camera) is smoothed with a zero-lag filter in seconds
 * (0.7 s), the offset of the placed positions from the subject is smoothed over 0.7 s; the height needed above the ground
 * (6 m), above the subject (5 m), 29 m above or beside every other vehicle and for a free line of sight is spread over
 * 3 s on both sides, so the camera rises early and sinks late. Between the 15 Hz samples the
 * path is a Catmull-Rom spline, so velocity is continuous. The result is a pure function of the film second.
 */
function makeRig(c: Ctx, sample: Sampler, clip: Clip, place: Placer, o: RigOptions = {}): (f: number) => CameraPose {
  const n = Math.max(3, Math.ceil(clip.duration * SAMPLE_RATE));
  const dt = clip.duration / n;
  const sec = (s: number) => Math.max(0, Math.round(s / dt));
  // Every series is computed over the shot plus a margin of 3 s on both sides (the world just before and after the cut), so
  // the zero-lag filters keep their promise at the ends of the shot instead of lagging behind a departing engine.
  const pad = Math.ceil(3 / dt), m = n + 2 * pad;
  const frames = Array.from({ length: m + 1 }, (_, j) => c.tl.evaluate(clip.at((j - pad) * dt).t));
  const subs = frames.map(sample);
  const col = (f: (s: Sub) => number) => subs.map(f);
  const look = { x: blur(col((s) => s.centre.x), sec(0.6)), y: blur(col((s) => s.centre.y), sec(0.6)), z: blur(col((s) => s.centre.z), sec(0.6)) };
  // The radius rises early and sinks late (see `spread`), so that the camera never pulls back for something the next shot will show.
  const rs = col((s) => s.R);
  const inside = (i: number) => clamp(i, pad, pad + n);
  /** Rises early and sinks late: a maximum filter as wide as the smoothing that follows, but only within the shot. */
  const spread = (a: number[], w = 1.5) => blur(a.map((_, i) => Math.max(...a.slice(inside(i - sec(w)), inside(i + sec(w)) + 1))), sec(0.5));
  const radius = spread(rs, 1.0);
  // Direction of motion over +-0.5 s; where the subject stands still, the nearest known direction (finally the engine's yaw).
  const unit = (x: number, z: number) => { const d = Math.hypot(x, z) || 1; return { x: x / d, z: z / d }; };
  const motion = look.x.map((_, i) => {
    const a = Math.max(0, i - sec(0.5)), b = Math.min(m, i + sec(0.5));
    const dx = look.x[b] - look.x[a], dz = look.z[b] - look.z[a];
    return Math.hypot(dx, dz) > 2 ? unit(dx, dz) : null;
  });
  const yawDir = subs.map((s) => ({ x: Math.sin(s.heading), z: Math.cos(s.heading) }));
  const filled = motion.map((mo, i) => {
    if (mo) return mo;
    for (let d = 1; d <= m; d++) { const p = motion[i - d] ?? motion[i + d]; if (p) return p; }
    return yawDir[i];
  });
  const fx = blur(filled.map((d) => d.x), sec(2)), fz = blur(filled.map((d) => d.z), sec(2));
  const endFwd = yawDir[pad + n];
  const vehicles = frames.map((fr) => [...fr.engines.map((e) => e.pos), ...fr.cars.map((k) => k.pos)]);
  const ground = (x: number, z: number) => c.geo.groundHeight(x, z);

  /** Lowest camera height at or above `y` from which the sight line to `tg` clears the terrain by 1.5 m. */
  const sight = (x: number, y: number, z: number, tg: P3): number => {
    for (let k = 0; k < 20; k++) {
      const dx = tg.x - x, dy = tg.y - y, dz = tg.z - z, len = Math.hypot(dx, dy, dz);
      let blocked = false;
      for (let q = 4; q < len - 4; q += 4) if (ground(x + (dx * q) / len, z + (dz * q) / len) + 1.5 > y + (dy * q) / len) { blocked = true; break; }
      if (!blocked) return y;
      y += 4;
    }
    return y;
  };
  /**
   * Lowest height a camera at (x, z) may have at sample `i`: above the ground and the subject, at least 29 m (3-D) from the
   * centre of every vehicle that is not part of the subject (a camera flies over a yard instead of through it), and with a
   * free line of sight over the terrain. Vehicles of the subject itself are cleared by the framing distance (the radius plus
   * 36 m at least), not here.
   */
  const lowest = (x: number, z: number, i: number): number => {
    let y = Math.max(ground(x, z) + GROUND_CLEARANCE, look.y[i] + MIN_ABOVE_SUBJECT);
    for (const q of vehicles[i]) {
      if (Math.hypot(q.x - look.x[i], q.z - look.z[i]) <= radius[i] + 8) continue;
      const d = Math.hypot(x - q.x, z - q.z);
      if (d < VEHICLE_CLEARANCE) y = Math.max(y, q.y + 2 + Math.sqrt(VEHICLE_CLEARANCE * VEHICLE_CLEARANCE - d * d));
    }
    return sight(x, y, z, { x: look.x[i], y: look.y[i], z: look.z[i] });
  };

  /**
   * Places the camera for framing distances stretched by `stretch[i]` at every sample: the placed positions, then the
   * camera positions after terrain, subject and line-of-sight heights.
   */
  const build = (stretch: number[]): { placed: { pos: P3; target: P3; fov: number }[]; pos: P3[] } => {
    const placed = subs.map((_, i) => {
      const D = Math.max(o.dMin ?? 44, 2.6 * radius[i], radius[i] + 36) * stretch[i];
      const r = place({ u: clamp((i - pad) / n, 0, 1), R: radius[i], D, stretch: stretch[i], anchor: { x: look.x[i], y: look.y[i], z: look.z[i] }, look: { x: look.x[i], y: look.y[i], z: look.z[i] }, fwd: { x: fx[i], z: fz[i] }, endFwd });
      return { pos: r.pos, target: r.target ?? { x: look.x[i], y: look.y[i], z: look.z[i] }, fov: r.fov ?? FOV };
    });
    let pos: P3[];
    if (o.fixed) {
      const p = placed[pad + Math.floor(n / 2)].pos;
      let y = Math.max(p.y, ground(p.x, p.z) + GROUND_CLEARANCE);
      for (let i = pad; i <= pad + n; i++) y = Math.max(y, sight(p.x, y, p.z, { x: look.x[i], y: look.y[i], z: look.z[i] }));
      pos = placed.map(() => ({ x: p.x, y, z: p.z }));
    } else {
      // The camera is rigidly bolted to the (smoothed) subject: only its offset from the subject is smoothed, so the view
      // direction does not swing when the subject speeds up or brakes. The height that terrain, vehicles and line of sight
      // demand is spread over +-3 s (rise early, sink late; the smoothing that follows reaches 3 x 1 s) before it is applied.
      const smoothed = (f: (p: P3, i: number) => number) => blur(placed.map((p, i) => f(p.pos, i)), sec(0.85));
      const ox = smoothed((p, i) => p.x - look.x[i]), oy = smoothed((p, i) => p.y - look.y[i]), oz = smoothed((p, i) => p.z - look.z[i]);
      const xz = placed.map((_, i) => ({ x: look.x[i] + ox[i], z: look.z[i] + oz[i] }));
      const need = xz.map((p, i) => lowest(p.x, p.z, i));
      const dilated = need.map((_, i) => Math.max(...need.slice(Math.max(0, i - sec(3)), Math.min(m, i + sec(3)) + 1)));
      const floor = blur(dilated, sec(1));
      pos = xz.map((p, i) => ({ x: p.x, y: softMax(look.y[i] + oy[i], floor[i]), z: p.z }));
    }
    return { placed, pos };
  };
  // Pass 1 frames every sample at its natural distance. Where the subject (which the camera, anchored to the smoothed
  // look-at point, does not follow into every fast move) then lies too far from the middle of the picture, the distance is
  // stretched in proportion and the camera is placed again; the stretch rises early and sinks late like the radius.
  let result = build(look.x.map(() => 1));
  if (!o.fixed) {
    const need = subs.map((sb, i) => {
      const cp = result.pos[i], tg = result.placed[i].target, tan = Math.tan(result.placed[i].fov / 2);
      const fw = norm3(tg.x - cp.x, tg.y - cp.y, tg.z - cp.z);
      const right = norm3(fw.z, 0, -fw.x);
      const up = { x: fw.y * right.z - fw.z * right.y, y: fw.z * right.x - fw.x * right.z, z: fw.x * right.y - fw.y * right.x };
      const d = { x: sb.centre.x - cp.x, y: sb.centre.y - cp.y, z: sb.centre.z - cp.z };
      const depth = Math.max(1, d.x * fw.x + d.y * fw.y + d.z * fw.z);
      const ox = Math.abs(d.x * right.x + d.y * right.y + d.z * right.z) + 0.3 * radius[i];
      const oy = Math.abs(d.x * up.x + d.y * up.y + d.z * up.z) + 0.3 * radius[i];
      return Math.max(1, Math.max(ox / (depth * tan * (16 / 9)), oy / (depth * tan)) / (o.share ?? CENTRAL_SHARE));
    });
    const stretch = spread(need, 0.7);
    result = build(stretch.map((v) => Math.max(1, v)));
  }
  const { placed, pos } = result;
  const series = {
    px: pos.map((p) => p.x), py: pos.map((p) => p.y), pz: pos.map((p) => p.z),
    tx: placed.map((p) => p.target.x), ty: placed.map((p) => p.target.y), tz: placed.map((p) => p.target.z), fov: placed.map((p) => p.fov)
  };
  return (f) => {
    const g = pad + (clamp(f, 0, clip.duration) / clip.duration) * n;
    return {
      pos: { x: spline(series.px, g), y: spline(series.py, g), z: spline(series.pz, g) },
      target: { x: spline(series.tx, g), y: spline(series.ty, g), z: spline(series.tz, g) },
      fov: spline(series.fov, g)
    };
  };
}

/** A rig built from a shot's clock. */
type RigFactory = (clip: Clip) => (f: number) => CameraPose;

/** Station axis unit vector (u) and perpendicular (v) in world xz. */
function axes(c: Ctx, station: string): { u: P3; v: P3; site: StationSite } {
  const s = c.geo.stations.get(station)!;
  return { u: { x: Math.cos(s.yaw), y: 0, z: Math.sin(s.yaw) }, v: { x: -Math.sin(s.yaw), y: 0, z: Math.cos(s.yaw) }, site: s };
}

/**
 * Three-quarter yard view as in the sketch: the camera stands beyond the throat
 * the engine faces (or `d` when given), on the side away from the depot, high
 * enough to see over the cars, and drifts a little round the subject during the shot.
 */
function threeQuarter(c: Ctx, station: string, o: { d?: 1 | -1; swing?: number; phi?: number; side?: 1 | -1 } = {}): Placer {
  const ax = axes(c, station);
  return (r) => {
    const d = o.d ?? (Math.sign(r.endFwd.x * ax.u.x + r.endFwd.z * ax.u.z) || 1);
    const phi = (o.phi ?? 0.55) + (o.swing ?? 0.3) * (r.u - 0.5);
    const along = d * r.D * Math.cos(phi), across = (o.side ?? 1) * r.D * Math.sin(phi);
    const h = clamp(0.4 * r.D, 12, 40) + 1.5 * r.u;
    return { pos: { x: r.anchor.x + ax.u.x * along + ax.v.x * across, y: r.anchor.y + h, z: r.anchor.z + ax.u.z * along + ax.v.z * across } };
  };
}

/** Yard, delivery, divergence and failure shots: the engine, its consist and the car being worked in a three-quarter view. */
function yardRig(c: Ctx, station: string, engine: string): RigFactory {
  // The camera stands on the side of the engine track away from the yard tracks (which fill the +v side first), so yard cars never hide the engine.
  return (clip) => makeRig(c, consistSampler(c.tl, engine), clip, threeQuarter(c, station, { swing: 0.3, side: -1, phi: 1.05 }));
}

/** Largest radius of an engine's subject over a shot, from five samples of its clock. */
function maxRadius(c: Ctx, engine: string, clip: Clip): number {
  const sample = consistSampler(c.tl, engine);
  return Math.max(...[0, 0.25, 0.5, 0.75, 1].map((u) => sample(c.tl.evaluate(clip.at(u * clip.duration).t)).R));
}

/**
 * Trackside or bridge pass: the camera stands still beside the track, at least 65 m (55 m on a bridge) away, and pans with the
 * train. The distance keeps the pan below 25 degrees per second at the film speed of a pass.
 */
function passRig(c: Ctx, leg: Leg, sMid: number, bridge: boolean): RigFactory {
  return (clip) => {
    const mid = leg.path.at(sMid);
    const right = { x: Math.cos(mid.yaw), z: -Math.sin(mid.yaw) };
    const lateral = Math.max(bridge ? 55 : 65, 3 * maxRadius(c, leg.engine, clip));
    const pos = { x: mid.p.x + right.x * lateral, y: mid.p.y + (bridge ? 16 : 12), z: mid.p.z + right.z * lateral };
    return makeRig(c, consistSampler(c.tl, leg.engine), clip, () => ({ pos }), { fixed: true });
  };
}

/** Chase shot: behind and to the side of the consist while it runs, slowly descending (as in the sketch). */
function chaseRig(c: Ctx, engine: string): RigFactory {
  return (clip) => makeRig(c, consistSampler(c.tl, engine), clip, (r) => {
    const D = Math.max(48, 2.6 * r.R) * r.stretch;
    const side = { x: r.fwd.z, z: -r.fwd.x };
    const h = clamp(0.28 * D, 10, 20) - 0.1 * D * r.u;
    return { pos: { x: r.anchor.x - r.fwd.x * 0.9 * D + side.x * 0.45 * D, y: r.anchor.y + h, z: r.anchor.z - r.fwd.z * 0.9 * D + side.z * 0.45 * D } };
  }, { share: 0.6 });
}

/**
 * Establishing crane: starts high and wide over the station, then descends
 * and settles on the three-quarter view of the first active engine, whose
 * consist is the look-at point at the end.
 */
function craneRig(c: Ctx, station: string, engine: string): RigFactory {
  const ax = axes(c, station);
  const site = { x: ax.site.x, y: ax.site.y + 2, z: ax.site.z };
  return (clip) => {
    const end = threeQuarter(c, station, { swing: 0, side: -1 });
    return makeRig(c, consistSampler(c.tl, engine), clip, (r) => {
      const e = smooth(r.u);
      const to = end({ ...r, u: 1 }).pos;
      const w = { x: to.x - r.anchor.x, z: to.z - r.anchor.z };
      const a = 0.6, ca = Math.cos(a), sa = Math.sin(a);
      const from = { x: r.anchor.x + (w.x * ca - w.z * sa) * 1.7, y: r.anchor.y + Math.max(55, 1.4 * r.D), z: r.anchor.z + (w.x * sa + w.z * ca) * 1.7 };
      // The first view takes in the station but already leans towards the engine, so the subject stays framed all along.
      return { pos: lerp(from, to, e), target: lerp(lerp(site, r.look, 0.5), r.look, e), fov: 0.75 - 0.05 * e };
    });
  };
}

/** Time lapse of travel: a high, wide, calm view that moves with the engine (constant offset, so the view hardly turns). */
function lapseRig(c: Ctx, engine: string): RigFactory {
  return (clip) => makeRig(c, consistSampler(c.tl, engine), clip, (r) => {
    const D = Math.max(70, 3.2 * r.R) * r.stretch;
    const side = { x: r.fwd.z, z: -r.fwd.x };
    return { pos: { x: r.anchor.x - r.fwd.x * 0.5 * D + side.x * 0.85 * D, y: r.anchor.y + 0.45 * D, z: r.anchor.z - r.fwd.z * 0.5 * D + side.z * 0.85 * D } };
  }, { share: 0.6 });
}

/** Set piece at a station throat (red signal, closed line), seen from inside the station. */
function throatRig(c: Ctx, station: string, end: 1 | -1): RigFactory {
  const ax = axes(c, station);
  const centre = stationPoint(ax.site, end * (STATION_HALF - 12), 0, 3);
  return (clip) => makeRig(c, fixedSampler(centre, 15, Math.atan2(ax.u.x * end, ax.u.z * end)), clip, threeQuarter(c, station, { d: (-end) as 1 | -1, swing: 0.2 }));
}

/** Set piece above the depot (the telegram), seen from the side of the station. */
function telegramRig(c: Ctx, station: string): RigFactory {
  const ax = axes(c, station);
  const centre = { x: ax.site.x - 4, y: ax.site.y + 8, z: ax.site.z };
  return (clip) => makeRig(c, fixedSampler(centre, 8), clip, threeQuarter(c, station, { d: 1, swing: 0.3 }));
}

/**
 * Slow descent over the whole network for the order sheet shot. The distance is fitted by projection:
 * every station lies in the central 70% of the 16:9 frame when the shot starts and in the central 76% when
 * it ends (the dolly-in shrinks the margin by 8%), so no station is cut off however the map is laid out.
 */
function networkRig(c: Ctx): RigFactory {
  const pts = [...c.geo.stations.values()];
  const cx = pts.reduce((s, p) => s + p.x, 0) / pts.length, cz = pts.reduce((s, p) => s + p.z, 0) / pts.length;
  const cy = pts.reduce((s, p) => s + p.y, 0) / pts.length;
  const fov = 0.75, pitch = 1.1, tan = Math.tan(fov / 2), aspect = 16 / 9;
  /** Largest normalised extent of the stations seen from `dist` away along yaw `yaw`. */
  const extent = (dist: number, yaw: number) => {
    const fx = -Math.sin(yaw) * Math.cos(pitch), fy = -Math.sin(pitch), fz = -Math.cos(yaw) * Math.cos(pitch);
    const rx = fz, rz = -fx, rl = Math.hypot(rx, rz);
    const right = { x: rx / rl, z: rz / rl };
    const up = { x: -fy * right.z, y: fx * right.z - fz * right.x, z: fy * right.x };
    let worst = 0;
    for (const p of pts) {
      const d = { x: p.x - cx, y: p.y - cy, z: p.z - cz };
      const depth = dist + d.x * fx + d.y * fy + d.z * fz;
      worst = Math.max(worst, Math.abs(d.x * right.x + d.z * right.z) / (depth * tan * aspect), Math.abs(d.x * up.x + d.y * up.y + d.z * up.z) / (depth * tan));
    }
    return worst;
  };
  let far = 100;
  while (far < 20000 && Math.max(extent(far, 0), extent(far, 0.15)) > 0.7) far *= 1.04;
  return (clip) => (f) => {
    const u = clip.duration > 0 ? f / clip.duration : 0;
    const dist = far * (1 - 0.08 * smooth(u)), yaw = 0.15 * u;
    return {
      pos: { x: cx + Math.sin(yaw) * Math.cos(pitch) * dist, y: cy + Math.sin(pitch) * dist, z: cz + Math.cos(yaw) * Math.cos(pitch) * dist },
      target: { x: cx, y: cy, z: cz }, fov
    };
  };
}

/** What a shot looks at. */
type Focus =
  /** `crossing` marks the arrival or departure stretch, which is a shot of its own so that the camera is not pulled back for a move that has not begun. */
  | { k: "consist"; engine: string; station: string; crossing?: "in" | "out" }
  | { k: "crane"; engine: string; station: string }
  | { k: "travel"; leg: Leg; chase: boolean }
  | { k: "pass"; leg: Leg; sMid: number; bridge: boolean }
  | { k: "network" }
  | { k: "throat"; station: string; end: 1 | -1 }
  | { k: "telegram"; station: string };

/** Identity of a focus: consecutive slices with the same key share one shot (and one camera). */
function focusKey(f: Focus): string {
  switch (f.k) {
    case "consist": return `c:${f.engine}:${f.station}:${f.crossing ?? ""}`;
    case "crane": return "crane";
    case "travel": return `travel:${f.leg.engine}:${f.leg.tDepart}`;
    case "pass": return `pass:${f.leg.engine}:${f.leg.tDepart}`;
    case "network": return "network";
    case "throat": return `throat:${f.station}:${f.end}`;
    case "telegram": return `telegram:${f.station}`;
  }
}

/** Something that happens in the run and must be seen: a sim window, film seconds, a focus and a caption. */
interface Activity {
  t0: number;
  t1: number;
  focus: Focus;
  /** Film seconds the whole window gets (shared among its slices when other activities interrupt it). */
  film: number;
  captions: Caption[];
  /** Shorter windows win overlaps; ties go to the higher priority. */
  priority: number;
  /** True for ops and failures: the activities a merged shot (`Budget.merge`) counts. */
  counted?: boolean;
}

/** Longest film in seconds. */
const MAX_FILM = 60;
/** Playback speed (sim seconds per film second) above which the HUD shows "fast-forward". */
const FAST_FROM = 4;
/** Metres from the stand an engine travels in its departure (and the last metres of an arrival) shot. */
const CROSS_M = 90;
/** Half length in metres of the stretch a trackside pass shows. */
const PASS_HALF = 45;
/** No shot is shorter than this; a shorter one is played slower so that it can be read. */
const MIN_SHOT = 2;
/** Film speeds: a leg time-lapsed and a chased leg (multiples of line speed), a trackside pass (metres per film second; it keeps the pan below 25 degrees per second). */
const SPEED = { lapse: 5, chase: 2.5, pass: 20 };
/** The least sim span in hours that is given a time-lapse look when nothing happens. */
const LAPSE_MIN_HOURS = 0.3;

/**
 * How many film seconds each kind of action gets, and which economies are applied. Sim time is never changed; the film
 * clock of each shot maps its whole sim window onto these seconds.
 */
interface Budget {
  /** A shunting move (couple, uncouple), however long its physical duration; a timed op (loading, processing), a delivery, an instant op. */
  shunt: number; timed: number; deliver: number; instant: number;
  /** A failure (abandoned or unreached op, hold, missed order, disruption) and a met order. */
  failure: number; disrupted: number; met: number;
  /** The establishing shot, an arrival or departure crossing, idle waiting per game hour. */
  crane: number; cross: number; idle: number;
  /** Film seconds of a time-lapsed leg and of a chased leg: lower and upper bound. */
  lapse: [number, number]; chase: [number, number];
  /** A trackside pass for every leg. */
  passes: boolean;
  /** Consecutive ops of one engine at one station share one shot of `base + each * (ops - 1)` seconds at most. */
  merge: { base: number; each: number } | null;
}

/**
 * The economies in the order they are applied until the film fits `MAX_FILM`: first full, then merged ops, then no
 * passes, then ops of 2 s with short travel (the last entry is scaled further by `squeeze` when even that is too long).
 */
const LEVELS: Budget[] = [
  { shunt: 4, timed: 3, deliver: 3, instant: 2.5, failure: 3.2, disrupted: 3.5, met: 2, crane: 3, cross: 1.5, idle: 0.6, lapse: [2, 6], chase: [3, 8], passes: true, merge: null },
  { shunt: 4, timed: 3, deliver: 3, instant: 2.5, failure: 3.2, disrupted: 3.5, met: 2, crane: 3, cross: 1.5, idle: 0.6, lapse: [2, 6], chase: [3, 8], passes: true, merge: { base: 3.4, each: 1.8 } },
  { shunt: 4, timed: 3, deliver: 3, instant: 2.5, failure: 3.2, disrupted: 3.5, met: 2, crane: 3, cross: 1.5, idle: 0.6, lapse: [2, 6], chase: [3, 8], passes: false, merge: { base: 3.4, each: 1.8 } },
  { shunt: 2, timed: 2, deliver: 2, instant: 2, failure: 2, disrupted: 2.5, met: 1.5, crane: 2.5, cross: 1.2, idle: 0.4, lapse: [1.5, 4], chase: [2.5, 5], passes: false, merge: { base: 2.2, each: 1.2 } }
];

/**
 * Film seconds an op needs: the budget's seconds of its kind, whatever the physical duration of its move is (a shunting
 * move is played at the speed that fits; the timeline's sim time stays physical).
 */
function opFilmSeconds(sp: OpSpan, b: Budget): number {
  if (sp.op.type === "couple" || sp.op.type === "uncouple") return b.shunt;
  return !sp.instant ? b.timed : sp.op.type === "deliver" ? b.deliver : b.instant;
}

/**
 * Builds the film for one run. Completeness comes first: nothing with an op, a failure or a delivery is ever dropped, it
 * only gets less film time. The film is composed with each `LEVELS` entry in turn until it lasts at most `MAX_FILM`; if
 * the last one is still too long, its shots are scaled down evenly (shots of ops and failures keep their 2 s).
 */
export function direct(run: LastRun, tl: Timeline, geo: Geography, names: Names): Film {
  let film = compose(run, tl, geo, names, LEVELS[0], 1);
  for (let i = 1; i < LEVELS.length && film.duration > MAX_FILM; i++) film = compose(run, tl, geo, names, LEVELS[i], 1);
  // Last resort: the film of the last level is scaled to fit, shrinking only what is not an op or a failure.
  for (let squeeze = 0.85; film.duration > MAX_FILM && squeeze > 0.2; squeeze *= 0.85) film = compose(run, tl, geo, names, LEVELS[LEVELS.length - 1], squeeze);
  return film;
}

/**
 * Composes the film with the film-time budget `b`. `squeeze` (at most 1) scales the film seconds of everything that is not an
 * op or a failure (crane, crossings, travel, idle, orders) for the last resort of `direct`.
 */
function compose(run: LastRun, tl: Timeline, geo: Geography, names: Names, b: Budget, squeeze: number): Film {
  const c: Ctx = { geo, tl, names };
  const events = tl.events;
  const engineStations = new Map(run.actual.initial.engines.map((e) => [e.id, e.station]));
  const start = run.actual.initial.hour;
  const deadlines = new Map(run.actual.initial.orders.map((o) => [o.id, o.deadline]));
  const outcomes = new Map(run.orders.map((o) => [o.id, o]));
  const say = (hour: number, text: string) => `${clock12(hour)} · ${text}`;
  const wire = (e: (typeof events)[number]) => ({ hour: e.hour, kind: e.kind });

  const acts: Activity[] = [];
  const push = (a: Activity) => acts.push(a);

  // The day begins: an establishing crane over the first active engine's station.
  const firstDepart = events.find((e) => e.kind.type === "depart");
  const withEngine = events.find((e) => "engine" in e.kind);
  const craneEngine = firstDepart && firstDepart.kind.type === "depart" ? firstDepart.kind.engine
    : withEngine ? (withEngine.kind as { engine: string }).engine : [...engineStations.keys()][0];
  const craneStation = engineStations.get(craneEngine) ?? [...engineStations.values()][0];
  push({
    t0: start - 0.2, t1: start, focus: { k: "crane", engine: craneEngine, station: craneStation }, film: b.crane * squeeze, priority: 20,
    captions: [{ t0: start - 0.2, t1: start, tone: "info", text: `The day begins. ${[...engineStations].map(([e, s]) => `${e} stands at ${names(s)}`).join(", ")}.` }]
  });

  // Ops: every op_done (with its op_started when it has a duration) is one activity.
  for (const sp of tl.spans) {
    const len = sp.t1 - sp.t0;
    const doneFrom = sp.instant ? sp.t0 : sp.t1 - Math.max(0.12 * len, 0.05);
    const startText = sp.instant
      ? narrate({ hour: sp.hour, kind: { type: "op_done", engine: sp.engine, station: sp.station, stop: sp.stop, index: sp.index, op: sp.op, cars: sp.cars } }, names)
      : narrate({ hour: sp.hour, kind: { type: "op_started", engine: sp.engine, station: sp.station, stop: sp.stop, index: sp.index, op: sp.op, finish_hour: sp.finishHour } }, names);
    const captions: Caption[] = [{ t0: sp.t0, t1: sp.instant ? sp.t1 : doneFrom, tone: "info", text: say(sp.hour, startText) }];
    if (!sp.instant) captions.push({ t0: doneFrom, t1: sp.t1, tone: "info", text: say(sp.finishHour, finishedText(sp.engine, sp.op, sp.station, names)) });
    push({ t0: sp.t0, t1: sp.t1, focus: { k: "consist", engine: sp.engine, station: sp.station }, film: opFilmSeconds(sp, b), captions, priority: 5, counted: true });
  }

  /** Sim windows of a leg: the first and last `d` metres are shown with the stations, the rest is travel. */
  const windowsOf = (leg: Leg) => {
    const d = Math.min(CROSS_M, 0.3 * (leg.s1 - leg.s0));
    return { d, out: tl.timeAtDistance(leg, leg.s0 + d), in: tl.timeAtDistance(leg, leg.s1 - d) };
  };

  // Other events.
  const activityOfEvent = new Map<(typeof events)[number], Activity>();
  const point = (e: (typeof events)[number], focus: Focus, film: number, tone: Tone, text: string, priority: number, t0 = e.t, t1 = e.t + ANIM_HOURS) => {
    const a: Activity = { t0, t1, focus, film, priority, counted: tone === "fail", captions: [{ t0, t1, tone, text: say(e.hour, text) }] };
    activityOfEvent.set(e, a);
    push(a);
  };
  for (const e of events) {
    const k = e.kind;
    switch (k.type) {
      case "arrive": {
        const leg = tl.legs.find((l) => l.engine === k.engine && l.to === k.station && Math.abs(l.tArrive - e.t) < 0.5);
        const w = leg ? windowsOf(leg) : null;
        point(e, { k: "consist", engine: k.engine, station: k.station, crossing: "in" }, b.cross * squeeze, "info", narrate(wire(e), names), 4, w ? Math.min(w.in, e.t) : e.t);
        break;
      }
      case "depart": {
        const leg = tl.legs.find((l) => l.engine === k.engine && l.from === k.from && Math.abs(l.tDepart - e.t) < 1e-6);
        const w = leg ? windowsOf(leg) : null;
        point(e, { k: "consist", engine: k.engine, station: k.from, crossing: "out" }, b.cross * squeeze, "info", narrate(wire(e), names), 4, e.t, w ? Math.max(w.out, e.t + ANIM_HOURS) : e.t + ANIM_HOURS);
        break;
      }
      case "hold": point(e, { k: "consist", engine: k.engine, station: k.station }, b.failure, "fail", narrate(wire(e), names), 6); break;
      case "op_abandoned": point(e, { k: "consist", engine: k.engine, station: k.station }, b.failure, "fail", describeAbandon(k, names), 7); break;
      case "op_unreached": point(e, { k: "consist", engine: k.engine, station: k.station }, b.failure, "fail", narrate(wire(e), names), 6); break;
      case "order_met": {
        const sp = [...tl.spans].reverse().find((s) => s.op.type === "deliver" && s.op.order === k.order && s.t0 <= e.t + 1e-6);
        const o = outcomes.get(k.order);
        const text = `${k.order} met: delivered${o?.delivered_at != null ? ` at ${clock12(o.delivered_at)}` : ""}${deadlines.has(k.order) ? ` (deadline ${clock12(deadlines.get(k.order)!)})` : ""}`;
        if (sp) point(e, { k: "consist", engine: sp.engine, station: sp.station }, b.met * squeeze, "good", text, 6);
        else point(e, { k: "network" }, b.met * squeeze, "good", text, 6);
        break;
      }
      case "order_missed": point(e, { k: "network" }, b.failure, "fail", describeMiss(k.order, k.why, deadlines.get(k.order) ?? null, names), 8); break;
      case "disrupted": {
        const what = k.what;
        let focus: Focus;
        if (what.type === "engine_failure") focus = { k: "consist", engine: what.engine, station: what.station };
        else if (what.type === "track_closure") {
          const found = geo.edgeBetween(what.a, what.b);
          focus = { k: "throat", station: what.a, end: found ? (found.forward ? found.edge.endA : found.edge.endB) : 1 };
        } else focus = { k: "telegram", station: geo.stations.has("corning") ? "corning" : [...geo.stations.keys()][0] };
        point(e, focus, b.disrupted, "fail", narrate(wire(e), names), 10, e.t, e.t + 0.4);
        break;
      }
      default: break;
    }
  }

  // Legs: the stretch between the departure and the arrival shots is travel. It is time-lapsed (5x line speed) by a
  // high camera that moves with the engine; the leg before the last delivery is chased (2.5x) closer; with passes, a
  // trackside pass shows the middle of each leg at 20 m/s.
  const lastDeliver = [...tl.spans].reverse().find((s) => s.op.type === "deliver");
  const chased = run.success && lastDeliver ? [...tl.legs].reverse().find((l) => l.engine === lastDeliver.engine && l.tArrive <= lastDeliver.t0 + 1e-6) : undefined;
  for (const leg of tl.legs) {
    const w = windowsOf(leg);
    if (w.in - w.out < 0.02) continue;
    const found = geo.edgeBetween(leg.from, leg.to);
    const bridge = !!found?.edge.bridge;
    const isChase = leg === chased;
    // Film time follows the leg's own comfortable time at line speed (`Leg.minSeconds`), scaled to the stretch shown.
    const shown = (leg.minSeconds * (leg.s1 - leg.s0 - 2 * w.d)) / (leg.s1 - leg.s0);
    const metres = leg.s1 - leg.s0 - 2 * w.d;
    const caption = (t0: number, t1: number): Caption => ({ t0, t1, tone: "info", text: `${leg.engine} runs ${names(leg.from)} to ${names(leg.to)}${bridge ? ", over the river" : ""}` });
    push({
      t0: w.out, t1: w.in, focus: { k: "travel", leg, chase: isChase }, priority: 1, captions: [caption(w.out, w.in)],
      film: squeeze * (isChase ? clamp(shown / SPEED.chase, ...b.chase) : clamp(shown / SPEED.lapse, ...b.lapse))
    });
    if (!b.passes || isChase || metres < 4 * PASS_HALF) continue;
    let sMid = leg.s0 + (leg.s1 - leg.s0) / 2;
    if (bridge && found?.edge.bridge) {
      const b = found.edge.bridge;
      const mp = found.edge.path.at((b.from + b.to) / 2).p;
      let best = Infinity;
      for (let s = leg.s0 + w.d + PASS_HALF; s <= leg.s1 - w.d - PASS_HALF; s += 8) {
        const q = leg.path.at(s).p, d = Math.hypot(q.x - mp.x, q.z - mp.z);
        if (d < best) { best = d; sMid = s; }
      }
    }
    const p0 = tl.timeAtDistance(leg, sMid - PASS_HALF), p1 = tl.timeAtDistance(leg, sMid + PASS_HALF);
    push({ t0: p0, t1: p1, focus: { k: "pass", leg, sMid, bridge }, priority: 2, captions: [caption(p0, p1)], film: (2 * PASS_HALF) / SPEED.pass });
  }

  // Divergence: the matching activity also says what the viewer's notes expected.
  const div = run.divergence;
  if (div) {
    const target = div.actual ? events.find((e) => e.hour === div.actual!.hour && JSON.stringify(e.kind) === JSON.stringify(div.actual!.kind)) : undefined;
    const exp = div.expected ? narrate(div.expected, names) : "nothing more";
    const note = `Your notes: ${exp}.`;
    const carrier = (target && activityOfEvent.get(target)) ?? acts.find((a) => a.t0 <= (target?.t ?? div.hour) + 1e-6 && a.t1 > (target?.t ?? div.hour) && a.focus.k === "consist");
    if (carrier) carrier.captions.forEach((cap) => { cap.note = note; });
    else {
      const t = target?.t ?? div.hour;
      const station = tl.engineStation(div.engine, t) ?? [...engineStations.values()][0];
      push({ t0: t, t1: t + ANIM_HOURS, focus: { k: "consist", engine: div.engine, station }, film: b.failure, priority: 9, counted: true, captions: [{ t0: t, t1: t + ANIM_HOURS, tone: "info", text: say(div.hour, `${div.engine}: nothing more happens`), note }] });
    }
  }

  // Sweep: cut sim time at every window boundary; the shortest window covering a slice owns it.
  const bounds = [...new Set(acts.flatMap((a) => [a.t0, a.t1, ...a.captions.flatMap((cp) => [cp.t0, cp.t1])]))].sort((a, b) => a - b);
  interface Slice { x: number; y: number; act: Activity | null; focus: Focus; key: string; caption: Caption | null; film: number }
  const slices: Slice[] = [];
  const nextConsist = (y: number) => acts.filter((a) => a.t0 >= y - 1e-6 && a.focus.k === "consist").sort((a, b) => a.t0 - b.t0)[0];
  for (let i = 0; i + 1 < bounds.length; i++) {
    const x = bounds[i], y = bounds[i + 1];
    if (y - x < 1e-6) continue;
    const cover = acts.filter((a) => a.t0 <= x + 1e-9 && a.t1 >= y - 1e-9).sort((a, b) => (a.t1 - a.t0) - (b.t1 - b.t0) || b.priority - a.priority);
    const act = cover[0] ?? null;
    if (act) {
      const mid = (x + y) / 2;
      const caption = act.captions.find((cp) => cp.t0 <= mid && mid < cp.t1) ?? act.captions[act.captions.length - 1] ?? null;
      slices.push({ x, y, act, focus: act.focus, key: focusKey(act.focus), caption, film: act.film * (y - x) / (act.t1 - act.t0) });
      continue;
    }
    // Idle: no engine moves and nothing happens. The camera stays at the next stop (or where it was) and the clock runs fast.
    const next = nextConsist(y);
    const prev = slices[slices.length - 1];
    const long = y - x >= LAPSE_MIN_HOURS;
    let focus: Focus, text = "";
    if (!long && prev) focus = prev.focus;
    else if (next && next.focus.k === "consist") { focus = next.focus; text = `${next.focus.engine} waits at ${names(next.focus.station)}`; }
    else focus = { k: "consist", engine: craneEngine, station: tl.engineStation(craneEngine, x) ?? craneStation };
    slices.push({
      x, y, act: null, focus, key: focusKey(focus),
      caption: text && long ? { t0: x, t1: y, tone: "info", text } : null, film: Math.max(0.25, (y - x) * b.idle * squeeze)
    });
  }

  // Merge consecutive slices with one key into shots. A shot of several counted activities (ops, failures) is cut to the
  // budget's merged length; a shot is never shorter than MIN_SHOT.
  const shots: Shot[] = [];
  let group: Slice[] = [];
  const flush = () => {
    if (!group.length) return;
    const first = group[0], last = group[group.length - 1];
    const t0 = first.x, t1 = last.y;
    const total = group.reduce((s, g) => s + g.film, 0);
    const counted = new Set(group.flatMap((g) => (g.act?.counted ? [g.act] : []))).size;
    const merged = b.merge && counted >= 2 ? Math.min(total, b.merge.base + b.merge.each * (counted - 1)) : total;
    const scale = Math.max(merged, MIN_SHOT) / total;
    const pieces: Piece[] = [];
    let f = 0;
    const captions: Caption[] = [];
    for (const s of group) {
      const film = s.film * scale;
      pieces.push({ f0: f, f1: f + film, t0: s.x, t1: s.y, fast: ((s.y - s.x) * SECONDS_PER_HOUR) / film > FAST_FROM });
      f += film;
      if (s.caption) {
        const prev = captions[captions.length - 1];
        if (prev && prev.text === s.caption.text && Math.abs(prev.t1 - s.x) < 1e-6) prev.t1 = s.y;
        else captions.push({ ...s.caption, t0: s.x, t1: s.y });
      }
    }
    const clip = makeClip(pieces);
    const focus = first.focus;
    let kind: ShotKind = "yard", rig: RigFactory;
    let engine: string | null = null;
    switch (focus.k) {
      case "consist": rig = yardRig(c, focus.station, focus.engine); engine = focus.engine; break;
      case "crane": rig = craneRig(c, focus.station, focus.engine); kind = "crane"; engine = focus.engine; break;
      case "travel": rig = focus.chase ? chaseRig(c, focus.leg.engine) : lapseRig(c, focus.leg.engine); kind = focus.chase ? "chase" : "lapse"; engine = focus.leg.engine; break;
      case "pass": rig = passRig(c, focus.leg, focus.sMid, focus.bridge); kind = "pass"; engine = focus.leg.engine; break;
      case "network": rig = networkRig(c); kind = "orders"; break;
      case "throat": rig = throatRig(c, focus.station, focus.end); kind = "disruption"; break;
      case "telegram": rig = telegramRig(c, focus.station); kind = "disruption"; break;
    }
    let built: ((f: number) => CameraPose) | null = null;
    shots.push({
      kind, duration: f, t0, t1, captions, engine,
      at: (ff) => clip.at(ff),
      camera: (ff) => { built ??= rig(clip); return built(ff); }
    });
    group = [];
  };
  for (const s of slices) {
    if (group.length && group[0].key !== s.key) flush();
    group.push(s);
  }
  flush();

  return { shots, duration: shots.reduce((x, s) => x + s.duration, 0), needsAcknowledge: true };
}
