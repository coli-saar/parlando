/**
 * Rolling stock: true-scale diesel road engine, ribbed boxcar and tank car built
 * from primitives in code (no model files).
 *
 * Owns the geometry, its materials and the per-unit animation handles: wheelsets
 * that turn with distance, window and lamp glow, the id lettering on the cab, the
 * cargo placard of a car and the exhaust particle system of an engine. Bodies are
 * merged into one mesh per kind; units are instances of those prototypes, so a
 * whole consist costs a handful of draw calls.
 * Local frame of every unit: +z forward, +y up from the rail head, x lateral.
 * Real dimensions (metres): engine 17 x 3 x 4.3, boxcar 15.2, tank car 12,
 * wheel diameter about 0.9, gauge 1.435.
 */
import {
  Color3, Color4, DynamicTexture, Engine, Mesh, MeshBuilder, ParticleSystem, Scene, StandardMaterial,
  TransformNode, Vector3, type AbstractMesh, type InstancedMesh
} from "@babylonjs/core";
import type { Commodity } from "../game";
import { PartBuilder } from "./parts";

/** Colour of the cargo placard / band per commodity. */
export const CARGO_COLORS: Record<Commodity, Color3> = {
  oranges: new Color3(1, 0.5, 0.05),
  bananas: new Color3(1, 0.88, 0.1),
  juice: new Color3(0.95, 0.62, 0.15)
};

/** Palette of the approved sketches. */
const DARK = "#1c1c1c", STEEL = "#55585c", CREAM = "#e8dcb5", GRAB = "#d9d2b8";
/** Engine liveries by number: E1 blue, E2 green, E3 maroon. */
const ENGINE_LIVERY = ["#1d3f6e", "#1f4d3a", "#6a2a22"];

/** A drawn engine or car with the handles the scene animates every frame. */
export interface StockUnit {
  root: TransformNode;
  wheelsets: (Mesh | InstancedMesh)[];
  /** Wheel radius in metres, to turn wheels with distance. */
  wheelRadius: number;
  /** Sets the cargo marking (null = empty). No effect on engines. */
  setLoad(load: Commodity | null): void;
  /** Shows a loading gauge board on the car's sides (commodity colour filled to `fill` 0..1), or hides it with null. No effect on engines. */
  setGauge(gauge: { commodity: Commodity; fill: number } | null): void;
  /** Sets exhaust output 0..1 (engines only). */
  setThrottle(level: number, failed: boolean): void;
  /** Sets headlamp and window glow 0..1 for a given darkness. */
  setLamps(level: number): void;
  /** All meshes that should cast shadows. */
  casters: AbstractMesh[];
  /** All meshes owned by the unit (for visibility toggles). */
  meshes: AbstractMesh[];
  dispose(): void;
}

/** Shared materials and prototypes used by every unit of one scene. */
export class Stock {
  private readonly body: StandardMaterial;
  private readonly glass: StandardMaterial;
  private readonly lamp: StandardMaterial;
  private readonly placards = new Map<Commodity, StandardMaterial>();
  private readonly puff: DynamicTexture;
  private protos = new Map<string, Mesh>();
  private counter = 0;
  private readonly letterMats = new Map<string, StandardMaterial>();
  private readonly beamMat: StandardMaterial;
  private readonly parts: PartBuilder;

