import { applyWingFrame, exchangeLanding, westOrigin } from "./hotel-layout";
import route from "./bridge-route.json";
import { DirectionalLight } from "@babylonjs/core/Lights/directionalLight";
import { ShadowGenerator } from "@babylonjs/core/Lights/Shadows/shadowGenerator";
import "@babylonjs/core/Lights/Shadows/shadowGeneratorSceneComponent";
import type { Scene } from "@babylonjs/core/scene";
import { Color3 } from "@babylonjs/core/Maths/math.color";
import { Vector3 } from "@babylonjs/core/Maths/math.vector";
import { Texture } from "@babylonjs/core/Materials/Textures/texture";
import { DynamicTexture } from "@babylonjs/core/Materials/Textures/dynamicTexture";
import { StandardMaterial } from "@babylonjs/core/Materials/standardMaterial";
import { Mesh } from "@babylonjs/core/Meshes/mesh";
import { MeshBuilder } from "@babylonjs/core/Meshes/meshBuilder";
import { mapExteriorBox, type createExteriorFinishes } from "./exterior-finishes";
import { VertexData } from "@babylonjs/core/Meshes/mesh.vertexData";
import { TransformNode } from "@babylonjs/core/Meshes/transformNode";
import { doorwayHalfWidth, doorwaySillDepth, doorwayWallEdge, rearWallZ } from "./room-details";

/** Joint shared by the hotel's access gallery and the ravine span, in shaft coordinates. */
export const galleryJoint = { x: route.shared[0][0], z: route.shared[0][1] };

/** Builds the fixed terraces and ravine in the hotel shaft’s world frame.
 * The clear deck runs from z=-6.5 to -12.5 between x=±3.5, with the threshold at x=0.
 * Art has no interaction targets. Walking and the cabin's independently closing doors belong
 * to the caller. The shaft contains all five terraces at four-metre intervals. The booking supplies the refuge elevation.
 */
