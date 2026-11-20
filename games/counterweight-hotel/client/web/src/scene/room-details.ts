import { Scene } from "@babylonjs/core/scene";
import { Color3 } from "@babylonjs/core/Maths/math.color";
import { DynamicTexture } from "@babylonjs/core/Materials/Textures/dynamicTexture";
import { StandardMaterial } from "@babylonjs/core/Materials/standardMaterial";
import { MeshBuilder } from "@babylonjs/core/Meshes/meshBuilder";
import { TransformNode } from "@babylonjs/core/Meshes/transformNode";
import { drawHotelDocument } from "../document-art";

/** Interior rear wall plane shared by the floor, doorway and walking controller. */
export const rearWallZ = -6.5;
/** Half the clear width of the avalanche service doorway. */
export const doorwayHalfWidth = 1.10;
/** Width of each moving jamb. The adjoining wall edges are concealed within this width. */
export const doorwayJambWidth = 0.11;
/** Wall opening edge measured from the doorway centerline, halfway into the jamb. */
export const doorwayWallEdge = doorwayHalfWidth + doorwayJambWidth / 2;
/** Depth of the cabin's brass sill centered on the rear wall plane. */
export const doorwaySillDepth = 0.42;

type Joinery = Record<"brass" | "gold" | "dark" | "wood" | "inset" | "paper" | "lamp", StandardMaterial>;
type Suite = "warm" | "cool";

/** Prints a two-colour alpine railway lithograph with a different sky and skyline for each suite. */
function lithograph(ctx: CanvasRenderingContext2D, room: Suite) {
  const warm = room === "warm";
  const ink = warm ? "#522f35" : "#20394a";
  ctx.fillStyle = "#ecdcad";
  ctx.fillRect(0, 0, 768, 960);
  ctx.fillStyle = ink;
  ctx.fillRect(25, 25, 718, 910);
  const sky = ctx.createLinearGradient(0, 48, 0, 704);
  sky.addColorStop(0, warm ? "#c55942" : "#17384b");
  sky.addColorStop(1, warm ? "#f6d89a" : "#a3bdb7");
  ctx.fillStyle = sky;
  ctx.fillRect(42, 42, 684, 679);
  ctx.save();
  ctx.beginPath();
  ctx.rect(42, 42, 684, 679);
  ctx.clip();
  ctx.fillStyle = "#f4e3b1";
  ctx.beginPath();
  ctx.arc(warm ? 491 : 261, 267, warm ? 90 : 61, 0, Math.PI * 2);
  ctx.fill();
  if (!warm) {
    for (let i = 0; i < 73; i++)
      ctx.fillRect(42 + ((i * 293) % 680), 48 + ((i * 113) % 328), i % 3 + 1, i % 3 + 1);
  }
  for (let layer = 0; layer < 3; layer++) {
    ctx.fillStyle = (warm ? ["#c98768", "#97604e", "#513b39"] : ["#809fa4", "#536f77", "#294952"])[layer];
    ctx.beginPath();
    ctx.moveTo(20, 725);
    for (let i = 0; i <= 35; i++) {
      const x = 20 + i * 21;
      const t = i + layer * 3 + (warm ? 0 : 8);
      const y = 435 + layer * 99 + 56 * Math.sin(t * 0.31) + 43 * Math.sin(t * 0.69) + 13 * Math.cos(t * 2);
      ctx.lineTo(x, y);
    }
    ctx.lineTo(755, 725);
    ctx.fill();
  }
  ctx.restore();
  ctx.textAlign = "center";
  ctx.fillStyle = "#f4e3b1";
  ctx.font = "25px Georgia";
  ctx.fillText("H Ô T E L   D U   C O N T R E P O I D S", 384, 784);
  ctx.font = "66px Georgia";
  ctx.fillText(warm ? "ALPENGLOW" : "NOCTURNE", 384, 876);
}