  constructor(private readonly scene: Scene) {
    this.parts = new PartBuilder(scene);
    this.body = new StandardMaterial("stock-body", scene);
    this.body.diffuseColor = new Color3(1, 1, 1);
    this.body.specularColor = new Color3(0.12, 0.12, 0.12);
    this.glass = new StandardMaterial("stock-glass", scene);
    this.glass.diffuseColor = new Color3(0.05, 0.07, 0.09);
    this.glass.specularColor = new Color3(0.6, 0.6, 0.6);
    this.glass.emissiveColor = new Color3(0, 0, 0);
    this.lamp = new StandardMaterial("stock-lamp", scene);
    this.lamp.diffuseColor = new Color3(0.3, 0.3, 0.25);
    this.lamp.emissiveColor = new Color3(0, 0, 0);
    for (const [k, c] of Object.entries(CARGO_COLORS)) {
      const m = new StandardMaterial(`placard-${k}`, scene);
      m.diffuseColor = c; m.emissiveColor = c.scale(0.35); m.specularColor = Color3.Black();
      this.placards.set(k as Commodity, m);
    }
    this.beamMat = new StandardMaterial("beam", scene);
    this.beamMat.emissiveColor = new Color3(1, 0.95, 0.78); this.beamMat.disableLighting = true;
    this.beamMat.alpha = 0; this.beamMat.alphaMode = Engine.ALPHA_ADD; this.beamMat.disableDepthWrite = true;
    this.beamMat.backFaceCulling = false; this.beamMat.fogEnabled = false;
    this.puff = new DynamicTexture("puff", { width: 64, height: 64 }, scene, true);
    const g = this.puff.getContext();
    const grad = g.createRadialGradient(32, 32, 2, 32, 32, 30);
    grad.addColorStop(0, "rgba(255,255,255,0.9)");
    grad.addColorStop(1, "rgba(255,255,255,0)");
    g.fillStyle = grad;
    g.fillRect(0, 0, 64, 64);
    this.puff.update();
    this.puff.hasAlpha = true;
  }

  /** Makes a coloured box part positioned in the local frame. */
  private box(w: number, h: number, d: number, x: number, y: number, z: number, color: string): Mesh {
    return this.parts.box(w, h, d, x, y, z, color);
  }

  /** Makes a coloured cylinder part; `axis` is the direction of its length. */
  private cyl(dia: number, len: number, x: number, y: number, z: number, axis: "x" | "y" | "z", color: string, tess = 14): Mesh {
    return this.parts.cyl(dia, len, x, y, z, axis, color, tess);
  }

  /** Merges parts into one mesh with the shared body material; the prototype stays hidden. */
  private merge(name: string, parts: Mesh[], material: StandardMaterial = this.body): Mesh {
    return this.parts.merge(name, parts, material, false);
  }

  /** Builds (once) and returns a named prototype. */
  private proto(name: string, build: () => Mesh): Mesh {
    let p = this.protos.get(name);
    if (!p) { p = build(); this.protos.set(name, p); }
    return p;
  }

  /** One wheelset: axle and two flanged wheels, axis along x. */
  private wheelsetProto(radius: number): Mesh {
    return this.proto(`wheelset-${radius}`, () => {
      const parts = [
        this.cyl(0.16, 1.6, 0, 0, 0, "x", STEEL, 8),
        this.cyl(radius * 2, 0.14, -0.72, 0, 0, "x", STEEL, 20),
        this.cyl(radius * 2, 0.14, 0.72, 0, 0, "x", STEEL, 20),
        // Cream counterweights so the turning wheels read at a distance.
        this.box(0.05, 0.22, 0.22, -0.8, radius * 0.6, 0, CREAM), this.box(0.05, 0.22, 0.22, 0.8, radius * 0.6, 0, CREAM)
      ];
      return this.merge("wheelset", parts);
    });
  }

  /** Stores a unit's instance, registering it for shadows and cleanup. */
  private inst(unit: { meshes: AbstractMesh[]; casters: AbstractMesh[] }, proto: Mesh, parent: TransformNode, caster = true): InstancedMesh {
    const i = proto.createInstance(`${proto.name}-i${this.counter++}`);
    i.parent = parent;
    unit.meshes.push(i);
    if (caster) unit.casters.push(i);
    return i;
  }