export function createAvalancheLanding(scene: Scene, shaft: TransformNode, bridgeGlass: StandardMaterial, exterior: ReturnType<typeof createExteriorFinishes>) {
  const root = new TransformNode("avalanche terrace", scene);
  root.parent = shaft;
  const refuge = new TransformNode("booked refuge gallery", scene);
  refuge.parent = root;
  // Set the distant gallery obliquely to the terraces so its eaves and side wall read in depth.
  const far = new Vector3(route.shared[1][0], -0.81, route.shared[1][1]);
  const threshold = new Vector3(route.shared[2][0], far.y, route.shared[2][1]);
  // Locate the lodge from its porch lip and door so human and porter walks share every bend.
  refuge.rotation.y = Math.atan2(far.x - threshold.x, far.z - threshold.z);
  const cos = Math.cos(refuge.rotation.y), sin = Math.sin(refuge.rotation.y);
  refuge.position.set(far.x - 9.7 * cos + 26.54 * sin, 0, far.z + 9.7 * sin + 26.54 * cos);
  let elevation = 0;
  const batches = new Map<StandardMaterial, Mesh[]>();
  /** Shares each architectural finish and retains some local colour in outdoor shadow. */
  function finish(name: string, hex: string, glow = 0.08) {
    const m = new StandardMaterial(name, scene);
    m.diffuseColor = Color3.FromHexString(hex);
    m.emissiveColor = m.diffuseColor.scale(glow);
    m.specularColor = new Color3(0.06, 0.07, 0.08);
    return m;
  }
  const limestone = exterior("dressing");
  const mortar = exterior("ashlar");
  const flagstone = exterior("flagstone");
  const wood = exterior("joinery");
  const deckWood = exterior("deck");
  const iron = exterior("steel");
  const gilt = finish("weathered bronze", "#ad8452");
  const snow = exterior("snow");
  const ice = exterior("ice");
  const shale = exterior("scree");
  const light = finish("amber refuge lantern glass", "#ffbf67", 0.86);
  // Light behind the lace curtains falls off toward the upper sash. Use an unlit print
  // so the distant windows retain their amber detail in direct sunlight.
  const windowPrint = new DynamicTexture("refuge lace curtains", {width: 256, height: 256}, scene, true);
  const windowInk = windowPrint.getContext();
  const lamplight = windowInk.createRadialGradient(103, 180, 12, 116, 146, 199);
  lamplight.addColorStop(0, "#ffe2a3");
  lamplight.addColorStop(0.48, "#d3a365");
  lamplight.addColorStop(1, "#705845");
  windowInk.fillStyle = lamplight;
  windowInk.fillRect(0, 0, 256, 256);
  for (const edge of [0, 256]) {
    const direction = edge === 0 ? 1 : -1;
    windowInk.fillStyle = "rgba(70,43,30,0.38)";
    windowInk.beginPath();
    windowInk.moveTo(edge, 0);
    windowInk.lineTo(edge + direction * 72, 0);
    windowInk.quadraticCurveTo(edge + direction * 67, 79, edge + direction * 21, 155);
    windowInk.lineTo(edge + direction * 41, 256);
    windowInk.lineTo(edge, 256);
    windowInk.fill();
  }
  windowPrint.update();
  const litSash = finish("curtained amber windows", "#ffffff", 0);
  litSash.disableLighting = true;
  litSash.emissiveTexture = windowPrint;
  /** Collects pieces in construction coordinates so each finish needs one draw call. */
  function piece(mesh: Mesh, mat: StandardMaterial, x: number, y: number, z: number) {
    mesh.position.set(x, y + elevation, z);
    mesh.material = mat;
    mesh.isPickable = false;
    // Keep the leaf and its hardware independently pickable from the lodge facade.
    if (mesh.name.startsWith("porch door")) {
      mesh.parent = refuge;
      mesh.metadata = "reach_refuge";
      mesh.isPickable = true;
      return mesh;
    }
    if (!batches.has(mat)) batches.set(mat, []);
    batches.get(mat)!.push(mesh);
    return mesh;
  }
  /** Places a dimensioned component in landing coordinates. */
  function box(name: string, x: number, y: number, z: number, width: number, height: number, depth: number, mat: StandardMaterial) {
    const mesh = MeshBuilder.CreateBox(name, {width, height, depth}, scene);
    mapExteriorBox(mesh, mat, x, y + elevation, z, width, height, depth);
    return piece(mesh, mat, x, y, z);
  }
  /** Sweeps a square iron bar between joints, including buckled and detached handrails. */
  function bar(name: string, a: number[], b: number[], size: number, mat: StandardMaterial) {
    const start = new Vector3(a[0], a[1], a[2]);
    const end = new Vector3(b[0], b[1], b[2]);
    const delta = end.subtract(start);
    const mid = start.add(end).scale(0.5);
    const mesh = box(name, mid.x, mid.y, mid.z, size, size, delta.length(), mat);
    mesh.lookAt(end.add(new Vector3(0, elevation, 0)));
  }
  /** Makes an angular bank with a worn convex crown, a visible fracture face and a buried toe. */
  function bank(name: string, x: number, y: number, z: number, rx: number, height: number, rz: number, seed: number, mat: StandardMaterial) {
    const pos: number[] = [], indices: number[] = [], normals: number[] = [];
    const uvs: number[] = [];
    const count = 19;
    const profile = [[1, 0], [0.95, 0.24], [0.82, 0.59], [0.56, 0.88], [0.26, 0.97]];
    const tile = mat === shale ? 4.6 : 3.2;
    for (const [ring, [radius, rise]] of profile.entries()) {
      for (let i = 0; i < count; i++) {
        const angle = 2 * Math.PI * i / count;
        const jitter = 1 + 0.12 * Math.sin(4 * angle + seed) + 0.10 * Math.cos(3 * angle - seed)
          + 0.047 * Math.sin(7 * angle + seed + ring * 0.85);
        const localX = rx * (radius * jitter * Math.cos(angle) + ring * 0.021);
        const localY = height * (rise + 0.045 * Math.sin(angle * 2 + seed) + 0.019 * Math.cos(angle * 5 + seed));
        const localZ = rz * (radius * jitter * Math.sin(angle) + ring * 0.027);
        pos.push(localX, localY, localZ);
        uvs.push((x + localX + 0.40 * localY) / tile, (z + localZ + 0.75 * localY) / tile);
        if (ring > 0) {
          const prev = (ring - 1) * count + i, next = (ring - 1) * count + (i + 1) % count;
          indices.push(prev, next, prev + count, next, next + count, prev + count);
        }
      }
    }
    const crown = pos.length / 3;
    pos.push(rx * 0.10, height, rz * 0.13);
    uvs.push((x + rx * 0.10 + 0.40 * height) / tile, (z + rz * 0.13 + 0.75 * height) / tile);
    const top = (profile.length - 1) * count;
    for (let i = 0; i < count; i++) indices.push(top + i, top + (i + 1) % count, crown);
    const mesh = new Mesh(name, scene);
    const data = new VertexData();
    VertexData.ComputeNormals(pos, indices, normals);
    data.positions = pos; data.indices = indices; data.normals = normals;
    data.uvs = uvs;
    data.applyToMesh(mesh);
    if (mat !== snow) mesh.convertToFlatShadedMesh();
    mat.backFaceCulling = false;
    piece(mesh, mat, x, y, z);
  }
  const signMaterials = new Map<string, StandardMaterial>();
  /** Paints enamel signs in world space, legible from the central walkable deck. */
  function sign(title: string, subtitle: string, x: number, y: number, z: number, width: number, yaw = Math.PI) {
    let m = signMaterials.get(title + subtitle);
    if (!m) {
      const tex = new DynamicTexture(title, {width: 1024, height: 256}, scene, true);
      const ctx = tex.getContext();
      ctx.fillStyle = "#24433f"; ctx.fillRect(0, 0, 1024, 256);
      ctx.strokeStyle = "#cea669"; ctx.lineWidth = 6; ctx.strokeRect(12, 12, 1000, 232);
      ctx.fillStyle = "#f2e6c5";
      ctx.font = "bold 74px Georgia"; ctx.fillText(title, (1024 - ctx.measureText(title).width) / 2, 114);
      ctx.font = "38px Georgia"; ctx.fillText(subtitle, (1024 - ctx.measureText(subtitle).width) / 2, 195);
      tex.update();
      m = finish(title, "#ffffff");
      m.diffuseTexture = tex;
      m.emissiveTexture = tex;
      m.emissiveColor = new Color3(0.37, 0.36, 0.32);
      signMaterials.set(title + subtitle, m);
    }
    const board = piece(MeshBuilder.CreatePlane(title, {width, height: width / 4}, scene), m, x, y, z);
    board.rotation.y = yaw;
  }

  const stairFloor = 2;
  const terraceNames = ["SERVICE COURT", "LOWER BALCONY", "STAIR TERRACE", "PROMENADE", "ROOF TERRACE"];

  /** Gives each intact deck its own silhouette at the edge of the shared walking area. */
  function terraceEdge(floor: number) {
    if (floor === 0 || floor === 4) {
      const height = floor === 0 ? 0.93 : 0.22;
      box("stone parapet", 0, height / 2, -12.40, 6.88, height, 0.22, mortar);
      box("parapet coping", 0, height + 0.04, -12.40, 7, 0.12, 0.28, limestone);
      for (const x of [-3.23, 3.23]) {
        box("parapet pier", x, 0.54, -12.40, 0.42, 1.08, 0.30, limestone);
        if (floor === 4) box("snow on roof pier", x, 1.11, -12.40, 0.43, 0.07, 0.31, snow);
      }
      if (floor === 4) {
        box("rooftop guardrail", 0, 1.02, -12.40, 6.88, 0.065, 0.075, iron);
        for (let i = -7; i <= 7; i++)
          box("roof rail spindle", i * 0.41, 0.65, -12.40, 0.04, 0.75, 0.045, iron);
      }
      return;
    }
    for (const y of [0.46, 1.03]) box("continuous balcony rail", 0, y, -12.40, 6.88, 0.065, 0.075, iron);
    for (let i = -8; i <= 8; i++) {
      const x = i * 0.39;
      if (floor === 1) {
        box("balcony spindle", x, 0.54, -12.40, 0.04, 1.03, 0.05, iron);
      } else {
        bar("promenade lattice", [x - 0.18, 0.47, -12.40], [x + 0.18, 1.03, -12.40], 0.042, iron);
        bar("promenade lattice", [x + 0.18, 0.47, -12.40], [x - 0.18, 1.03, -12.40], 0.042, iron);
      }
    }
    if (floor === 3) {
      for (const x of [-3.26, 3.26]) {
        box("promenade lantern post", x, 0.92, -12.40, 0.11, 1.84, 0.11, iron);
        box("promenade lantern", x, 1.97, -12.40, 0.24, 0.28, 0.24, light);
        box("promenade lantern hood", x, 2.14, -12.40, 0.32, 0.08, 0.32, iron);
      }
    }
  }

  const windowGlass = finish("smoked blue hotel glazing", "#697f89", 0.19);
  windowGlass.specularColor = new Color3(0.38, 0.43, 0.47);
  // Each shaft encloses exactly one full-size saloon. The valley-facing glazing
  // extends to the rails, and the central exchange and gallery corridor stays open.
  const facadeOuterEdge = 8.3;
  const wingShell = new TransformNode("East shaft and service terraces", scene);
  wingShell.parent = root;
  const shaftGlass = finish("clear rolled shaft glass", "#97beca", 0.12);
  shaftGlass.alpha = 0.035;
  shaftGlass.backFaceCulling = false;
  const rear = -6.92, front = 3.72;
  for (const [name, y, height, material] of [
    ["shaft bedrock plinth", -17.2, 34, mortar], ["shaft roof", 20.32, 0.37, mortar],
    ["snow on shaft roof", 20.54, 0.12, snow],
  ] as const) box(name, -1.79, y, (rear + front) / 2, 13.07, height, front - rear, material);
  for (const x of [-4.57, 4.6]) {
    for (const z of [rear + 0.19, front - 0.09])
      box("lift guide rail", x, 10, z, 0.115, 20.3, 0.15, iron);
    for (let floor = 0; floor < 5; floor++) {
      box("shaft sash", x, floor * 4, -1.58, 0.12, 0.11, 10.5, iron);
      // The negative side has an open baggage dock beside the front window.
      const end = x < 0 ? 0.3 : front;
      box("shaft observation glass", x, floor * 4 + 1.98, (rear + end) / 2,
        0.026, 3.84, end - rear, shaftGlass);
    }
  }
  // The fixed baggage department occupies the side wing. A lined recess frames the
  // actual racks at x=-5.35, while the porter well at z=-0.70 runs behind the counter.
  for (let floor = 0; floor < 5; floor++) {
    const y = floor * 4;
    for (const [z, depth] of [[-4.16, 5.52], [2.11, 3.22]])
      box("baggage room floor", -6.46, y - 0.13, z, 3.65, 0.20, depth, flagstone);
    box("baggage room back panel", -6.15, y + 1.89, 2.24, 0.16, 3.78, 2.92, wood);
    for (const z of [1.10, 2.22, 3.34])
      box("porter counter recessed panel", -6.05, y + 1.48, z, 0.07, 2.62, 0.89, deckWood);
    for (const h of [0.23, 2.91])
      box("porter counter brass moulding", -6.00, y + h, 2.22, 0.045, 0.055, 2.85, gilt);
    box("baggage counter lamp hood", -5.94, y + 3.24, 2.22, 0.41, 0.09, 1.30, iron);
    box("baggage counter opal strip", -5.94, y + 3.17, 2.22, 0.31, 0.08, 1.12, light);
  }
  flush(wingShell);
  // The masonry service wing ends behind the baggage equipment, leaving each
  // observation window open to the mountain and to its opposite number.
  box("service wing return", -8.21, 9.96, (rear + front) / 2, 0.26, 20.08, front - rear, mortar);
  box("service wing end", -6.44, 9.96, front - 0.14, 3.68, 20.08, 0.25, mortar);
  for (let floor = 0; floor < 5; floor++) {
    const y = 4 * floor;
    // A stone surround defines the East observation window and the adjoining
    // escape portal. The entire span above the saloon dado remains clear.
    for (const [x, width] of [[-4.44, 0.32], [0.64, 0.32], [4.42, 0.32]])
      box("East hotel front pilaster", x, y + 2, front + 0.02, width, 4, 0.23, mortar);
    box("East observation spandrel", -1.90, y + 0.22, front + 0.02, 4.80, 0.44, 0.23, mortar);
    // Keep the fixed masonry below the underside of the next floor’s docking plate.
    box("East front stringcourse", 0, y + 3.76, front + 0.04, 9.22, 0.20, 0.31, limestone);
    for (const z of [-4.85, -1.65, 1.55]) {
      box("service wing sash surround", -8.37, y + 2, z, 0.11, 1.94, 1.4, limestone);
      box("service wing casement", -8.44, y + 2, z, 0.045, 1.64, 1.12, windowGlass);
      box("service wing mullion", -8.48, y + 2, z, 0.045, 1.66, 0.06, wood);
    }
    box("East service room window casing", -6.46, y + 1.98, front + 0.02, 1.68, 2.17, 0.16, limestone);
    box("East service room curtained sash", -6.46, y + 1.98, front + 0.13, 1.40, 1.92, 0.07, litSash);
    box("East service room sash rail", -6.46, y + 2.24, front + 0.18, 1.43, 0.075, 0.06, wood);
    box("service wing cornice", -8.32, y + 0.16, (rear + front) / 2, 0.26, 0.18, front - rear, limestone);
  }
  flush(root);
  // The West pavilion surrounds the baggage bay and full travel envelope. Its central
  // windows expose the saloon, while the inboard piers leave the docking passage clear.
  const pavilion = new TransformNode("West hotel pavilion", scene);
  pavilion.parent = root;
  applyWingFrame(pavilion, true);
  for (let floor = 0; floor < 5; floor++) {
    const y = floor * 4;
    const x = 4.56;
    for (const [z, width] of [[-6.15, 1.54], [-1.8, 0.5], [3.10, 1.24]]) {
      box("pavilion masonry pier", x, y + 2, z, 0.36, 4, width, mortar);
    }
    box("pavilion masonry apron", x, y + 0.30, -1.60, 0.36, 0.60, 10.64, mortar);
    box("pavilion lintel", x, y + 3.74, -1.60, 0.36, 0.52, 10.64, limestone);
    box("projecting sill course", x + 0.10, y + 0.64, -1.60, 0.48, 0.12, 10.64, limestone);
    for (const [z, width] of [[-3.70, 3.35], [0.46, 4.03]]) {
      box("glazed observation bay", x, y + 2.02, z, 0.036, 2.82, width, shaftGlass);
      box("pavilion sash transom", x + 0.09, y + 2.93, z, 0.12, 0.08, width, wood);
      box("pavilion window muntin", x + 0.09, y + 2.03, z, 0.12, 2.82, 0.07, wood);
    }
    // Residential rooms fill the unused return beyond the counter. Their curtained
    // casements and belt courses continue around the bridge-facing street corner.
    box("hotel residential return", -8.13, y + 2, -1.60, 0.38, 4, 10.64, mortar);
    for (const z of [-5.15, -1.60, 1.95]) {
      box("residential window stone architrave", -8.36, y + 2.04, z, 0.16, 2.32, 1.62, limestone);
      box("residential window walnut lining", -8.46, y + 2.04, z, 0.09, 2.04, 1.38, wood);
      box("residential curtained sash", -8.52, y + 2.04, z, 0.055, 1.88, 1.23, litSash);
      box("residential meeting rail", -8.56, y + 2.31, z, 0.06, 0.075, 1.26, wood);
      box("residential stone window sill", -8.43, y + 0.92, z, 0.51, 0.12, 1.76, limestone);
    }
    box("pavilion front belt course", -6.32, y + 0.17, 3.89, 3.98, 0.22, 0.22, limestone);
    // The inner elevation frames the picture window and the open exchange gate separately.
    for (const [x, width] of [[-6.32, 3.98], [0.66, 0.52], [4.25, 0.77]]) {
      box("pavilion inner pier", x, y + 2, 3.73, width, 4, 0.25, mortar);
    }
    box("pavilion bay sill", -1.97, y + 0.36, 3.73, 4.75, 0.72, 0.26, mortar);
    box("pavilion inner lintel", -1.81, y + 3.66, 3.72, 12.70, 0.40, 0.27, limestone);
    box("guest room window frame", -6.35, y + 1.95, 3.90, 1.47, 2.04, 0.14, wood);
    box("guest room sash", -6.35, y + 1.95, 3.99, 1.25, 1.81, 0.06, litSash);
    box("guest room meeting rail", -6.35, y + 2.11, 4.04, 1.28, 0.075, 0.05, wood);
    box("guest room mullion", -6.35, y + 1.95, 4.04, 0.075, 1.84, 0.05, wood);
    // The terrace facade ends at the door surround. Turn its masonry back into the
    // pavilion side wall, keeping a recessed sash opposite the moving writing desk.
    const start = doorwayWallEdge + 0.01 + 0.2;
    const end = x; // centre of the pavilion side wall
    const mid = (start + end) / 2, span = end - start;
    const windowWidth = 1.42, jamb = (span - windowWidth) / 2;
    for (const j of [start + jamb / 2, end - jamb / 2])
      box("West terrace window jamb", j, y + 2, -6.68, jamb, 4, 0.31, mortar);
    box("West terrace apron", mid, y + 0.60, -6.68, windowWidth, 1.20, 0.31, mortar);
    box("West terrace window head", mid, y + 3.55, -6.68, windowWidth, 0.90, 0.31, mortar);
    box("West desk window", mid, y + 2.15, -6.68, windowWidth, 1.90, 0.034, shaftGlass);
    box("West desk mullion", mid, y + 2.15, -6.72, 0.07, 1.90, 0.085, wood);
    box("West desk transom", mid, y + 2.64, -6.72, windowWidth, 0.07, 0.085, wood);
    box("West desk sill", mid, y + 1.18, -6.72, windowWidth + 0.20, 0.11, 0.42, limestone);
    box("West terrace belt course", mid, y + 3.38, -6.80, span, 0.16, 0.40, wood);
    for (const [pierX, width] of [[-7.8, 0.67], [-4.65, 0.62]])
      box("pavilion rear pier", pierX, y + 2, rear, width, 4, 0.26, mortar);
  }
  // Snow-covered gable over the residential bays stops before the sheave machinery.
  for (let row = 0; row < 12; row++) {
    const halfDepth = 5.36 * (1 - row / 12);
    box("timber attic", -3.96, 20.67 + row * 0.19, -1.60, 8.80, 0.20, halfDepth * 2, wood);
  }
  for (const side of [-1, 1]) {
    const roof = box("pavilion snowy pitched roof", -3.96, 21.72, -1.6 + side * 2.78,
      9.32, 0.22, 6.08, snow);
    roof.rotation.x = side * Math.atan2(2.35, 5.56);
  }
  box("pavilion chimney", -5.8, 22.54, -2.7, 0.72, 2.4, 0.66, mortar);
  box("pavilion chimney cap", -5.8, 23.77, -2.7, 0.91, 0.15, 0.84, snow);
  flush(pavilion);

  // A permanent enclosed range ties the fixed shaft houses together beside the
  // exchange landings. Its courtyard wall sits outside the two cabin sweeps.
  const linkWest = -1.0, linkEast = exchangeLanding.left - 0.20;
  const linkX = (linkWest + linkEast) / 2, linkWidth = linkEast - linkWest;
  const linkZ = westOrigin.z / 2, linkDepth = westOrigin.z - 2 * front;
  for (let floor = 0; floor < 5; floor++) {
    const y = floor * 4;
    box("connecting gallery floor", linkX, y - 0.12, linkZ, linkWidth, 0.24, linkDepth, limestone);
    box("connecting range outer wall", linkWest + 0.11, y + 2, linkZ, 0.22, 4, linkDepth, mortar);
    box("connecting gallery dado", linkEast - 0.11, y + 0.68, linkZ, 0.22, 1.36, linkDepth, mortar);
    box("connecting gallery lintel", linkEast - 0.11, y + 3.62, linkZ, 0.22, 0.76, linkDepth, mortar);
    for (const z of [front + 0.18, westOrigin.z - front - 0.18])
      box("connecting gallery pier", linkEast - 0.11, y + 2.30, z, 0.22, 1.88, 0.36, mortar);
    box("connecting gallery sash", linkEast - 0.12, y + 2.30, linkZ, 0.06, 1.88, linkDepth - 0.72, litSash);
    box("connecting gallery window rail", linkEast - 0.06, y + 2.62, linkZ, 0.07, 0.07, linkDepth - 0.72, wood);
  }
  box("connecting range foundation", linkX, -17.12, linkZ, linkWidth, 34, linkDepth, mortar);
  box("connecting range roof", linkX, 20.25, linkZ, linkWidth + 0.12, 0.50, linkDepth + 0.12, limestone);
  box("snow on connecting roof", linkX, 20.54, linkZ, linkWidth + 0.14, 0.08, linkDepth, snow);
  flush(root);

  const rosePrint = new DynamicTexture("inlaid hotel compass", {width: 1024, height: 1024}, scene, true);
  const ink = rosePrint.getContext() as CanvasRenderingContext2D;
  ink.fillStyle = "#b89a63"; ink.fillRect(0, 0, 1024, 1024);
  ink.translate(512, 512);
  ink.fillStyle = "#273937";
  ink.beginPath(); ink.arc(0, 0, 504, 0, 2 * Math.PI); ink.fill();
  for (const r of [490, 472, 369]) {
    ink.strokeStyle = "#d6b773"; ink.lineWidth = r === 490 ? 10 : 4;
    ink.beginPath(); ink.arc(0, 0, r, 0, 2 * Math.PI); ink.stroke();
  }
  for (let i = 0; i < 8; i++) {
    ink.save(); ink.rotate(i * Math.PI / 4);
    const length = i % 2 ? 195 : 352;
    for (const side of [-1, 1]) {
      ink.fillStyle = side > 0 ? "#f0d79c" : "#ad8148";
      ink.beginPath(); ink.moveTo(0, -length); ink.lineTo(side * 51, -50);
      ink.lineTo(0, 0); ink.closePath(); ink.fill();
    }
    ink.restore();
  }
  ink.font = "bold 116px Georgia"; ink.textAlign = "center"; ink.textBaseline = "middle";
  ink.fillStyle = "#fff0c8";
  for (const [letter, x, y] of [["N", 0, -416], ["E", 416, 0], ["S", 0, 416], ["W", -416, 0]] as const)
    ink.fillText(letter, x, y);
  rosePrint.update();
  const mosaic = finish("brass and green stone inlay", "#ffffff", 0.12);
  mosaic.diffuseTexture = rosePrint;
  const { left, right, near: nearEdge, far: farEdge } = exchangeLanding;
  const landingX = (left + right) / 2;
  const landingZ = (nearEdge + farEdge) / 2;
  const landingWidth = right - left;
  const landingDepth = farEdge - nearEdge;
  // Both finishes share the circular seam. There is exactly one top face at every point.
  const radius = Math.min(landingWidth, landingDepth) * 0.38;
  const rim = new VertexData(), disk = new VertexData();
  rim.positions = []; rim.normals = []; rim.indices = [];
  disk.positions = [0, 0, 0]; disk.normals = [0, 1, 0]; disk.indices = []; disk.uvs = [0.5, 0.5];
  const east = new Vector3(-westOrigin.x, 0, -westOrigin.z).normalize();
  const corners = [[-1, -1], [1, -1], [1, 1], [-1, 1]];
  const perSide = 24, segments = 4 * perSide;
  for (let i = 0; i < segments; i++) {
    const side = Math.floor(i / perSide), t = (i % perSide) / perSide;
    const a = corners[side], b = corners[(side + 1) % 4];
    const x = (a[0] + t * (b[0] - a[0])) * landingWidth / 2;
    const z = (a[1] + t * (b[1] - a[1])) * landingDepth / 2;
    const length = Math.hypot(x, z), dx = x / length, dz = z / length;
    const next = (i + 1) % segments;
    rim.positions.push(x, 0, z, radius * dx, 0, radius * dz);
    rim.normals.push(0, 1, 0, 0, 1, 0);
    const outer = i * 2, nextOuter = next * 2;
    rim.indices.push(outer, nextOuter, nextOuter + 1, outer, nextOuter + 1, outer + 1);
    disk.positions.push(radius * dx, 0, radius * dz); disk.normals.push(0, 1, 0);
    disk.indices.push(0, i + 1, next + 1);
    // Project onto the hotel's compass axes so E points toward the East Room.
    disk.uvs.push(0.5 + 0.5 * (dx * east.x + dz * east.z), 0.5 + 0.5 * (-dx * east.z + dz * east.x));
  }
  for (let floor = 0; floor < 5; floor++) {
    elevation = floor * 4;
    // The deeper bearing course reaches both shaft walls below the separate docking plates.
    box("exchange landing bearing course", landingX, -0.24, landingZ, landingWidth, 0.30, landingDepth + 0.22, mortar);
    const slab = box("exchange landing limestone paving", landingX, -0.045, landingZ, landingWidth, 0.09, landingDepth, flagstone);
    const normals = slab.getVerticesData("normal")!;
    slab.setIndices(Array.from(slab.getIndices()!).filter(index => normals[index * 3 + 1] < 0.5));
    const surround = new Mesh("cut limestone surround", scene);
    rim.applyToMesh(surround);
    mapExteriorBox(surround, flagstone, landingX, elevation, landingZ, landingWidth, 0, landingDepth);
    piece(surround, flagstone, landingX, 0, landingZ);
    const rose = new Mesh("flush compass rose", scene);
    disk.applyToMesh(rose);
    piece(rose, mosaic, landingX, 0, landingZ);
  }
  flush(root);
  elevation = 0;
  const pilasterInnerEdge = doorwayHalfWidth + 0.03;
  const ashlarInnerEdge = doorwayWallEdge + 0.01;
  const trimInnerEdge = doorwayWallEdge + 0.025;
  for (let floor = 0; floor < 5; floor++) {
    elevation = floor * 4;
    // A broad unbroken floor leaves the rail, the wreck and the distant gallery visible while walking.
    box("terrace slab", 0, -0.20, -9.50, 7.0, 0.36, 6.0, mortar);
    for (let row = 0; row < 6; row++) {
      const farEdge = -7 - row - 0.987 / 2;
      // The terrace finish ends at the outer edge of the moving car's brass sill.
      const nearEdge = row === 0 ? rearWallZ - doorwaySillDepth / 2 : farEdge + 0.987;
      for (let col = 0; col < 5; col++) {
        box("flagstone", (col - 2) * 1.4, -0.024, (nearEdge + farEdge) / 2, 1.386, 0.05, nearEdge - farEdge, flagstone);
      }
    }
    for (const side of [-1, 1]) {
      // Recess the stone edges behind the clear jamb face and stagger adjacent stone returns.
      const stoneEdge = side > 0 ? ashlarInnerEdge + 0.2 : facadeOuterEdge;
      box("ashlar facade", side * (ashlarInnerEdge + stoneEdge) / 2, 1.85, -6.68,
        stoneEdge - ashlarInnerEdge, 4, 0.31, mortar);
      box("door pilaster", side * (pilasterInnerEdge + 0.13), 1.48, -6.84, 0.26, 2.96, 0.28, limestone);
      for (const sx of [2.75, 5.6, 8.45].filter(x => x < stoneEdge)) {
        const y = 1.9;
        box("hotel window casing", side * sx, y, -6.88, 1.43, 1.94, 0.25, limestone);
        box("recessed sash", side * sx, y, -7.02, 1.16, 1.67, 0.06, windowGlass);
        box("sash mullion", side * sx, y, -7.08, 0.055, 1.7, 0.07, gilt);
        box("sash transom", side * sx, y + 0.32, -7.08, 1.17, 0.05, 0.07, gilt);
      }
      for (const y of [0.10, 3.3])
        box("string course", side * (trimInnerEdge + stoneEdge) / 2, y, -6.85,
          stoneEdge - trimInnerEdge, 0.19, 0.48, limestone);
      for (const z of [-7.4, -9.8, -12.35]) {
        box("terrace stanchion", side * 3.43, 0.56, z, 0.10, 1.13, 0.10, iron);
        box("bronze post cap", side * 3.43, 1.14, z, 0.15, 0.09, 0.15, gilt);
      }
      for (const y of [0.49, 1.03]) box("side railing", side * 3.43, y, -9.6, 0.07, 0.065, 5.65, iron);
    }
    // Keep the head of each opening beneath the next terrace deck. Its cornice
    // returns stop at the next doorway so the docked sill stays flush underfoot.
    box("lintel", 0, 3.35, -6.72, (trimInnerEdge + 0.02) * 2, 0.90, 0.4, limestone);
    for (const side of [-1, 1]) {
      const edge = side < 0 ? facadeOuterEdge + 0.10 : ashlarInnerEdge + 0.32;
      const inner = floor === 4 ? 0 : trimInnerEdge;
      const center = side * (edge + inner) / 2;
      box("hotel cornice", center, 4.02, -6.80, edge - inner, 0.21, 0.65, wood);
      if (floor === 4)
        box("snow on roof cornice", center, 4.16, -6.80, edge - inner, 0.12, 0.65, snow);
    }
    sign(terraceNames[floor], floor === 0 ? "GROUND FLOOR" : "FLOOR " + floor, 0, 3.43, -6.95, 3.15, 0);
    if (floor !== stairFloor) {
      terraceEdge(floor);
      continue;
    }
    for (const x of [-2.95, -1.0, 1.0, 2.95]) box("rail standard", x, 0.54, -12.40, 0.08, 1.06, 0.08, iron);
    // The stairs once left the gallery through this opening. A bent rail hangs across the break.
    for (const y of [0.46, 1.03]) {
      box("terrace end railing", 1.21, y, -12.40, 4.48, 0.065, 0.075, iron);
      box("short corner rail", -3.2, y, -12.40, 0.49, 0.065, 0.075, iron);
    }
    bar("torn guardrail", [-2.95, 1.03, -12.40], [-2.16, 0.64, -12.75], 0.065, iron);
    // The first treads survive at the left. A tilted, partly buried flight continues below the break.
    for (let i = 0; i < 4; i++) {
      const drop = i < 3 ? i * 0.23 : 1.35;
      const step = box("fractured escape stair", -2.12 - Math.max(i - 2, 0) * 0.14, -drop - 0.15, -12.67 - i * 0.44,
        2.05, 0.28, 0.49, limestone);
      step.rotation.z = i < 3 ? 0 : 0.26;
    }
    for (const x of [-3.08, -1.16]) {
      bar("buckled stair handrail", [x, 1.00, -12.45], [x, 0.50, -13.8], 0.085, iron);
      bar("sheared stair handrail", [x, 0.50, -13.8], [x - 0.58, -0.32, -14.33], 0.085, iron);
      bar("buried stair support", [x - 0.56, -0.42, -14.4], [x - 0.92, -3.0, -16.4], 0.09, iron);
    }
    sign("VALLEY STAIRS", "LOWER STATION", -0.15, 0.77, -12.32, 1.46);
  }
  // The avalanche tore the insulated steam main off the underside of the stair
  // terrace. The bent end and the broken matching flange remain visible from above.
  elevation = stairFloor * 4;
  bar("steam main beneath stairs", [-3.04, 0.31, -10.6], [-3.04, 0.31, -12.35], 0.24, iron);
  bar("ruptured steam main", [-3.04, 0.31, -12.35], [-2.73, -0.05, -12.86], 0.24, iron);
  bar("split steam flange", [-2.77, -0.01, -12.80], [-2.73, -0.05, -12.86], 0.39, gilt);
  bar("sheared steam return", [-2.54, -0.62, -13.22], [-2.04, -1.14, -14.96], 0.23, iron);
  bar("fallen steam flange", [-2.54, -0.62, -13.22], [-2.52, -0.64, -13.29], 0.39, gilt);
  sign("HOTEL HEATING", "BOILER MAIN", -2.84, 0.82, -12.28, 1.1);
  elevation = 0;
  flush(wingShell);
  const westShell = wingShell.clone("West shaft and service terraces", root)!;
  applyWingFrame(westShell, true);
  // Runout descends diagonally from the hotel's western buttress into the valley.
  // The broken crown towers beside the upper terraces; lower decks face the blue compacted toe.
  bank("exposed avalanche chute", -15, -5.8, -19.8, 10.9, 23.8, 7.7, 1, shale);
  bank("upper avalanche crown", -14.5, 4.2, -18.7, 8.8, 14.4, 6.1, 0.2, snow);
  bank("avalanche pressure ridge", -8.9, -4.6, -17.4, 7.9, 17.6, 4.2, 2, ice);
  bank("sunlit pressure crust", -9.5, 5.8, -17.7, 6.0, 8.0, 3.5, 3, snow);
  bank("lower avalanche debris fan", -5.7, -4.6, -16.8, 7.0, 9.4, 4.0, 2, snow);
  for (let i = 0; i < 12; i++) {
    const x = -11 + i * 0.62;
    bank("serac fragments", x, 8.6 - i * 0.92, -14.8 - (i % 3) * 1.1, 1.05, 2.6, 1.1, i, i % 3 === 0 ? ice : snow);
  }
  bank("drift curling through the stair terrace railing", -3.87, stairFloor * 4 - 0.62, -12.44, 1.72, 1.4, 1.48, 10.82, snow);
  /** Places the stair debris beneath the same fixed opening as the surviving treads. */
  function fallenFlight() {
    // The fallen flight lies below the broken stair bay. Its torn railing emerges from the crust.
    const [x, y, z] = [-3.6, stairFloor * 4 - 1.2, -15.0];
    const tip = x + 3.5;
    for (const edge of [-0.62, 0.62]) {
      bar("split oak stair stringer", [x, y - 1, z + edge], [tip, y + 0.44, z + edge], 0.17, wood);
      bar("torn bronze banister", [x, y + 0.20, z + edge], [tip - 0.7, y + 1.42, z + edge + 0.24], 0.08, iron);
      bar("bent terminal banister", [tip - 0.70, y + 1.42, z + edge + 0.24], [tip + 0.22, y + 1.13, z + edge + 0.45], 0.08, iron);
      for (const n of [1, 4]) {
        const dx = n * 0.54;
        bar("uprooted stair spindle", [x + dx, y - 0.94 + dx * 0.41, z + edge], [x + dx + 0.18, y + 0.21 + dx * 0.43, z + edge + 0.15], 0.065, iron);
      }
    }
    for (let n = 0; n < 7; n++) {
      if (n === 4) continue;
      const tread = box("splintered stair tread", x + n * 0.55, y - 0.81 + n * 0.22, z, 0.48, 0.12, 1.5, wood);
      tread.rotation.z = 0.13; tread.rotation.x = 0.14 + n * 0.03;
      box("snow clinging to steps", x + n * 0.55, y - 0.72 + n * 0.22, z + 0.15, 0.42, 0.07, 0.85, snow).rotation.x = tread.rotation.x;
    }
  }
  fallenFlight();
  bank("blue ice fracture", -3.85, -2.04, -16.7, 3.35, 2.42, 2.16, 0.7, ice);
  bank("slab upper crust", -4.16, -0.61, -17.2, 3.35, 1.55, 2.12, 1.1, snow);
  for (let i = 0; i < 16; i++) {
    const x = -9 + (i * 31 % 15) * 0.80, z = -14.8 - (i * 7 % 11) * 0.56;
    bank("tumbled avalanche blocks", x, -2.8, z, 0.77 + i % 3 * 0.20, 1.16 + i % 5 * 0.40, 0.9, i, i % 5 === 0 ? shale : snow);
  }
  // A second tongue of avalanche debris buries the service steps behind the West shaft.
  bank("west valley debris", 9.4, -4.2, 25.6, 6.3, 9.8, 5.0, 3, ice);
  bank("west stair drift", 9.0, 4.7, 24.8, 4.1, 4.2, 2.5, 7, snow);
  // Exposed rock under the West gallery anchors the four switchback trestles below the hotel.
  bank("gallery trestle bedrock", 14.5, -35.6, -11, 9.8, 29.6, 25.5, 2.6, shale);
  flush(root);
  // The opposite bank and lodge use the same rotation and translation at the bridgehead.
  bank("opposite gorge wall", 12.5, -20, -32, 14.6, 18.7, 10.1, 2.5, shale);
  bank("opposite snow shelf", 12.0, -2.4, -32.7, 12.0, 1.48, 8.3, 2.0, snow);
  /** Builds a continuous outcrop under the lodge. Snow meets exposed rock along a shared
   * lip, and the talus widens below the visible gorge bed even at the highest booking. */
  function refugeOutcrop() {
    const segments = 48;
    const rings = [
      [1.85, -56], [1.54, -30], [1.23, -16], [1.12, -8], [1.04, -5.2],
      [1.05, -4.5], [1.00, -2.65], [0.98, -2.32], [0.87, -2.07], [0.48, -1.97], [0, -1.98],
    ];
    /** Samples the same ledge for each material so the snow fits the undulating rim exactly. */
    function point(ring: number, i: number) {
      const angle = (i % segments) * 2 * Math.PI / segments;
      const [r, y] = rings[ring];
      const shoulder = 1 + 0.09 * Math.sin(3 * angle + 1) + 0.06 * Math.cos(5 * angle - 0.4)
        + 0.025 * Math.sin(11 * angle + ring * 0.32);
      const broken = (0.31 * Math.cos(4 * angle) + 0.16 * Math.sin(9 * angle + 0.5)) * Math.min(r, 1);
      return new Vector3(10 + 10.7 * r * shoulder * Math.cos(angle),
        y + broken, -32.8 + 8.9 * r * shoulder * Math.sin(angle));
    }
    /** Builds each band in cliff-length coordinates so rock bedding runs horizontally along the ledge. */
    function band(first: number, last: number, material: StandardMaterial) {
      const positions: number[] = [], normals: number[] = [], indices: number[] = [], uvs: number[] = [];
      const stride = segments + 1;
      for (let ring = first; ring <= last; ring++) {
        for (let i = 0; i <= segments; i++) {
          const p = point(ring, i);
          positions.push(p.x, p.y, p.z);
          if (material === snow) uvs.push(p.x / 3.2, p.z / 3.2);
          else uvs.push(i * 14 / segments, (p.y + 0.23 * p.x) / 4.6);
          if (ring > first && i > 0) {
            const n = (ring - first) * stride + i;
            indices.push(n - stride - 1, n - stride, n - 1, n - stride, n, n - 1);
          }
        }
      }
      const data = new VertexData();
      VertexData.ComputeNormals(positions, indices, normals);
      Object.assign(data, {positions, normals, uvs, indices});
      const mesh = new Mesh("refuge cliff band", scene);
      data.applyToMesh(mesh);
      piece(mesh, material, 0, 0, 0);
    }
    band(0, 7, shale);
    band(7, rings.length - 1, snow);
  }
  refugeOutcrop();
  for (const x of [8.75, 10.65]) {
    box("refuge viaduct pier", x, -10.93, -27, 0.58, 20, 1.40, mortar);
    box("refuge pier capital", x, -1.35, -27, 0.91, 0.7, 1.52, limestone);
  }
  box("refuge foundation", 9.7, -11.26, -29.4, 5.80, 20, 3.6, mortar);
  box("refuge lodge", 9.7, 0.50, -29.4, 5.8, 3.55, 3.6, mortar);
  for (const side of [-1, 1]) {
    const roof = box("refuge pitched roof", 9.7 + side * 1.57, 2.88, -29.4, 3.63, 0.24, 4.18, snow);
    roof.rotation.z = -side * 0.39;
  }
  // Timber beneath the snowy eaves and a chimney break up the silhouette against the massif.
  for (let i = 0; i < 5; i++) {
    box("gable boarding", 9.7, 2.27 + i * 0.19, -29.4, 5.9 - i * 1.0, 0.20, 3.60, wood);
  }
  // Project the roof timbers beyond the gable so the eaves have depth from below the bridge.
  for (const x of [6.78, 12.62]) {
    box("eaves plate", x, 2.26, -29.4, 0.18, 0.22, 4.14, wood);
    for (const z of [-31.04, -29.4, -27.76]) {
      const overhang = x < 9.7 ? -0.32 : 0.32;
      bar("eaves knee brace", [x, 1.93, z], [x + overhang, 2.29, z], 0.1, wood);
    }
  }
  for (const side of [-1, 1]) {
    bar("carved bargeboard", [9.7, 3.48, -27.32], [9.7 + side * 3.18, 2.17, -27.32], 0.15, wood);
    bar("gable brace", [9.7 + side * 1.48, 2.30, -27.50], [9.7 + side * 0.44, 2.92, -27.50], 0.12, wood);
  }
  box("gable king post", 9.7, 2.83, -27.5, 0.14, 1.07, 0.15, wood);
  box("stone chimney", 11.2, 3.44, -30.1, 0.58, 1.75, 0.55, mortar);
  box("snowy chimney cap", 11.2, 4.33, -30.1, 0.72, 0.12, 0.68, snow);
  for (const z of [-28.5, -30.2]) {
    box("refuge side sash", 6.75, 0.64, z, 0.2, 1.27, 0.78, wood);
    box("refuge side glass", 6.61, 0.64, z, 0.05, 1.00, 0.54, litSash);
  }
  for (const x of [7.91, 11.49]) box("refuge sill course", x, -0.66, -27.51, 2.32, 0.16, 0.31, wood);
  box("refuge lintel course", 9.7, 1.61, -27.51, 5.9, 0.16, 0.31, wood);
  box("refuge threshold", 9.7, 0.21, -27.55, 1.28, 2.60, 0.15, iron);
  box("refuge door lining", 9.7, 0.19, -27.44, 1.16, 2.34, 0.11, wood);
  box("porch door", 9.7, 0.18, -27.36, 0.96, 2.16, 0.08, iron);
  box("porch door glazing", 9.7, 0.68, -27.306, 0.63, 0.79, 0.025, litSash);
  box("porch door astragal", 9.7, 0.68, -27.28, 0.045, 0.80, 0.035, wood);
  box("porch door cross rail", 9.7, 0.66, -27.27, 0.65, 0.043, 0.035, wood);
  box("porch door kick plate", 9.7, -0.68, -27.30, 0.90, 0.21, 0.025, gilt);
  box("porch door brass push bar", 9.7, 0.08, -27.18, 0.78, 0.082, 0.09, gilt);
  for (const x of [9.39, 10.01]) box("porch door bar mount", x, 0.08, -27.25, 0.065, 0.095, 0.14, gilt);
  box("refuge coir doormat", 9.7, -0.795, -27.02, 1.56, 0.035, 0.58, wood);
  for (const x of [7.67, 11.75]) {
    box("refuge window trim", x, 0.75, -27.55, 1.31, 1.4, 0.18, wood);
    box("lamplit window", x, 0.75, -27.44, 1.06, 1.16, 0.08, litSash);
    box("window head drip", x, 1.49, -27.47, 1.51, 0.10, 0.33, wood);
    box("projecting window sill", x, 0.04, -27.40, 1.48, 0.12, 0.39, limestone);
    box("cottage transom", x, 0.92, -27.36, 1.10, 0.055, 0.065, iron);
    box("refuge mullion", x, 0.75, -27.37, 0.07, 1.18, 0.06, iron);
  }
  // The approach runs down the gorge to a short landing square to the lodge door.
  box("refuge entry landing", 9.7, -0.94, -27.04, 2.4, 0.26, 1.22, deckWood);
  for (const x of [8.54, 10.86]) {
    box("entry landing rail", x, 0.18, -27.04, 0.09, 0.09, 1.22, gilt);
    for (const z of [-26.47, -27.54]) {
      box("entry lantern post", x, -0.23, z, 0.14, 1.16, 0.14, iron);
      box("entry lantern", x, 0.47, z, 0.23, 0.30, 0.23, light);
    }
  }
  // Separate enamel annunciators register the arrivals permanently for both guests.
  const arrivalGlass = ["EAST", "WEST"].map((wing, index) => {
    const x = 8.70 + 2 * index;
    const glass = finish(wing + " arrival lamp", "#292f29", 0);
    box("arrival lamp backplate", x, 1.01, -27.40, 0.44, 0.64, 0.16, iron);
    box("arrival lamp bezel", x, 1.08, -27.28, 0.32, 0.31, 0.10, gilt);
    box("arrival light", x, 1.08, -27.21, 0.24, 0.23, 0.06, glass);
    sign(wing, "ARRIVAL", x, 0.76, -27.29, 0.66);
    return glass;
  });
  // Split logs under the eaves and smoke above the chimney identify the staffed shelter.
  for (let row = 0; row < 3; row++) for (let i = 0; i < 6 - row; i++) {
    const log = MeshBuilder.CreateCylinder("shelter firewood", {height: 0.61, diameter: 0.23, tessellation: 8}, scene);
    piece(log, wood, 11.1 + row * 0.11 + i * 0.226, -0.75 + row * 0.195, -27.05);
    log.rotation.x = Math.PI / 2;
  }
  sign("REFUGE", "MOUNTAIN PATROL", 9.72, 2.04, -27.43, 2.68);
  flush(refuge);
  // A string of coloured bulbs lights in sequence above the porch as the guest arrives.
  const welcome = new TransformNode("refuge welcome lights", scene);
  welcome.parent = refuge;
  for (const [index, color] of ["#ef6558", "#ffce58", "#6be8cc", "#ffce58", "#ef6558"].entries()) {
    const bulb = finish("welcome bulb " + index, color, 1);
    const lamp = MeshBuilder.CreateSphere("porch festoon", { diameter: 0.16, segments: 8 }, scene);
    lamp.position.set(8.4 + 0.65 * index, 1.45 + Math.abs(index - 2) * 0.12, -27.22);
    lamp.material = bulb;
    lamp.parent = welcome;
  }
  welcome.setEnabled(false);
  // Build the span in shaft coordinates so its near end meets the hotel access gallery.
  // The far joint is the centre of the entry landing lip in the rotated lodge frame.
  const crossing = new TransformNode("permanent refuge footbridge", scene);
  crossing.parent = root;
  const near = new Vector3(galleryJoint.x, 0, galleryJoint.z);
  const direction = far.subtract(near);
  const yaw = Math.atan2(direction.x, direction.z);
  const length = Math.hypot(direction.x, direction.z);
  const side = new Vector3(Math.cos(yaw), 0, -Math.sin(yaw));
  /** Locates a structural joint by fraction along the sloping walkway, offset across it and above it. */
  function joint(t: number, across: number, above: number) {
    return near.add(direction.scale(t)).add(side.scale(across)).add(new Vector3(0, above, 0)).asArray();
  }
  // Four opposed pairs lead the eye across the gorge when the West master feeds the gallery.
  for (const t of [0.12, 0.38, 0.64, 0.90]) {
    for (const edge of [-0.92, 0.92]) {
      const [x, y, z] = joint(t, edge, 1.30);
      box("bronze lantern foot", x, y - 0.15, z, 0.22, 0.18, 0.22, gilt);
      box("bridge lantern glass", x, y + 0.08, z, 0.25, 0.29, 0.25, bridgeGlass);
      box("storm lantern hood", x, y + 0.24, z, 0.32, 0.05, 0.32, iron);
    }
  }
  const bays = 9;
  const boardsPerBay = 8;
  const deckPitch = -Math.atan2(direction.y, length);
  const boardLength = direction.length() / (bays * boardsPerBay);
  for (let i = 0; i < bays; i++) {
    for (let board = 0; board < boardsPerBay; board++) {
      const progress = (i + (board + 0.5) / boardsPerBay) / bays;
      const [x, y, z] = joint(progress, 0, -0.10);
      const deck = box("transverse deck plank", x, y, z, 1.84, 0.20, boardLength - 0.012, deckWood);
      deck.rotation.set(deckPitch, yaw, 0);
    }
    for (const edge of [-0.92, 0.92]) {
      const a = i / bays, b = (i + 1) / bays;
      // Connect the two trusses underneath the boards, visible from the hotel terraces.
      bar("underdeck sway brace", joint(a, edge, -0.79), joint(b, -edge, -0.79), 0.063, iron);
      const [gx, gy, gz] = joint(a, edge * 1.065, -0.2);
      box("girder splice plate", gx, gy, gz, 0.045, 0.37, 0.30, iron).rotation.y = yaw;
      bar("riveted lower chord", joint(a, edge, -0.85), joint(b, edge, -0.85), 0.16, iron);
      bar("bridge edge girder", joint(a, edge, -0.12), joint(b, edge, -0.12), 0.22, iron);
      bar("diagonal girder web", joint(a, edge, -0.12), joint((a + b) / 2, edge, -0.85), 0.10, iron);
      bar("diagonal girder web", joint((a + b) / 2, edge, -0.85), joint(b, edge, -0.12), 0.10, iron);
      bar("bronze handrail", joint(a, edge, 1.04), joint(b, edge, 1.04), 0.09, gilt);
      bar("bridge guardrail", joint(a, edge, 0.51), joint(b, edge, 0.51), 0.07, iron);
    }
  }
  for (let i = 0; i <= bays; i++) {
    bar("floor cross girder", joint(i / bays, -0.96, -0.25), joint(i / bays, 0.96, -0.25), 0.19, iron);
    for (const edge of [-0.92, 0.92]) {
      bar("bridge standard", joint(i / bays, edge, -0.87), joint(i / bays, edge, 1.09), 0.12, iron);
    }
  }
  // The pointed bronze portals repeat the silhouette of the gallery entrance in the cabin.
  for (const t of [0.06, 0.58]) {
    for (const edge of [-0.92, 0.92]) {
      bar("gallery portal standard", joint(t, edge, 1.04), joint(t, edge, 2.43), 0.115, iron);
      bar("peaked bronze portal", joint(t, edge, 2.43), joint(t, 0, 2.92), 0.14, gilt);
    }
  }
  flush(crossing);
  /** Bakes world geometry independently from the destination carried by the guest booking. */
  function flush(parent: TransformNode) {
    for (const [mat, pieces] of batches) {
      const mesh = pieces.length === 1 ? pieces[0] : Mesh.MergeMeshes(pieces, true, true)!;
      mesh.name = mat.name;
      mesh.parent = parent;
      mesh.isPickable = false;
    }
    batches.clear();
  }
  // Raking winter light picks out the wearing surfaces and places the railings' shadows on the stone.
  // All casters are static between bookings, including the two assemblies set to the booked floor.
  const sunlight = new DirectionalLight("winter sun", new Vector3(-0.48, -0.64, 0.60).normalize(), scene);
  sunlight.parent = shaft;
  sunlight.position.set(33, 44, -56);
  sunlight.diffuse = Color3.FromHexString("#ffead5");
  sunlight.intensity = 0.50;
  sunlight.shadowMinZ = 0.5;
  sunlight.shadowMaxZ = 130;
  sunlight.shadowFrustumSize = 78;
  sunlight.includedOnlyMeshes = root.getChildMeshes();
  const shadows = new ShadowGenerator(1536, sunlight);
  shadows.usePercentageCloserFiltering = true;
  shadows.filteringQuality = ShadowGenerator.QUALITY_LOW;
  shadows.bias = 0.0004;
  shadows.normalBias = 0.025;
  for (const mesh of sunlight.includedOnlyMeshes) {
    mesh.receiveShadows = true;
    shadows.addShadowCaster(mesh, false);
  }
  const shadowMap = shadows.getShadowMap();
  if (shadowMap) shadowMap.refreshRate = 0;
  // Slow chimney smoke stays local to the shelter across all booked elevations.
  // Its soft edges share one small texture and it does not cast architectural shadows.
  const smokePrint = new DynamicTexture("woodsmoke falloff", 64, scene, false);
  const smokeInk = smokePrint.getContext() as CanvasRenderingContext2D;
  const falloff = smokeInk.createRadialGradient(32, 32, 0, 32, 32, 32);
  falloff.addColorStop(0, "rgba(238,223,208,0.55)");
  falloff.addColorStop(0.45, "rgba(225,215,207,0.32)");
  falloff.addColorStop(1, "rgba(220,216,211,0)");
  smokeInk.fillStyle = falloff;
  smokeInk.fillRect(0, 0, 64, 64);
  smokePrint.hasAlpha = true; smokePrint.update();
  const soot = finish("refuge woodsmoke", "#ffffff");
  soot.diffuseTexture = smokePrint; soot.useAlphaFromDiffuseTexture = true;
  soot.disableLighting = true; soot.emissiveColor = new Color3(0.90, 0.87, 0.83);
  const plume = Array.from({length: 7}, (_, i) => {
    const puff = MeshBuilder.CreatePlane("smoke from refuge stove", {size: 1}, scene);
    puff.parent = refuge; puff.material = soot; puff.isPickable = false;
    puff.billboardMode = Mesh.BILLBOARDMODE_ALL;
    return { puff, phase: i / 7 };
  });
  scene.onBeforeRenderObservable.add(() => {
    const wind = performance.now() / 7800;
    for (const {puff, phase} of plume) {
      const age = (wind + phase) % 1;
      puff.position.set(11.2 + 2.7 * age * age, 4.42 + 4.8 * age, -30.1 - age * age * 1.6);
      puff.scaling.setAll(0.40 + age * 2.5);
      puff.visibility = 0.78 * Math.sin(Math.PI * age);
    }
  });
  return {
    near, far, threshold,
    /** Each lamp follows its own wing across reconnects and remote arrivals. */
    arrivals(localEast: boolean, local: boolean, partner: boolean) {
      arrivalGlass.forEach((glass, index) => {
        const on = (index === 0) === localEast ? local : partner;
        glass.diffuseColor = Color3.FromHexString(on ? "#ffcb72" : "#394138");
        glass.emissiveColor = glass.diffuseColor.scale(on ? 0.95 : 0.04);
      });
    },
    /** Illuminates the lodge porch for the arriving guest. */
    welcome(active: boolean) {
      welcome.setEnabled(active);
      if (active) welcome.getChildren().forEach((light, i) =>
        light.setEnabled(performance.now() % 1500 > i * 100));
    },
    /** Sets the refuge elevation from the rescue ledger. The damaged hotel stays fixed in the shaft. */
    update(bridgeFloor: number) {
      if (crossing.position.y !== bridgeFloor * 4) shadowMap?.resetRefreshCounter();
      crossing.position.y = refuge.position.y = bridgeFloor * 4;
    },
  };
}
