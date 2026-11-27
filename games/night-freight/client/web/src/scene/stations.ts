/**
 * Station set pieces: a recognizable building group at each of the five stations.
 *
 * Owns the static architecture around the yards, built from merged primitives in
 * each station's local frame (x along the tracks, z across, origin at the station
 * centre): the orange dock and brick depot at Corning, the banana warehouse at
 * Avon, the juice plant with storage tanks and a stack at Elmira, a signal cabin
 * and car-storage yard at Bath, a water tower and engine shed at Dansville.
 * Every station also gets a depot sign with its name and lamp posts whose heads
 * and windows glow at night. Callers use `setGlow` for the night lighting and
 * `depotPoint` to place the telegram of a late-order disruption.
 */
import { Color3, Color4, DynamicTexture, ParticleSystem, Matrix, Mesh, MeshBuilder, Quaternion, Scene, StandardMaterial, TransformNode, Vector3 } from "@babylonjs/core";
import { hash2, type Geography, type P3, type StationSite } from "./geography";
import { PartBuilder } from "./parts";

/** Handles to station-owned visuals. */
export interface Stations {
  /** Sets window and lamp glow 0..1. */
  setGlow(level: number): void;
  /** Makes the factory (if any) steam and glow while an op is processing cars. */
  setFactoryWorking(on: boolean): void;
  /** Point just above the depot of a station (for the telegram), in world coordinates. */
  depotPoint(id: string): P3;
  casters: Mesh[];
}

const BRICK = "#8a4a38", WOOD = "#7a5a3a", WHITE = "#e6e0d0", ROOF = "#4a4f55", GREY = "#8c8f92", DARK = "#2a2d31";

/** Creates the sign board texture for a station. */
function signMaterial(scene: Scene, text: string, bg: string, fg: string): StandardMaterial {
  const tex = new DynamicTexture(`sign-${text}`, { width: 512, height: 128 }, scene, true);
  const g = tex.getContext() as CanvasRenderingContext2D;
  g.fillStyle = bg; g.fillRect(0, 0, 512, 128);
  g.strokeStyle = fg; g.lineWidth = 6; g.strokeRect(6, 6, 500, 116);
  g.fillStyle = fg; g.font = "bold 70px Georgia, serif"; g.textAlign = "center"; g.textBaseline = "middle";
  g.fillText(text, 256, 68);
  tex.update();
  const m = new StandardMaterial(`sign-mat-${text}`, scene);
  m.diffuseTexture = tex; m.emissiveColor = new Color3(0.18, 0.18, 0.18); m.specularColor = Color3.Black();
  return m;
}