  /** Truck frames, journal boxes and bolster at forward offset `z` (part of the static hull). */
  private truckParts(p: Mesh[], z: number): void {
    for (const x of [-0.95, 0.95]) {
      p.push(this.box(0.16, 0.28, 3.6, x, 0.52, z, DARK));
      p.push(this.box(0.2, 0.36, 0.7, x, 0.4, z, DARK));
    }
    p.push(this.box(2, 0.28, 0.5, 0, 0.66, z, DARK));
  }

  /** Cached lettering material: bold text on a transparent plane texture. */
  private lettering(text: string, color: string): StandardMaterial {
    const key = `${text}|${color}`;
    let m = this.letterMats.get(key);
    if (!m) {
      const tex = new DynamicTexture(`txt-${key}`, { width: 256, height: 128 }, this.scene, true);
      const g = tex.getContext() as CanvasRenderingContext2D;
      g.clearRect(0, 0, 256, 128);
      g.fillStyle = color; g.font = `bold ${text.length > 4 ? 52 : 92}px Helvetica, Arial, sans-serif`;
      g.textAlign = "center"; g.textBaseline = "middle"; g.fillText(text, 128, 66);
      tex.update(); tex.hasAlpha = true;
      m = new StandardMaterial(`txt-mat-${key}`, this.scene);
      m.diffuseTexture = tex; m.opacityTexture = tex; m.specularColor = Color3.Black();
      m.emissiveColor = new Color3(0.25, 0.25, 0.23);
      this.letterMats.set(key, m);
    }
    return m;
  }

  /** Adds lettering planes on both sides of a unit. */
  private letterSides(unit: StockUnit, text: string, color: string, w: number, h: number, x: number, y: number, z: number): void {
    const mat = this.lettering(text, color);
    for (const side of [1, -1]) {
      const plane = MeshBuilder.CreatePlane(`lt-${this.counter++}`, { width: w, height: h }, this.scene);
      plane.material = mat; plane.parent = unit.root;
      plane.position.set(side * x, y, z);
      plane.rotation.y = side === 1 ? -Math.PI / 2 : Math.PI / 2;
      unit.meshes.push(plane);
    }
  }

