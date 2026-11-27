/**
 * Landscape: the continuous autumn terrain, woodland, river and telegraph poles.
 *
 * Owns the terrain mesh (one vertex-coloured grid following `Geography.groundHeight`),
 * the tree population (thin instances of a trunk, a broadleaf crown and a conifer,
 * tinted per instance in autumn colours), the river ribbon and the pole line along
 * the tracks. Everything is generated from the deterministic geography, so both
 * players see the same country. Callers receive the meshes that should receive or
 * cast shadows.
 */
import {
  Color3, Matrix, Mesh, MeshBuilder, Quaternion, Scene, StandardMaterial, Vector3, VertexBuffer, VertexData
} from "@babylonjs/core";
import { fbm, hash2, smoothstep, type Geography } from "./geography";
import { PartBuilder } from "./parts";

/** Handles to the landscape pieces so the world can configure shadows and fog. */
export interface Landscape {
  terrain: Mesh;
  water: Mesh;
  forest: Mesh[];
  waterMaterial: StandardMaterial;
}

const BOUNDS = { x0: -2400, x1: 2400, z0: -3200, z1: 1400 };
const STEP = 16;

/** Autumn ground colour from height, slope and noise (meadow, stubble, leaf litter, rock). */
function groundColor(geo: Geography, x: number, z: number, h: number, slope: number, nearTrack: number): [number, number, number] {
  const patch = fbm(x * 0.004, z * 0.004, 3, 71);
  const fine = fbm(x * 0.05, z * 0.05, 2, 5) * 0.12;
  const meadow: [number, number, number] = [0.4 + patch * 0.12, 0.52 + patch * 0.1, 0.27];
  const stubble: [number, number, number] = [0.6, 0.5, 0.25];
  const litter: [number, number, number] = [0.44, 0.36, 0.2];
  const rock: [number, number, number] = [0.42, 0.4, 0.37];
  const wS = smoothstep(0.46, 0.6, patch), forest = fbm(x * 0.0022 + 9, z * 0.0022, 3, 12), wF = smoothstep(0.42, 0.62, forest);
  let c: [number, number, number] = [meadow[0] + (stubble[0] - meadow[0]) * wS, meadow[1] + (stubble[1] - meadow[1]) * wS, meadow[2] + (stubble[2] - meadow[2]) * wS];
  c = [c[0] + (litter[0] - c[0]) * wF, c[1] + (litter[1] - c[1]) * wF, c[2] + (litter[2] - c[2]) * wF];
  const t = smoothstep(0.35, 0.8, slope);
  c = [c[0] + (rock[0] - c[0]) * t, c[1] + (rock[1] - c[1]) * t, c[2] + (rock[2] - c[2]) * t];
  const dr = geo.riverDistance(x, z);
  if (dr < 22) { const w = smoothstep(22, 12, dr); c = [c[0] * (1 - w) + 0.36 * w, c[1] * (1 - w) + 0.33 * w, c[2] * (1 - w) + 0.25 * w]; }
  if (nearTrack < 12) { const w = smoothstep(12, 3, nearTrack) * 0.35; c = [c[0] * (1 - w) + 0.47 * w, c[1] * (1 - w) + 0.45 * w, c[2] * (1 - w) + 0.4 * w]; }
  void h;
  return [c[0] + fine, c[1] + fine, c[2] + fine * 0.5];
}

