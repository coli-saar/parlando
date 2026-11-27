/**
 * Geography of the Night Freight world: where the stations lie, how each edge of
 * the railway map is laid out as a track spline, where the river and its bridge
 * are, and how high the ground is.
 *
 * The module is pure TypeScript (no Babylon types) so that the replay timeline can
 * ask it for train paths and the scene builders can ask it for terrain heights.
 * It owns: station frames (centre, axis, track slots), edge splines (700 to 1200 m
 * whatever the travel time), the river polyline, the bridge span, and the terrain
 * height field. Coordinates are metres; x east, z north, y up; rails sit at the
 * stored track height.
 */
import type { GameMap, StationId } from "../game";

/** A plain 3-vector so geography does not depend on the 3D engine. */
export interface P3 { x: number; y: number; z: number }

/** Axis coordinate of the throats: all yard tracks of a station coincide at `|u| = STATION_HALF`. */
export const STATION_HALF = 125;
/** Length of the straight, shared track (`neck`) between a throat and the junction where the edges leave the station. */
export const NECK_LENGTH = 20;
/** Axis coordinate of the junctions, where the edge tracks begin. */
export const STATION_END = STATION_HALF + NECK_LENGTH;
/** Length of the straight headshunt that runs on beyond each junction: room for a train to clear the turnouts before it reverses. */
export const HEADSHUNT_LENGTH = 110;
/** Axis coordinate at which the headshunts end: the extent of every yard track. */
export const STATION_EXTENT = STATION_END + HEADSHUNT_LENGTH;
/** Distance between neighbouring yard tracks, centre to centre, in metres. */
export const TRACK_SPACING = 4.5;
/** Radius in metres of the circular arcs of every turnout (an S-curve of two arcs that moves a track by `TRACK_SPACING`). */
export const TURNOUT_RADIUS = 160;
/** Length along the axis of one turnout: the two arcs of radius `TURNOUT_RADIUS` that shift a track by `TRACK_SPACING`. */
export const TURNOUT_LENGTH = 2 * Math.sqrt(TURNOUT_RADIUS * TRACK_SPACING - (TRACK_SPACING * TRACK_SPACING) / 4);
/** Outermost yard track on either side of the running line (levels run from -3 to 3). */
export const MAX_LEVEL = 3;
/** Distance of an engine's centre from the station centre along its track while it stands. */
export const STAND_CENTRE = 29.5;
/** Smallest distance between the centre lines of two tracks at which vehicles on both may stand or pass (vehicle width plus margin). */
export const FOUL_CLEARANCE = 3.4;
/** Distance in metres at which two tracks no longer foul each other for vehicles standing on both. */
export const JUNCTION_SEPARATION = 4;
/** Standard gauge in metres. */
export const GAUGE = 1.435;

/** Fixed layout of the five stations on the classic map (x east, z north). */
const STATION_SITES: Record<string, { x: number; z: number; yaw: number; y: number }> = {
  avon: { x: 0, z: 0, yaw: 0.1, y: 12 },
  dansville: { x: -850, z: -300, yaw: 0.35, y: 14 },
  bath: { x: 700, z: -380, yaw: -0.25, y: 18 },
  corning: { x: -150, z: -850, yaw: -0.64, y: 8 },
  elmira: { x: -200, z: -1850, yaw: 1.4, y: 5 }
};

/** A deterministic hash in [0,1) used for all procedural placement. */
export function hash2(x: number, z: number, seed = 0): number {
  let h = Math.imul(x | 0, 374761393) ^ Math.imul(z | 0, 668265263) ^ Math.imul(seed | 0, 1274126177);
  h = Math.imul(h ^ (h >>> 13), 1274126177);
  return ((h ^ (h >>> 16)) >>> 0) / 4294967296;
}

/** Smooth value noise in [0,1]. */
export function noise2(x: number, z: number, seed = 0): number {
  const xi = Math.floor(x), zi = Math.floor(z);
  const xf = x - xi, zf = z - zi;
  const u = xf * xf * (3 - 2 * xf), v = zf * zf * (3 - 2 * zf);
  const a = hash2(xi, zi, seed), b = hash2(xi + 1, zi, seed), c = hash2(xi, zi + 1, seed), d = hash2(xi + 1, zi + 1, seed);
  return a + (b - a) * u + (c - a) * v + (a - b - c + d) * u * v;
}

