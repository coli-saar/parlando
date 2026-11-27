/**
 * The Night Freight 3D world: one Babylon scene with landscape, track, stations,
 * rolling stock, light and sound, driven by `Frame`s from the replay timeline.
 *
 * Owns the engine/scene lifecycle, the two cameras (an orbit camera for the
 * planning overview and a free camera that the film director aims every frame),
 * sky and light following a clock hour, the pool of drawn engines and cars,
 * wheel rotation and rail clacks from travelled distance, signal states,
 * and the telegram prop. It never decides what happens; callers pass frames.
 * Contract: `showFrame` is cheap to call every render tick; `cut()` must be called
 * when the film cuts so no smoothing is carried across the cut.
 */
import {
  ArcRotateCamera, Color3, DirectionalLight, DynamicTexture, Engine, FreeCamera, HemisphericLight, Mesh, MeshBuilder,
  Scene, ShadowGenerator, SpotLight, StandardMaterial, Vector3, type AbstractMesh
} from "@babylonjs/core";
import type { Commodity, GameMap } from "../game";
import type { CarFrame, EngineFrame, Frame } from "../replay/timeline";
import { CargoEffects, type CargoWork } from "./effects";
import { Geography, type P3 } from "./geography";
import { createLandscape } from "./landscape";
import { Sky } from "./sky";
import { createFreightSound, type FreightSound } from "./sound";
import { createStations, type Stations } from "./stations";
import { Stock, type StockUnit } from "./stock";
import { RAIL_TOP, createTrack, type Track } from "./track";

/** One drawn engine or car: its unit, where it was last placed and whether it casts shadows. */
interface Shown {
  unit: StockUnit;
  last: P3 | null;
  caster: boolean;
}

/** Camera placement for the film. */
export interface CameraPose { pos: P3; target: P3; fov: number }

/** The scene wrapper used by planning overview and replay. */
export class NightFreightWorld {
  readonly engine: Engine;
  readonly scene: Scene;
  readonly geo: Geography;
  readonly sound: FreightSound;
  private readonly sky: Sky;
  private readonly sun: DirectionalLight;
  private readonly hemi: HemisphericLight;
  private readonly spots: SpotLight[] = [];
  private readonly shadows: ShadowGenerator;
  private readonly stock: Stock;
  private readonly cargo: CargoEffects;
  private readonly track: Track;
  private readonly stations: Stations;
  private readonly overviewCamera: ArcRotateCamera;
  private readonly filmCamera: FreeCamera;
  private readonly engines = new Map<string, Shown>();
  private readonly cars = new Map<number, Shown>();
  private readonly labels: Mesh[] = [];
  private readonly telegram: Mesh;
  private hour = 12;
  private hourOverride: number | null = null;
  private cutPending = true;
  private clackDistance = 0;
  private ticker: ((dt: number) => void) | null = null;
  private resize = () => this.engine.resize();
  /** Speed of the fastest engine near the camera in m/s of film time (for sound). */
  private focusSpeed = 0;

