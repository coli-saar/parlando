/**
 * Track: ballast, sleepers and rails along every edge spline and through every
 * station yard (running line, engine tracks and sidings joined by turnouts), the river bridge, signals at the station throats and the
 * telegraph pole line.
 *
 * Owns the static railway geometry (merged ribbons plus thin-instanced sleepers
 * and poles) and the signal heads, whose red/green state the replay toggles when
 * an engine is held or a line is closed. Stock sits on rails at the stored path
 * height plus `RAIL_TOP`. Callers get the signal controller and the bridge
 * meshes (for shadows).
 */
import { Color3, Matrix, Mesh, MeshBuilder, Quaternion, Scene, StandardMaterial, TransformNode, Vector3 } from "@babylonjs/core";
import { GAUGE, MAX_LEVEL, Path, STATION_EXTENT, STATION_HALF, hash2, mergeEnd, stationPoint, trackPoints, type Geography, type P3, type StationSite } from "./geography";
import { PartBuilder } from "./parts";

/** Height of the rail head above the stored track height. */
export const RAIL_TOP = 0.2;

/** Controls the signal heads of one scene. */
export interface Signals {
  /** Sets the signal at a station throat end to red (true) or green. */
  set(station: string, end: 1 | -1, red: boolean): void;
  /** Sets every signal on both ends of an edge's throats (line closure). */
  setEdge(a: string, b: string, red: boolean): void;
  clear(): void;
}

/** Everything the world needs from the track builder. */
export interface Track { signals: Signals; casters: Mesh[] }

/** Builds the tracks, bridge, signals and poles. */
export function createTrack(scene: Scene, geo: Geography): Track {
  const parts = new PartBuilder(scene);
  const ballastMat = new StandardMaterial("ballast", scene);
  ballastMat.diffuseColor = new Color3(0.38, 0.36, 0.33); ballastMat.specularColor = Color3.Black();
  const railMat = new StandardMaterial("rail", scene);
  railMat.diffuseColor = new Color3(0.32, 0.3, 0.29); railMat.specularColor = new Color3(0.5, 0.45, 0.4);
  const tieMat = new StandardMaterial("ties", scene);
  tieMat.diffuseColor = new Color3(0.2, 0.15, 0.1); tieMat.specularColor = Color3.Black();

  const ballast: Mesh[] = [], rails: Mesh[] = [];
  const ties: number[][] = [];

  /** Lays ballast, two rails and sleepers along a polyline. */
  const lay = (pts: P3[], withBallast = true) => {
    const path = new Path(pts);
    if (path.length < 2) return;
    const n = Math.max(2, Math.ceil(path.length / 2.5));
    const samples: { p: P3; nx: number; nz: number; yaw: number }[] = [];
    for (let i = 0; i <= n; i++) {
      const s = (path.length * i) / n;
      const { p, yaw } = path.at(s);
      samples.push({ p, yaw, nx: Math.cos(yaw), nz: -Math.sin(yaw) });
    }
    const row = (off: number, dy: number) => samples.map((s) => new Vector3(s.p.x + s.nx * off, s.p.y + dy, s.p.z + s.nz * off));
    if (withBallast) ballast.push(MeshBuilder.CreateRibbon("ballast", { pathArray: [row(-3.3, -0.35), row(-1.9, 0.03), row(1.9, 0.03), row(3.3, -0.35)] }, scene));
    for (const side of [-1, 1]) {
      const c = side * GAUGE / 2;
      rails.push(MeshBuilder.CreateRibbon("rail", {
        pathArray: [row(c - 0.07, 0.1), row(c - 0.035, 0.2), row(c + 0.035, 0.2), row(c + 0.07, 0.1)]
      }, scene));
    }
    for (let s = 0; s <= path.length; s += 0.72) {
      const { p, yaw } = path.at(s);
      ties.push(Matrix.Compose(new Vector3(1, 1, 1), Quaternion.FromEulerAngles(0, yaw, 0), new Vector3(p.x, p.y + 0.06, p.z)).toArray() as number[]);
    }
  };

  // Edges.
  for (const e of geo.edges) lay(e.path.points);
  // Station yards: the running line (which runs on beyond both throats as the neck and the headshunt; the
  // edges leave it at the junctions), the engine tracks beside it and the sidings. Every track runs parallel in the
  // middle of the station and merges into its inner neighbour through a turnout of radius 160 m
  // at both throats, exactly where `trackLateral` puts it, so rolling stock that follows
  // `trackPoint` stays on the drawn rails. A track is laid only as far as it differs from its
  // neighbour, so merged rails are not drawn twice.
  for (const site of geo.stations.values()) {
    for (let level = -MAX_LEVEL; level <= MAX_LEVEL; level++) {
      const reach = level === 0 ? STATION_EXTENT : mergeEnd(Math.abs(level));
      lay(trackPoints(site, level, -reach, reach));
    }
  }
  const ballastMesh = ballast.length ? Mesh.MergeMeshes(ballast, true, true)! : null;
  const railMesh = rails.length ? Mesh.MergeMeshes(rails, true, true)! : null;
  if (ballastMesh) { ballastMesh.material = ballastMat; ballastMesh.receiveShadows = true; ballastMesh.freezeWorldMatrix(); }
  if (railMesh) { railMesh.material = railMat; railMesh.freezeWorldMatrix(); }
  const tie = MeshBuilder.CreateBox("tie", { width: 2.6, height: 0.16, depth: 0.26 }, scene);
  tie.material = tieMat;
  const buf = new Float32Array(ties.length * 16);
  ties.forEach((m, i) => buf.set(m, i * 16));
  tie.thinInstanceSetBuffer("matrix", buf, 16, true);
  tie.alwaysSelectAsActiveMesh = true;
  tie.thinInstanceRefreshBoundingInfo(true);
  tie.receiveShadows = true;

  const casters: Mesh[] = [];
  casters.push(...buildBridges(scene, geo, parts));
  const signals = buildSignals(scene, geo, parts);
  buildPoles(scene, geo, parts);
  return { signals, casters };
}