/** Fractal noise in roughly [0,1]. */
export function fbm(x: number, z: number, octaves = 4, seed = 0): number {
  let sum = 0, amp = 0.5, norm = 0;
  for (let i = 0; i < octaves; i++) {
    sum += amp * noise2(x, z, seed + i);
    norm += amp;
    x *= 2.03; z *= 2.03; amp *= 0.5;
  }
  return sum / norm;
}

/** Hermite smoothstep from edge e0 to e1 (works in either direction). */
export function smoothstep(e0: number, e1: number, x: number): number {
  const t = Math.min(1, Math.max(0, (x - e0) / (e1 - e0)));
  return t * t * (3 - 2 * t);
}

/** A station site: its frame (centre, axis angle) and platform height. */
export interface StationSite {
  id: StationId;
  name: string;
  x: number; z: number; y: number;
  /** Angle of the station axis u in the xz plane (u = (cos yaw, sin yaw)). */
  yaw: number;
  factory: boolean;
}

/** Converts station-local (u along the tracks, v across) to world coordinates. */
export function stationPoint(site: StationSite, u: number, v: number, dy = 0): P3 {
  const c = Math.cos(site.yaw), s = Math.sin(site.yaw);
  return { x: site.x + u * c - v * s, y: site.y + dy, z: site.z + u * s + v * c };
}

/**
 * Axis coordinate at which yard track `|k|` has merged into track `|k| - 1`.
 * The turnouts form a ladder: track 1 leaves the running line over the last
 * `TURNOUT_LENGTH` metres before the throat, and each further track starts half
 * a turnout length further in, so that the second arc of one turnout runs beside
 * the first arc of the next and the curvature of any track never exceeds
 * `1 / TURNOUT_RADIUS`.
 */
export function mergeEnd(k: number): number { return STATION_HALF - ((k - 1) * TURNOUT_LENGTH) / 2; }

/** Axis coordinate at which yard track `|k|` starts to separate from track `|k| - 1` (the toe of its turnout). */
export function mergeStart(k: number): number { return mergeEnd(k) - TURNOUT_LENGTH; }

/** Lateral shift in metres of a turnout after `x` metres along it: arc, then counter-arc, over `TURNOUT_LENGTH`. */
function turnoutShift(x: number): number {
  const L = TURNOUT_LENGTH, R = TURNOUT_RADIUS;
  if (x <= 0) return 0;
  if (x >= L) return TRACK_SPACING;
  const arc = (y: number) => R - Math.sqrt(R * R - y * y);
  return x <= L / 2 ? arc(x) : TRACK_SPACING - arc(L - x);
}

/**
 * Lateral offset (across the axis) of yard track `level` at axis coordinate `u`.
 * Level 0 is the running line; levels +-1 are the engine tracks beside it and
 * +-2, +-3 are the sidings. Every track runs parallel at `level * TRACK_SPACING`
 * in the middle of the station and merges into its inner neighbour through a
 * turnout of radius `TURNOUT_RADIUS`, symmetrically at both throats, so all
 * tracks coincide beyond the throats (on the neck) and a vehicle can switch
 * between any two tracks there without leaving the rails.
 */
export function trackLateral(level: number, u: number): number {
  const a = Math.abs(u);
  let v = 0;
  for (let k = 1; k <= Math.abs(level); k++) v += TRACK_SPACING - turnoutShift(a - mergeStart(k));
  return Math.sign(level) * v;
}

/** World point on yard track `level` at axis coordinate `u` (straight on beyond the throats). */
export function trackPoint(site: StationSite, level: number, u: number): P3 {
  return stationPoint(site, u, trackLateral(level, u));
}

/** Points along yard track `level` from axis coordinate `u0` to `u1` (either order), about `step` metres apart. */
export function trackPoints(site: StationSite, level: number, u0: number, u1: number, step = 2.5): P3[] {
  const n = Math.max(1, Math.ceil(Math.abs(u1 - u0) / step));
  const out: P3[] = [];
  for (let i = 0; i <= n; i++) out.push(trackPoint(site, level, u0 + ((u1 - u0) * i) / n));
  return out;
}

/** Engine track (level 0, +1 or -1) assigned to an engine by its index. */
export function engineLevel(index: number): number { return [0, 1, -1][index % 3]; }

/** Levels of the sidings, in the order in which they are preferred. */
export const SIDING_LEVELS: readonly number[] = [2, -2, 3, -3];

/**
 * Axis coordinate beyond which two tracks are the same rails (within two millimetres): the
 * point where a train that came out of one must have cleared the turnouts before it can
 * reverse onto the other. Tracks on the same level never differ.
 */