/** Builds the terrain grid mesh. */
function buildTerrain(scene: Scene, geo: Geography): Mesh {
  const nx = Math.round((BOUNDS.x1 - BOUNDS.x0) / STEP) + 1;
  const nz = Math.round((BOUNDS.z1 - BOUNDS.z0) / STEP) + 1;
  const positions = new Float32Array(nx * nz * 3);
  const heights = new Float32Array(nx * nz);
  for (let j = 0; j < nz; j++) for (let i = 0; i < nx; i++) {
    const x = BOUNDS.x0 + i * STEP, z = BOUNDS.z0 + j * STEP;
    const h = geo.groundHeight(x, z);
    const k = j * nx + i;
    heights[k] = h;
    positions[k * 3] = x; positions[k * 3 + 1] = h; positions[k * 3 + 2] = z;
  }
  const indices: number[] = [];
  for (let j = 0; j < nz - 1; j++) for (let i = 0; i < nx - 1; i++) {
    const a = j * nx + i, b = a + 1, c = a + nx, d = c + 1;
    indices.push(a, b, c, b, d, c);
  }
  const normals = new Float32Array(nx * nz * 3);
  VertexData.ComputeNormals(positions, indices, normals);
  const colors = new Float32Array(nx * nz * 4);
  for (let j = 0; j < nz; j++) for (let i = 0; i < nx; i++) {
    const k = j * nx + i;
    const x = positions[k * 3], z = positions[k * 3 + 2];
    const slope = 1 - normals[k * 3 + 1];
    const [r, g, b] = groundColor(geo, x, z, heights[k], slope * 3, geo.trackDistance(x, z, 40));
    colors.set([r, g, b, 1], k * 4);
  }
  const mesh = new Mesh("terrain", scene);
  const vd = new VertexData();
  vd.positions = positions; vd.indices = indices; vd.normals = normals; vd.colors = colors;
  vd.applyToMesh(mesh);
  const mat = new StandardMaterial("terrain-mat", scene);
  mat.specularColor = Color3.Black();
  mesh.material = mat;
  mesh.receiveShadows = true;
  mesh.freezeWorldMatrix();
  return mesh;
}

/** Plants trees with thin instances, avoiding tracks, stations and the water. */
function buildForest(scene: Scene, geo: Geography): Mesh[] {
  const parts = new PartBuilder(scene);
  const white = new StandardMaterial("tree-mat", scene);
  white.specularColor = Color3.Black();
  const trunk = parts.merge("trunk", [parts.cyl(0.6, 4.5, 0, 2.25, 0, "y", "#4a3526", 5, 0.4)], white, true);
  const crown = parts.merge("crown", [
    parts.ball(8, 0, 7.2, 0, "#ffffff", 0.85), parts.ball(5.5, 1.8, 5.6, 0.6, "#ffffff", 0.8), parts.ball(5.2, -1.6, 5.9, -0.9, "#ffffff", 0.8)
  ], white, true);
  const conifer = parts.merge("conifer", [
    parts.cyl(6.4, 5, 0, 4.5, 0, "y", "#ffffff", 7, 0.6), parts.cyl(4.8, 4, 0, 7.8, 0, "y", "#ffffff", 7, 0.4),
    parts.cyl(0.5, 3, 0, 1.5, 0, "y", "#ffffff", 5)
  ], white, true);
  const hexc = (h: number) => [((h >> 16) & 255) / 255, ((h >> 8) & 255) / 255, (h & 255) / 255];
  // Leaf palette of the approved sketches.
  const autumn = [0xc8641e, 0xd9a521, 0x9c3a1c, 0x5f7d2e, 0xe0b53a, 0x6c8a36, 0xb9501c].map(hexc);
  const greens = [[0.12, 0.3, 0.17], [0.15, 0.34, 0.2], [0.1, 0.26, 0.15]];
  type Inst = { m: number[]; c: number[] };
  const lists: Record<"trunk" | "crown" | "conifer", Inst[]> = { trunk: [], crown: [], conifer: [] };
  const cell = 24;
  for (let z = BOUNDS.z0 + 100; z < BOUNDS.z1 - 100; z += cell) for (let x = BOUNDS.x0 + 100; x < BOUNDS.x1 - 100; x += cell) {
    const jx = x + (hash2(x, z, 1) - 0.5) * cell, jz = z + (hash2(x, z, 2) - 0.5) * cell;
    const density = fbm(jx * 0.0022 + 9, jz * 0.0022, 3, 12);
    if (hash2(x, z, 3) > smoothstep(0.38, 0.62, density) * 0.95 + 0.04) continue;
    if (geo.trackDistance(jx, jz, 26) < 26 || geo.riverDistance(jx, jz) < 24) continue;
    const y = geo.groundHeight(jx, jz) - 0.2;
    const s = 0.8 + hash2(x, z, 4) * 0.8;
    const rot = hash2(x, z, 5) * 6.28;
    const matrix = Matrix.Compose(new Vector3(s, s * (0.9 + hash2(x, z, 6) * 0.3), s), Quaternion.FromEulerAngles(0, rot, 0), new Vector3(jx, y, jz)).toArray() as number[];
    const conif = hash2(x, z, 7) < 0.28 + smoothstep(60, 110, y) * 0.3;
    const col = conif ? greens[Math.floor(hash2(x, z, 8) * greens.length)] : autumn[Math.floor(hash2(x, z, 9) * autumn.length)];
    const shade = 0.85 + hash2(x, z, 10) * 0.3;
    lists[conif ? "conifer" : "crown"].push({ m: matrix, c: [col[0] * shade, col[1] * shade, col[2] * shade, 1] });
    if (!conif) lists.trunk.push({ m: matrix, c: [0.35, 0.26, 0.18, 1] });
  }
  const apply = (mesh: Mesh, list: Inst[]) => {
    const m = new Float32Array(list.length * 16), c = new Float32Array(list.length * 4);
    list.forEach((e, i) => { m.set(e.m, i * 16); c.set(e.c, i * 4); });
    mesh.thinInstanceSetBuffer("matrix", m, 16, true);
    mesh.thinInstanceSetBuffer("color", c, 4, true);
    mesh.alwaysSelectAsActiveMesh = true;
    mesh.thinInstanceRefreshBoundingInfo(true);
    mesh.isVisible = list.length > 0;
  };
  trunk.setVerticesData(VertexBuffer.ColorKind, trunk.getVerticesData(VertexBuffer.ColorKind)!.map(() => 1));
  apply(trunk, lists.trunk.concat(lists.conifer.map((e) => ({ m: e.m, c: [0.3, 0.22, 0.15, 1] }))));
  apply(crown, lists.crown);
  apply(conifer, lists.conifer);
  return [trunk, crown, conifer];
}

