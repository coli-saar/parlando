/**
 * Sky and daylight: maps the simulated clock hour to sun/moon direction, light
 * colours and intensities, fog, and the sky dome texture (gradient plus stars).
 *
 * Owns: the pure function `skyState(hour)` that every light, fog and lamp
 * setting derives from, and the `Sky` object that draws the dome, the sun and
 * moon discs and keeps them centred on the camera. Callers set the hour through
 * `Sky.update`; the dome texture is redrawn only when the hour moved noticeably
 * so animation stays cheap.
 */
import {
  Color3, DynamicTexture, Mesh, MeshBuilder, Scene, StandardMaterial, Vector3, type AbstractMesh
} from "@babylonjs/core";
import { smoothstep } from "./geography";

/** All lighting parameters for one clock hour. */
export interface SkyState {
  /** Unit vector pointing from the world towards the sun (or moon at night). */
  toLight: Vector3;
  sunUp: number;           // 0 night .. 1 full day
  zenith: Color3;
  horizon: Color3;
  sunColor: Color3;
  sunIntensity: number;
  hemiIntensity: number;
  hemiColor: Color3;
  hemiGround: Color3;
  fogColor: Color3;
  fogDensity: number;
  /** 0 by day, 1 at night: headlights, window glow and lamps. */
  lampLevel: number;
  stars: number;
  /** Sun elevation, -1..1. */
  elevation: number;
}

const mix = (a: Color3, b: Color3, t: number) => new Color3(a.r + (b.r - a.r) * t, a.g + (b.g - a.g) * t, a.b + (b.b - a.b) * t);

/** Computes the sky for a clock hour (0 = midnight; sunrise 6.5, sunset 18). */
export function skyState(hourIn: number): SkyState {
  const hour = ((hourIn % 24) + 24) % 24;
  const a = (Math.PI * (hour - 6.5)) / 11.5;
  const elev = hour >= 6.5 && hour <= 18 ? Math.sin(a) : -0.35 * Math.abs(Math.sin(((hour < 6.5 ? hour + 24 : hour) - 18) / 12.5 * Math.PI));
  const day = smoothstep(-0.04, 0.3, elev);
  const dawn = Math.exp(-Math.pow(elev / 0.2, 2)) * (1 - day * 0.4);
  const zenith = mix(new Color3(0.02, 0.04, 0.09), new Color3(0.4, 0.62, 0.85), day);
  let horizon = mix(new Color3(0.043, 0.086, 0.149), new Color3(0.81, 0.89, 0.93), day);
  horizon = mix(horizon, new Color3(0.86, 0.72, 0.61), dawn * 0.85);
  const sunUp = elev > -0.04 ? 1 : 0;
  // Sun azimuth sweeps east - south - west; at night the "light" is the moon on the opposite side.
  const sa = hour >= 6.5 && hour <= 18 ? a : a + Math.PI;
  const height = sunUp ? Math.max(0.02, elev) : Math.max(0.15, -elev + 0.3);
  const toLight = new Vector3(Math.cos(sa), height * 1.1, -0.45).normalize();
  const sunColor = sunUp ? mix(new Color3(1, 0.85, 0.66), new Color3(1, 0.89, 0.72), smoothstep(0, 0.5, elev)) : new Color3(0.55, 0.64, 0.88);
  return {
    toLight, sunUp: day, zenith, horizon, sunColor, elevation: elev,
    sunIntensity: sunUp ? 0.4 + 0.7 * day : 0.7,
    hemiIntensity: 0.5 + 0.3 * day + 0.25 * (1 - day),
    hemiColor: mix(new Color3(0.34, 0.42, 0.64), new Color3(0.86, 0.92, 0.96), day),
    hemiGround: mix(new Color3(0.1, 0.11, 0.17), new Color3(0.44, 0.42, 0.33), day),
    fogColor: mix(horizon, zenith, 0.15),
    fogDensity: 0.00045 + (1 - day) * 0.0004 + dawn * 0.0003,
    lampLevel: 1 - smoothstep(-0.02, 0.28, elev),
    stars: 1 - smoothstep(-0.1, 0.12, elev)
  };
}