export function divergencePoint(a: number, b: number): number {
  if (a === b) return 0;
  for (let u = STATION_HALF; u > 0; u -= 0.25) if (Math.abs(trackLateral(a, u) - trackLateral(b, u)) > 0.002) return u + 0.25;
  return 0;
}

/**
 * Half length of the part of siding `level` on which a vehicle keeps `FOUL_CLEARANCE`
 * metres from the neighbouring tracks: a vehicle parked there never fouls a train that
 * runs on any other track. Beyond it the sidings converge towards the throat.
 */
export function sidingReach(level: number): number {
  const neighbours = [level - Math.sign(level), level + Math.sign(level)].filter((n) => Math.abs(n) <= MAX_LEVEL && n !== level);
  let u = 0;
  while (u < STATION_HALF && neighbours.every((n) => Math.abs(trackLateral(level, u + 0.25) - trackLateral(n, u + 0.25)) >= FOUL_CLEARANCE)) u += 0.25;
  return u;
}

/** Heading (radians, `atan2(dx, dz)`) of a vehicle standing on the axis with its nose towards `head` (+1 or -1). */
export function axisYaw(site: StationSite, head: 1 | -1): number { return Math.atan2(head * Math.cos(site.yaw), head * Math.sin(site.yaw)); }

/** A station track sampled along the axis, with the arc length of any axis coordinate. */
export interface Lane {
  /** The track from axis coordinate `-dir * STATION_EXTENT` to `+dir * STATION_EXTENT` (including both necks and headshunts). */
  path: Path;
  /** Arc length of the point at axis coordinate `u`. */
  arcAt(u: number): number;
  /** Axis coordinate of the point at arc length `s` (straight on beyond both ends). */
  uAt(s: number): number;
}

/** A polyline with cumulative arc length, extrapolated straight beyond both ends. */
export class Path {
  readonly points: P3[];
  readonly cum: number[];
  readonly length: number;
  constructor(points: P3[]) {
    const kept: P3[] = [];
    for (const p of points) {
      const last = kept[kept.length - 1];
      if (!last || Math.hypot(p.x - last.x, p.z - last.z) > 1e-3) kept.push(p);
    }
    this.points = kept;
    this.cum = [0];
    for (let i = 1; i < kept.length; i++) {
      const a = kept[i - 1], b = kept[i];
      this.cum.push(this.cum[i - 1] + Math.hypot(b.x - a.x, b.y - a.y, b.z - a.z));
    }
    this.length = this.cum[this.cum.length - 1];
  }

  /** Position and heading (radians of the direction vector, atan2(dx, dz)) at arc length s. */
  at(s: number): { p: P3; yaw: number } {
    const n = this.points.length;
    let i: number;
    if (s <= 0) i = 1;
    else if (s >= this.length) i = n - 1;
    else {
      let lo = 1, hi = n - 1;
      while (lo < hi) {
        const mid = (lo + hi) >> 1;
        if (this.cum[mid] < s) lo = mid + 1; else hi = mid;
      }
      i = lo;
    }
    const a = this.points[i - 1], b = this.points[i];
    const seg = this.cum[i] - this.cum[i - 1] || 1;
    const t = (s - this.cum[i - 1]) / seg;
    const dx = b.x - a.x, dz = b.z - a.z;
    return {
      p: { x: a.x + (b.x - a.x) * t, y: a.y + (b.y - a.y) * Math.max(0, Math.min(1, t)), z: a.z + (b.z - a.z) * t },
      yaw: Math.atan2(dx, dz)
    };
  }

  /** Returns a path running the other way. */
  reversed(): Path { return new Path([...this.points].reverse()); }

  /**
   * The polyline between arc lengths `s0` and `s1` (clamped to the path), including both
   * end points, running from `s0` to `s1` (backwards when `s1 < s0`). Its length is `|s1 - s0|`.
   */
  slice(s0: number, s1: number): P3[] {
    const a = Math.min(Math.max(Math.min(s0, s1), 0), this.length), b = Math.min(Math.max(Math.max(s0, s1), 0), this.length);
    const out: P3[] = [this.at(a).p];
    for (let i = 0; i < this.points.length; i++) if (this.cum[i] > a + 1e-9 && this.cum[i] < b - 1e-9) out.push(this.points[i]);
    out.push(this.at(b).p);
    return s1 < s0 ? out.reverse() : out;
  }
}

/** Resamples a polyline at roughly even spacing in the xz plane. */
function resample(points: P3[], spacing: number): P3[] {
  const path = new Path(points);
  const n = Math.max(2, Math.round(path.length / spacing));
  const out: P3[] = [];
  for (let i = 0; i <= n; i++) out.push(path.at((path.length * i) / n).p);
  return out;
}

