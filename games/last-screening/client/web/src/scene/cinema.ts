import "@babylonjs/core/Culling/ray";
import { Color3, Color4 } from "@babylonjs/core/Maths/math.color";
import { DynamicTexture } from "@babylonjs/core/Materials/Textures/dynamicTexture";
import { Engine } from "@babylonjs/core/Engines/engine";
import { FreeCamera } from "@babylonjs/core/Cameras/freeCamera";
import { HemisphericLight } from "@babylonjs/core/Lights/hemisphericLight";
import { PointLight } from "@babylonjs/core/Lights/pointLight";
import { Quaternion, Vector3 } from "@babylonjs/core/Maths/math.vector";
import { Scene } from "@babylonjs/core/scene";
import { StandardMaterial } from "@babylonjs/core/Materials/standardMaterial";
import { Texture } from "@babylonjs/core/Materials/Textures/texture";
import { DirectionalLight } from "@babylonjs/core/Lights/directionalLight";
import { ShadowGenerator } from "@babylonjs/core/Lights/Shadows/shadowGenerator";
import { CreateBox } from "@babylonjs/core/Meshes/Builders/boxBuilder";
import { CreateCylinder } from "@babylonjs/core/Meshes/Builders/cylinderBuilder";
import { CreatePlane } from "@babylonjs/core/Meshes/Builders/planeBuilder";
import { CreateTorus } from "@babylonjs/core/Meshes/Builders/torusBuilder";
import { CreateSphere } from "@babylonjs/core/Meshes/Builders/sphereBuilder";
import { flat, lettering, moon, rod, ship } from "./scenery";

export type CinemaRole = "projectionist" | "stagehand";
/** Public stage picture plus the operator's own equipment, as reported by the server. */
export interface CinemaState {
  level: string;
  phase: string;
  screen: {
    window: number;
    tower: number;
    moon: number | null;
    light: number | null;
    ship: number | null;
    lit: boolean;
  };
  lanterns?: ReadonlyArray<{ id: number; aim: number; slide: string }> | null;
}