/** Girder bridges over the river: truss sides, piers and abutments along the edge's own spline. */
function buildBridges(scene: Scene, geo: Geography, parts: PartBuilder): Mesh[] {
  const steel = new StandardMaterial("steel", scene);
  steel.specularColor = new Color3(0.25, 0.22, 0.2);
  const concrete = new StandardMaterial("pier", scene);
  concrete.specularColor = Color3.Black();
  const out: Mesh[] = [];
  const steelParts: Mesh[] = [], stoneParts: Mesh[] = [];
  for (const e of geo.edges) {
    if (!e.bridge) continue;
    const { from, to } = e.bridge;
    const step = 6;
    const count = Math.max(2, Math.round((to - from) / step));
    for (let i = 0; i < count; i++) {
      const s0 = from + ((to - from) * i) / count, s1 = from + ((to - from) * (i + 1)) / count;
      const a = e.path.at(s0), b = e.path.at(s1);
      const len = Math.hypot(b.p.x - a.p.x, b.p.z - a.p.z);
      const cx = (a.p.x + b.p.x) / 2, cz = (a.p.z + b.p.z) / 2, cy = (a.p.y + b.p.y) / 2;
      const yaw = Math.atan2(b.p.x - a.p.x, b.p.z - a.p.z);
      const place = (m: Mesh, lx: number, ly: number, lz = 0, roll = 0) => {
        // Move to local (lx,ly,lz) in the segment frame.
        const c = Math.cos(yaw), s = Math.sin(yaw);
        m.position.set(cx + lx * c + lz * s, cy + ly, cz - lx * s + lz * c);
        m.rotation.y = yaw; m.rotation.z = roll;
        return m;
      };
      // Plate girders in dark brown with a deck slab, as in the approved sketch.
      steelParts.push(place(parts.box(5.2, 0.2, len + 0.05, 0, 0, 0, "#3a2a24"), 0, -0.15));
      for (const side of [-1, 1]) steelParts.push(place(parts.box(0.3, 1.9, len + 0.05, 0, 0, 0, "#3a2a24"), side * 2.7, 0.6));
    }
    // Piers on the banks and abutments at the ends.
    const ground = (s: number) => { const p = e.path.at(s).p; return geo.groundHeight(p.x, p.z); };
    for (const s of [from, to]) {
      const p = e.path.at(s).p, yaw = e.path.at(s).yaw;
      const top = p.y - 0.8, bottom = ground(s) - 1;
      const m = parts.box(5.2, Math.max(1, top - bottom), 3.5, p.x, (top + bottom) / 2, p.z, "#9a958a");
      m.rotation.y = yaw; stoneParts.push(m);
    }
    for (const f of [0.33, 0.67]) {
      const s = from + (to - from) * f;
      const p = e.path.at(s).p, yaw = e.path.at(s).yaw;
      const top = p.y - 0.8, bottom = geo.groundHeight(p.x, p.z) - 3;
      const m = parts.box(4.2, Math.max(1, top - bottom), 2.2, p.x, (top + bottom) / 2, p.z, "#9a958a");
      m.rotation.y = yaw; stoneParts.push(m);
    }
  }
  if (steelParts.length) { const m = parts.merge("bridge-steel", steelParts, steel); out.push(m); }
  if (stoneParts.length) { const m = parts.merge("bridge-piers", stoneParts, concrete); out.push(m); }
  return out;
}