/**
 * Points of the cubic Bezier curve from `p0` to `p1` that leaves `p0` along the unit vector `out0`
 * and arrives at `p1` against the unit vector `out1` (both pointing away from their stations), with
 * handles of the given lengths; y is interpolated linearly.
 */
function bezier(p0: P3, out0: P3, k0: number, p1: P3, out1: P3, k1: number, count: number): P3[] {
  const c0 = { x: p0.x + out0.x * k0, z: p0.z + out0.z * k0 }, c1 = { x: p1.x + out1.x * k1, z: p1.z + out1.z * k1 };
  const out: P3[] = [];
  for (let i = 0; i <= count; i++) {
    const t = i / count, u = 1 - t;
    out.push({
      x: u * u * u * p0.x + 3 * u * u * t * c0.x + 3 * u * t * t * c1.x + t * t * t * p1.x,
      z: u * u * u * p0.z + 3 * u * u * t * c0.z + 3 * u * t * t * c1.z + t * t * t * p1.z,
      y: p0.y + (p1.y - p0.y) * t
    });
  }
  return out;
}

/**
 * Smallest radius of curvature (metres) of the cubic Bezier curve from `p0` to `p1` with the
 * given handles (see `bezier`), from its analytic derivatives.
 */
function bezierRadius(p0: P3, out0: P3, k0: number, p1: P3, out1: P3, k1: number): number {
  const c0 = { x: p0.x + out0.x * k0, z: p0.z + out0.z * k0 }, c1 = { x: p1.x + out1.x * k1, z: p1.z + out1.z * k1 };
  let best = Infinity;
  for (let i = 0; i <= 80; i++) {
    const t = i / 80, u = 1 - t;
    const dx = 3 * (u * u * (c0.x - p0.x) + 2 * u * t * (c1.x - c0.x) + t * t * (p1.x - c1.x));
    const dz = 3 * (u * u * (c0.z - p0.z) + 2 * u * t * (c1.z - c0.z) + t * t * (p1.z - c1.z));
    const ddx = 6 * (u * (c1.x - 2 * c0.x + p0.x) + t * (p1.x - 2 * c1.x + c0.x));
    const ddz = 6 * (u * (c1.z - 2 * c0.z + p0.z) + t * (p1.z - 2 * c1.z + c0.z));
    const cross = Math.abs(dx * ddz - dz * ddx);
    if (cross > 1e-9) best = Math.min(best, Math.pow(dx * dx + dz * dz, 1.5) / cross);
  }
  return best;
}

/** One undirected edge of the map with its laid track. */
export interface EdgeTrack {
  a: StationId;
  b: StationId;
  /** Throat end (+1 or -1 on the station axis) used at a and at b. */
  endA: 1 | -1;
  endB: 1 | -1;
  /** Track centre line from the junction of a to the junction of b (it leaves the stations' necks there), with rail heights. */
  path: Path;
  /** Arc-length interval of the bridge on this edge, if any. */
  bridge: { from: number; to: number } | null;
}

/** River centre line (xz) with its water-surface height at each vertex. */
const RIVER: { x: number; z: number }[] = [
  { x: 1500, z: -1250 }, { x: 900, z: -1330 }, { x: 420, z: -1230 }, { x: -20, z: -1370 },
  { x: -430, z: -1300 }, { x: -900, z: -1440 }, { x: -1500, z: -1380 }
];

/** Distance from a point to a polyline and the index/parameter of the closest point. */
function nearestOnPolyline(poly: { x: number; z: number }[], x: number, z: number): { d: number; i: number; t: number } {
  let best = { d: Infinity, i: 0, t: 0 };
  for (let i = 0; i < poly.length - 1; i++) {
    const a = poly[i], b = poly[i + 1];
    const dx = b.x - a.x, dz = b.z - a.z;
    const t = Math.max(0, Math.min(1, ((x - a.x) * dx + (z - a.z) * dz) / (dx * dx + dz * dz || 1)));
    const d = Math.hypot(x - (a.x + dx * t), z - (a.z + dz * t));
    if (d < best.d) best = { d, i, t };
  }
  return best;
}

/** A sampled corridor point used to flatten terrain around tracks. */
interface Sample { x: number; z: number; y: number; bridge: boolean }