/** Builds the rear enclosure and writing station. Its return value applies the suite's print and leather colour. */
export function createRoomDetails(scene: Scene, m: Joinery) {
  /** Applies an existing joinery finish to a non-interactive rectangular solid. */
  function box(name: string, x: number, y: number, z: number, w: number, h: number, d: number, mat: StandardMaterial) {
    const mesh = MeshBuilder.CreateBox(name, { width: w, height: h, depth: d }, scene);
    mesh.position.set(x, y, z);
    mesh.material = mat;
    mesh.isPickable = false;
    return mesh;
  }
  /** Creates printed stationery or artwork at sufficient resolution for inspection from the desk. */
  function print(name: string, width: number, height: number, draw: (ctx: CanvasRenderingContext2D) => void) {
    const tex = new DynamicTexture(name, { width, height }, scene, true);
    draw(tex.getContext() as CanvasRenderingContext2D);
    tex.update();
    const material = new StandardMaterial(name, scene);
    material.diffuseTexture = tex;
    material.specularColor = Color3.Black();
    material.emissiveColor = new Color3(0.14, 0.14, 0.14);
    return material;
  }
  /** Faces a framed print toward the guest. */
  function face(name: string, x: number, y: number, z: number, w: number, h: number, mat: StandardMaterial) {
    const mesh = MeshBuilder.CreatePlane(name, { width: w, height: h }, scene);
    mesh.position.set(x, y, z);
    mesh.rotation.y = Math.PI;
    mesh.material = mat;
    mesh.isPickable = false;
    return mesh;
  }

  /** Builds a rear wall bay, leaving the service door clear between adjacent bays. */
  function wallBay(x: number, width: number) {
    if (x < 0) {
      box("rear silk", x, 2.38, rearWallZ, width, 2.8, 0.15, m.paper);
    } else {
      // Picture glazing behind the writing desk exposes this moving room
      // through the fixed tower when guests look back from the span.
      for (const dx of [-width / 2, 0, width / 2])
        box("desk window sash", x + dx, 2.36, rearWallZ, 0.08, 2.62, 0.12, m.brass);
      box("desk window transom", x, 3.04, rearWallZ, width, 0.07, 0.12, m.brass);
    }
    box("rear wainscot", x, 0.55, rearWallZ, width, 1.1, 0.18, m.wood);
    for (const [height, depth, mat] of [[0.1, 0.34, m.dark], [1.03, 0.36, m.dark], [1.1, 0.37, m.brass], [3.62, 0.4, m.wood], [3.69, 0.49, m.gold]] as const)
      box("rear moulding", x, height, rearWallZ, width, 0.075, depth, mat);
  }
  // A cold cast-iron radiator carries the night desk's bulletin where an arriving
  // guest can see it. Its printed surface and close-up share one document.
  for (let fin = 0; fin < 12; fin++) {
    box("radiator section", -1.80 + fin * 0.105, 0.50, 3.12, 0.075, 0.70, 0.24, m.dark);
  }
  for (const h of [0.25, 0.76])
    box("radiator header", -1.23, h, 3.16, 1.46, 0.085, 0.12, m.dark);
  box("steam inlet", -0.52, 0.17, 3.16, 0.085, 0.34, 0.085, m.brass);
  const bulletin = print("night desk house bulletin", 960, 640,
    ctx => drawHotelDocument(ctx, "bulletin", {room: "warm"}));
  const notice = MeshBuilder.CreatePlane("house bulletin on radiator", {width: 1.38, height: 0.92}, scene);
  notice.position.set(-1.23, 0.99, 2.96);
  notice.material = bulletin; notice.metadata = "bulletin"; notice.isPickable = true;
  for (const x of [-1.68, -0.78])
    box("brass bulletin clip", x, 1.43, 2.95, 0.085, 0.06, 0.035, m.brass);
  const bayWidth = 4.44 - doorwayWallEdge;
  for (const side of [-1, 1]) {
    wallBay(side * (doorwayWallEdge + bayWidth / 2), bayWidth);
    for (const x of [1.72, 2.77, 3.82])
      box("raised dado panel", side * x, 0.55, -6.39, 0.9, 0.71, 0.07, m.inset);
  }

  const prints = {
    warm: print("Alpenglow lithograph", 768, 960, ctx => lithograph(ctx, "warm")),
    cool: print("Nocturne lithograph", 768, 960, ctx => lithograph(ctx, "cool")),
  };
  const poster = face("suite lithograph", -2.58, 2.34, -6.305, 1.4, 1.75, prints.warm);
  {
    const x = -2.58;
    box("fielded wall panel", x, 2.34, -6.39, 1.66, 1.99, 0.065, m.wood);
    box("panel slip", x, 2.34, -6.347, 1.58, 1.91, 0.035, m.brass);
    box("inset silk", x, 2.34, -6.324, 1.54, 1.87, 0.024, m.paper);
  }
  for (const x of [-3.78, 3.78]) {
    box("sconce plate", x, 2.32, -6.27, 0.19, 0.46, 0.08, m.brass);
    box("sconce bracket", x, 2.25, -6.17, 0.055, 0.065, 0.26, m.gold);
    const globe = MeshBuilder.CreateSphere("opal wall light", {diameter: 0.26, segments: 16}, scene);
    globe.position.set(x, 2.45, -6.06);
    globe.scaling.y = 1.48;
    globe.material = m.lamp;
    globe.isPickable = false;
  }

  const deskStart = scene.meshes.length;
  const desk = new TransformNode("writing desk", scene);
  desk.position.x = 2.71;
  const deskFootprint = { minX: desk.position.x - 1.58, maxX: desk.position.x + 1.58, minZ: rearWallZ, maxZ: -5.62 };
  const leather = new StandardMaterial("inlaid writing leather", scene);
  box("writing desk top", 0, 1.03, -6.04, 3.16, 0.11, 0.84, m.wood);
  box("desk gilt lip", 0, 0.977, -6.04, 3.12, 0.018, 0.82, m.brass);
  box("leather blotter", 0, 1.092, -6.04, 2.92, 0.014, 0.73, leather);
  for (const x of [-1.19, 1.19]) {
    box("desk pedestal", x, 0.5, -6.08, 0.61, 0.95, 0.68, m.wood);
    for (const y of [0.27, 0.54, 0.81]) {
      box("drawer front", x, y, -5.725, 0.53, 0.23, 0.04, m.inset);
      box("brass pull", x, y, -5.688, 0.15, 0.025, 0.06, m.brass);
    }
  }
  box("inkstand tray", -0.95, 1.12, -6.06, 0.47, 0.045, 0.28, m.brass);
  box("cut glass inkwell", -1.03, 1.20, -6.08, 0.15, 0.14, 0.15, m.dark);
  box("inkwell cap", -1.03, 1.28, -6.08, 0.13, 0.035, 0.13, m.gold);
  const pen = box("fountain pen", -0.81, 1.154, -6.02, 0.021, 0.021, 0.23, m.dark);
  pen.rotation.y = 0.22;
  // The desk and accessories share a local frame beside the service doorway.
  for (const mesh of scene.meshes.slice(deskStart)) mesh.parent = desk;
  /** Updates the local suite's desk leather and framed lithograph. */
  function update(suite: Suite) {
    leather.diffuseColor = Color3.FromHexString(suite === "warm" ? "#6a332c" : "#294d51");
    poster.material = prints[suite];
  }
  // Independent finishes let the visible peer desk and lithograph match its wing.
  const peerLeather = leather.clone("opposite writing leather")!;
  const peerLithograph = prints.cool.clone("opposite suite lithograph")!;
  /** Updates the permanent desk leather and lithograph for the visible peer suite. */
  function dressPeer(room: Suite) {
    peerLeather.diffuseColor = Color3.FromHexString(room === "warm" ? "#6a332c" : "#294d51");
    peerLithograph.diffuseTexture = prints[room].diffuseTexture;
  }
  return { deskFootprint, update, dressPeer,
    peerFinishes: new Map([[leather, peerLeather], [prints.warm, peerLithograph]]) };
}