/** Builds all station set pieces. */
export function createStations(scene: Scene, geo: Geography): Stations {
  const parts = new PartBuilder(scene);
  const body = new StandardMaterial("station-body", scene);
  body.specularColor = Color3.Black();
  const glow = new StandardMaterial("station-glow", scene);
  glow.diffuseColor = new Color3(0.2, 0.18, 0.1); glow.emissiveColor = Color3.Black(); glow.specularColor = Color3.Black();
  const casters: Mesh[] = [];
  const depots = new Map<string, P3>();
  const oranges: Matrix[] = [], bananas: Matrix[] = [];

  for (const site of geo.stations.values()) {
    const node = new TransformNode(`station-${site.id}`, scene);
    node.position.set(site.x, site.y - 0.5, site.z);
    node.rotation.y = -site.yaw;
    const solid: Mesh[] = [], lights: Mesh[] = [];
    const b = (w: number, h: number, d: number, x: number, y: number, z: number, c: string) => { const m = parts.box(w, h, d, x, y + h / 2, z, c); solid.push(m); return m; };
    const win = (w: number, h: number, d: number, x: number, y: number, z: number) => lights.push(parts.box(w, h, d, x, y, z, "#ffd9a0"));

    // Depot: platform, brick building, hip-like gable roof, canopy, windows, chimney.
    const side = site.id === "avon" || site.id === "elmira" ? -1 : -1;
    const dz = side * 33;
    b(36, 1.0, 6, 0, 0, dz - 1.5, "#a9a397"); b(36, 0.12, 0.3, 0, 1.0, dz + 1.3, "#d6c25a");
    b(14, 4.2, 7, -10, 1.0, dz - 3.5 * -side, "#d8c9a3");
    solid.push(parts.gable(8.4, 15.4, 2.6, -10, 5.2, dz - 3.5 * -side, ROOF));
    b(1.0, 2.6, 1.0, -14, 5.2, dz - 3.5 * -side, "#6b5a50");
    b(16, 0.25, 3.2, -10, 3.7, dz + 2.2 * -side * -1, "#3d3a37");
    for (const wx of [-14, -11, -8, -5]) win(1.3, 1.5, 0.15, wx, 3.0, dz - 3.5 * -side + 3.55 * (-side * -1) * -1);
    b(1.2, 2.2, 0.2, -10, 1.0, dz + 0.1, "#3a2a20");
    // Name board on the depot.
    const sign = MeshBuilder.CreatePlane(`sign-${site.id}`, { width: 7, height: 1.75 }, scene);
    sign.material = signMaterial(scene, site.name.toUpperCase(), "#1d3a2f", "#f0e8c8");
    sign.parent = node; sign.position.set(-10, 5.3, dz + 0.55 * -side * -1 * -1 + 0.1); sign.rotation.y = 0;
    sign.position.z = dz - 3.5 * -side + (side === -1 ? 3.6 : -3.6);
    sign.rotation.y = side === -1 ? Math.PI : 0;
    depots.set(site.id, { x: site.x, y: site.y + 8, z: site.z });

    // Lamp posts along the platform.
    for (let u = -26; u <= 10; u += 12) {
      solid.push(parts.cyl(0.18, 6, u, 3 - 0.5 + 0.5, dz + 3, "y", DARK, 6));
      lights.push(parts.box(0.9, 0.3, 0.5, u, 6.1, dz + 3, "#fff2c0"));
    }

    if (site.id === "corning") buildOrangeDock(parts, b, win, solid, lights, oranges, site);
    else if (site.id === "avon") buildBananaWarehouse(scene, parts, b, win, solid, lights, bananas, node);
    else if (site.id === "elmira") buildJuicePlant(parts, b, win, solid, lights);
    else if (site.id === "bath") buildSignalCabin(parts, b, win, solid, lights);
    else buildWaterTower(parts, b, win, solid, lights);

    const merged = parts.merge(`station-${site.id}-solid`, solid, body);
    merged.parent = node; merged.receiveShadows = true;
    casters.push(merged);
    const lit = parts.merge(`station-${site.id}-lights`, lights, glow);
    lit.parent = node;
    sign.parent = node;
    node.computeWorldMatrix(true);
    // Instanced fruit in the docks (in node space): created after the node exists.
  }
  // Fruit piles: shared sphere instances in world coordinates.
  const fruit = (list: Matrix[], color: Color3, name: string) => {
    if (!list.length) return;
    const s = MeshBuilder.CreateBox(name, { size: 0.9 }, scene);
    const m = new StandardMaterial(`${name}-mat`, scene);
    m.diffuseColor = color; m.specularColor = Color3.Black();
    s.material = m;
    const buf = new Float32Array(list.length * 16);
    list.forEach((mat, i) => mat.copyToArray(buf, i * 16));
    s.thinInstanceSetBuffer("matrix", buf, 16, true);
    s.alwaysSelectAsActiveMesh = true; s.thinInstanceRefreshBoundingInfo(true);
  };
  fruit(oranges, new Color3(0.878, 0.541, 0.118), "oranges");
  fruit(bananas, new Color3(0.96, 0.82, 0.12), "bananas");

  // Factory steam: a plume from the juice plant's stack that only runs while cars are processed.
  const puff = new DynamicTexture("factory-puff", { width: 64, height: 64 }, scene, true);
  const pg = puff.getContext();
  const grad = pg.createRadialGradient(32, 32, 2, 32, 32, 30);
  grad.addColorStop(0, "rgba(255,255,255,0.95)"); grad.addColorStop(1, "rgba(255,255,255,0)");
  pg.fillStyle = grad; pg.fillRect(0, 0, 64, 64); puff.update(); puff.hasAlpha = true;
  const steams: ParticleSystem[] = [];
  for (const site of geo.stations.values()) {
    if (!site.factory) continue;
    const ps = new ParticleSystem(`steam-${site.id}`, 220, scene);
    ps.particleTexture = puff;
    ps.emitter = rotateInto(site, 24, 34.5, 36);
    ps.minEmitBox = new Vector3(-1, 0, -1); ps.maxEmitBox = new Vector3(1, 0, 1);
    ps.direction1 = new Vector3(-0.6, 3, -0.4); ps.direction2 = new Vector3(0.9, 4, 0.4);
    ps.minLifeTime = 2.5; ps.maxLifeTime = 4.5; ps.minSize = 3; ps.maxSize = 8; ps.minEmitPower = 1; ps.maxEmitPower = 2.2;
    ps.color1 = new Color4(0.95, 0.95, 0.95, 0.75); ps.color2 = new Color4(0.8, 0.82, 0.86, 0.6); ps.colorDead = new Color4(0.8, 0.8, 0.85, 0);
    ps.gravity = new Vector3(1.2, 0.3, 0); ps.emitRate = 0; ps.start();
    steams.push(ps);
  }

  return {
    casters,
    setFactoryWorking: (on) => { for (const ps of steams) ps.emitRate = on ? 90 : 0; },
    depotPoint: (id) => depots.get(id) ?? { x: 0, y: 10, z: 0 },
    setGlow: (level) => { glow.emissiveColor = new Color3(1, 0.82, 0.45).scale(level); }
  };
}