/** The whole static geography of one map: stations, tracks, river, height field. */
export class Geography {
  readonly stations = new Map<StationId, StationSite>();
  readonly edges: EdgeTrack[] = [];
  readonly river: { x: number; z: number; y: number }[];
  /** Half width of the water surface in metres. */
  readonly riverHalfWidth = 14;
  private readonly samples: Sample[] = [];
  private readonly grid = new Map<string, number[]>();
  private readonly cell = 60;
  private readonly lanes = new Map<string, Lane>();
  private readonly clearances = new Map<string, number>();

  constructor(map: GameMap) {
    map.stations.forEach((s, i) => {
      const site = STATION_SITES[s.id] ?? {
        x: 900 * Math.cos(i * 1.3), z: -900 + 900 * Math.sin(i * 1.3), yaw: 0, y: 10
      };
      this.stations.set(s.id, { id: s.id, name: s.name, factory: s.factory, ...site });
    });
    // River water level descends from east to west; clamp it below the nearby track.
    this.river = RIVER.map((p, i) => ({ x: p.x, z: p.z, y: 3.5 - i * 0.2 }));
    for (const e of map.edges) this.edges.push(this.layEdge(e.a, e.b));
    this.edges.forEach((e) => this.indexEdge(e));
    // Station yards are flat areas as well.
    for (const site of this.stations.values()) {
      for (let u = -STATION_EXTENT; u <= STATION_EXTENT; u += 15) {
        for (const v of [-27, -18, -9, 0, 9, 18, 27]) {
          const p = stationPoint(site, u, v);
          this.addSample({ x: p.x, z: p.z, y: site.y, bridge: false });
        }
      }
    }
  }

  /** Throat end of a station used by the edge towards `other`: the axis end facing it. */
  private throatEnd(site: StationSite, other: StationSite, taken: number[]): 1 | -1 {
    const dot = (other.x - site.x) * Math.cos(site.yaw) + (other.z - site.z) * Math.sin(site.yaw);
    void taken;
    return dot >= 0 ? 1 : -1;
  }

  /** Lays a smooth curve from the junction of `a` to the junction of `b`, leaving and entering along the stations' axes. */
  private layEdge(a: StationId, b: StationId): EdgeTrack {
    const sa = this.stations.get(a)!, sb = this.stations.get(b)!;
    const endA = this.throatEnd(sa, sb, []);
    const endB = this.throatEnd(sb, sa, []);
    const pa = stationPoint(sa, endA * STATION_END, 0, 0);
    const pb = stationPoint(sb, endB * STATION_END, 0, 0);
    const outA = { x: Math.cos(sa.yaw) * endA, y: 0, z: Math.sin(sa.yaw) * endA };
    const outB = { x: Math.cos(sb.yaw) * endB, y: 0, z: Math.sin(sb.yaw) * endB };
    const chord = Math.hypot(pb.x - pa.x, pb.z - pa.z);
    // A cubic Bezier curve that leaves and enters along the stations' axes; the handle lengths are
    // chosen to keep the sharpest bend as gentle as the stations' orientations allow.
    let best: { r: number; a: number; b: number } | null = null;
    for (let fa = 0.15; fa <= 1.001; fa += 0.05) {
      for (let fb = 0.15; fb <= 1.001; fb += 0.05) {
        const r = bezierRadius(pa, outA, fa * chord, pb, outB, fb * chord);
        if (!best || r > best.r + 1e-9) best = { r, a: fa, b: fb };
      }
    }
    const flat = resample(bezier(pa, outA, best!.a * chord, pb, outB, best!.b * chord, 200), 3);
    // Rail heights: linear between the stations plus a gentle rolling bump.
    const total = new Path(flat.map((p) => ({ ...p, y: 0 }))).length;
    let run = 0;
    const heighted = flat.map((p, i) => {
      if (i > 0) run += Math.hypot(p.x - flat[i - 1].x, p.z - flat[i - 1].z);
      const t = run / total;
      const base = sa.y + (sb.y - sa.y) * t;
      const bump = (fbm(p.x * 0.004, p.z * 0.004, 3, 9) - 0.5) * 7 * Math.sin(Math.PI * t) ** 2;
      return { x: p.x, z: p.z, y: base + bump };
    });
    // Find the river crossing and flatten a bridge deck over it.
    let bridge: EdgeTrack["bridge"] = null;
    const path0 = new Path(heighted);
    const inRiver = heighted.map((p) => nearestOnPolyline(this.river, p.x, p.z).d < 34);
    const first_i = inRiver.indexOf(true);
    if (first_i >= 0) {
      const last_i = inRiver.lastIndexOf(true);
      const i0 = Math.max(0, first_i - 1), i1 = Math.min(heighted.length - 1, last_i + 1);
      const y0 = heighted[i0].y, y1 = heighted[i1].y;
      for (let i = i0; i <= i1; i++) heighted[i].y = y0 + ((y1 - y0) * (i - i0)) / Math.max(1, i1 - i0);
      bridge = { from: path0.cum[i0], to: path0.cum[i1] };
    }
    return { a, b, endA, endB, path: new Path(heighted), bridge };
  }