  constructor(readonly canvas: HTMLCanvasElement, map: GameMap) {
    this.engine = new Engine(canvas, true, { preserveDrawingBuffer: true, antialias: true, powerPreference: "high-performance" });
    this.engine.setHardwareScalingLevel(Math.max(1, 1 / Math.min(window.devicePixelRatio || 1, 1.5)));
    this.scene = new Scene(this.engine);
    this.scene.fogMode = Scene.FOGMODE_EXP2;
    this.geo = new Geography(map);
    this.sky = new Sky(this.scene);
    this.hemi = new HemisphericLight("hemi", new Vector3(0, 1, 0), this.scene);
    this.sun = new DirectionalLight("sun", new Vector3(-0.3, -1, 0.3), this.scene);
    this.sun.autoUpdateExtends = false;
    this.sun.orthoLeft = -140; this.sun.orthoRight = 140; this.sun.orthoTop = 140; this.sun.orthoBottom = -140;
    this.sun.shadowMinZ = 1; this.sun.shadowMaxZ = 1000;
    this.shadows = new ShadowGenerator(1024, this.sun);
    this.shadows.usePercentageCloserFiltering = true;
    this.shadows.bias = 0.0008;
    this.shadows.normalBias = 0.02;
    this.shadows.darkness = 0.25;
    for (let i = 0; i < 2; i++) {
      const s = new SpotLight(`head-${i}`, Vector3.Zero(), new Vector3(0, 0, 1), 0.9, 2, this.scene);
      s.intensity = 0; s.range = 220; s.diffuse = new Color3(1, 0.93, 0.75); s.specular = Color3.Black();
      this.spots.push(s);
    }
    const landscape = createLandscape(this.scene, this.geo);
    this.track = createTrack(this.scene, this.geo);
    this.stations = createStations(this.scene, this.geo);
    this.stock = new Stock(this.scene);
    this.cargo = new CargoEffects(this.scene);
    for (const m of [...this.track.casters, ...this.stations.casters]) this.shadows.addShadowCaster(m);
    void landscape;
    this.overviewCamera = new ArcRotateCamera("overview", -1.75, 0.95, 2500, new Vector3(-150, 20, -950), this.scene);
    this.overviewCamera.lowerRadiusLimit = 150; this.overviewCamera.upperRadiusLimit = 4200;
    this.overviewCamera.upperBetaLimit = 1.4; this.overviewCamera.lowerBetaLimit = 0.25;
    this.overviewCamera.minZ = 5; this.overviewCamera.maxZ = 12000;
    this.overviewCamera.wheelPrecision = 0.4; this.overviewCamera.fov = 0.7;
    // Panning: right-drag (or Ctrl/Shift-drag) slides the target across the
    // ground plane (`mapPanning`). Babylon's default sensibility of 1000 moves
    // millimetres per pixel, which is imperceptible on a kilometre-scale map;
    // 5 gives roughly one to two metres per pixel at the default distance.
    this.overviewCamera.panningSensibility = 5;
    this.overviewCamera.mapPanning = true;
    this.filmCamera = new FreeCamera("film", new Vector3(0, 30, 0), this.scene);
    this.filmCamera.minZ = 0.4; this.filmCamera.maxZ = 12000; this.filmCamera.inputs.clear();
    this.scene.activeCamera = this.overviewCamera;
    this.telegram = MeshBuilder.CreateBox("telegram", { width: 3, height: 2, depth: 0.15 }, this.scene);
    const tm = new StandardMaterial("telegram-mat", this.scene);
    tm.emissiveColor = new Color3(1, 0.97, 0.85); tm.disableLighting = true;
    this.telegram.material = tm; this.telegram.setEnabled(false);
    for (const site of this.geo.stations.values()) this.labels.push(this.makeLabel(site.id, site.name, site.x, site.y + 70, site.z));
    this.sound = createFreightSound();
    this.setHour(9);
    window.addEventListener("resize", this.resize);
    this.engine.runRenderLoop(() => {
      this.tick(this.engine.getDeltaTime() / 1000);
      this.scene.render();
    });
  }

  /** A camera-facing text label for overview mode, scaled each frame to stay legible. */
  private makeLabel(id: string, text: string, x: number, y: number, z: number): Mesh {
    const tex = new DynamicTexture(`label-${id}`, { width: 512, height: 128 }, this.scene, true);
    const g = tex.getContext() as CanvasRenderingContext2D;
    g.fillStyle = "rgba(14,17,22,0.72)"; g.fillRect(0, 0, 512, 128);
    g.strokeStyle = "#e8c76a"; g.lineWidth = 6; g.strokeRect(3, 3, 506, 122);
    g.fillStyle = "#f5ecd0"; g.font = "bold 66px Georgia, serif"; g.textAlign = "center"; g.textBaseline = "middle";
    g.fillText(text.toUpperCase(), 256, 68);
    tex.update();
    const m = new StandardMaterial(`label-mat-${id}`, this.scene);
    m.diffuseTexture = tex; m.emissiveColor = Color3.White(); m.disableLighting = true; m.fogEnabled = false;
    m.useAlphaFromDiffuseTexture = false;
    const p = MeshBuilder.CreatePlane(`label-${id}`, { width: 4, height: 1 }, this.scene);
    p.material = m; p.billboardMode = Mesh.BILLBOARDMODE_ALL; p.position.set(x, y, z);
    p.renderingGroupId = 1; p.isPickable = false;
    return p;
  }

  /** Sets the per-frame callback (film time step in seconds) run before each render. */
  setTicker(fn: ((dt: number) => void) | null): void { this.ticker = fn; }

  /** Marks the next frames as continuous with a cut: no hand-over smoothing is applied. */
  cut(): void { this.cutPending = true; }