type BoxFn = (w: number, h: number, d: number, x: number, y: number, z: number, c: string) => Mesh;
type WinFn = (w: number, h: number, d: number, x: number, y: number, z: number) => void;

/** Orange dock: long loading dock, packing shed and crates of fruit. */
function buildOrangeDock(parts: PartBuilder, b: BoxFn, win: WinFn, solid: Mesh[], lights: Mesh[], fruit: Matrix[], site: StationSite): void {
  void lights; void win;
  b(46, 1.5, 7, 12, 0, 32, "#a9a397"); b(46, 0.12, 0.3, 12, 1.5, 28.7, "#d6c25a");
  b(30, 6, 9, 20, 1.5, 41, "#c5662b");
  solid.push(parts.gable(11, 31, 3, 20, 7.5, 41, "#4a3a30"));
  b(34, 0.3, 4.5, 12, 4.4, 30.2, "#5a4636");
  for (const u of [-3, 9, 21, 33]) b(0.4, 4.4, 0.4, u, 1.5, 28.2, "#5a4636");
  for (let i = 0; i < 18; i++) {
    const u = -8 + (i % 9) * 3.8, row = Math.floor(i / 9);
    b(2.4, 1.0, 1.6, u, 1.5, 31 + row * 2.2, WOOD);
    for (let k = 0; k < 4; k++) {
      const x = u + (k % 2 ? 0.6 : -0.6), z = 31 + row * 2.2 + (k < 2 ? 0 : 0.0);
      const p = Matrix.Compose(new Vector3(1, 1, 1), Quaternion.FromEulerAngles(0, -site.yaw, 0), new Vector3(0, 0, 0));
      p.setTranslation(rotateInto(site, x, 2.55 + (k < 2 ? 0 : 0.9), z));
      fruit.push(p);
    }
  }
  b(7, 0.15, 2.8, 14, 6.6, 36, "#c5662b");
  solid.push(parts.cyl(0.4, 8, 36, 5.5, 33, "y", DARK, 6));
  b(7, 3, 0.5, 36, 9, 33, "#e08a2a");
}

/** Converts a station-local point (relative to the node with y offset -0.5) to world coordinates. */
function rotateInto(site: StationSite, u: number, y: number, v: number): Vector3 {
  const c = Math.cos(site.yaw), s = Math.sin(site.yaw);
  return new Vector3(site.x + u * c - v * s, site.y - 0.5 + y, site.z + u * s + v * c);
}

/** Banana warehouse: long shed with corrugated roof, covered dock and lettering. */
function buildBananaWarehouse(scene: Scene, parts: PartBuilder, b: BoxFn, win: WinFn, solid: Mesh[], lights: Mesh[], fruit: Matrix[], node: TransformNode): void {
  void win; void fruit;
  b(54, 9, 16, 4, 0, 46, "#d7c27a");
  solid.push(parts.gable(17, 55, 4, 4, 9, 46, "#5d7a52"));
  b(54, 1.3, 6, 4, 0, 34.5, "#b8b0a0");
  b(56, 0.3, 7.5, 4, 5.4, 34.5, "#5d7a52");
  for (const u of [-22, -8, 6, 20, 30]) b(0.4, 4, 0.4, u, 1.3, 31.5, "#4a4036");
  for (const u of [-18, -6, 6, 18]) { b(5, 4.6, 0.3, u, 1.3, 38.2, "#6b4a2b"); lights.push(parts.box(0.5, 0.3, 0.3, u, 6.3, 38.0, "#ffe9a0")); }
  for (let i = 0; i < 14; i++) b(1.6, 1.0, 1.2, -20 + (i % 7) * 5.2, 1.3, 34 + Math.floor(i / 7) * 2.5, "#e0c030");
  const sign = MeshBuilder.CreatePlane("banana-sign", { width: 16, height: 2.6 }, scene);
  sign.material = signMaterial(scene, "AVON BANANA CO.", "#f2d14a", "#2c4a22");
  sign.parent = node; sign.position.set(4, 8.0, 37.7); sign.rotation.y = Math.PI;
}