  /** Registers an edge's samples in the corridor index. */
  private indexEdge(e: EdgeTrack): void {
    const n = Math.ceil(e.path.length / 5);
    for (let i = 0; i <= n; i++) {
      const s = (e.path.length * i) / n;
      const { p } = e.path.at(s);
      const onBridge = !!e.bridge && s >= e.bridge.from && s <= e.bridge.to;
      this.addSample({ x: p.x, z: p.z, y: p.y, bridge: onBridge });
    }
  }

  /** Stores a flattening sample in the spatial grid. */
  private addSample(s: Sample): void {
    const idx = this.samples.push(s) - 1;
    const key = `${Math.floor(s.x / this.cell)},${Math.floor(s.z / this.cell)}`;
    const bucket = this.grid.get(key);
    if (bucket) bucket.push(idx); else this.grid.set(key, [idx]);
  }

  /** Nearest corridor sample within `radius` (default 5 grid cells), or null. */
  private nearestSample(x: number, z: number, radius: number): { s: Sample; d: number } | null {
    const cx = Math.floor(x / this.cell), cz = Math.floor(z / this.cell);
    const r = Math.ceil(radius / this.cell);
    let best: Sample | null = null, bd = Infinity;
    for (let i = -r; i <= r; i++) for (let j = -r; j <= r; j++) {
      const bucket = this.grid.get(`${cx + i},${cz + j}`);
      if (!bucket) continue;
      for (const idx of bucket) {
        const s = this.samples[idx];
        const d = Math.hypot(s.x - x, s.z - z);
        if (d < bd) { bd = d; best = s; }
      }
    }
    return best && bd <= radius ? { s: best, d: bd } : null;
  }

  /** Distance from (x,z) to the nearest track or yard sample, or `radius` if none is closer. */
  trackDistance(x: number, z: number, radius = 60): number {
    const n = this.nearestSample(x, z, radius);
    return n ? n.d : radius;
  }

  /** Distance from (x,z) to the river centre line. */
  riverDistance(x: number, z: number): number { return nearestOnPolyline(this.river, x, z).d; }

  /** Water-surface height of the river nearest to (x,z). */
  riverLevel(x: number, z: number): number {
    const n = nearestOnPolyline(this.river, x, z);
    return this.river[n.i].y + (this.river[n.i + 1].y - this.river[n.i].y) * n.t;
  }

  /** Raw rolling hills before any corridor is cut through them. */
  hills(x: number, z: number): number {
    const big = fbm(x * 0.0011 + 5, z * 0.0011 - 3, 4, 21);
    const small = fbm(x * 0.007, z * 0.007, 3, 33);
    return 4 + Math.pow(big, 1.5) * 150 + small * 6;
  }

  /** Terrain height at (x,z): hills, flattened around tracks, with a gorge for the river. */
  groundHeight(x: number, z: number): number {
    let h = this.hills(x, z);
    const dr = this.riverDistance(x, z);
    const ry = this.riverLevel(x, z);
    h = h + (ry + 3.6 - h) * smoothstep(170, 16, dr);
    const near = this.nearestSample(x, z, 320);
    if (near) {
      const wide = smoothstep(320, 12, near.d);
      h = h + (near.s.y - 0.5 - h) * wide * wide * 0.9;
      if (!near.s.bridge) h = h + (near.s.y - 0.5 - h) * smoothstep(20, 9, near.d);
    }
    h -= 6 * smoothstep(26, 5, dr);
    return h;
  }

  /** Rail height of the station platform area. */
  stationHeight(id: StationId): number { return this.stations.get(id)!.y; }

  /** Finds the edge connecting two stations and whether it is stored in the given direction. */
  edgeBetween(a: StationId, b: StationId): { edge: EdgeTrack; forward: boolean } | null {
    for (const e of this.edges) {
      if (e.a === a && e.b === b) return { edge: e, forward: true };
      if (e.a === b && e.b === a) return { edge: e, forward: false };
    }
    return null;
  }