  /** Applies the clock hour to sky, lights, fog, lamps. */
  setHour(hour: number): void {
    this.hour = hour;
    const cam = (this.scene.activeCamera as ArcRotateCamera | FreeCamera).position;
    const s = this.sky.update(hour, cam);
    this.scene.clearColor = s.fogColor.toColor4(1);
    this.scene.fogColor = s.fogColor;
    // High cameras (overview, order-sheet shot) look through far more air: thin the fog with height.
    const height = cam.y;
    this.scene.fogDensity = s.fogDensity * (this.scene.activeCamera === this.overviewCamera ? 0.5 : 1.4 / (1 + Math.max(0, height - 60) / 25));
    this.sun.diffuse = s.sunColor; this.sun.specular = s.sunColor.scale(0.4);
    this.sun.intensity = s.sunIntensity;
    this.sun.direction = s.toLight.scale(-1);
    this.hemi.intensity = s.hemiIntensity; this.hemi.diffuse = s.hemiColor; this.hemi.groundColor = s.hemiGround;
    this.stations.setGlow(s.lampLevel);
  }

  /** Fixes the displayed hour (planning overview in daylight); null follows the film clock. */
  setHourOverride(hour: number | null): void { this.hourOverride = hour; if (hour !== null) this.setHour(hour); }

  /** Current clock hour. */
  get clockHour(): number { return this.hour; }

  /** Shows the planning overview camera and station labels. */
  useOverview(): void {
    this.scene.activeCamera = this.overviewCamera;
    this.overviewCamera.attachControl(this.canvas, true);
    for (const l of this.labels) l.setEnabled(true);
    this.telegram.setEnabled(false);
    this.cutPending = true;
  }

  /** Switches to the film camera and hides the labels. */
  useFilm(): void {
    this.overviewCamera.detachControl();
    this.scene.activeCamera = this.filmCamera;
    for (const l of this.labels) l.setEnabled(false);
    this.cutPending = true;
  }

  /** Places the film camera. */
  setCamera(pose: CameraPose): void {
    this.filmCamera.position.set(pose.pos.x, pose.pos.y, pose.pos.z);
    this.filmCamera.setTarget(new Vector3(pose.target.x, pose.target.y, pose.target.z));
    this.filmCamera.fov = pose.fov;
  }

  /** Current film camera position (for sound proximity). */
  get cameraPosition(): Vector3 { return (this.scene.activeCamera as FreeCamera).position; }

  /** Per-frame work: labels, shadow frustum, callbacks. */
  private tick(dt: number): void {
    const cam = this.scene.activeCamera as ArcRotateCamera | FreeCamera;
    this.ticker?.(Math.min(dt, 0.1));
    this.sky.update(this.hour, cam.position);
    const focus = this.scene.activeCamera === this.overviewCamera ? (this.overviewCamera.target) : cam.getTarget();
    const toLight = this.sky.state.toLight;
    this.sun.position = focus.add(toLight.scale(450));
    for (const l of this.labels) {
      const d = Vector3.Distance(cam.position, l.position);
      l.scaling.setAll(Math.max(1, d * 0.02));
    }
    this.telegram.rotation.y += dt * 1.4;
    this.telegram.position.y += Math.sin(performance.now() / 400) * 0.01;
    this.cutPending = false;
  }

  /** Gets or creates the drawn object of an engine or car in a pool. */
  private entry<K>(pool: Map<K, Shown>, key: K, make: () => StockUnit): Shown {
    let s = pool.get(key);
    if (!s) {
      const unit = make();
      s = { unit, last: null, caster: false };
      for (const m of unit.casters) this.shadows.addShadowCaster(m);
      pool.set(key, s);
    }
    return s;
  }

  /**
   * Places one drawn object exactly at its pose and returns the distance it
   * travelled since the last frame. The replay timeline already moves every
   * vehicle along rails, so there is no hand-over smoothing: a vehicle is never
   * lifted, turned or slid sideways by the scene.
   */
  private pose(s: Shown, p: P3, yaw: number): number {
    Stock.place(s.unit, p.x, p.y + RAIL_TOP, p.z, yaw);
    let travelled = 0;
    if (s.last && !this.cutPending) {
      const dx = p.x - s.last.x, dz = p.z - s.last.z;
      const d = Math.hypot(dx, dz);
      const fwd = dx * Math.sin(yaw) + dz * Math.cos(yaw);
      travelled = d < 400 ? (fwd >= 0 ? d : -d) : 0;
      Stock.spin(s.unit, Math.max(-20, Math.min(20, travelled)));
    }
    s.last = { x: p.x, y: p.y, z: p.z };
    return Math.abs(travelled);
  }