/** The dome, sun disc and moon disc around the camera. */
export class Sky {
  readonly dome: Mesh;
  private readonly sun: Mesh;
  private readonly moon: Mesh;
  private readonly texture: DynamicTexture;
  private lastDrawn = -99;
  private readonly stars: { x: number; y: number; r: number }[] = [];
  state: SkyState = skyState(12);

  constructor(private readonly scene: Scene) {
    this.texture = new DynamicTexture("sky-tex", { width: 512, height: 256 }, scene, false);
    const mat = new StandardMaterial("sky-mat", scene);
    mat.emissiveTexture = this.texture;
    mat.disableLighting = true;
    mat.backFaceCulling = false;
    mat.fogEnabled = false;
    this.dome = MeshBuilder.CreateSphere("sky", { diameter: 9000, segments: 24, sideOrientation: Mesh.BACKSIDE }, scene);
    this.dome.material = mat;
    this.dome.infiniteDistance = true;
    this.dome.isPickable = false;
    this.dome.renderingGroupId = 0;
    for (let i = 0; i < 260; i++) this.stars.push({ x: Math.random() * 512, y: Math.random() * 120, r: Math.random() * 1.1 + 0.3 });
    this.sun = this.disc("sun", 1, new Color3(1, 0.93, 0.7), 420);
    this.moon = this.disc("moon", 1, new Color3(0.85, 0.9, 1), 260);
  }

  /** A flat emissive disc facing the camera. */
  private disc(name: string, _s: number, color: Color3, size: number): Mesh {
    const d = MeshBuilder.CreateDisc(name, { radius: size / 2, tessellation: 32 }, this.scene);
    const m = new StandardMaterial(`${name}-mat`, this.scene);
    m.emissiveColor = color; m.disableLighting = true; m.fogEnabled = false; m.backFaceCulling = false;
    d.material = m; d.billboardMode = Mesh.BILLBOARDMODE_ALL; d.isPickable = false;
    return d;
  }

  /** Meshes that must not be lit or cast shadows (for callers that enumerate the scene). */
  get meshes(): AbstractMesh[] { return [this.dome, this.sun, this.moon]; }

  /** Applies the clock hour and recentres the discs on the camera position. */
  update(hour: number, camera: Vector3): SkyState {
    const s = skyState(hour);
    this.state = s;
    if (Math.abs(hour - this.lastDrawn) > 0.15) { this.draw(s); this.lastDrawn = hour; }
    const place = (disc: Mesh, dir: Vector3, up: boolean) => {
      disc.position.copyFrom(camera).addInPlace(dir.scale(3800));
      disc.setEnabled(up);
    };
    place(this.sun, s.toLight, s.elevation > -0.05);
    place(this.moon, s.toLight, s.elevation <= -0.05);
    return s;
  }

  /** Redraws the gradient and star field into the dome texture. */
  private draw(s: SkyState): void {
    const g = this.texture.getContext();
    const grad = g.createLinearGradient(0, 0, 0, 256);
    const css = (c: Color3) => `rgb(${Math.round(c.r * 255)},${Math.round(c.g * 255)},${Math.round(c.b * 255)})`;
    grad.addColorStop(0, css(s.zenith));
    grad.addColorStop(0.45, css(mix(s.zenith, s.horizon, 0.7)));
    grad.addColorStop(0.5, css(s.horizon));
    grad.addColorStop(0.55, css(mix(s.horizon, new Color3(0.1, 0.1, 0.1), 0.6)));
    grad.addColorStop(1, css(mix(s.horizon, new Color3(0.1, 0.1, 0.1), 0.6)));
    g.fillStyle = grad;
    g.fillRect(0, 0, 512, 256);
    if (s.stars > 0.02) {
      g.fillStyle = `rgba(255,255,245,${(0.85 * s.stars).toFixed(2)})`;
      for (const st of this.stars) { g.beginPath(); g.arc(st.x, st.y, st.r, 0, Math.PI * 2); g.fill(); }
    }
    this.texture.update();
  }
}
