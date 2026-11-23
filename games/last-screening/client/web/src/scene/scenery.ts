import { Color3 } from "@babylonjs/core/Maths/math.color";
import { DynamicTexture } from "@babylonjs/core/Materials/Textures/dynamicTexture";
import { Scene } from "@babylonjs/core/scene";
import { StandardMaterial } from "@babylonjs/core/Materials/standardMaterial";
import { Vector3 } from "@babylonjs/core/Maths/math.vector";
import { CreatePlane } from "@babylonjs/core/Meshes/Builders/planeBuilder";
import { CreateTube } from "@babylonjs/core/Meshes/Builders/tubeBuilder";

type Paint = (ctx: CanvasRenderingContext2D, w: number, h: number) => void;

/** Builds a lit, painted scenic flat. Canvas artwork stays sharp when the player inspects it. */
export function flat(
  scene: Scene,
  name: string,
  x: number,
  y: number,
  z: number,
  w: number,
  h: number,
  paint: Paint,
  transparent = false
) {
  const mesh = CreatePlane(
    name,
    { width: w, height: h, sideOrientation: 2 },
    scene
  );
  mesh.position.set(x, y, z);
  const width = Math.min(1536, Math.max(256, Math.round(w * 120)));
  const height = Math.max(100, Math.round((width * h) / w));
  const tex = new DynamicTexture(name, { width, height }, scene, true);
  const ctx = tex.getContext() as unknown as CanvasRenderingContext2D;
  paint(ctx, width, height);
  tex.hasAlpha = transparent;
  tex.update();
  const mat = new StandardMaterial(name, scene);
  mat.diffuseTexture = tex;
  mat.emissiveColor = new Color3(0.47, 0.47, 0.47);
  mat.specularColor = Color3.Black();
  mat.backFaceCulling = false;
  mesh.material = mat;
  mesh.isPickable = false;
  return mesh;
}

/** Deterministic grain, stars and brush marks keep scenery fixed across both roles. */
function random(seed: number) {
  return () => {
    seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0;
    return seed / 4294967296;
  };
}

/** Produces a painted full moon with an engraved halo, craters and a luminous rim. */
export function moon(c: CanvasRenderingContext2D, w: number, h: number) {
  const r = w * 0.38,
    x = w / 2,
    y = h / 2,
    rand = random(9);
  c.strokeStyle = "#c3ae7d";
  c.lineWidth = 1;
  for (let i = 1; i < 4; i++) {
    c.beginPath();
    c.arc(x, y, r + i * w * 0.023, 0, 2 * Math.PI);
    c.stroke();
  }
  c.beginPath();
  c.arc(x, y, r, 0, Math.PI * 2);
  c.fillStyle = "#f6dfaa";
  c.fill();
  c.clip();
  for (let i = 0; i < 150; i++) {
    c.fillStyle = "rgba(143,118,78,.095)";
    c.beginPath();
    c.ellipse(
      rand() * w,
      rand() * h,
      rand() * 9 + 1,
      rand() * 6 + 1,
      -0.6,
      0,
      Math.PI * 2
    );
    c.fill();
  }
}

/** Paints the projected ocean liner with a gilded hull and three smoking funnels. */
export function ship(c: CanvasRenderingContext2D, w: number, h: number) {
  c.scale(w / 320, h / 130);
  c.fillStyle = "#f6d39a";
  c.beginPath();
  c.moveTo(13, 82);
  c.lineTo(299, 74);
  c.lineTo(273, 109);
  c.lineTo(56, 114);
  c.closePath();
  c.fill();
  c.fillStyle = "#5d5148";
  c.fillRect(62, 73, 190, 4);
  c.fillStyle = "#ecd29f";
  c.fillRect(62, 66, 182, 10);
  c.fillRect(90, 54, 131, 12);
  for (let i = 0; i < 3; i++) {
    c.fillStyle = "#c37b49";
    c.fillRect(105 + i * 36, 33, 14, 23);
    c.fillStyle = "#3e4546";
    c.fillRect(105 + i * 36, 31, 14, 5);
  }
  c.fillStyle = "#5e5c50";
  for (let i = 0; i < 17; i++) c.fillRect(68 + i * 10, 68, 5, 4);
  c.strokeStyle = "#edd19a";
  c.lineWidth = 2;
  c.beginPath();
  c.moveTo(43, 81);
  c.lineTo(43, 28);
  c.lineTo(93, 63);
  c.moveTo(256, 76);
  c.lineTo(256, 41);
  c.stroke();
  c.strokeStyle = "#90b4b4";
  for (let i = 0; i < 3; i++) {
    c.beginPath();
    c.moveTo(28 + i * 22, 115 + i * 4);
    c.lineTo(284 - i * 14, 115 + i * 4);
    c.stroke();
  }
}

/** Renders a brass-bordered equipment nameplate with a legible hand-set label. */
export function lettering(title: string, small = ""): Paint {
  return (c, w, h) => {
    c.fillStyle = "#172626";
    c.fillRect(0, 0, w, h);
    c.strokeStyle = "#bd9b5c";
    c.lineWidth = Math.max(2, h * 0.025);
    c.strokeRect(w * 0.018, h * 0.09, w * 0.964, h * 0.82);
    c.textAlign = "center";
    c.textBaseline = "middle";
    c.fillStyle = "#eddbac";
    c.font =
      "bold " +
      Math.min(h * (small ? 0.38 : 0.47), w / (title.length * 0.69)) +
      "px Georgia";
    c.fillText(title, w / 2, h * (small ? 0.42 : 0.51));
    if (small) {
      c.font = Math.min(h * 0.18, w / (small.length * 0.68)) + "px monospace";
      c.fillStyle = "#c9ac79";
      c.fillText(small, w / 2, h * 0.72);
    }
  };
}

/** Builds a rod between its actual endpoints for frame members, fly lines and handrails. */
export function rod(
  scene: Scene,
  name: string,
  a: Vector3,
  b: Vector3,
  diameter: number,
  material: StandardMaterial
) {
  const mesh = CreateTube(
    name,
    { path: [a, b], radius: diameter / 2, tessellation: 10 },
    scene
  );
  mesh.material = material;
  mesh.isPickable = false;
  return mesh;
}