  /** Builds a diesel road engine in the livery of its id (E1 blue, E2 green, E3 maroon). */
  createEngine(id: string): StockUnit {
    const root = new TransformNode(`engine-${id}`, this.scene);
    const unit: StockUnit = {
      root, wheelsets: [], wheelRadius: 0.46, casters: [], meshes: [],
      setLoad() {}, setGauge() {}, setThrottle() {}, setLamps() {}, dispose() { root.dispose(false, false); }
    };
    const livery = ENGINE_LIVERY[(Math.max(1, parseInt(id.replace(/\D/g, ""), 10) || 1) - 1) % ENGINE_LIVERY.length];
    this.inst(unit, this.proto(`engine-hull-${livery}`, () => this.buildEngineHull(livery)), root);
    this.inst(unit, this.proto("engine-windows", () => this.buildEngineWindows()), root, false);
    this.inst(unit, this.proto("engine-lamp", () => this.merge("engine-lamp", [this.box(0.5, 0.26, 0.1, 0, 2.62, 6.87, "#fff3c4")], this.lamp)), root, false);
    const wheelset = this.wheelsetProto(0.46);
    for (const z of [-6.7, -4.1, 4.1, 6.7]) {
      const w = this.inst(unit, wheelset, root);
      w.position.set(0, 0.46, z);
      unit.wheelsets.push(w);
    }
    this.letterSides(unit, id, "#e8dcb5", 1.6, 0.8, 1.52, 2.42, 3.2);
    // Headlight beam: a faint additive cone, only visible in the dark.
    const beam = MeshBuilder.CreateCylinder(`beam-${id}`, { diameterTop: 11, diameterBottom: 0.4, height: 44, tessellation: 20, cap: Mesh.NO_CAP }, this.scene);
    beam.parent = root; beam.rotation.x = Math.PI / 2; beam.position.set(0, 2.3, 29); beam.material = this.beamMat; beam.isPickable = false;
    beam.setEnabled(false);
    unit.meshes.push(beam);
    // Exhaust.
    const stack = MeshBuilder.CreateBox(`stack-${id}`, { size: 0.1 }, this.scene);
    stack.parent = root; stack.position.set(0, 3.9, -5.1); stack.isVisible = false;
    unit.meshes.push(stack);
    const smoke = new ParticleSystem(`smoke-${id}`, 70, this.scene);
    smoke.particleTexture = this.puff;
    smoke.emitter = stack;
    smoke.minEmitBox = new Vector3(-0.3, 0, -0.3); smoke.maxEmitBox = new Vector3(0.3, 0, 0.3);
    smoke.direction1 = new Vector3(-0.3, 1.5, -0.2); smoke.direction2 = new Vector3(0.3, 2.2, 0.2);
    smoke.minLifeTime = 1.4; smoke.maxLifeTime = 2.8;
    smoke.minEmitPower = 0.8; smoke.maxEmitPower = 1.6;
    smoke.gravity = new Vector3(0.6, 0.1, 0);
    smoke.colorDead = new Color4(0.5, 0.5, 0.52, 0);
    smoke.emitRate = 0;
    smoke.blendMode = ParticleSystem.BLENDMODE_STANDARD;
    smoke.start();
    unit.setThrottle = (level, failed) => {
      smoke.emitRate = failed ? 40 : 4 + level * 26;
      smoke.minSize = failed ? 1.2 : 0.5; smoke.maxSize = failed ? 3 : 1.3;
      smoke.color1 = failed ? new Color4(0.08, 0.08, 0.08, 0.8) : new Color4(0.45, 0.45, 0.47, 0.5);
      smoke.color2 = failed ? new Color4(0.15, 0.15, 0.15, 0.7) : new Color4(0.3, 0.3, 0.32, 0.4);
    };
    unit.setLamps = (level) => {
      this.lamp.emissiveColor = new Color3(1, 0.95, 0.75).scale(0.9 + 0.1 * level);
      this.glass.emissiveColor = new Color3(1, 0.78, 0.44).scale(level * 1.6);
      // Night readability: a faint self-lit lift keeps dark liveries and cars legible, and the number boards glow.
      this.body.emissiveColor = new Color3(0.07, 0.08, 0.11).scale(level);
      for (const m of this.letterMats.values()) m.emissiveColor = new Color3(0.25, 0.25, 0.23).add(new Color3(0.5, 0.48, 0.4).scale(level));
      this.beamMat.alpha = 0.07 * level;
      beam.setEnabled(level > 0.05);
    };
    unit.dispose = () => { smoke.dispose(); root.dispose(false, false); };
    return unit;
  }