  /** Shows one frame of the world; `dt` is the real seconds since the previous frame (0 after a cut). */
  showFrame(frame: Frame, dt: number): void {
    this.setHour(this.hourOverride ?? this.hourOf(frame.t));
    const cam = (this.scene.activeCamera as FreeCamera | ArcRotateCamera).position;
    let nearestSpeed = 0, clack = 0;
    const lampLevel = this.sky.state.lampLevel;
    const byDistance = [...frame.engines].sort((a, b) => dist2(a.pos, cam) - dist2(b.pos, cam));
    frame.engines.forEach((e) => {
      const s = this.entry(this.engines, e.id, () => this.stock.createEngine(e.id));
      const d = this.pose(s, e.pos, e.yaw);
      s.unit.setThrottle(e.moving ? 0.35 + Math.min(0.65, d / 6) : 0, e.failed);
      s.unit.setLamps(lampLevel);
      if (e === byDistance[0]) { nearestSpeed = dt > 0 ? d / dt : 0; clack = d; }
    });
    this.focusSpeed = nearestSpeed;
    this.clackDistance += clack;
    while (this.clackDistance > 12) { this.clackDistance -= 12; this.sound.clack(Math.min(1, 80 / (1 + Math.sqrt(dist2(byDistance[0]?.pos ?? cam, cam))))); }
    const spots = byDistance.slice(0, this.spots.length);
    this.spots.forEach((spot, i) => {
      const e = spots[i];
      if (!e || lampLevel < 0.05) { spot.intensity = 0; return; }
      // The lit headlight is at the end that leads: the nose, or the tail while the engine runs tail first.
      const f = { x: Math.sin(e.yaw) * e.lead, z: Math.cos(e.yaw) * e.lead };
      spot.position.set(e.pos.x + f.x * 8.4, e.pos.y + RAIL_TOP + 3.7, e.pos.z + f.z * 8.4);
      spot.direction.set(f.x, -0.06, f.z);
      spot.intensity = 3 * lampLevel;
    });
    const seen = new Set<number>();
    for (const c of frame.cars) {
      seen.add(c.id);
      const s = this.entry(this.cars, c.id, () => this.stock.createCar(c.kind, (c.kind === "boxcar" ? 4015 : 7300) + c.id * 3));
      this.pose(s, c.pos, c.yaw);
      s.unit.setLoad(c.gauge ? null : (c.load as Commodity | null));
      s.unit.setGauge(c.gauge ?? null);
    }
    this.showCargoWork(frame);
    this.track.signals.clear();
    for (const r of frame.reds) this.track.signals.set(r.station, r.end, true);
    if (frame.disrupted?.type === "track_closure") this.track.signals.setEdge(frame.disrupted.a, frame.disrupted.b, true);
    if (frame.disrupted?.type === "late_order") {
      const p = this.stations.depotPoint("corning");
      this.telegram.setEnabled(true); this.telegram.position.set(p.x - 4, p.y + 3, p.z);
    } else this.telegram.setEnabled(false);
  }

  /** Gantries over the cars being loaded or unloaded, and the factory steam while cars are processed. */
  private showCargoWork(frame: Frame): void {
    const items: CargoWork[] = [];
    let processing = false;
    for (const w of frame.work) {
      if (w.op.type === "process") { processing = true; continue; }
      if (w.op.type !== "load" && w.op.type !== "deliver") continue;
      const car = frame.cars.find((c) => c.id === w.cars[0]);
      if (!car) continue;
      items.push({ pos: car.pos, yaw: car.yaw, commodity: w.op.type === "load" ? w.op.commodity : car.gauge?.commodity ?? "oranges", mode: w.op.type === "load" ? "load" : "unload" });
    }
    this.cargo.update(items, performance.now() / 1000);
    this.stations.setFactoryWorking(processing);
  }

  /** Clock hour of a sim time (hours since midnight; may exceed 24). */
  private hourOf(t: number): number { return t; }

  /** Hides all rolling stock (before the first frame of a new film or overview). */
  clearStock(): void {
    for (const pool of [this.engines, this.cars] as Map<unknown, Shown>[]) {
      for (const s of pool.values()) {
        for (const m of s.unit.casters) this.shadows.removeShadowCaster(m as AbstractMesh);
        s.unit.dispose();
      }
      pool.clear();
    }
    this.track.signals.clear();
    this.cargo.update([], 0);
    this.stations.setFactoryWorking(false);
    this.cutPending = true;
  }

  /** Speed of the engine nearest the camera in metres per second of film time. */
  get speedNearCamera(): number { return this.focusSpeed; }

  /** Releases the scene, engine and audio. */
  dispose(): void {
    window.removeEventListener("resize", this.resize);
    this.engine.stopRenderLoop();
    this.sound.dispose();
    this.scene.dispose();
    this.engine.dispose();
  }
}

/** Squared horizontal distance between a point and a Babylon vector. */
function dist2(p: P3, v: Vector3): number { return (p.x - v.x) ** 2 + (p.z - v.z) ** 2; }
