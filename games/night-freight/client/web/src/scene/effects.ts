/**
 * Cargo effects: the loading gantry with falling crates that stands over a car
 * while it is loaded (or unloaded for a delivery).
 *
 * Owns a small pool of gantries (two posts, a beam and a hopper in the cargo
 * colour) and the crates that fall from the hopper into the car, or rise out of
 * it when a delivery empties it. Callers pass, every frame, the cars being worked
 * on with their pose and progress; the effect moves its crates with wall-clock
 * time so they keep falling even while the simulated clock runs slowly.
 * Contract: `update` with an empty list hides everything; effects hold no game state.
 */
import { Color3, Mesh, MeshBuilder, Scene, StandardMaterial, TransformNode } from "@babylonjs/core";
import type { Commodity } from "../game";
import type { P3 } from "./geography";
import { CARGO_COLORS } from "./stock";

const CRATES = 9;
const STEEL = new Color3(0.36, 0.38, 0.4);

/** One car being worked on. */
export interface CargoWork {
  pos: P3;
  yaw: number;
  commodity: Commodity;
  /** "load" drops crates into the car, "unload" lifts them out. */
  mode: "load" | "unload";
}

interface Gantry { root: TransformNode; hopper: Mesh; crates: Mesh[] }

/** Pool of gantries for the cars being loaded or unloaded. */
export class CargoEffects {
  private readonly pool: Gantry[] = [];
  private readonly steel: StandardMaterial;
  private readonly crateMats = new Map<Commodity, StandardMaterial>();

  constructor(private readonly scene: Scene) {
    this.steel = new StandardMaterial("gantry-steel", scene);
    this.steel.diffuseColor = STEEL; this.steel.specularColor = Color3.Black();
    this.steel.emissiveColor = new Color3(0.06, 0.06, 0.07);
  }

  /** Material of a crate in the cargo colour, glowing a little so it reads at night. */
  private crateMat(c: Commodity): StandardMaterial {
    let m = this.crateMats.get(c);
    if (!m) {
      m = new StandardMaterial(`crate-${c}`, this.scene);
      m.diffuseColor = CARGO_COLORS[c]; m.emissiveColor = CARGO_COLORS[c].scale(0.55); m.specularColor = Color3.Black();
      this.crateMats.set(c, m);
    }
    return m;
  }

  /** Builds one gantry: posts and beam straddling the car, a hopper above its middle, and a set of crates. */
  private build(): Gantry {
    const root = new TransformNode("gantry", this.scene);
    const part = (m: Mesh, x: number, y: number, z: number) => { m.parent = root; m.position.set(x, y, z); m.material = this.steel; m.isPickable = false; return m; };
    for (const x of [-3, 3]) for (const z of [-2.2, 2.2]) part(MeshBuilder.CreateBox("post", { width: 0.3, height: 8, depth: 0.3 }, this.scene), x, 4, z);
    part(MeshBuilder.CreateBox("beam", { width: 6.6, height: 0.5, depth: 5 }, this.scene), 0, 8.1, 0);
    const hopper = MeshBuilder.CreateCylinder("hopper", { diameterTop: 3, diameterBottom: 0.9, height: 1.8, tessellation: 14 }, this.scene);
    hopper.parent = root; hopper.position.set(0, 6.9, 0); hopper.isPickable = false;
    const crates = Array.from({ length: CRATES }, () => {
      const c = MeshBuilder.CreateBox("crate", { width: 0.7, height: 0.55, depth: 0.7 }, this.scene);
      c.parent = root; c.isPickable = false; return c;
    });
    root.setEnabled(false);
    return { root, hopper, crates };
  }

  /** Shows a gantry over every listed car and animates its crates; hides unused gantries. */
  update(work: CargoWork[], seconds: number): void {
    while (this.pool.length < work.length) this.pool.push(this.build());
    this.pool.forEach((g, i) => {
      const w = work[i];
      g.root.setEnabled(!!w);
      if (!w) return;
      g.root.position.set(w.pos.x, w.pos.y + 0.5, w.pos.z);
      g.root.rotation.y = w.yaw;
      const mat = this.crateMat(w.commodity);
      g.hopper.material = mat;
      g.crates.forEach((c, k) => {
        c.material = mat;
        const phase = (seconds * 1.1 + k / CRATES) % 1;
        const f = w.mode === "load" ? phase : 1 - phase;
        // Crates leave the hopper mouth and drop to the car's roof; unloading runs the same path backwards.
        c.position.set(Math.sin(k * 2.3) * 0.5, 6.0 - f * 1.7, Math.cos(k * 1.7) * 2.2 * f);
        c.rotation.y = k;
        c.scaling.setAll(0.55 + 0.45 * Math.sin(Math.PI * Math.min(1, phase * 1.15)));
      });
    });
  }
}