  /** Merged hull of the engine, following the approved sketch: long hood to the rear, cab, short hood in front. */
  private buildEngineHull(body: string): Mesh {
    const p: Mesh[] = [];
    const b = (w: number, h: number, d: number, x: number, y: number, z: number, c: string) => p.push(this.box(w, h, d, x, y, z, c));
    b(3, 0.22, 17, 0, 1.25, 0, DARK); b(2.2, 0.3, 16.6, 0, 1.0, 0, DARK);
    b(1.85, 2.25, 9.6, 0, 2.49, -2.9, body); b(1.5, 0.12, 9.6, 0, 3.67, -2.9, body);
    b(3, 2.75, 2.5, 0, 2.74, 3.2, body); b(3.15, 0.14, 2.8, 0, 4.18, 3.2, DARK);
    b(1.85, 1.55, 2.4, 0, 2.14, 5.65, body);
    [[-2.9, 9.62, 1.87], [3.2, 2.52, 3.02], [5.65, 2.42, 1.87]].forEach(([z, d, w]) => b(w, 0.28, d, 0, 1.75, z, CREAM));
    for (const x of [-0.8, 0.8]) b(0.6, 0.3, 0.1, x * 1.1, 4.0, 4.5, CREAM);
    for (const z of [-6.6, -5.1, -3.6]) p.push(this.cyl(1.2, 0.12, 0, 3.78, z, "y", DARK, 20));
    b(0.9, 0.32, 0.5, 0, 3.85, -0.8, DARK);
    p.push(this.cyl(0.12, 0.7, 0.4, 4.38, 3.2, "z", STEEL, 8));
    p.push(this.cyl(1.44, 4.2, 0, 0.72, 0, "z", DARK, 20));
    for (const x of [-1.1, 1.1]) p.push(this.cyl(0.4, 2.2, x, 1.02, -0.2, "z", STEEL, 10));
    for (const z of [-8.9, 8.9]) b(0.26, 0.22, 0.9, 0, 0.9, z, DARK);
    for (const z of [-8.55, 8.55]) {
      b(2.9, 0.9, 0.15, 0, 0.72, z, DARK); b(2.9, 0.05, 0.05, 0, 2.3, z * 0.985, GRAB);
      for (const x of [-1.4, 0, 1.4]) b(0.05, 1, 0.05, x, 1.85, z * 0.985, GRAB);
    }
    for (const x of [-1.45, 1.45]) {
      b(0.05, 0.05, 10.2, x, 2.3, -3.2, GRAB); b(0.05, 0.05, 3.8, x, 2.3, 6.4, GRAB);
      for (let z = -8.3; z <= 1.9; z += 1.7) b(0.05, 1, 0.05, x, 1.85, z, GRAB);
      for (const z of [4.6, 6.4, 8.3]) b(0.05, 1, 0.05, x, 1.85, z, GRAB);
    }
    this.truckParts(p, -5.4); this.truckParts(p, 5.4);
    return this.merge("engine-hull", p);
  }

  /** Front and side windows of the cab (glow at night). */
  private buildEngineWindows(): Mesh {
    const p: Mesh[] = [];
    for (const x of [-0.8, 0.8]) p.push(this.box(0.95, 0.75, 0.06, x, 3.45, 4.46, "#10171c"));
    for (const x of [-1.51, 1.51]) p.push(this.box(0.06, 0.8, 1.3, x, 3.35, 3.2, "#10171c"));
    return this.merge("engine-windows", p, this.glass);
  }