/** Juice plant: brick hall, three storage tanks with ladders, pipes and a tall stack. */
function buildJuicePlant(parts: PartBuilder, b: BoxFn, win: WinFn, solid: Mesh[], lights: Mesh[]): void {
  b(42, 11, 22, 8, 0, 50, "#9a8d7c");
  solid.push(parts.gable(24, 43, 4, 8, 11, 50, "#3f4a50"));
  for (let u = -8; u <= 24; u += 4.5) win(2, 2.4, 0.2, u, 6.5, 38.8);
  for (const u of [-34, -20, -6]) {
    solid.push(parts.cyl(10, 15, u, 7.5, 38, "y", "#c9ccce", 24));
    solid.push(parts.ball(10, u, 15, 38, "#b6babd", 0.5));
    solid.push(parts.box(0.4, 15, 0.4, u + 5.2, 7.5, 38, "#55585c"));
    for (let k = 0; k < 8; k++) solid.push(parts.box(1.0, 0.1, 0.5, u + 5.2, 1.5 + k * 1.8, 38, "#55585c"));
    solid.push(parts.cyl(0.6, 10, u, 0.3 + 1, 31.5, "z", "#8a6a3a", 8));
    lights.push(parts.box(0.8, 0.3, 0.4, u, 17.6, 38, "#ffe9b0"));
  }
  solid.push(parts.cyl(0.9, 20, -8, 1.5, 34, "x", "#7a5a2a", 8));
  b(10, 3, 5, -20, 0, 28, "#707478");
  b(6, 0.2, 5, 4, 3.6, 28, "#555a5e");
  solid.push(parts.cyl(2.6, 32, 24, 16, 36, "y", "#b04a3a", 12, 1.6));
  solid.push(parts.cyl(2.2, 3, 24, 32.5, 36, "y", "#e8e0d0", 12, 1.8));
  lights.push(parts.box(0.5, 0.5, 0.5, 24, 34.5, 36, "#ff5a40"));
}

/** Signal cabin and storage-yard clutter for Bath. */
function buildSignalCabin(parts: PartBuilder, b: BoxFn, win: WinFn, solid: Mesh[], lights: Mesh[]): void {
  void lights;
  b(6, 3.5, 6, 22, 0, 32, "#8a8f94");
  b(7, 3, 7, 22, 3.5, 32, "#a7a08a");
  solid.push(parts.gable(8.4, 8, 2, 22, 6.5, 32, ROOF));
  for (const u of [19.5, 22, 24.5]) win(1.4, 1.2, 0.15, u, 5.0, 28.4);
  b(20, 5, 12, -22, 0, 44, "#6f5a46");
  solid.push(parts.gable(14, 21, 3, -22, 5, 44, "#3f3a36"));
  for (let i = 0; i < 6; i++) b(8, 0.6, 0.6, -36 + (i % 3) * 2, 0.0 + Math.floor(i / 3) * 0.6, 28 + (i % 2), WOOD);
  for (const [u, v] of [[-4, 40], [4, 41]]) solid.push(parts.cyl(5, 4, u, 2, v, "y", "#9a9a95", 14));
}

/** Water tower and engine shed for Dansville. */
function buildWaterTower(parts: PartBuilder, b: BoxFn, win: WinFn, solid: Mesh[], lights: Mesh[]): void {
  void lights;
  for (const [x, z] of [[-2, -2], [2, -2], [-2, 2], [2, 2]]) solid.push(parts.box(0.4, 11, 0.4, 26 + x, 5.5, 38 + z, "#4a3a2b"));
  solid.push(parts.cyl(7, 5, 26, 13.5, 38, "y", "#6b4a2b", 16));
  solid.push(parts.cyl(7.6, 0.3, 26, 11, 38, "y", "#3a3a3a", 16));
  solid.push(parts.cyl(7, 0.3, 26, 16.3, 38, "y", "#4a3a2b", 16, 1));
  b(26, 6.5, 12, -22, 0, 42, "#9a4f3a");
  solid.push(parts.gable(14, 27, 3, -22, 6.5, 42, "#3f3a36"));
  for (const u of [-30, -22, -14]) { b(5, 5, 0.3, u, 0, 35.9, "#3a2a20"); win(2, 1.2, 0.1, u, 7.3, 35.8); }
  b(14, 0.5, 7, 4, 0, 29.5, "#7d7b73");
  for (let i = 0; i < 5; i++) b(0.4, 0.4, 7.5, 40, 0.2 + i * 0.45, 40 - i * 0.2, WOOD);
}