  /**
   * Metres beyond the junction at one end of a station after which every edge that leaves there
   * is at least `JUNCTION_SEPARATION` metres from the headshunt and from every other edge: a
   * train further out than this does not foul a train on the headshunt or on a sister edge.
   */
  junctionClearance(id: StationId, end: 1 | -1): number {
    const key = `${id}:${end}`;
    const known = this.clearances.get(key);
    if (known !== undefined) return known;
    const site = this.stations.get(id)!;
    const c = Math.cos(site.yaw), s = Math.sin(site.yaw);
    const mine = this.edges.filter((e) => (e.a === id && e.endA === end) || (e.b === id && e.endB === end));
    const pointOf = (e: EdgeTrack, d: number) => (e.a === id ? e.path.at(d).p : e.path.at(e.path.length - d).p);
    let far = 0;
    for (const e of mine) {
      // Distance from the axis, and from each sister edge, at the same distance from the junction.
      for (let d = 0; d < 400; d += 1) {
        const p = pointOf(e, d);
        const v = Math.abs(-(p.x - site.x) * s + (p.z - site.z) * c);
        let apart = v;
        for (const o of mine) if (o !== e) { const q = pointOf(o, d); apart = Math.min(apart, Math.hypot(p.x - q.x, p.z - q.z)); }
        if (apart < JUNCTION_SEPARATION) far = Math.max(far, d + 1);
      }
    }
    this.clearances.set(key, far);
    return far;
  }

  /** Throat end (+1/-1) of station `id` used by the edge towards `other`. */
  throatToward(id: StationId, other: StationId): 1 | -1 {
    const found = this.edgeBetween(id, other);
    if (!found) return 1;
    return found.forward ? found.edge.endA : found.edge.endB;
  }

  /**
   * The station track `level` of station `id` as a lane running in axis direction
   * `dir` (+1 towards increasing u): a polyline from one neck end to the other that
   * follows the turnouts exactly, with arc-length lookup by axis coordinate.
   */
  lane(id: StationId, level: number, dir: 1 | -1): Lane {
    const key = `${id}:${level}:${dir}`;
    let lane = this.lanes.get(key);
    if (lane) return lane;
    const site = this.stations.get(id)!;
    const n = Math.round((2 * STATION_EXTENT) / 2.5);
    const path = new Path(Array.from({ length: n + 1 }, (_, i) => trackPoint(site, level, dir * (-STATION_EXTENT + (2 * STATION_EXTENT * i) / n))));
    const frac = (u: number) => Math.min(n, Math.max(0, ((dir * u + STATION_EXTENT) * n) / (2 * STATION_EXTENT)));
    lane = {
      path,
      arcAt: (u) => {
        const f = frac(u), i = Math.min(n - 1, Math.floor(f));
        return path.cum[i] + (path.cum[i + 1] - path.cum[i]) * (f - i);
      },
      uAt: (s) => {
        if (s <= 0) return -dir * STATION_EXTENT + dir * s;
        if (s >= path.length) return dir * STATION_EXTENT + dir * (s - path.length);
        let lo = 1, hi = n;
        while (lo < hi) { const mid = (lo + hi) >> 1; if (path.cum[mid] < s) lo = mid + 1; else hi = mid; }
        const t = (s - path.cum[lo - 1]) / (path.cum[lo] - path.cum[lo - 1]);
        return dir * (-STATION_EXTENT + (2 * STATION_EXTENT * (lo - 1 + t)) / n);
      }
    };
    this.lanes.set(key, lane);
    return lane;
  }

  /** Identifier of yard track `level` of station `id` (see `track`); its arc length runs from the headshunt end at `u = -STATION_EXTENT`. */
  static yardTrack(id: StationId, level: number): string { return `yard:${id}:${level}`; }

  /** Identifier of the edge between `a` and `b` as stored (see `track`); its arc length runs from the neck end of `a`. */
  static edgeTrack(a: StationId, b: StationId): string { return `edge:${a}:${b}`; }

  /**
   * The centre line of a named track, the reference for every vehicle address `(track, s)`:
   * a yard track (`Geography.yardTrack`) or an edge (`Geography.edgeTrack`).
   */
  track(trackId: string): Path {
    const [kind, a, b] = trackId.split(":");
    if (kind === "yard") return this.lane(a, Number(b), 1).path;
    const e = this.edges.find((x) => x.a === a && x.b === b);
    if (!e) throw new Error(`unknown track ${trackId}`);
    return e.path;
  }

  /** Builds a route (a chain of pieces of named tracks, each run in either direction) from the given pieces. */
  route(pieces: RoutePiece[]): Route { return new Route((id) => this.track(id), pieces); }