  /** Builds a boxcar or tank car with its reporting mark. */
  createCar(kind: "boxcar" | "tanker", number = 4000): StockUnit {
    const root = new TransformNode(`car-${kind}-${this.counter++}`, this.scene);
    const unit: StockUnit = {
      root, wheelsets: [], wheelRadius: 0.46, casters: [], meshes: [],
      setLoad() {}, setGauge() {}, setThrottle() {}, setLamps() {}, dispose() { root.dispose(false, false); }
    };
    this.inst(unit, this.proto(`${kind}-hull`, () => (kind === "boxcar" ? this.buildBoxcar() : this.buildTanker())), root);
    const wheelset = this.wheelsetProto(0.46);
    const truckZ = kind === "boxcar" ? 5.3 : 4.9;
    for (const t of [-truckZ, truckZ]) for (const a of [-1.3, 1.3]) {
      const w = this.inst(unit, wheelset, root);
      w.position.set(0, 0.46, t + a);
      unit.wheelsets.push(w);
    }
    if (kind === "boxcar") this.letterSides(unit, `NF ${number}`, "#e8dcb5", 3, 0.7, 1.6, 3.2, -4.6);
    else this.letterSides(unit, `NF ${number}`, "#2c2c2a", 3, 0.7, 1.42, 2.75, 2.4);
    // Cargo marking: placards on both sides of a boxcar door, a band round a tank.
    const mark = kind === "boxcar" ? this.proto("boxcar-placard", () => {
      const a = MeshBuilder.CreatePlane("pl-a", { width: 0.9, height: 0.9 }, this.scene);
      a.position.set(1.62, 2.6, 0); a.rotation.y = -Math.PI / 2;
      const b = MeshBuilder.CreatePlane("pl-b", { width: 0.9, height: 0.9 }, this.scene);
      b.position.set(-1.62, 2.6, 0); b.rotation.y = Math.PI / 2;
      const m = Mesh.MergeMeshes([a, b], true, true)!;
      m.isVisible = false; return m;
    }) : this.proto("tank-band", () => {
      const m = MeshBuilder.CreateCylinder("band", { diameter: 2.9, height: 1.4, tessellation: 24 }, this.scene);
      m.rotation.x = Math.PI / 2; m.bakeCurrentTransformIntoVertices(); m.position.y = 2.65;
      m.bakeCurrentTransformIntoVertices(); m.isVisible = false; return m;
    });
    const marker = mark.clone(`mark-${this.counter++}`);
    marker.parent = root; marker.isVisible = false; marker.setEnabled(false);
    unit.meshes.push(marker);
    unit.setLoad = (load) => {
      marker.setEnabled(load !== null);
      marker.isVisible = load !== null;
      if (load) marker.material = this.placards.get(load)!;
    };
    // Loading gauge: a dark board on each side with a bar in the cargo colour that fills from the bottom.
    const boxcar = kind === "boxcar";
    const gx = boxcar ? 1.68 : 1.46, gh = boxcar ? 2.4 : 1.3, gw = boxcar ? 1.8 : 1.0, gy = boxcar ? 2.6 : 2.65;
    const boards = [1, -1].map((side) => {
      const mk = (w: number, h: number, dx: number) => {
        const m = MeshBuilder.CreatePlane(`gauge-${this.counter++}`, { width: w, height: h }, this.scene);
        m.parent = root; m.rotation.y = side === 1 ? -Math.PI / 2 : Math.PI / 2; m.setEnabled(false);
        m.position.set(side * (gx + dx), gy, 0); m.isPickable = false; unit.meshes.push(m);
        return m;
      };
      const frame = mk(gw + 0.2, gh + 0.2, 0);
      frame.material = this.gaugeFrame();
      return { frame, fill: mk(gw, gh, 0.03) };
    });
    unit.setGauge = (g) => {
      for (const b of boards) {
        b.frame.setEnabled(g !== null); b.fill.setEnabled(g !== null && g.fill > 0.01);
        if (!g) continue;
        const f = Math.min(1, Math.max(0.001, g.fill));
        b.fill.scaling.y = f; b.fill.position.y = gy - gh / 2 + (gh * f) / 2;
        b.fill.material = this.placards.get(g.commodity)!;
      }
    };
    return unit;
  }

  private gaugeFrameMat: StandardMaterial | null = null;
  /** Shared dark material of the gauge boards. */
  private gaugeFrame(): StandardMaterial {
    if (!this.gaugeFrameMat) {
      const m = new StandardMaterial("gauge-frame", this.scene);
      m.diffuseColor = new Color3(0.08, 0.09, 0.1); m.emissiveColor = new Color3(0.05, 0.05, 0.06); m.specularColor = Color3.Black();
      this.gaugeFrameMat = m;
    }
    return this.gaugeFrameMat;
  }