/** Signal posts at every used throat, with red/green heads. */
function buildSignals(scene: Scene, geo: Geography, parts: PartBuilder): Signals {
  const mat = (name: string, c: Color3, glow: number) => {
    const m = new StandardMaterial(name, scene);
    m.diffuseColor = c.scale(0.4); m.emissiveColor = c.scale(glow); m.specularColor = Color3.Black();
    return m;
  };
  const redOn = mat("red-on", new Color3(1, 0.1, 0.05), 1), redOff = mat("red-off", new Color3(0.4, 0.05, 0.04), 0.12);
  const greenOn = mat("green-on", new Color3(0.1, 1, 0.3), 1), greenOff = mat("green-off", new Color3(0.05, 0.3, 0.1), 0.1);
  const postMat = new StandardMaterial("signal-post", scene);
  postMat.specularColor = Color3.Black();
  const heads = new Map<string, { red: Mesh; green: Mesh }>();
  const used = new Set<string>();
  for (const e of geo.edges) { used.add(`${e.a}:${e.endA}`); used.add(`${e.b}:${e.endB}`); }
  const posts: Mesh[] = [];
  for (const key of used) {
    const [id, endStr] = key.split(":");
    const site = geo.stations.get(id) as StationSite;
    const end = Number(endStr) as 1 | -1;
    const base = stationPoint(site, end * (STATION_HALF + 12), 5, 0);
    // A short mast with two lamp heads facing the approaching train.
    const mast = new TransformNode(`signal-${key}`, scene);
    mast.position.set(base.x, base.y, base.z);
    mast.rotation.y = -site.yaw + (end === 1 ? Math.PI / 2 : -Math.PI / 2);
    const pole = parts.cyl(0.22, 5.2, 0, 2.6, 0, "y", "#2a2d31", 8);
    const head = parts.box(0.7, 1.7, 0.4, 0, 5.2, 0, "#1a1c1f");
    posts.push(pole, head);
    const merged = parts.merge(`signal-post-${key}`, posts.splice(0), postMat);
    merged.parent = mast;
    const red = MeshBuilder.CreateSphere(`sig-red-${key}`, { diameter: 0.42, segments: 6 }, scene);
    const green = MeshBuilder.CreateSphere(`sig-green-${key}`, { diameter: 0.42, segments: 6 }, scene);
    red.parent = mast; green.parent = mast;
    red.position.set(0, 5.6, -0.22); green.position.set(0, 4.8, -0.22);
    red.material = redOff; green.material = greenOn;
    heads.set(key, { red, green });
  }
  const apply = (key: string, red: boolean) => {
    const h = heads.get(key);
    if (!h) return;
    h.red.material = red ? redOn : redOff;
    h.green.material = red ? greenOff : greenOn;
  };
  return {
    set: (station, end, red) => apply(`${station}:${end}`, red),
    setEdge: (a, b, red) => {
      const found = geo.edgeBetween(a, b);
      if (!found) return;
      apply(`${found.edge.a}:${found.edge.endA}`, red);
      apply(`${found.edge.b}:${found.edge.endB}`, red);
    },
    clear: () => { for (const k of heads.keys()) apply(k, false); }
  };
}

/** Telegraph poles beside every edge, instanced. */
function buildPoles(scene: Scene, geo: Geography, parts: PartBuilder): void {
  const mat = new StandardMaterial("pole", scene);
  mat.specularColor = Color3.Black();
  const pole = parts.merge("pole", [
    parts.cyl(0.28, 8.5, 0, 4.25, 0, "y", "#4b3a2b", 6), parts.box(2.2, 0.14, 0.14, 0, 8.0, 0, "#4b3a2b"), parts.box(1.6, 0.12, 0.12, 0, 7.3, 0, "#4b3a2b")
  ], mat);
  const m: number[][] = [];
  for (const e of geo.edges) {
    for (let s = 90; s < e.path.length - 90; s += 55) {
      if (e.bridge && s > e.bridge.from - 15 && s < e.bridge.to + 15) continue;
      const { p, yaw } = e.path.at(s);
      const off = 7 + hash2(s | 0, 1) * 0.6;
      m.push(Matrix.Compose(new Vector3(1, 1, 1), Quaternion.FromEulerAngles(0, yaw, 0), new Vector3(p.x + Math.cos(yaw) * off, p.y - 0.4, p.z - Math.sin(yaw) * off)).toArray() as number[]);
    }
  }
  const buf = new Float32Array(m.length * 16);
  m.forEach((a, i) => buf.set(a, i * 16));
  pole.thinInstanceSetBuffer("matrix", buf, 16, true);
  pole.alwaysSelectAsActiveMesh = true;
  pole.thinInstanceRefreshBoundingInfo(true);
}