/** The river ribbon at the water level, a little wider than the carved bed. */
function buildRiver(scene: Scene, geo: Geography): { mesh: Mesh; material: StandardMaterial } {
  const left: Vector3[] = [], right: Vector3[] = [];
  const pts = geo.river;
  const dense: { x: number; z: number; y: number }[] = [];
  for (let i = 0; i < pts.length - 1; i++) {
    for (let t = 0; t < 1; t += 0.1) {
      const p0 = pts[Math.max(0, i - 1)], p1 = pts[i], p2 = pts[i + 1], p3 = pts[Math.min(pts.length - 1, i + 2)];
      const cr = (a: number, b: number, c: number, d: number) =>
        0.5 * (2 * b + (-a + c) * t + (2 * a - 5 * b + 4 * c - d) * t * t + (-a + 3 * b - 3 * c + d) * t * t * t);
      dense.push({ x: cr(p0.x, p1.x, p2.x, p3.x), z: cr(p0.z, p1.z, p2.z, p3.z), y: p1.y + (p2.y - p1.y) * t });
    }
  }
  dense.push(pts[pts.length - 1]);
  dense.forEach((p, i) => {
    const a = dense[Math.max(0, i - 1)], b = dense[Math.min(dense.length - 1, i + 1)];
    const dx = b.x - a.x, dz = b.z - a.z, len = Math.hypot(dx, dz) || 1;
    const nx = -dz / len, nz = dx / len, w = geo.riverHalfWidth + 1.5;
    left.push(new Vector3(p.x + nx * w, p.y, p.z + nz * w));
    right.push(new Vector3(p.x - nx * w, p.y, p.z - nz * w));
  });
  const mesh = MeshBuilder.CreateRibbon("river", { pathArray: [left, right] }, scene);
  const mat = new StandardMaterial("water", scene);
  mat.diffuseColor = new Color3(0.14, 0.26, 0.3);
  mat.specularColor = new Color3(0.8, 0.85, 0.9);
  mat.specularPower = 90;
  mat.alpha = 0.88;
  mat.backFaceCulling = false;
  mesh.material = mat;
  return { mesh, material: mat };
}

/** Builds the whole landscape for one geography. */
export function createLandscape(scene: Scene, geo: Geography): Landscape {
  const terrain = buildTerrain(scene, geo);
  const { mesh: water, material } = buildRiver(scene, geo);
  const forest = buildForest(scene, geo);
  return { terrain, water, forest, waterMaterial: material };
}