  /** Ribbed boxcar: brick-red body, darker sliding doors with track rails, end ladders. */
  private buildBoxcar(): Mesh {
    const p: Mesh[] = [];
    const b = (w: number, h: number, d: number, x: number, y: number, z: number, c: string) => p.push(this.box(w, h, d, x, y, z, c));
    const body = "#7a3a26", door = "#8a4630";
    b(3, 2.95, 15.2, 0, 2.675, 0, body); b(3.15, 0.1, 15.4, 0, 4.2, 0, body); b(1.6, 0.1, 15.4, 0, 4.29, 0, body);
    for (let z = -7.2; z <= 7.25; z += 1.03) {
      if (Math.abs(z) < 1.7) continue;
      b(0.08, 2.85, 0.1, 1.54, 2.675, z, body); b(0.08, 2.85, 0.1, -1.54, 2.675, z, body);
    }
    for (const x of [-1.55, 1.55]) {
      b(0.1, 2.7, 3, x, 2.6, 0, door);
      b(0.1, 0.08, 6.2, x, 4.0, 1.5, STEEL); b(0.1, 0.08, 6.2, x, 1.25, 1.5, STEEL);
      b(0.06, 0.5, 0.08, x * 1.04, 2.5, -1.2, STEEL);
    }
    for (const z of [-7.3, 7.3]) for (const x of [-1.57, 1.57]) for (let y = 1.5; y <= 3.9; y += 0.4) b(0.06, 0.04, 0.5, x, y, z, GRAB);
    b(2.4, 0.35, 15, 0, 1.05, 0, DARK);
    this.truckParts(p, -5.3); this.truckParts(p, 5.3);
    for (const z of [-8, 8]) b(0.26, 0.22, 0.9, 0, 0.9, z, DARK);
    return this.merge("boxcar-hull", p);
  }

  /** Tank car: off-white shell with domed ends, dome and platform, side ladder, dark saddles. */
  private buildTanker(): Mesh {
    const p: Mesh[] = [];
    const b = (w: number, h: number, d: number, x: number, y: number, z: number, c: string) => p.push(this.box(w, h, d, x, y, z, c));
    const shell = "#e4e1d6";
    p.push(this.cyl(2.8, 11.8, 0, 2.65, 0, "z", shell, 28));
    for (const z of [-5.9, 5.9]) { const s = this.parts.ball(2.8, 0, 2.65, z, shell); s.scaling.z = 0.45; p.push(s); }
    for (const z of [-3.5, 3.5]) p.push(this.cyl(2.86, 0.12, 0, 2.65, z, "z", DARK, 28));
    p.push(this.cyl(1.2, 0.55, 0, 4.25, 0, "y", shell, 20)); p.push(this.cyl(1.32, 0.08, 0, 4.55, 0, "y", DARK, 20));
    b(2, 0.06, 2.2, 0, 4.1, 0, DARK);
    for (const x of [-1, 1]) { b(0.05, 0.05, 2.2, x, 4.75, 0, GRAB); for (const z of [-1.05, 1.05]) b(0.05, 0.65, 0.05, x, 4.43, z, GRAB); }
    for (const x of [-1.5, 1.5]) {
      for (const z of [-0.25, 0.25]) b(0.05, 2.9, 0.05, x, 2.65, z, GRAB);
      for (let y = 1.4; y <= 4; y += 0.4) b(0.05, 0.04, 0.5, x, y, 0, GRAB);
    }
    b(1.2, 0.3, 13.4, 0, 1.1, 0, DARK);
    for (const z of [-4.6, 4.6]) b(2.2, 0.6, 0.8, 0, 1.45, z, DARK);
    for (const z of [-6.5, 6.5]) b(2.8, 0.1, 0.8, 0, 1.3, z, DARK);
    this.truckParts(p, -4.9); this.truckParts(p, 4.9);
    for (const z of [-7.15, 7.15]) b(0.26, 0.22, 0.9, 0, 0.9, z, DARK);
    return this.merge("tank-hull", p);
  }

  /** Applies a world pose to a unit's root. */
  static place(unit: StockUnit, x: number, y: number, z: number, yaw: number): void {
    unit.root.position.set(x, y, z);
    unit.root.rotation.y = yaw;
  }

  /** Spins a unit's wheels for `distance` metres travelled in its forward direction. */
  static spin(unit: StockUnit, distance: number): void {
    const a = distance / unit.wheelRadius;
    for (const w of unit.wheelsets) w.rotation.x += a;
  }
}