  /**
   * Departure-to-arrival route of one leg for an engine on yard track
   * `engineLevel(index)`, whose nose points along the axis direction `nose`
   * while it stands at `from`. It starts at the far neck end of the departure
   * track, runs along that track to the neck end used for leaving, along the
   * edge and onto the arrival track to its far neck end, so every vehicle of the
   * consist, which follows the engine at fixed arc spacing, stays on rails from the
   * first metre. The engine itself runs from `s0` (its stand at the departure
   * station) to `s1` (its stand at the arrival station).
   *
   * An engine never turns: if `nose` points away from the throat it leaves by,
   * it runs tail first (`sigma` is -1) and its cars, which hang at its tail,
   * lead the train. `noseEnd` is the axis direction its nose then points at the
   * arrival station.
   */
  legPath(from: StationId, to: StationId, index: number, nose: 1 | -1): {
    route: Route; path: Path; s0: number; s1: number; dFrom: 1 | -1; dTo: 1 | -1; sigma: 1 | -1; noseEnd: 1 | -1;
  } | null {
    const found = this.edgeBetween(from, to);
    if (!found) return null;
    const { edge, forward } = found;
    const eA = forward ? edge.endA : edge.endB;
    const eB = forward ? edge.endB : edge.endA;
    const level = engineLevel(index);
    const laneA = this.lane(from, level, 1), laneB = this.lane(to, level, 1);
    const aStart = laneA.arcAt(-eA * STATION_END), aEnd = laneA.arcAt(eA * STATION_END);
    const bStart = laneB.arcAt(eB * STATION_END), bEnd = laneB.arcAt(-eB * STATION_END);
    const route = this.route([
      { track: Geography.yardTrack(from, level), from: aStart, to: aEnd },
      forward ? { track: Geography.edgeTrack(edge.a, edge.b), from: 0, to: edge.path.length } : { track: Geography.edgeTrack(edge.a, edge.b), from: edge.path.length, to: 0 },
      { track: Geography.yardTrack(to, level), from: bStart, to: bEnd }
    ]);
    const sigma = (nose * eA) as 1 | -1;
    const noseEnd = (sigma * -eB) as 1 | -1;
    const lenA = Math.abs(aEnd - aStart), lenB = Math.abs(bEnd - bStart);
    return {
      route, path: route.path, dFrom: eA, dTo: (-eB) as 1 | -1, sigma, noseEnd,
      s0: Math.abs(laneA.arcAt(nose * STAND_CENTRE) - aStart),
      s1: lenA + edge.path.length + Math.abs(laneB.arcAt(noseEnd * STAND_CENTRE) - bStart)
    };
  }
}

/** A stretch of a named track from arc length `from` to arc length `to` (run backwards when `to < from`). */
export interface RoutePiece { track: string; from: number; to: number }

/**
 * A route: pieces of named tracks chained into one polyline, the way a train runs over
 * switches. Every point of it is addressed twice: by its arc length on the route and
 * by `(track, s)`, the track it lies on and the arc length on that track. Pieces are
 * exact slices of the tracks' own polylines, so a vehicle placed on a route is on the
 * centre line of its track by construction. Beyond both ends the route continues
 * straight (callers pad it where vehicles may reach that far).
 */
export class Route {
  readonly path: Path;
  /** The pieces with the route arc length at which each starts. */
  readonly pieces: (RoutePiece & { arc0: number })[] = [];
  constructor(lookup: (track: string) => Path, pieces: RoutePiece[]) {
    const points: P3[] = [];
    let arc = 0;
    for (const piece of pieces) {
      const pts = lookup(piece.track).slice(piece.from, piece.to);
      // Consecutive pieces meet where two tracks coincide to within a couple of millimetres; join them exactly.
      if (points.length) pts[0] = points[points.length - 1];
      points.push(...(points.length ? pts.slice(1) : pts));
      this.pieces.push({ ...piece, arc0: arc });
      arc += Math.abs(piece.to - piece.from);
    }
    this.path = new Path(points);
  }
  /** Length of the route in metres. */
  get length(): number { return this.path.length; }
  /** The track and the arc length on that track of the point at route arc length `arc`. */
  locate(arc: number): { track: string; s: number } {
    let i = this.pieces.length - 1;
    while (i > 0 && arc < this.pieces[i].arc0) i--;
    const piece = this.pieces[i];
    const dir = piece.to >= piece.from ? 1 : -1;
    return { track: piece.track, s: piece.from + dir * (arc - piece.arc0) };
  }
}