/** Mounts one first-person auditorium view. Equipment clicks belong to the surrounding participant UI. */
export function mountCinema(
  canvas: HTMLCanvasElement,
  role: CinemaRole,
  onSelect: (station: string) => void
) {
  const engine = new Engine(canvas, true, {
    stencil: true,
    preserveDrawingBuffer: false,
  });
  engine.setHardwareScalingLevel(
    1 / Math.min(window.devicePixelRatio || 1, 1.6)
  );
  const scene = new Scene(engine);
  scene.clearColor = new Color4(0.035, 0.052, 0.059, 1);
  const booth = role === "projectionist",
    v = (x: number, y: number, z: number) => new Vector3(x, y, z);
  const home = booth ? v(0, 5.9, -18.6) : v(-7.9, 3.8, -4.8);
  const homeAim = booth ? v(0, 3.65, 3.4) : v(0.6, 3.65, 7.5);
  const camera = new FreeCamera("operator eyes", home.clone(), scene);
  camera.minZ = 0.09;
  camera.fov = booth ? 0.78 : 0.98;
  camera.setTarget(homeAim);
  const sky = new HemisphericLight(
    "reflected house light",
    v(0.1, 1, -0.4),
    scene
  );
  sky.intensity = 0.47;
  sky.diffuse = Color3.FromHexString("#d0c9bd");
  sky.groundColor = Color3.FromHexString("#483b42");
  /** Creates a shared scenic surface and optionally paints fine grain into the finish. */
  function finish(name: string, color: string, grain = false, glow = 0) {
    const m = new StandardMaterial(name, scene),
      c = Color3.FromHexString(color);
    m.diffuseColor = c;
    m.emissiveColor = c.scale(glow);
    m.specularColor = Color3.FromHexString("#302b24");
    if (grain) {
      const t = new DynamicTexture(name + " grain", 256, scene, false),
        ctx = t.getContext();
      ctx.fillStyle = "#dedbd2";
      ctx.fillRect(0, 0, 256, 256);
      for (let i = 0; i < 2200; i++) {
        ctx.fillStyle = i % 3 ? "rgba(48,34,28,.07)" : "rgba(255,249,225,.21)";
        ctx.fillRect(
          (i * 179) % 257,
          Math.floor(i * 1.618 * 89) % 256,
          3 + (i % 22),
          1
        );
      }
      t.update();
      m.diffuseTexture = t;
    }
    return m;
  }
  const burgundy = finish("old oxblood velvet", "#742938", true),
    ruby = finish("lit velvet folds", "#9f4050", true),
    wood = finish("walnut boards", "#704a36", true),
    dark = finish("lacquered walnut", "#312827", true),
    gold = finish("aged gilt", "#b1874d", true, 0.12),
    edge = finish("polished brass", "#dfbc78", false, 0.28),
    iron = finish("blackened iron", "#293332", true),
    teal = finish("patinated enamel", "#48685e", true),
    globe = finish("tungsten glass", "#ffcf83", false, 1),
    black = finish("stage masking", "#141f24"),
    wall = finish("aged plaster", "#645548", true),
    upholstery = finish("seat mohair", "#672d3d", true);
  /** Registers an individual mesh as part of an operable station. */
  function pick<T extends { isPickable: boolean; metadata: any }>(
    m: T,
    id?: string
  ) {
    m.isPickable = !!id;
    if (id) m.metadata = { station: id };
    return m;
  }
  /** Places a rectangular scenic or architectural member in auditorium coordinates. */
  function box(
    name: string,
    x: number,
    y: number,
    z: number,
    w: number,
    h: number,
    d: number,
    m: StandardMaterial,
    id?: string
  ) {
    const b = CreateBox(name, { width: w, height: h, depth: d }, scene);
    b.position.set(x, y, z);
    b.material = m;
    return pick(b, id);
  }
  /** Places a turned circular component; its axis starts vertical. */
  function drum(
    name: string,
    x: number,
    y: number,
    z: number,
    diameter: number,
    h: number,
    m: StandardMaterial,
    id?: string
  ) {
    const b = CreateCylinder(
      name,
      { diameter, height: h, tessellation: 32 },
      scene
    );
    b.position.set(x, y, z);
    b.material = m;
    return pick(b, id);
  }
  /** Makes luminous practical lamps illuminate the immediate scenery. */
  function lamp(
    x: number,
    y: number,
    z: number,
    intensity: number,
    range: number
  ) {
    const l = new PointLight("amber practical", v(x, y, z), scene);
    l.diffuse = Color3.FromHexString("#ffc78d");
    l.intensity = intensity * 0.24;
    l.range = range;
  }
  /** Mounts a front-facing brass and enamel plate. */
  function plate(
    name: string,
    sub: string,
    x: number,
    y: number,
    z: number,
    w: number,
    h: number,
    id?: string
  ) {
    const m = flat(scene, name, x, y, z, w, h, lettering(name, sub));
    return pick(m, id);
  }
  const key = new DirectionalLight(
    "high amber spill",
    v(0.37, -0.87, 0.34),
    scene
  );
  key.position = v(-5, 12, -9);
  key.diffuse = Color3.FromHexString("#ffd3a0");
  key.intensity = 0.83;
  key.shadowMinZ = 0.5;
  key.shadowMaxZ = 43;
  const shadows = new ShadowGenerator(1024, key);
  shadows.usePercentageCloserFiltering = true;
  shadows.filteringQuality = ShadowGenerator.QUALITY_LOW;
  shadows.bias = 0.001;
  shadows.normalBias = 0.012;
  shadows.setDarkness(0.27);
  const sx = (slot: number) => (slot - 3) * 1.59;
  const backdrop = CreatePlane(
    "painted Bell Rock seascape",
    { width: 17.6, height: 8.1 },
    scene
  );
  backdrop.position.set(0, 4.65, 11.6);
  backdrop.isPickable = false;
  const scenicPaint = new StandardMaterial(
    "hand-painted maritime cyclorama",
    scene
  );
  scenicPaint.diffuseTexture = new Texture(
    new URL("../art/bell-rock.jpg", import.meta.url).href,
    scene
  );
  scenicPaint.emissiveColor = new Color3(0.68, 0.68, 0.68);
  scenicPaint.specularColor = Color3.Black();
  backdrop.material = scenicPaint;
  const disc = flat(scene, "moon slide", 0, 5.42, 10, 1.65, 1.65, moon, true);
  const liner = flat(
    scene,
    "ship slide",
    0,
    3.62,
    9.98,
    1.75,
    0.84,
    ship,
    true
  );
  const light = flat(
    scene,
    "light slide",
    0,
    5.42,
    9.95,
    2.8,
    2.8,
    (c, w, h) => {
      const g = c.createRadialGradient(w / 2, h / 2, 0, w / 2, h / 2, w * 0.5);
      g.addColorStop(0, "#ffdf9e");
      g.addColorStop(0.2, "rgba(251,204,116,.67)");
      g.addColorStop(1, "rgba(250,214,127,0)");
      c.fillStyle = g;
      c.fillRect(0, 0, w, h);
    },
    true
  );
  const aperture = flat(
    scene,
    "traveling sash",
    0,
    4.7,
    9.65,
    2.55,
    3.9,
    (c, w, h) => {
      c.scale(w / 240, h / 360);
      c.fillStyle = "#3c4542";
      c.fillRect(7, 50, 18, 310);
      c.fillRect(215, 50, 18, 310);
      c.strokeStyle = "#3c4542";
      c.lineWidth = 18;
      c.beginPath();
      c.moveTo(16, 58);
      c.quadraticCurveTo(120, -28, 224, 58);
      c.stroke();
      c.strokeStyle = "#e1bd7e";
      c.lineWidth = 3;
      c.beginPath();
      c.moveTo(17, 356);
      c.lineTo(17, 56);
      c.quadraticCurveTo(120, -24, 224, 56);
      c.lineTo(224, 356);
      c.stroke();
      c.fillStyle = "#806e55";
      c.fillRect(15, 202, 210, 7);
      c.fillRect(2, 335, 236, 18);
    },
    true
  );
  const tower = CreatePlane(
    "painted lighthouse flat",
    { width: 2.8, height: 5.6 },
    scene
  );
  tower.position.set(0, 4.35, 9.35);
  const towerPaint = new StandardMaterial("scenic lighthouse paint", scene);
  towerPaint.diffuseTexture = new Texture(
    new URL("../art/lighthouse.webp", import.meta.url).href,
    scene
  );
  towerPaint.diffuseTexture.hasAlpha = true;
  towerPaint.useAlphaFromDiffuseTexture = true;
  towerPaint.transparencyMode = StandardMaterial.MATERIAL_ALPHATESTANDBLEND;
  towerPaint.emissiveColor = new Color3(0.53, 0.53, 0.53);
  towerPaint.specularColor = Color3.Black();
  tower.material = towerPaint;
  tower.isPickable = false;
  // Seven brass marks are shared sightlines for both operators.
  for (let i = 0; i < 7; i++)
    plate(String(i + 1), "", sx(i), 1.08, 5.415, 0.62, 0.4);
  box("raised stage", 0, 0.77, 9, 20, 1.48, 7, wood);
  for (let i = 0; i < 21; i++)
    box("apron fluting", i * 0.94 - 9.4, 0.75, 5.46, 0.035, 1.21, 0.08, gold);
  box("stage lip", 0, 1.46, 5.47, 20, 0.14, 0.15, edge);
  for (let side of [-1, 1]) {
    for (let i = 0; i < 12; i++) {
      const x = side * (6.22 + i * 0.22),
        z = 7.3 + Math.sin(i * 2) * 0.12;
      const fold = drum(
        "hanging velvet fold",
        x,
        5.55,
        z,
        0.45,
        8.1,
        i % 3 === 0 ? ruby : burgundy
      );
      fold.scaling.z = 0.75;
    }
    for (let i = 0; i < 3; i++)
      box(
        "gilt pilaster",
        side * (8.95 + i * 0.25),
        5.35,
        7.16,
        0.15,
        9,
        0.32,
        i % 2 ? edge : gold
      );
    for (let i = 0; i < 4; i++)
      box(
        "proscenium relief",
        side * (8.8 + i * 0.22),
        8.9 + i * 0.17,
        7.04,
        0.42,
        0.95,
        0.42,
        gold
      );
  }
  for (let i = 0; i < 3; i++)
    box(
      "stepped proscenium",
      0,
      8.75 + i * 0.22,
      7.26 - i * 0.04,
      18.2 + i * 0.55,
      0.16,
      0.45,
      i === 1 ? edge : gold
    );
  for (let i = -12; i <= 12; i++) {
    const val = drum(
      "scalloped valance",
      i * 0.55,
      8.55 + Math.abs(Math.sin(i * 0.36)) * 0.25,
      7.65,
      0.71,
      0.9,
      ruby
    );
    val.scaling.z = 0.55;
  }
  plate("THE ORPHEUM", "P I C T U R E   P A L A C E", 0, 9.17, 6.94, 4.4, 0.75);
  for (let i = -8; i <= 8; i++) {
    const foot = drum("footlight", i * 0.82, 1.58, 6.83, 0.22, 0.18, globe);
    foot.scaling.z = 0.8;
  }
  box("auditorium floor", 0, -0.3, -4, 24, 0.4, 27, dark);
  for (let row = 0; row < 6; row++)
    for (let col = -5; col <= 5; col++) {
      if (col === 0) continue;
      const x = col * 1.45,
        z = 3 - row * 2.2,
        y = 0.12 + row * 0.1;
      const seat = CreateSphere(
        "curved mohair seat back",
        { diameter: 1, segments: 12 },
        scene
      );
      seat.position.set(x, y + 0.66, z);
      seat.scaling.set(1.28, 1.2, 0.4);
      seat.rotation.x = -0.17;
      seat.material = upholstery;
      seat.isPickable = false;
      const cushion = CreateSphere(
        "upholstered cushion",
        { diameter: 1, segments: 8 },
        scene
      );
      cushion.position.set(x, y + 0.3, z + 0.34);
      cushion.scaling.set(1.24, 0.21, 0.81);
      cushion.material = upholstery;
      cushion.isPickable = false;
      box("seat pedestal", x, y + 0.13, z + 0.12, 0.32, 0.4, 0.32, iron);
      box("seat arm", x + 0.6, y + 0.56, z + 0.24, 0.09, 0.1, 0.73, wood);
    }
  // Coffers and the dark rear wall close the sightlines above and around the flown set.
  box("auditorium ceiling", 0, 10.8, -4, 23, 0.3, 33, dark);
  box("back of stage", 0, 5.5, 12.4, 23, 11, 0.4, black);
  for (let z = -13; z <= 8; z += 3.7) {
    box("coffer beam", 0, 10.52, z, 22, 0.37, 0.37, wood);
    box("coffer gilt pinstripe", 0, 10.31, z, 22, 0.035, 0.19, gold);
  }
  for (const x of [-7.2, -3.6, 0, 3.6, 7.2])
    box("longitudinal coffer", x, 10.62, -3, 0.19, 0.23, 30, wood);
  const housePanel = new StandardMaterial("stencilled deco wall bay", scene);
  housePanel.diffuseTexture = new Texture(
    new URL("../art/house-panel.jpg", import.meta.url).href,
    scene
  );
  housePanel.emissiveColor = new Color3(0.19, 0.17, 0.15);
  housePanel.specularColor = Color3.Black();
  for (let side of [-1, 1]) {
    for (let z = -14.4; z < 10; z += 4.4) {
      const panel = CreatePlane(
        "embossed wall bay",
        { width: 4.4, height: 8.9 },
        scene
      );
      panel.rotation.y = (side * Math.PI) / 2;
      panel.position.set(side * 10.73, 5.05, z);
      panel.material = housePanel;
      panel.isPickable = false;
    }
    box("house side wall", side * 11, 5, -2, 0.45, 11, 29, wall);
    for (const h of [1.6, 1.73, 6.85, 7.04, 9.92])
      box("wall moulding", side * 10.71, h, -2, 0.16, 0.075, 29, gold);
    for (let z = -15; z < 8; z += 4.4) {
      box("wall pilaster", side * 10.7, 5, z, 0.39, 10, 0.21, gold);
      box("sconce back", side * 10.38, 4.4, z, 0.32, 0.88, 0.36, gold);
      drum("opal sconce", side * 10.17, 4.7, z, 0.39, 0.52, globe);
      for (let j = 0; j < 3; j++) {
        rod(
          scene,
          "sconce sun ray",
          v(side * 10.41, 4.7, z),
          v(
            side * 10.41,
            5.52 + 0.18 * (1 - Math.abs(j - 1)),
            z + (j - 1) * 0.35
          ),
          0.045,
          gold
        );
      }
    }
  }
  const targets: Record<string, Vector3> = {};
  const barrels: Array<ReturnType<typeof box>> = [];
  /** Constructs a period magic lantern, including its reel spokes, barrel rings and numbered badge. */
  function lantern(index: number, x: number) {
    const id = "lantern-" + index,
      y = 4.13,
      z = -15.05;
    box("projection bench", x, y - 0.85, z, 2.45, 0.2, 2, dark);
    box("lantern enamel chassis", x, y, z, 1.12, 0.96, 1.64, teal, id);
    for (let i = 0; i < 6; i++)
      box(
        "cooling louvre",
        x,
        y + 0.26 - i * 0.077,
        z - 0.83,
        0.73,
        0.023,
        0.035,
        iron,
        id
      );
    const barrel = box(
      "lens carriage",
      x,
      y + 0.07,
      z + 0.92,
      0.4,
      0.45,
      0.66,
      iron,
      id
    );
    barrels.push(barrel);
    for (let i = 0; i < 3; i++) {
      const ring = drum(
        "objective collar",
        x,
        y + 0.1,
        z + 1.12 + i * 0.1,
        0.52 - i * 0.055,
        0.1,
        i === 2 ? globe : gold,
        id
      );
      ring.rotation.x = Math.PI / 2;
    }
    for (let j = 0; j < 2; j++) {
      const rz = z - 0.41 + j * 0.89,
        ry = y + 0.94;
      const reel = drum("film magazine", x, ry, rz, 0.89, 0.23, iron, id);
      reel.rotation.z = Math.PI / 2;
      const face = drum(
        "reel flange",
        x + (index === 0 ? 0.14 : -0.14),
        ry,
        rz,
        0.84,
        0.04,
        gold,
        id
      );
      face.rotation.z = Math.PI / 2;
      for (let k = 0; k < 5; k++) {
        const angle = (k * Math.PI * 2) / 5;
        const hole = drum(
          "reel cutout",
          face.position.x + (index === 0 ? 0.026 : -0.026),
          ry + Math.sin(angle) * 0.25,
          rz + Math.cos(angle) * 0.25,
          0.22,
          0.015,
          iron,
          id
        );
        hole.rotation.z = Math.PI / 2;
      }
    }
    plate(
      index === 0 ? "LANTERN I" : "LANTERN II",
      "OPTICAL DEPARTMENT",
      x,
      y - 0.29,
      z - 0.843,
      1.03,
      0.32,
      id
    );
    for (const sign of [-1, 1]) {
      box(
        "rounded access hatch",
        x + sign * 0.575,
        y,
        z,
        0.045,
        0.66,
        0.97,
        iron,
        id
      );
      box("hatch inset", x + sign * 0.605, y, z, 0.024, 0.52, 0.82, teal, id);
      for (let k = -2; k < 3; k++)
        box(
          "ribbed casting",
          x + sign * 0.62,
          y + 0.032 * k,
          z,
          0.028,
          0.014,
          0.66,
          gold,
          id
        );
      for (const zz of [-0.65, 0.65]) {
        const foot = drum(
          "rubber vibration foot",
          x + sign * 0.37,
          y - 0.61,
          z + zz,
          0.28,
          0.27,
          iron,
          id
        );
        foot.scaling.z = 1.25;
      }
    }
    const dial = drum(
      "focus knob",
      x + 0.6,
      y + 0.23,
      z + 0.61,
      0.32,
      0.22,
      gold,
      id
    );
    dial.rotation.z = Math.PI / 2;
    box("motor bedplate", x, y - 0.52, z, 1.32, 0.13, 1.87, iron, id);
    targets[id] = v(x, y + 0.3, z);
  }
  if (booth) {
    box("projection booth floor", 0, 2.65, -17, 14, 0.35, 8, wood);
    for (const x of [-4.91, 4.91])
      box("observation window jamb", x, 6.15, -13.4, 0.2, 7.8, 0.45, dark);
    box("observation sill", 0, 3.39, -13.4, 10, 0.2, 0.52, wood);
    box("observation architrave", 0, 8.7, -13.4, 10.1, 0.26, 0.6, wood);
    for (const x of [-6.25, 6.25])
      box("booth panelling", x, 5.9, -13.4, 2.5, 6.6, 0.5, wall);
    lantern(0, -1.93);
    lantern(1, 1.93);
    lamp(-2, 6, -17, 4, 9);
    lamp(2, 6, -17, 3, 8);
  } else {
    box("stage wing platform", -8.7, 1.33, -1.2, 5.65, 0.36, 12.1, wood);
    for (let i = 0; i < 12; i++)
      box(
        "floorboard joint",
        -11.38 + i * 0.47,
        1.517,
        -1.2,
        0.014,
        0.009,
        12,
        dark
      );
    for (let i = 0; i < 2; i++) {
      const id = i ? "tower" : "window",
        x = -8.4 + i * 1.65,
        z = -0.12;
      box("fly rail pedestal", x, 2.03, z, 1.39, 1.04, 0.85, teal, id);
      plate(
        i ? "TOWER" : "WINDOW",
        "FLY RAIL  /  " + (i ? "02" : "01"),
        x,
        2.16,
        z - 0.438,
        1.14,
        0.36,
        id
      );
      const hub = drum("winch drum", x, 2.82, z, 0.55, 0.63, iron, id);
      hub.rotation.z = Math.PI / 2;
      const wheel = CreateTorus(
        "hand wheel",
        { diameter: 0.78, thickness: 0.073, tessellation: 32 },
        scene
      );
      wheel.material = gold;
      wheel.position.set(x, 2.84, z - 0.33);
      wheel.rotation.x = Math.PI / 2;
      pick(wheel, id);
      for (let j = 0; j < 3; j++) {
        const a = (j * Math.PI) / 3;
        rod(
          scene,
          "wheel spoke",
          v(x - 0.36 * Math.cos(a), 2.84 - 0.36 * Math.sin(a), z - 0.33),
          v(x + 0.36 * Math.cos(a), 2.84 + 0.36 * Math.sin(a), z - 0.33),
          0.04,
          gold
        );
      }
      for (let j = 0; j < 2; j++)
        rod(
          scene,
          "fly cable",
          v(x - 0.28 + j * 0.55, 2.75, z),
          v(x - 0.28 + j * 0.55, 9.8, z),
          0.035,
          gold
        );
      targets[id] = v(x, 2.61, z);
    }
    lamp(-7.4, 4.2, -1, 5, 8);
  }
  // Soft cones follow the objective axes and fade toward the projected image.
  const shafts = [0, 1].map((i) => {
    const shaft = CreateCylinder(
      "lantern light in dust",
      { diameterBottom: 0.13, diameterTop: 1.65, height: 1, tessellation: 32 },
      scene
    );
    const m = finish("transmitted light " + i, "#edce8f", false, 1);
    m.disableLighting = true;
    m.alpha = 0.088;
    m.backFaceCulling = true;
    m.disableDepthWrite = true;
    shaft.material = m;
    shaft.isPickable = false;
    shaft.setEnabled(false);
    return shaft;
  });
  const origins = [v(-1.93, 4.24, -13.71), v(1.93, 4.24, -13.71)];
  /** Directs a translucent light cone from the lens to the active slide position. */
  function project(index: number, mark: number, slide: string) {
    const shaft = shafts[index],
      target = v(sx(mark), slide === "ship" ? 3.62 : 5.42, 9.91);
    const delta = target.subtract(origins[index]);
    shaft.position.copyFrom(origins[index].add(target).scale(0.5));
    shaft.scaling.y = delta.length();
    shaft.rotationQuaternion = Quaternion.FromUnitVectorsToRef(
      Vector3.Up(),
      delta.normalize(),
      Quaternion.Identity()
    );
    shaft.setEnabled(slide !== "empty");
  }
  const dx = booth ? 5.68 : -10.1,
    dy = booth ? 4.42 : 2.92,
    dz = booth ? -13.85 : 1.33;
  box("escape door frame", dx, dy, dz, 1.69, 3.24, 0.3, gold, "exit");
  box("escape door", dx, dy, dz - 0.18, 1.49, 3.02, 0.16, teal, "exit");
  box("door panel", dx, dy + 0.34, dz - 0.29, 1.12, 1.25, 0.06, dark, "exit");
  box("panic bar", dx, dy - 0.28, dz - 0.34, 1.12, 0.11, 0.12, edge, "exit");
  const curtain = flat(
    scene,
    "striped fire curtain",
    dx,
    dy,
    dz - 0.37,
    1.49,
    3.02,
    (c, w, h) => {
      const gradient = c.createLinearGradient(0, 0, w, 0);
      for (let i = 0; i <= 20; i++)
        gradient.addColorStop(i / 20, i % 2 ? "#783840" : "#482634");
      c.fillStyle = gradient;
      c.fillRect(0, 0, w, h);
      c.fillStyle = "#c09a64";
      c.fillRect(0, h * 0.92, w, h * 0.012);
      c.fillStyle = "#d3b693";
      c.textAlign = "center";
      c.font = "bold " + w * 0.087 + "px Georgia";
      c.fillText("SAFETY", w / 2, h * 0.48);
      c.fillText("CURTAIN", w / 2, h * 0.55);
    }
  );
  pick(curtain, "exit");
  let lifted = false;
  const sign = plate("EXIT", "", dx, dy + 1.87, dz - 0.22, 1.35, 0.49, "exit");
  targets.exit = v(dx, dy + 0.2, dz);
  for (const mesh of scene.meshes) {
    if (
      mesh.name.includes("slide") ||
      mesh === disc ||
      mesh === liner ||
      mesh === tower ||
      mesh === light ||
      mesh === aperture ||
      shafts.includes(mesh as typeof shafts[number])
    )
      continue;
    mesh.receiveShadows = true;
    if (
      [
        "curved mohair seat back",
        "seat arm",
        "stage lip",
        "hanging velvet fold",
        "fly rail pedestal",
        "lantern enamel chassis",
        "film magazine",
        "observation window jamb",
        "winch drum",
        "coffer beam",
        "escape door frame",
      ].includes(mesh.name)
    )
      shadows.addShadowCaster(mesh);
  }
  const destination = home.clone(),
    look = homeAim.clone(),
    currentLook = homeAim.clone();
  let station = "home",
    frame: number | undefined;
  /** Frames equipment or restores the wide sightline without teleporting the viewer. */
  function focus(id: string) {
    if (id === "exit" && performance.now() < revealExitAfter) {
      requestedFocus = id;
      return;
    }
    requestedFocus = null;
    station = targets[id] ? id : "home";
    if (station === "home") {
      destination.copyFrom(home);
      look.copyFrom(homeAim);
      return;
    }
    if (station !== "exit") {
      const target = targets[station];
      destination.copyFrom(home);
      destination.x += (target.x - home.x) * 0.35;
      destination.y -= 0.22;
      look.copyFrom(booth ? v(target.x * 0.43, 3.3, 8) : v(-1.5, 3.35, 7));
      return;
    }
    const target = targets[station],
      distance = 4;
    const horizontal = home.subtract(target);
    horizontal.y = 0;
    horizontal.normalize();
    destination.copyFrom(target.add(horizontal.scale(distance)));
    destination.y = target.y + 1.23;
    look.copyFrom(target);
  }
  const tracks = new Map<typeof aperture, number>();
  /** Moves visible scenery between marked positions and places newly exposed slides immediately. */
  function track(mesh: typeof aperture, mark: number | null) {
    if (mark === null) {
      tracks.delete(mesh);
      mesh.setEnabled(false);
      return;
    }
    const location = sx(mark);
    if (!tracks.has(mesh)) mesh.position.x = location;
    mesh.setEnabled(true);
    tracks.set(mesh, location);
  }
  let passage:
    | { from: number; to: number; start: number; duration: number }
    | undefined;
  let shipDestination: number | null = null;
  let shipBehindTower = false;
  let revealExitAfter = 0;
  let requestedFocus: string | null = null;
  /** Carries the ship through the actual occluder, including the otherwise invisible leg at the tower. */
  function sail(state: CinemaState) {
    const { screen, phase } = state;
    const hidden = screen.ship === null && phase === "emerge";
    const mark = screen.ship ?? (hidden ? screen.tower : null);
    if (mark === null) {
      liner.setEnabled(false);
      shipDestination = null;
      passage = undefined;
      return;
    }
    const to = sx(mark);
    if (shipDestination === null) {
      liner.position.x = to;
      liner.visibility = hidden ? 0 : 1;
    } else if (to !== shipDestination || hidden !== shipBehindTower) {
      const distance = Math.abs(to - liner.position.x);
      const duration = 260 + Math.min(1, distance / (6 * 1.59)) * 440;
      passage = {
        from: liner.position.x,
        to,
        start: performance.now(),
        duration,
      };
      if (phase === "escape") revealExitAfter = passage.start + duration + 160;
    }
    liner.setEnabled(true);
    shipDestination = to;
    shipBehindTower = hidden;
  }
  /** Holds the final projected image while the ship crosses the tower's narrow silhouette. */
  function advanceShip(now: number) {
    if (!passage) return;
    const t = Math.min(1, (now - passage.start) / passage.duration);
    const progress = t * t * (3 - 2 * t);
    liner.position.x = passage.from + (passage.to - passage.from) * progress;
    const gap = Math.abs(liner.position.x - tower.position.x);
    liner.visibility =
      t === 1
        ? shipBehindTower
          ? 0
          : 1
        : Math.min(1, Math.max(0, (gap - 0.16) / 0.72));
    if (t === 1) passage = undefined;
  }
  /** Applies the public projection and local machine settings to the physical stage. */
  function update(s: CinemaState) {
    const p = s.screen;
    track(aperture, p.window);
    track(tower, p.tower);
    track(disc, p.moon);
    sail(s);
    track(light, p.light);
    if (s.lanterns) {
      for (const l of s.lanterns) {
        project(l.id, l.aim, l.slide);
        if (barrels[l.id]) barrels[l.id].rotation.y = (l.aim - 3) * 0.065;
      }
    } else {
      // Wing operators see only the rays of slides which reach the stage picture.
      const visible = [
        ["moon", p.moon],
        ["light", p.light],
        ["ship", p.ship],
      ] as const;
      let i = 0;
      for (const [slide, mark] of visible)
        if (mark !== null && i < 2) project(i++, mark, slide);
      while (i < 2) shafts[i++].setEnabled(false);
    }
    lifted = s.phase === "escape";
    sign.visibility = s.phase === "escape" ? 1 : 0.78;
    (aperture.material as StandardMaterial).emissiveColor.setAll(
      p.lit ? 0.95 : 0.4
    );
  }
  /** Resolves a visible component to its owning station. */
  function hit(e: PointerEvent) {
    const rect = canvas.getBoundingClientRect();
    return scene.pick(e.clientX - rect.left, e.clientY - rect.top)?.pickedMesh
      ?.metadata?.station as string | undefined;
  }
  let dragStart: { x: number; y: number } | null = null,
    dragged = false;
  /** Captures the origin of a mouse or single-finger look gesture. */
  function down(e: PointerEvent) {
    dragStart = { x: e.clientX, y: e.clientY };
    dragged = false;
    canvas.setPointerCapture(e.pointerId);
  }
  /** Turns the operator's head within the playable sightline while dragging. */
  function move(e: PointerEvent) {
    if (!dragStart) {
      canvas.style.cursor = hit(e) ? "pointer" : "grab";
      return;
    }
    const dx = e.clientX - dragStart.x,
      dy = e.clientY - dragStart.y;
    if (!dragged && Math.abs(dx) + Math.abs(dy) < 5) return;
    dragged = true;
    canvas.style.cursor = "grabbing";
    dragStart = { x: e.clientX, y: e.clientY };
    const d = look.subtract(destination),
      len = d.length(),
      yaw = Math.atan2(d.x, d.z) - dx * 0.004;
    const pitch = Math.max(
      -1.12,
      Math.min(1.12, Math.asin(d.y / len) + dy * 0.004)
    );
    look.copyFrom(
      destination.add(
        v(
          Math.sin(yaw) * Math.cos(pitch),
          Math.sin(pitch),
          Math.cos(yaw) * Math.cos(pitch)
        ).scale(len)
      )
    );
  }
  /** Opens a clicked machine after distinguishing a tap from looking around. */
  function up(e: PointerEvent) {
    if (dragStart && !dragged) {
      const id = hit(e);
      if (id) onSelect(id);
    }
    cancel();
  }
  /** Clears pointer state when a gesture ends or the browser interrupts it. */
  function cancel() {
    dragStart = null;
    dragged = false;
    canvas.style.cursor = "grab";
  }
  canvas.style.touchAction = "none";
  canvas.addEventListener("pointerdown", down);
  canvas.addEventListener("pointermove", move);
  canvas.addEventListener("pointerup", up);
  canvas.addEventListener("pointercancel", cancel);
  const observer = new ResizeObserver(() => {
    if (frame !== undefined) cancelAnimationFrame(frame);
    frame = requestAnimationFrame(() => {
      engine.resize();
      frame = undefined;
    });
  });
  observer.observe(canvas);
  engine.runRenderLoop(() => {
    const now = performance.now();
    advanceShip(now);
    if (requestedFocus && now >= revealExitAfter) focus(requestedFocus);
    const blend = 1 - Math.exp(-engine.getDeltaTime() * 0.008);
    for (const [mesh, target] of tracks)
      mesh.position.x += (target - mesh.position.x) * blend;
    curtain.position.y +=
      ((lifted ? dy + 2.96 : dy) - curtain.position.y) * blend * 0.47;
    Vector3.LerpToRef(camera.position, destination, blend, camera.position);
    Vector3.LerpToRef(currentLook, look, blend, currentLook);
    camera.setTarget(currentLook);
    scene.render();
  });
  /** Releases WebGL resources and input listeners on participant-view unmount. */
  function dispose() {
    observer.disconnect();
    if (frame !== undefined) cancelAnimationFrame(frame);
    canvas.removeEventListener("pointerdown", down);
    canvas.removeEventListener("pointermove", move);
    canvas.removeEventListener("pointerup", up);
    canvas.removeEventListener("pointercancel", cancel);
    scene.dispose();
    engine.dispose();
  }
  return { update, focus, dispose };
}
