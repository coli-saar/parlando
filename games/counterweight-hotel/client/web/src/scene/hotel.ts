import "@babylonjs/core/Meshes/instancedMesh";
import { createAlpineLandscape } from "./alpine-landscape";
import { Engine } from "@babylonjs/core/Engines/engine";
import { SelectionOutlineLayer } from "@babylonjs/core/Layers/selectionOutlineLayer";
import { Scene } from "@babylonjs/core/scene";
import { FreeCamera } from "@babylonjs/core/Cameras/freeCamera";
import { Vector3 } from "@babylonjs/core/Maths/math.vector";
import { Color3, Color4 } from "@babylonjs/core/Maths/math.color";
import { StandardMaterial } from "@babylonjs/core/Materials/standardMaterial";
import { Texture } from "@babylonjs/core/Materials/Textures/texture";
import { DynamicTexture } from "@babylonjs/core/Materials/Textures/dynamicTexture";
import { MeshBuilder } from "@babylonjs/core/Meshes/meshBuilder";
import { TransformNode } from "@babylonjs/core/Meshes/transformNode";
import { HemisphericLight } from "@babylonjs/core/Lights/hemisphericLight";
import { PointLight } from "@babylonjs/core/Lights/pointLight";
import { VertexData } from "@babylonjs/core/Meshes/mesh.vertexData";
import { Mesh } from "@babylonjs/core/Meshes/mesh";
import { createBaggageTransfers, rackPosition } from "./baggage";
import { createSuspension } from "./suspension";
import { installAirWell } from "./air-well";
import { createGuestAvatar } from "./guest-avatar";
import type { BridgePose, Observation, Seal } from "../game";
import { drawSealMark, type Booking } from "../document-art";
import { applyWingFrame, exchangeLanding, wingPoint } from "./hotel-layout";
import { createRoomDetails, doorwayHalfWidth, doorwayJambWidth, doorwaySillDepth, doorwayWallEdge, rearWallZ } from "./room-details";
import { createHotelStory, storyWallBays } from "./story";
import { createLiftSound } from "./lift-sound";
import { createLiftPlayback } from "./lift-playback";
import { createRefugeWalk } from "./refuge-walk";
import { createWalkingControls, guestRadius, walkSpeed } from "./walking";
import { createExteriorFinishes, mapExteriorBox } from "./exterior-finishes";
import { createAvalancheLanding } from "./avalanche-landing";
import { bridgeApproach } from "./bridge-route";

/** Authoritative lift, brake, luggage and booking state visible to one guest. */
export interface HotelSceneState extends Booking {
  floor: number;
  partnerFloor: number;
  partnerDestination: number;
  gateOpen: boolean;
  /** A guest opens their own rescue gate independently of the other car. */
  partnerGateOpen: boolean;
  /** The West master relay has latched after the brake key exchange. */
  bridgePowered: boolean;
  /** Freight contact state, or null on a booking without a manifest. */
  freightClear: boolean | null;
  /** The other guest has reached the refuge. */
  partnerEscaped: boolean;
  /** Latest public pose of the guest in the other wing. */
  partnerBridgePose: BridgePose | null;
  lockTripped: boolean;
  /** The other gate's inspection spring has tripped independently of this one. */
  partnerLockTripped: boolean;
  hasBrakeKey: boolean;
  loads: number;
  partnerLoads: number;
  route: Observation["route"];
  /** Both cars are braked at the paired rescue landings after the key exchange. */
  atRescueBridge: boolean;
  landingLoads: number[];
  partnerLandingLoads: number[];
  escaped: boolean;
  /** The guest has crossed to the fixed service landing. */
  onLanding: boolean;
  /** The game permits leaving this car through its service doorway. */
  canStepOntoLanding: boolean;
  /** Whether clicking a case on the cabin rack can dispatch it. */
  canUnloadCase: boolean;
  freightStatus: Observation["freight_status"];
  freightFloor: number | null;
  canCollectFreight: boolean;
  /** Service landings from which the game currently permits direct loading. */
  loadableRackFloors: readonly number[];
}

/** Live room renderer owned by the participant view. */
export type HotelScene = ReturnType<typeof createHotelScene>;

/** Bakes rigid opaque siblings in assembly-local coordinates, retaining their material and pick identity. */
function batchRigidParts(assembly: TransformNode) {
  const groups = new Map<string, Mesh[]>();
  for (const mesh of assembly.getChildMeshes(true)) {
    if (!(mesh instanceof Mesh) || !mesh.material || mesh.visibility !== 1 ||
        mesh.material.needAlphaBlendingForMesh(mesh) || mesh.getChildren().length) continue;
    const key = JSON.stringify([mesh.material.uniqueId, mesh.metadata, mesh.isPickable, mesh.getVerticesDataKinds().sort()]);
    const group = groups.get(key);
    if (group) group.push(mesh);
    else groups.set(key, [mesh]);
  }
  for (const parts of groups.values()) {
    if (parts.length < 2) continue;
    const { metadata, isPickable, name } = parts[0];
    for (const part of parts) part.parent = null;
    const combined = Mesh.MergeMeshes(parts, true, true)!;
    combined.name = name;
    combined.metadata = metadata;
    combined.isPickable = isPickable;
    combined.parent = assembly;
  }
}

/** Builds the moving suite and reports actionable picks, transit and completed landings. */
export function createHotelScene(
  canvas: HTMLCanvasElement,
  onPick: (target: string) => void,
  onTargetChange?: (target: string | null) => void,
  onTravelChange?: (moving: boolean) => void,
  onFloorChange?: (floor: number) => void,
  onBridgePose?: (pose: BridgePose | null) => void,
  onDoorRange?: (reachable: boolean) => void,
) {
  const engine = new Engine(
    canvas,
    true,
    { stencil: true },
    false,
  );
  // Keep the 3D framebuffer at CSS resolution. The DOM controls retain native display density.
  const scene = new Scene(engine);
  scene.skipPointerMovePicking = true;
  scene.skipPointerDownPicking = true;
  scene.skipPointerUpPicking = true;
  const exterior = createExteriorFinishes(scene);
  scene.clearColor = new Color4(0.16, 0.19, 0.19, 1);
  // A diagonal from the rear quarter frames the rug, furniture and far wall at eye level.
  const entryPosition = new Vector3(-3.32, 1.73, rearWallZ + 1.18);
  const entryLookAt = new Vector3(0.45, entryPosition.y, 1.42);
  const camera = new FreeCamera("guest", entryPosition.clone(), scene);
  camera.minZ = 0.04;
  camera.maxZ = 800;
  camera.fov = Math.PI / 3;
  camera.setTarget(entryLookAt);
  scene.activeCamera = camera;
  const fill = new HemisphericLight(
    "daylight",
    new Vector3(-0.2, 0.8, -0.5),
    scene,
  );
  fill.intensity = 1.02;
  fill.groundColor = new Color3(0.42, 0.32, 0.23);
  /** Gives room components named, shared finishes and a controllable specular response. */
  function finish(name: string, hex: string, gloss = 0.2) {
    const m = new StandardMaterial(name, scene);
    m.diffuseColor = Color3.FromHexString(hex);
    m.specularColor = new Color3(gloss, gloss * 0.86, gloss * 0.65);
    m.specularPower = 45;
    return m;
  }
  /** Loads a bundled artwork with independently adjustable self-illumination. */
  function art(name: string, url: string, glow = 0) {
    const m = finish(name, "#ffffff", 0);
    m.diffuseTexture = new Texture(url, scene);
    m.emissiveColor = new Color3(glow, glow, glow);
    return m;
  }
  const brass = finish("antique brass", "#bb924e", 0.63),
    gold = finish("gilt edge", "#eed097", 0.55);
  const dark = finish("ebonised mouldings", "#33251e"),
    wood = finish("walnut panels", "#69442e"),
    inset = finish("raised timber", "#846043");
  const cream = finish("plaster", "#d7bf99"),
    paper = art(
      "woven silk",
      new URL("../art/wallpaper.jpg", import.meta.url).href,
    );
  const fabric = finish("silk velvet", "#874635"),
    stripe = finish("silk folds", "#603224"),
    rug = finish("carpet", "#63342c");
  const leather = finish("steamer trunk leather", "#775035"),
    iron = finish("forged iron", "#272b29");
  const lamp = finish("opal glass", "#fff2ce");
  lamp.emissiveColor = new Color3(0.93, 0.71, 0.39);
  /** Places and optionally groups a finished mesh, marking its interaction target. */
  function place(
    m: Mesh,
    x: number,
    y: number,
    z: number,
    mat: StandardMaterial,
    id?: string,
    parent?: TransformNode,
  ) {
    m.position.set(x, y, z);
    m.material = mat;
    m.isPickable = !!id;
    m.metadata = id;
    m.parent = parent || null;
    return m;
  }
  /** Rectangular joinery, fabric panels and nearby architectural surfaces. */
  function box(
    x: number,
    y: number,
    z: number,
    w: number,
    h: number,
    d: number,
    mat: StandardMaterial,
    id?: string,
    parent?: TransformNode,
  ) {
    const mesh = MeshBuilder.CreateBox("joinery", { width: w, height: h, depth: d }, scene);
    mapExteriorBox(mesh, mat, x, y, z, w, h, d);
    return place(mesh, x, y, z, mat, id, parent);
  }
  /** Turned metal and timber details, optionally tapered. */
  function rod(
    x: number,
    y: number,
    z: number,
    r: number,
    h: number,
    mat: StandardMaterial,
    id?: string,
    parent?: TransformNode,
    top = r,
  ) {
    return place(
      MeshBuilder.CreateCylinder(
        "turning",
        {
          height: h,
          diameterBottom: r * 2,
          diameterTop: top * 2,
          tessellation: 16,
        },
        scene,
      ),
      x,
      y,
      z,
      mat,
      id,
      parent,
    );
  }
  /** Small spherical finials and frosted-glass lamp globes. */
  function ball(
    x: number,
    y: number,
    z: number,
    r: number,
    mat: StandardMaterial,
    id?: string,
    parent?: TransformNode,
  ) {
    return place(
      MeshBuilder.CreateSphere(
        "finial",
        { diameter: r * 2, segments: 12 },
        scene,
      ),
      x,
      y,
      z,
      mat,
      id,
      parent,
    );
  }
  /** Draws a non-interactive enamel label for the associated place or mechanism. */
  function plaque(
    label: string,
    x: number,
    y: number,
    z: number,
    w: number,
    h: number,
    parent?: TransformNode,
    enamel?: string,
  ) {
    const tex = new DynamicTexture(
        label,
        { width: 768, height: 160 },
        scene,
        false,
      ),
      ctx = tex.getContext() as CanvasRenderingContext2D;
    ctx.fillStyle = enamel ?? "#233832";
    ctx.fillRect(0, 0, 768, 160);
    ctx.strokeStyle = "#cfaf71";
    ctx.lineWidth = 5;
    ctx.strokeRect(10, 10, 748, 140);
    ctx.fillStyle = "#f7e4bb";
    ctx.font = enamel ? "bold 104px Georgia" : "54px Georgia";
    ctx.textAlign = "center";
    ctx.fillText(label, 384, 99);
    tex.update();
    const m = finish(label, "#ffffff");
    m.diffuseTexture = tex;
    m.emissiveColor = new Color3(0.18, 0.17, 0.14);
    return place(
      MeshBuilder.CreatePlane(label, { width: w, height: h }, scene),
      x,
      y,
      z,
      m,
      undefined,
      parent,
    );
  }
  /** Deterministic tonal grain gives the millwork and parquet a varnished wood surface. */
  function walnutGrain() {
    const tex = new DynamicTexture("quarter-sawn walnut", 512, scene, false);
    const ctx = tex.getContext() as CanvasRenderingContext2D;
    ctx.fillStyle = "#f1ddc1";
    ctx.fillRect(0, 0, 512, 512);
    for (let i = 0; i < 340; i++) {
      ctx.strokeStyle = i % 7 ? "rgba(73,41,19,.16)" : "rgba(255,249,226,.78)";
      ctx.lineWidth = i % 9 === 0 ? 1.75 : 0.7;
      ctx.beginPath();
      for (let x = 0; x <= 512; x += 4) {
        const y =
          i * 1.54 +
          2 * Math.sin(x / 59 + i * 0.13) +
          1.5 * Math.cos(x / 35 + i * 0.025);
        if (x === 0) ctx.moveTo(x, y);
        else ctx.lineTo(x, y);
      }
      ctx.stroke();
    }
    tex.update();
    return tex;
  }
  const grain = walnutGrain();
  for (const m of [wood, inset, dark]) m.diffuseTexture = grain;
  rug.diffuseTexture = new Texture(
    new URL("../art/carpet.jpg", import.meta.url).href,
    scene,
  );
  const timber = ["#835c40", "#72503a", "#946c4b", "#7b563e"].map((color, i) => {
    const material = finish("parquet " + i, color);
    material.diffuseTexture = grain;
    return material;
  });
  const shellStart = scene.meshes.length;
  // Clip alternating plank runs to the cabin ends so no parquet projects into the shaft.
  const frontEdge = 3.68;
  const rearEdge = rearWallZ - 0.1;
  const cabinDepth = frontEdge - rearEdge;
  const cabinCenterZ = (frontEdge + rearEdge) / 2;
  for (let i = 0; i < 22; i++) {
    const start = rearEdge - (i % 2) * 0.9;
    for (let z = start, j = 0; z < frontEdge; z += 3, j++) {
      const lo = Math.max(z, rearEdge);
      const hi = Math.min(z + 3, frontEdge);
      box(-4.41 + i * 0.42, -0.06, (lo + hi) / 2, 0.415, 0.1, hi - lo - 0.005, timber[(i * 7 + j * 5) % 4]);
    }
  }
  // A solid subfloor closes the fine seams between the staggered parquet boards.
  box(0, -0.14, cabinCenterZ, 8.95, 0.16, cabinDepth, dark);
  box(0, 3.81, cabinCenterZ, 8.95, 0.14, cabinDepth, cream);
  box(-0.4, 0.011, -1, 4.08, 0.028, 7, brass);
  box(-0.4, 0.032, -1, 3.99, 0.025, 6.93, rug);
  for (const x of [-2.24, 1.44]) box(x, 0.052, -1, 0.035, 0.01, 6.68, gold);
  const pictureGlass = finish("observation window glass", "#b9d4d2", 0.84);
  pictureGlass.alpha = 0.085;
  // Side walls use silk above the dado and individually framed recessed panels below.
  for (const side of [-1, 1] as const) {
    const x = side * 4.4;
    const sections = side === -1 ? [[rearEdge, 1.13], [3.38, frontEdge]] : [[rearEdge, frontEdge]];
    for (const [start, end] of sections) {
      // The workshop plate occupies a continuous wall. The opposite wall overlooks the valley.
      const wallRuns = side < 0 ? [[start, end]] : [[start, Math.min(end, -4.8)], [Math.max(start, 0.85), end]];
      for (const [lo, hi] of wallRuns)
        if (hi > lo) box(x, 2.38, (lo + hi) / 2, 0.18, 2.8, hi - lo, paper);
      box(x, 0.55, (start + end) / 2, 0.2, 1.1, end - start, wood);
    }
    if (side === 1) {
      // The pier and two window openings fill the wall between the dado and header.
      // Each jamb occupies its own strip beside the paper, with the glass set back in the frame.
      const { center, width } = storyWallBays[side];
      const pierLeft = center - width / 2, pierRight = center + width / 2;
      const wallLeft = -4.8, wallRight = 0.85;
      const sill = 1.10, head = 3.27, ceiling = 3.78;
      const stile = 0.10, rail = 0.10;
      box(x, (head + sill) / 2, center, 0.18, head - sill, width, paper);
      box(x, (ceiling + head) / 2, center, 0.22, ceiling - head, wallRight - wallLeft, wood);
      for (const [lo, hi] of [[wallLeft, pierLeft], [pierRight, wallRight]]) {
        const middle = (lo + hi) / 2, clearWidth = hi - lo - 2 * stile;
        for (const z of [lo + stile / 2, hi - stile / 2])
          box(x, (head + sill) / 2, z, 0.26, head - sill, stile, inset);
        for (const y of [sill + rail / 2, head - rail / 2])
          box(x, y, middle, 0.26, rail, clearWidth, inset);
        box(x + 0.055, (head + sill) / 2, middle, 0.022, head - sill - 2 * rail, clearWidth, pictureGlass);
      }
    }
    if (side === -1) {
      box(x, 0.28, 2.255, 0.26, 0.56, 2.25, wood);
      box(x, 3.38, 2.255, 0.26, 0.88, 2.25, wood);
      for (const z of [1.13, 3.38]) box(x, 1.76, z, 0.31, 2.48, 0.09, dark);
      for (const y of [0.57, 2.94]) box(x, y, 2.255, 0.36, 0.08, 2.32, brass);
    }
    for (let j = 0; j < 8; j++)
      if (side > 0 || -5.95 + j * 1.27 + 0.565 <= 1.13) box(x - side * 0.115, 0.54, -5.95 + j * 1.27, 0.055, 0.72, 1.13, inset);
    for (const [y, depth, mat] of [
      [0.1, 0.34, dark],
      [1.03, 0.36, dark],
      [1.1, 0.37, brass],
      [3.62, 0.4, wood],
      [3.69, 0.49, gold],
    ] as const)
      for (const [start, end] of (side < 0 && y > 0.6 && y < 2.94 ? sections : [[rearEdge, frontEdge]]))
        box(x, y, (start + end) / 2, depth, 0.075, end - start, mat);
  }
  const cabin = new TransformNode("moving guest cabin", scene);
  for (const piece of scene.meshes.slice(shellStart)) piece.parent = cabin;
  const world = new TransformNode("hotel world", scene);
  const shaft = new TransformNode("landings", scene),
    partner = new TransformNode("opposing car", scene);
  const beaconGlass = finish("master-fed bridge lanterns", "#364242");
  const coldGlass = Color3.FromHexString("#364242");
  const hotGlass = Color3.FromHexString("#ffdb89");
  const avalancheLanding = createAvalancheLanding(scene, world, beaconGlass, exterior);
  const serviceDoorway = new TransformNode("avalanche service doorway", scene);
  const shaftDoors = Array.from({ length: 5 }, (_, level) => {
    const door = new TransformNode("landing shaft safety gate", scene);
    door.parent = shaft;
    door.position.set(0, level * 4, rearWallZ - 0.2);
    for (const x of [-0.96, -0.64, -0.32, 0, 0.32, 0.64, 0.96])
      box(x, 1.45, 0, 0.035, 2.9, 0.06, dark, undefined, door);
    for (const y of [0.08, 1.03, 2.82])
      box(0, y, 0, doorwayHalfWidth * 2, 0.09, 0.07, dark, undefined, door);
    return door;
  });
  const serviceLeaves = [-1, 1].map(side => {
    const panel = new TransformNode("sliding service door panel", scene);
    panel.parent = serviceDoorway;
    // The upper grille keeps the passing hotel floors visible with the car doors shut.
    box(0, 0.57, rearWallZ, doorwayHalfWidth, 1.14, 0.10, wood, undefined, panel);
    box(0, 0.56, rearWallZ + 0.06, 0.84, 0.86, 0.04, inset, undefined, panel);
    for (const x of [-0.50, 0.50]) box(x, 2, rearWallZ, 0.10, 1.72, 0.10, wood, undefined, panel);
    for (const x of [-0.19, 0.19]) box(x, 2, rearWallZ, 0.026, 1.72, 0.045, brass, undefined, panel);
    box(0, 2.80, rearWallZ, doorwayHalfWidth, 0.12, 0.10, wood, undefined, panel);
    box(-side * 0.37, 1.02, rearWallZ + 0.11, 0.04, 0.20, 0.07, brass, undefined, panel);
    return panel;
  });
  for (const x of [-1, 1]) box(x * doorwayWallEdge, 1.46, rearWallZ, doorwayJambWidth, 2.92, 0.31, dark, undefined, serviceDoorway);
  box(0, 3.35, rearWallZ, doorwayHalfWidth * 2, 0.92, 0.20, wood, undefined, serviceDoorway);
  box(0, 2.91, rearWallZ, doorwayHalfWidth * 2, 0.10, 0.32, brass, undefined, serviceDoorway);
  box(0, -0.02, rearWallZ, doorwayHalfWidth * 2, 0.056, doorwaySillDepth, brass, undefined, serviceDoorway);
  plaque("PALIER DE SERVICE", 0, 3.2, rearWallZ + 0.11, 1.53, 0.23, serviceDoorway).rotation.y = Math.PI;
  // The masonry pier divides the outdoor bay from the enclosed lift well at every elevation.
  const masonry = exterior("ashlar");
  const shaftMetal = finish("painted lift steel", "#384740", 0.28);
  const reveal = finish("sandstone window reveals", "#b6a68c", 0.08);
  // Shallow reveals fit inside the walnut frame so oblique views still open onto the valley.
  for (const x of [-4.04, 0.54])
    box(x, 1.97, 3.53, 0.26, 3.85, 0.20, reveal);
  for (const y of [0.51, 3.56])
    box(-1.75, y, 3.53, 4.85, 0.69, 0.20, reveal);
  const baggageBays = new TransformNode("hotel baggage bays", scene);
  baggageBays.parent = shaft;
  for (let i = 0; i < 5; i++) {
    const level = i * 4;
    // Slender rack uprights keep the moving room visible through the service bay.
    for (const z of [0.96, 3.48])
      box(-5.97, level + 1.95, z, 0.09, 4, 0.10, shaftMetal, undefined, baggageBays);
    box(-5.97, level + 2.36, 2.22, 0.09, 0.13, 2.53, shaftMetal, undefined, baggageBays);
    box(-5.24, level - 0.06, 2.32, 1.65, 0.12, 3.00, masonry, undefined, baggageBays);
    box(-5.35, level + 0.56, 2.22, 1.05, 0.13, 2.38, wood, undefined, baggageBays);
    box(-4.82, level + 0.56, 2.22, 0.03, 0.055, 2.38, brass, undefined, baggageBays);
    for (const z of [1.12, 3.31])
      for (const x of [-5.79, -4.91]) rod(x, level + 0.26, z, 0.05, 0.52, brass, undefined, baggageBays);
    // The landing-side track meets the cabin rail at each stop; the branch serves both stacks and the porter well.
    box(-4.97, level + 2.78, 2.04, 1.08, 0.08, 0.09, brass, undefined, baggageBays);
    box(-5.35, level + 2.78, 1.44, 0.09, 0.08, 2.85, brass, undefined, baggageBays);
    for (const z of [0.12, 2.76]) {
      box(-5.70, level + 2.86, z, 0.75, 0.08, 0.11, iron, undefined, baggageBays);
    }
    box(-5.93, level + 1.85, -0.70, 0.07, 4, 0.08, brass, undefined, baggageBays);
    const sign = plaque("SUITCASES  ·  " + (i || "RDC"), -5.975, level + 2.35, 2.22, 1.64, 0.34, baggageBays);
    sign.rotation.y = -Math.PI / 2;

  }
  const oppositeStation = new TransformNode("opposite wing service station", scene);
  oppositeStation.parent = world;
  baggageBays.clone("opposite baggage bays", oppositeStation);
  const oppositeDoors = shaftDoors.map(door => door.clone("opposite shaft safety gate", oppositeStation)!);

  // Both galleries are assembled in the East shaft frame. Only the East gallery
  // stays level: the West gallery climbs outside the tower to the same bridgehead.
  const bridge = new TransformNode("rescue crossings", scene);
  bridge.parent = world;
  const deck = exterior("deck");
  const posts = exterior("steel");
  let galleries: ReturnType<typeof buildGallery>[] = [];
  let bridgeFloor: number | null = null;
  let eastWalk: ReturnType<typeof createRefugeWalk>;
  let westWalk: ReturnType<typeof createRefugeWalk>;
  let ringOnDocking = false;
  const landingBell = new Audio(new URL("../art/bridge-bell.wav", import.meta.url).href);
  landingBell.volume = 0.5;

  /** Links the cabin sill to the fixed landing and continues in timber around the shaft. */
  function buildGallery(approach: readonly Vector3[], west: boolean) {
    const gallery = new TransformNode(west ? "West rescue gallery" : "East rescue gallery", scene);
    gallery.parent = bridge;
    const entry = approach[0];
    const corner = approach[1];
    const hallway = corner.subtract(entry).normalize();
    // The walking guide starts inside the car. Its first turn is on the stone landing.
    const points = [entry.add(hallway.scale(1.18)), ...approach.slice(1)];
    // The steel spans only the running clearance between the parquet and the stone.
    const sillZ = wingPoint(0, frontEdge, west).z;
    const landingEdge = west ? exchangeLanding.far : exchangeLanding.near;
    const dock = box(entry.x, entry.y - 0.05, (sillZ + landingEdge) / 2,
      1.8, 0.10, Math.abs(sillZ - landingEdge), posts, undefined, gallery);
    dock.name = "retractable threshold plate";
    dock.setEnabled(false);
    const normals = points.slice(1).map((point, i) => {
      const delta = point.subtract(points[i]);
      return new Vector3(delta.z, 0, -delta.x).normalize();
    });

    /** Locates a miter along a deck edge so railings meet on their centerline. */
    function edgeAt(index: number, side: number) {
      const before = normals[Math.max(0, index - 1)];
      const after = normals[Math.min(normals.length - 1, index)];
      const miter = before.add(after);
      return points[index].add(miter.scale(side * 0.9 / Vector3.Dot(miter, before)));
    }

    for (let i = 1; i < points.length; i++) {
      if (i > 1) {
        // The first timber run joins the stone landing without covering the compass.
        const start = points[i - 1].clone(), end = points[i];
        if (i === 2) start.x = exchangeLanding.right;
        const change = end.subtract(start);
        const length = change.length();
        const yaw = Math.atan2(change.x, change.z);
        const pitch = -Math.atan2(change.y, Math.hypot(change.x, change.z));
        const count = Math.ceil(length / 0.32);
        const spacing = length / count;
        for (let j = 0; j < count; j++) {
          const board = Vector3.Lerp(start, end, (j + 0.5) / count);
          box(board.x, board.y - 0.075, board.z, 1.8, 0.15, spacing - 0.011, deck, undefined, gallery)
            .rotation.set(pitch, yaw, 0);
        }
        if (i < points.length - 1)
          box(end.x, end.y - 0.10, end.z, 1.8, 0.19, 1.8, deck, undefined, gallery);
        // Carry the exposed timber runs on paired steel stringers under the deck.
        const offset = normals[i - 1].scale(0.72);
        const middle = Vector3.Center(start, end);
        for (const side of [-1, 1]) {
          const beam = middle.add(offset.scale(side));
          box(beam.x, beam.y - 0.27, beam.z, 0.14, 0.36, length, posts, undefined, gallery)
            .rotation.set(pitch, yaw, 0);
        }
      }
      for (const side of [-1, 1]) {
        const a = edgeAt(i - 1, side);
        const b = edgeAt(i, side);
        // Leave the inside of the East–West junction open to both entries.
        if (i === points.length - 1) {
          if (!west && side === -1) b.z += 0.9;
          if (west && side === 1) b.z -= 0.9;
        }
        const rail = b.subtract(a);
        const center = Vector3.Center(a, b);
        const railLength = rail.length();
        const railYaw = Math.atan2(rail.x, rail.z);
        const railPitch = -Math.atan2(rail.y, Math.hypot(rail.x, rail.z));
        for (const height of [0.49, 1.07]) {
          box(center.x, center.y + height, center.z, 0.065, 0.07, railLength, brass, undefined, gallery)
            .rotation.set(railPitch, railYaw, 0);
        }
        const intervals = Math.ceil(railLength / 2.2);
        for (let j = 0; j <= intervals; j++) {
          const point = Vector3.Lerp(a, b, j / intervals);
          box(point.x, point.y + 0.54, point.z, 0.07, 1.08, 0.07, posts, undefined, gallery);
        }
      }
    }
    if (west) {
      // Braced trestles ground each exposed change of direction on the lower rock shelf.
      const foot = -8 - bridge.position.y;
      for (const index of [2, 3, 4, 5]) {
        const point = points[index];
        const along = index === 2 || index === 5;
        for (const side of [-1, 1]) {
          const leg = point.add(new Vector3(along ? side * 0.65 : 0, 0, along ? 0 : side * 0.65));
          const height = point.y - foot;
          box(leg.x, foot + height / 2, leg.z, 0.18, height, 0.18, posts, undefined, gallery);
        }
        for (let y = point.y - 0.40; y > foot; y -= 3.2) {
          const tie = box(point.x, y, point.z, 0.13, 0.13, 1.50, posts, undefined, gallery);
          if (along) tie.rotation.y = Math.PI / 2;
          if (y - 3.2 <= foot) continue;
          for (const side of [-1, 1]) {
            const brace = box(point.x, y - 1.6, point.z, 0.085, Math.hypot(1.3, 3.2), 0.085, posts, undefined, gallery);
            brace.rotation.set(along ? 0 : side * Math.atan2(1.3, 3.2), 0, along ? side * Math.atan2(1.3, 3.2) : 0);
          }
        }
      }
      // Fill the common turn where the ramp meets the level approach and the shared span.
      const joint = points[points.length - 1];
      box(joint.x, -0.075, joint.z, 1.8, 0.15, 1.8, deck, undefined, gallery);
    }
    // Keep the opening threshold separate from the static deck batch.
    dock.parent = null;
    batchRigidParts(gallery);
    dock.parent = gallery;
    return { gallery, dock };
  }

  /** Books a single bridge elevation and attaches the two physically separate approaches. */
  function placeGallery(eastFloor: number) {
    if (bridgeFloor === eastFloor) return;
    bridgeFloor = eastFloor;
    bridge.position.y = eastFloor * 4;
    const east = bridgeApproach(false, 0);
    const west = bridgeApproach(true, (4 - 2 * eastFloor) * 4);
    galleries.forEach(({ gallery }) => gallery.dispose());
    galleries = [buildGallery(east, false), buildGallery(west, true)];
    const { far, threshold } = avalancheLanding;
    eastWalk = createRefugeWalk(east, far, threshold);
    westWalk = createRefugeWalk(west, far, threshold);
  }

  /** Extends both dock plates across the sill gaps and signals a newly secured cable. */
  function dockGallery(arrived: boolean) {
    galleries.forEach(({ dock }) => dock.setEnabled(arrived));
    if (arrived && ringOnDocking) {
      ringOnDocking = false;
      landingBell.currentTime = 0;
      // The passage remains visible when the browser silences audio.
      if (!document.hidden) void landingBell.play().catch(() => {});
    }
  }
  const partnerRack = new TransformNode("peer cabin luggage rack", scene);
  partnerRack.parent = partner;
  installAirWell(scene, shaft, brass, dark, masonry, plaque);
  const windowStart = scene.meshes.length;
  // Picture window on the left and glazed lift gate on the right share a deep walnut cornice.
  for (const [x, w] of [
    [-4.15, 0.55],
    [0.64, 0.58],
    [4.08, 0.73],
  ])
    box(x, 1.9, 3.5, w, 3.8, 0.35, wood);
  box(0, 3.53, 3.54, 8.83, 0.54, 0.24, wood);
  box(-1.68, 0.43, 3.5, 4.45, 0.86, 0.32, wood);
  for (let i = 0; i < 5; i++)
    box(-3.5 + i * 0.91, 0.44, 3.31, 0.76, 0.56, 0.07, inset);
  for (const [y, h, mat] of [
    [0.88, 0.12, dark],
    [0.955, 0.045, gold],
    [3.3, 0.15, wood],
    [3.42, 0.05, gold],
    [3.56, 0.23, dark],
    [3.69, 0.065, brass],
  ] as const)
    box(y < 1 ? -1.68 : 0, y, 3.4, y < 1 ? 4.65 : 8.8, h, 0.46, mat);
  for (const x of [-3.87, -2.55, -1.1, 0.37])
    box(x, 2.1, 3.51, 0.075, 2.35, 0.18, brass);
  box(-1.75, 2.81, 3.51, 4.22, 0.055, 0.18, brass);
  // Pleated drapes gather towards the sill. Individual folds retain a sculptural silhouette.
  for (const [x, s] of [
    [-3.87, 1],
    [0.32, -1],
  ])
    for (let i = 0; i < 8; i++) {
      const fold = rod(
        x + s * i * 0.064,
        2.15,
        3.06 + Math.cos(i * 1.6) * 0.075,
        0.05 + i * 0.002,
        2.48,
        i % 3 ? fabric : stripe,
        undefined,
        undefined,
        0.09,
      );
      fold.rotation.z = -s * (i - 2) * 0.023;
    }
  for (const x of [-3.75, 0.22]) box(x, 1.51, 2.96, 0.38, 0.065, 0.14, brass);
  // Each swag is a pleated hanging textile, with scalloped trim following its lower edge.
  for (let panel = 0; panel < 3; panel++) {
    const vertices: number[] = [],
      indices: number[] = [],
      normals: number[] = [];
    const edge: Vector3[] = [];
    for (let v = 0; v <= 12; v++)
      for (let u = 0; u <= 40; u++) {
        const t = u / 40,
          h = v / 12,
          curve = Math.sin(Math.PI * t),
          drop = 0.14 + 0.31 * curve;
        const x = -3.95 + (panel + t) * 1.46,
          y = 3.38 - h * drop,
          z =
            3.06 - 0.09 * curve - 0.055 * Math.cos(h * Math.PI * 6 + curve * 2);
        vertices.push(x, y, z);
        if (v === 12) edge.push(new Vector3(x, y, z));
        if (v && u) {
          const j = v * 41 + u;
          indices.push(j - 42, j - 41, j, j - 42, j, j - 1);
        }
      }
    const data = new VertexData();
    VertexData.ComputeNormals(vertices, indices, normals);
    data.positions = vertices;
    data.indices = indices;
    data.normals = normals;
    const mesh = new Mesh("draped valance", scene);
    data.applyToMesh(mesh);
    fabric.backFaceCulling = false;
    mesh.material = fabric;
    mesh.isPickable = false;
    MeshBuilder.CreateTube(
      "braided edging",
      { path: edge, radius: 0.014, tessellation: 5 },
      scene,
    ).material = gold;
  }
  for (const x of [1.04, 3.72]) {
    box(x, 1.69, 3.37, 0.18, 3.38, 0.25, dark);
    box(x, 1.7, 3.22, 0.048, 3.34, 0.08, brass);
  }
  for (const piece of scene.meshes.slice(windowStart)) piece.parent = cabin;
  batchRigidParts(cabin);
  // The lock is bolted to the fixed window-side jamb beside the sliding grille.
  const bridgeLock = new TransformNode("bridge inspection lock", scene);
  bridgeLock.position.set(0.74, 1.64, 3.24);
  box(0, 0, 0.01, 0.43, 1.05, 0.18, brass, "door", bridgeLock);
  box(0, 0, -0.087, 0.40, 1.02, 0.026, gold, "door", bridgeLock);
  const lockPlate = new DynamicTexture(
    "bridge lock nameplate",
    { width: 300, height: 720 },
    scene,
    false,
  );
  const lockInk = lockPlate.getContext() as CanvasRenderingContext2D;
  lockInk.fillStyle = "#332a20";
  lockInk.fillRect(0, 0, 300, 720);
  lockInk.strokeStyle = "#a58454";
  lockInk.lineWidth = 4;
  lockInk.strokeRect(8, 8, 284, 704);
  lockInk.fillStyle = "#ffde9e";
  lockInk.textAlign = "center";
  lockInk.font = "bold 35px Georgia";
  lockInk.fillText("BRIDGE LOCK", 150, 100);
  lockInk.fillRect(36, 131, 228, 3);
  const lockPlateFinish = finish("engraved bridge lock plate", "#ffffff", 0.08);
  lockPlateFinish.diffuseTexture = lockPlate;
  lockPlateFinish.emissiveColor = new Color3(0.14, 0.12, 0.09);
  place(
    MeshBuilder.CreatePlane("bridge lock nameplate", { width: 0.37, height: 0.98 }, scene),
    0, 0, -0.101, lockPlateFinish, "door", bridgeLock,
  );
  const releaseControls: ReadonlyArray<{ seal: Seal; label: string; row: number }> = [
    { seal: "stag", label: "Stag", row: 245 },
    { seal: "rose", label: "Rose", row: 418 },
    { seal: "key", label: "Key", row: 591 },
  ];
  for (const { seal, label, row } of releaseControls) {
    const y = (0.5 - row / 720) * 0.98;
    const bezel = rod(-0.073, y, -0.108, 0.066, 0.023, brass, "door", bridgeLock);
    bezel.rotation.x = Math.PI / 2;
    const badge = new DynamicTexture(`${label} release button`, 128, scene, false);
    const ink = badge.getContext() as CanvasRenderingContext2D;
    ink.translate(64, 64);
    ink.scale(1.43, 1.43);
    drawSealMark(ink, seal);
    badge.update();
    badge.hasAlpha = true;
    const badgeFinish = finish(`${label} enamel`, "#ffffff", 0.1);
    badgeFinish.diffuseTexture = badge;
    badgeFinish.emissiveColor = new Color3(0.12, 0.10, 0.08);
    place(
      MeshBuilder.CreateDisc(`${label} release button`, { radius: 0.056, tessellation: 32 }, scene),
      -0.073, y, -0.12, badgeFinish, "door", bridgeLock,
    );
    lockInk.font = "43px Georgia";
    lockInk.fillText(label, 208, row + 15);
  }
  lockPlate.update();
  for (const x of [-0.176, 0.176])
    for (const y of [-0.45, 0.45]) {
      const screw = rod(x, y, -0.107, 0.013, 0.022, gold, "door", bridgeLock);
      screw.rotation.x = Math.PI / 2;
    }
  const leaf = new TransformNode("opening grille", scene);
  for (let i = 0; i < 9; i++) {
    const x = 1.2 + i * 0.294;
    if (x > 1.88 && x < 3.24) {
      rod(x, 0.24, 3.39, 0.021, 0.40, brass, "door");
      rod(x, 2.27, 3.39, 0.021, 2.02, brass, "door");
    } else {
      rod(x, 1.66, 3.39, 0.021, 3.24, brass, "door");
    }
    const arc = ball(x, 2.77, 3.37, 0.045, gold, "door");
    arc.scaling.y = 1.9;
  }
  for (const y of [0.24, 0.32, 1.24, 2.72, 3.16])
    box(2.38, y, 3.38, 2.65, 0.045, 0.07, brass, "door");
  for (let i = 0; i < 8; i++)
    for (const sign of [-1, 1]) {
      const bar = box(
        1.34 + i * 0.294,
        1.77,
        3.37,
        0.018,
        0.61,
        0.04,
        brass,
        "door",
      );
      bar.rotation.z = sign * 0.48;
    }
  // The guest-facing hatch is inset into the grille below its main click area.
  const hatchPick = box(2.56, 0.83, 3.22, 1.46, 0.79, 0.035, dark, "partner", leaf);
  hatchPick.name = "exchange hatch opening";
  hatchPick.visibility = 0.001;
  box(2.56, 0.44, 3.31, 1.52, 0.10, 0.41, brass, "partner", leaf);
  box(2.56, 1.16, 3.28, 1.52, 0.17, 0.27, wood, "partner", leaf);
  for (const x of [1.88, 3.24])
    box(x, 0.84, 3.28, 0.09, 0.72, 0.24, brass, "partner", leaf);
  plaque("PASSAGE", 2.56, 1.16, 3.13, 0.90, 0.16, leaf);
  const doorPick = box(2.38, 2.23, 3.44, 2.62, 1.89, 0.035, iron, "door");
  doorPick.visibility = 0.001;
  // Two electric wall sconces throw warm pools across brass and velvet.
  for (const x of [-4.08, 0.72, 3.99]) {
    box(x, 2.34, 3.18, 0.15, 0.33, 0.12, brass);
    rod(x, 2.43, 2.94, 0.07, 0.27, brass);
    const globe = ball(x, 2.65, 2.93, 0.146, lamp);
    globe.scaling.y = 1.37;
    const light = new PointLight("sconce", new Vector3(x, 2.64, 2.64), scene);
    light.diffuse = new Color3(1, 0.77, 0.48);
    light.intensity = 0.54;
    light.range = 4;
  }
  // Luggage rack and individually strapped cases occupy the near side of the window.
  box(-2.65, 0.49, 2.04, 2.26, 0.24, 0.88, wood, "load");
  box(-2.65, 0.37, 1.61, 2.26, 0.035, 0.055, brass, "load");
  for (const z of [1.70, 1.92, 2.14, 2.36])
    box(-2.65, 0.615, z, 2.12, 0.025, 0.063, gold, "load");
  for (const x of [-3.61, -1.69])
    for (const z of [1.68, 2.4]) rod(x, 0.24, z, 0.043, 0.48, brass, "load");
  const trunkFace = art(
    "travel labels",
    new URL("../art/trunk.jpg", import.meta.url).href,
    0.09,
  );
  /** Builds a case in rack coordinates, with inspection meshes on the guest's own cases. */
  function makeCase(name: string, target?: string) {
    const c = new TransformNode(name, scene);
    c.setEnabled(false);
    box(0, 0, 0, 0.98, 0.4, 0.63, leather, target, c);
    place(
      MeshBuilder.CreatePlane(
        "case panels",
        { width: 0.977, height: 0.398 },
        scene,
      ),
      0,
      0,
      -0.322,
      trunkFace,
      target,
      c,
    );
    for (const x of [-0.31, 0.31]) {
      box(x, 0, 0, 0.056, 0.41, 0.64, dark, target, c);
      box(x, 0.015, -0.329, 0.088, 0.081, 0.019, gold, target, c);
    }
    for (const x of [-0.16, 0.16]) {
      box(x, 0.04, -0.35, 0.075, 0.10, 0.055, brass, target, c);
      box(x, 0.018, -0.41, 0.05, 0.054, 0.10, gold, target, c);
    }
    box(0, 0.005, -0.465, 0.34, 0.052, 0.064, dark, target, c);
    if (target) {
      const handlePick = box(0, 0.023, -0.465, 0.42, 0.18, 0.064, dark, target, c);
      handlePick.visibility = 0.001;
    }
    return c;
  }
  const cases = Array.from({ length: 18 }, (_, i) => {
    const name = `case ${i}`;
    return makeCase(name, name);
  });
  /** Fits a leather document pocket and the brass strap ends to the outward
   * face of the travelling rack's cases, visible from the gallery. */
  function fitOutwardCaseFace(item: TransformNode) {
    const panel = place(MeshBuilder.CreatePlane("outward travel labels", {width: 0.977, height: 0.398}, scene),
      0, 0, 0.322, trunkFace, undefined, item);
    panel.rotation.y = Math.PI;
    for (const x of [-0.31, 0.31])
      box(x, 0.015, 0.33, 0.088, 0.081, 0.024, gold, undefined, item);
    box(0, 0.18, 0.29, 0.27, 0.036, 0.13, brass, undefined, item);
  }
  const partnerCases = [-3.13, -2.08, -3.13].map((x, slot) => {
    const item = makeCase(`opposite ballast case ${slot}`);
    fitOutwardCaseFace(item);
    item.parent = partnerRack;
    item.position.set(x, slot === 2 ? 1.255 : 0.835, 2.04);

    return item;
  });
  // A sealed green dispensary chest has its own stores shelf above the loose ballast.
  const medicalPaint = finish("dispensary green enamel", "#1c5b50");
  /** Fits the same dispensary emblem to the chest and its counterpart in the passing car. */
  function markMedicalCase(item: TransformNode, target?: string) {
    for (const z of [-0.34, 0.34]) {
      box(0, 0, z, 0.61, 0.35, 0.022, medicalPaint, target, item);
      const face = z + Math.sign(z) * 0.018;
      box(0, 0.018, face, 0.08, 0.24, 0.02, cream, target, item);
      box(0, 0.018, face, 0.24, 0.08, 0.02, cream, target, item);
    }
  }
  const medicalCase = makeCase("collect_rescue_case", "collect_rescue_case");
  markMedicalCase(medicalCase, medicalCase.name);
  const oppositeMedical = makeCase("opposite dispensary chest");
  fitOutwardCaseFace(oppositeMedical);
  markMedicalCase(oppositeMedical);
  oppositeMedical.parent = partnerRack;
  oppositeMedical.position.copyFrom(partnerCases[0].position);
  oppositeMedical.scaling.copyFrom(partnerCases[0].scaling);
  const dispensary = new TransformNode("dispensary shelf", scene);
  dispensary.parent = shaft;
  box(-5.35, 2.02, 1.75, 0.9, 0.10, 1.15, medicalPaint, undefined, dispensary);
  for (const z of [1.33, 2.17])
    box(-5.74, 1.87, z, 0.18, 0.30, 0.1, brass, undefined, dispensary);
  // A suspended monorail stops just inside the cabin edge so it clears the stationary landing track.
  box(-3.08, 2.78, 2.04, 2.58, 0.08, 0.09, brass);
  for (const x of [-4.12, -1.87]) {
    rod(x, 3.27, 2.04, 0.027, 0.93, iron);
    box(x, 3.72, 2.04, 0.17, 0.045, 0.17, brass);
  }
  const trolley = new TransformNode("travelling baggage winch", scene);
  box(0, -0.07, 0, 0.27, 0.15, 0.22, iron, undefined, trolley);
  for (const x of [-0.09, 0.09]) {
    const wheel = rod(x, -0.005, -0.055, 0.059, 0.12, brass, undefined, trolley);
    wheel.rotation.x = Math.PI / 2;
  }
  const drum = rod(0, -0.16, 0, 0.07, 0.23, brass, undefined, trolley);
  drum.rotation.z = Math.PI / 2;
  // The short travelling section bridges the overhead track into the clear vertical well.
  const shuttle = new TransformNode("porter well rail shuttle", scene);
  box(0, 0, 0.35, 0.09, 0.08, 0.74, brass, undefined, shuttle);
  box(-0.28, 0.07, 0, 0.65, 0.08, 0.10, iron, undefined, shuttle);
  const cable = rod(0, 0, 0, 0.018, 1, iron);
  const grip = new TransformNode("swivelling trunk clamp", scene);
  box(0, 0.28, 0, 0.72, 0.076, 0.094, brass, undefined, grip);
  // Twin jaws bear on the case's existing leather straps, clear of its neighbours in either stack.
  for (const x of [-0.31, 0.31]) {
    box(x, 0.245, 0, 0.047, 0.055, 0.70, iron, undefined, grip);
    for (const z of [-0.336, 0.336]) {
      box(x, 0.18, z, 0.095, 0.16, 0.045, brass, undefined, grip);
    }
  }
  /** Runs the trolley on the rail and pays out the cable until the clamp contacts the trunk. */
  function positionHoist(x: number, y: number, z: number, yaw: number, railOffset: number) {
    trolley.position.set(x, 2.78 + railOffset, z);
    shuttle.position.set(-5.35, trolley.position.y, -0.70);
    grip.position.set(x, y, z);
    grip.rotation.y = yaw;
    const bottom = y + 0.30;
    const top = trolley.position.y - 0.16;
    cable.position.set(x, (bottom + top) / 2, z);
    cable.scaling.y = top - bottom;
  }
  positionHoist(-2.64, 2.17, 2.04, 0, 0);
  const baggage = createBaggageTransfers(cases, medicalCase, cabin, shaft, positionHoist);
  plaque("SUITCASES", -2.65, 0.48, 1.575, 1.02, 0.19);
  // A pedestal-mounted hand brake pivots as the server acknowledges its position.
  rod(0.64, 0.13, 2.13, 0.32, 0.24, dark, "brake", undefined, 0.26);
  rod(0.64, 0.71, 2.13, 0.18, 0.97, brass, "brake", undefined, 0.13);
  ball(0.64, 1.19, 2.13, 0.18, brass, "brake");
  const lever = new TransformNode("brake pivot", scene);
  lever.position.set(0.64, 1.19, 2.13);
  rod(0, 0.23, 0, 0.042, 0.47, iron, "brake", lever);
  ball(0, 0.5, 0, 0.115, dark, "brake", lever);
  plaque("FREIN", 0.64, 0.79, 1.94, 0.36, 0.12);

  const localKeySlot = new Vector3(0.84, 1.10, 1.94);
  const oppositeSlot = wingPoint(localKeySlot.x, localKeySlot.z, true);
  const peerKeySlot = new Vector3(oppositeSlot.x, localKeySlot.y, oppositeSlot.z);
  /** Adds a keyed switch with a dark keyway and a separate interaction from the brake lever. */
  function keySwitch(name: string, slot: Vector3, parent?: TransformNode) {
    const { x, y, z } = slot;
    const target = parent ? undefined : "brake key switch";
    box(x - 0.06, y - 0.087, z + 0.07, 0.28, 0.13, 0.23, iron, target, parent).name = name + " housing";
    rod(x, y - 0.017, z, 0.11, 0.048, brass, target, parent).name = name + " bezel";
    rod(x, y + 0.009, z, 0.079, 0.012, gold, target, parent).name = name + " face";
    box(x, y + 0.016, z, 0.056, 0.004, 0.022, iron, target, parent).name = name + " keyway";
  }
  keySwitch("brake key switch", localKeySlot);

  /** Marks the key-operated contactor with the foundry's key-and-bridge emblem. */
  function circuitBadge(parent: TransformNode, x: number, y: number, z: number, width: number) {
    const name = parent.name + " circuit emblem";
    const face = new DynamicTexture(name, {width: 256, height: 96}, scene, false);
    const ink = face.getContext() as CanvasRenderingContext2D;
    ink.fillStyle = "#163536";
    ink.fillRect(0, 0, 256, 96);
    ink.strokeStyle = "#f2d399";
    ink.lineWidth = 7;
    ink.beginPath();
    ink.arc(33, 47, 16, 0, Math.PI * 2);
    ink.moveTo(49, 47); ink.lineTo(99, 47); ink.lineTo(99, 62);
    ink.moveTo(86, 47); ink.lineTo(86, 57);
    ink.moveTo(119, 67); ink.lineTo(237, 67);
    ink.moveTo(131, 76); ink.lineTo(131, 45); ink.lineTo(178, 18);
    ink.lineTo(224, 45); ink.lineTo(224, 76);
    ink.moveTo(154, 32); ink.lineTo(154, 67);
    ink.moveTo(201, 32); ink.lineTo(201, 67);
    ink.stroke();
    face.update();
    const enamelFace = finish(name, "#ffffff");
    enamelFace.diffuseTexture = face;
    enamelFace.emissiveColor = new Color3(0.18, 0.18, 0.13);
    place(MeshBuilder.CreatePlane(name, {width, height: width * 0.375}, scene),
      x, y, z, enamelFace, undefined, parent);
  }
  const master = new TransformNode("West key-operated bridge contactor", scene);
  master.position.set(1.01, 1.07, 2.07);
  box(0, 0, 0, 0.31, 0.53, 0.11, iron, "brake key switch", master);
  plaque("WEST ROOM", 0, 0.19, -0.061, 0.30, 0.08, master);
  circuitBadge(master,0,0.112,-0.061,0.21);
  const knife = new TransformNode("bridge knife contact", scene);
  knife.parent = master;
  knife.position.set(-0.075,-0.16,-0.085);
  box(0, 0.11, 0, 0.027, 0.22, 0.032, gold, "brake key switch", knife);
  box(-0.075, 0.064, -0.08, 0.061, 0.042, 0.035, brass, "brake key switch", master);
  const pilot = finish("West bridge circuit opal", "#33413b");
  ball(0.085, -0.04, -0.074, 0.045, pilot, "brake key switch", master);
  const freight = new TransformNode("freight scale interlock", scene);
  freight.parent = bridgeLock;
  freight.position.y = -0.69;
  box(0, 0, 0, 0.54, 0.27, 0.15, iron, "door", freight);
  plaque("FREIGHT", 0, 0.052, -0.081, 0.43, 0.11, freight);
  const freightGlass = finish("rectangular freight semaphore", "#ac552a");
  box(0, -0.06, -0.081, 0.27, 0.045, 0.018, freightGlass, "door", freight);
  const enamel = finish("brake key teal enamel", "#266e69", 0.46);
  const brakeKey = new TransformNode("transferable brake key", scene);
  const keyTarget = "brake key";
  const bow = place(MeshBuilder.CreateTorus("brake key brass bow", {
    diameter: 0.17, thickness: 0.025, tessellation: 24,
  }, scene), 0, 0, 0, gold, keyTarget, brakeKey);
  bow.rotation.x = Math.PI / 2;
  bow.scaling.x = 1.2;
  box(0, -0.105, 0, 0.042, 0.162, 0.028, brass, keyTarget, brakeKey).name = "brake key stem";
  box(0.02, -0.164, 0, 0.045, 0.033, 0.028, gold, keyTarget, brakeKey).name = "brake key bit";
  box(0, 0, 0, 0.122, 0.071, 0.029, enamel, keyTarget, brakeKey).name = "brake key enamel bow";
  box(0, 0, -0.016, 0.048, 0.012, 0.004, gold, keyTarget, brakeKey).name = "brake key bow inlay";

  const keySeatingHeight = 0.176;
  const localKeySeat = localKeySlot.add(new Vector3(0, keySeatingHeight, 0));
  const peerKeySeat = peerKeySlot.add(new Vector3(0, keySeatingHeight, 0));
  const oppositeHatch = wingPoint(2.53, 3.34, true);
  const keyPath = [
    { position: localKeySeat, pitch: 0, time: 0 },
    { position: localKeySeat.add(new Vector3(0, 0.22, 0)), pitch: 0, time: 0.18 },
    { position: new Vector3(2.53, 0.81, 3.34), pitch: -0.55, time: 0.76 },
    { position: new Vector3(oppositeHatch.x, 0.81, oppositeHatch.z), pitch: -0.55, time: 1.26 },
    { position: peerKeySeat.add(new Vector3(0, 0.18, 0)), pitch: 0, time: 1.60 },
    { position: peerKeySeat, pitch: 0, time: 1.77 },
  ];
  const keyTravelTime = keyPath[keyPath.length - 1].time;
  let soundRelayOnSeat = false;

  /** Samples the transfer between the two switches, keeping the seated key in its car's frame. */
  function positionBrakeKey(keyTime: number, otherCarOffset: number) {
    const segment = keyPath.findIndex((pose, i) => i > 0 && keyTime <= pose.time);
    const start = keyPath[segment - 1];
    const end = keyPath[segment];
    const fraction = (keyTime - start.time) / (end.time - start.time);
    const ease = fraction * fraction * (3 - 2 * fraction);
    Vector3.LerpToRef(start.position, end.position, ease, brakeKey.position);
    // Blend in the facing car's vertical offset as the key crosses the gap.
    brakeKey.position.y += otherCarOffset * Math.min(1, Math.max(0, (keyTime - 0.76) / 0.50));
    brakeKey.rotation.x = start.pitch + (end.pitch - start.pitch) * ease;
  }
  /** Soft contact patches ground the furnishings against the parquet. */
  function contact(
    x: number,
    z: number,
    w: number,
    d: number,
    parent?: TransformNode,
    enamel?: string,
  ) {
    const tex = new DynamicTexture("contact falloff", 128, scene, false),
      ctx = tex.getContext() as CanvasRenderingContext2D;
    const fade = ctx.createRadialGradient(64, 64, 6, 64, 64, 64);
    fade.addColorStop(0, "rgba(26,15,10,.65)");
    fade.addColorStop(0.5, "rgba(29,16,10,.37)");
    fade.addColorStop(1, "rgba(28,15,10,0)");
    ctx.fillStyle = fade;
    ctx.fillRect(0, 0, 128, 128);
    tex.hasAlpha = true;
    tex.update();
    const material = finish("cast shadow", "#ffffff", 0);
    material.diffuseTexture = tex;
    material.useAlphaFromDiffuseTexture = true;
    material.disableLighting = true;
    material.emissiveColor = Color3.White();
    const plane = place(
      MeshBuilder.CreateGround("contact", { width: w, height: d }, scene),
      x,
      0.067,
      z,
      material,
      undefined,
      parent,
    );
    plane.alphaIndex = 1;
  }
  contact(-2.65, 2.03, 2.9, 1.5);
  contact(0.64, 2.13, 1.05, 0.85);
  const velvet = finish("moss velvet", "#53614b", 0.22);
  /** Upholstered tub chair with a piped buttoned back and carved wooden feet. */
  function armchair(parent: TransformNode) {
    const cushion = ball(0, 0.47, 0, 0.5, velvet, undefined, parent);
    cushion.scaling.set(1.24, 0.36, 1);
    const back = ball(0, 0.93, 0.26, 0.5, velvet, undefined, parent);
    back.scaling.set(1.27, 1.14, 0.31);
    back.rotation.x = 0.13;
    for (const side of [-1, 1]) {
      const arm = ball(
        side * 0.48,
        0.7,
        -0.025,
        0.25,
        velvet,
        undefined,
        parent,
      );
      arm.scaling.set(0.61, 0.81, 1.95);
      for (const z of [-0.32, 0.3]) {
        const foot = rod(
          side * 0.39,
          0.21,
          z,
          0.058,
          0.36,
          wood,
          undefined,
          parent,
          0.089,
        );
        foot.rotation.z = side * 0.1;
        ball(side * 0.39, 0.36, z, 0.095, wood, undefined, parent);
      }
    }
    for (let j = 0; j < 3; j++)
      for (let k = 0; k < 3; k++)
        ball(
          (k - 1) * 0.23 + (j % 2) * 0.08,
          0.8 + j * 0.19,
          0.115,
          0.026,
          brass,
          undefined,
          parent,
        );
    const welt = MeshBuilder.CreateTube(
      "back piping",
      {
        path: Array.from(
          { length: 49 },
          (_, i) =>
            new Vector3(
              0.61 * Math.cos((i * Math.PI) / 24),
              0.94 + 0.55 * Math.sin((i * Math.PI) / 24),
              0.23,
            ),
        ),
        radius: 0.012,
        tessellation: 5,
      },
      scene,
    );
    welt.parent = parent;
    welt.material = gold;
    welt.isPickable = false;
  }
  const chair = new TransformNode("reading chair", scene);
  chair.position.set(-3.15, 0, 0.47);
  chair.rotation.y = -0.65;
  armchair(chair);
  contact(0, 0, 1.72, 1.35, chair);
  // A marble-topped occasional table holds the room's reading lamp and a leather-bound book.
  const table = new TransformNode("salon table", scene);
  table.position.set(3.08, 0, 0.92);
  rod(0, 0.1, 0, 0.36, 0.13, wood, undefined, table, 0.26);
  rod(0, 0.41, 0, 0.11, 0.58, wood, undefined, table, 0.14);
  ball(0, 0.36, 0, 0.17, inset, undefined, table);
  rod(0, 0.75, 0, 0.6, 0.09, brass, undefined, table);
  rod(0, 0.81, 0, 0.61, 0.065, cream, undefined, table);
  const book = box(
    -0.21,
    0.88,
    -0.19,
    0.36,
    0.071,
    0.24,
    fabric,
    undefined,
    table,
  );
  book.rotation.y = 0.23;
  rod(0.14, 0.88, 0.04, 0.16, 0.07, brass, undefined, table);
  rod(0.14, 1.12, 0.04, 0.038, 0.46, brass, undefined, table);
  ball(0.14, 1.12, 0.04, 0.085, brass, undefined, table);
  rod(0.14, 1.43, 0.04, 0.275, 0.33, lamp, undefined, table, 0.13);
  rod(0.14, 1.27, 0.04, 0.28, 0.027, brass, undefined, table);
  for (let i = 0; i < 16; i++) {
    const t = (i * Math.PI) / 8;
    const rib = rod(
      0.14 + 0.2 * Math.cos(t),
      1.43,
      0.04 + 0.2 * Math.sin(t),
      0.005,
      0.35,
      brass,
      undefined,
      table,
    );
    rib.rotation.z = 0.39 * Math.cos(t);
    rib.rotation.x = -0.39 * Math.sin(t);
  }
  ball(0.14, 1.64, 0.04, 0.043, gold, undefined, table);
  contact(0, 0, 1.52, 1.25, table);
  // A cabin guide sheave rolls along the fixed shaft rope as the cabin travels.
  const sheave = new TransformNode("cable guide axle", scene);
  sheave.position.set(0.67, 2.98, 3.09);
  const pulley = MeshBuilder.CreateTorus(
    "cable sheave",
    { diameter: 0.36, thickness: 0.06, tessellation: 24 },
    scene,
  );
  place(pulley, 0, 0, 0, iron, "pulley", sheave);
  pulley.rotation.x = Math.PI / 2;
  rod(0.5, 2.57, 3.09, 0.017, 2.29, iron, "pulley").name = "shaft guide rope";
  for (let i = 0; i < 5; i++) {
    const spoke = box(0, 0, 0, 0.018, 0.34, 0.026, brass, "pulley", sheave);
    spoke.rotation.z = (i * Math.PI) / 5;
  }
  for (const side of [-1, 1]) {
    const x = side < 0 ? -3.69 : 0.15;
    rod(x, 1.44, 2.87, 0.018, 0.2, gold);
    rod(x, 1.29, 2.87, 0.043, 0.13, brass, undefined, undefined, 0.018);
  }
  for (const m of scene.meshes)
    if (m.metadata === "door" && !m.parent && m.position.y < 3.4 && m.position.x > 1) {
      m.parent = leaf;
    }
  let state: HotelSceneState = {
    room: "warm",
    floor: 0,
    partnerFloor: 4,
    partnerDestination: 2,
    gateOpen: false,
    partnerGateOpen: false,
    bridgePowered: false,
    freightClear: null,
    partnerEscaped: false,
    partnerBridgePose: null,
    lockTripped: false,
    partnerLockTripped: false,
    hasBrakeKey: true,
    loads: 0,
    partnerLoads: 0,
    route: "escape",
    atRescueBridge: false,
    landingLoads: [0, 0, 0, 0, 0],
    partnerLandingLoads: [],
    escaped: false,
    onLanding: false,
    canStepOntoLanding: false,
    canUnloadCase: false,
    freightStatus: null, freightFloor: null, canCollectFreight: false,
    loadableRackFloors: [],
  };
  // Only physical suite fixtures are copied. The local camera, documents, live
  // freight and the landing-side transfer shuttle have separate ownership.
  const localOnly = new Set([world, cabin, shaft, partner, brakeKey, ...cases, medicalCase,
    shuttle]);
  const suiteFixtures = scene.rootNodes.filter((node): node is TransformNode =>
    node instanceof TransformNode && !localOnly.has(node));
  const updateStory = createHotelStory(scene);
  const beforeDetails = new Set(scene.rootNodes);
  const roomDetails = createRoomDetails(scene, { brass, gold, dark, wood, inset, paper, lamp });
  const chairFootprint = { minX: 0, maxX: 0, minZ: -0.26, maxZ: 1.21 };
  const tableFootprint = { minX: 0, maxX: 0, minZ: 0.31, maxZ: 1.53 };

  /** Aligns the collision footprints with the furniture's wing-specific placements. */
  function positionFurniture(room: HotelSceneState["room"]) {
    chair.position.x = room === "warm" ? -3.15 : 3.04;
    chair.rotation.y = room === "warm" ? -0.65 : 0.71;
    chairFootprint.minX = chair.position.x - 0.77;
    chairFootprint.maxX = chair.position.x + 0.77;
    table.position.x = room === "warm" ? 3.08 : -3.29;
    tableFootprint.minX = table.position.x - 0.61;
    tableFootprint.maxX = table.position.x + 0.61;
  }

  // Each assembly moves as a unit. Keep separate batches for distinct selectable objects.
  for (const assembly of [leaf, bridgeLock, chair, table, sheave, ...shaftDoors, ...serviceLeaves,
    ...cases, ...partnerCases, medicalCase, oppositeMedical]) batchRigidParts(assembly);

  // Both views use the same stationary floor-and-slot addresses for public luggage.
  const peerLandingSlots = shaftDoors.map((_, floor) => Array.from({ length: 6 }, (_, slot) => {
    const stand = new TransformNode("opposite suitcase on landing", scene);
    stand.parent = oppositeStation;
    stand.position.copyFrom(rackPosition(floor + 1, slot));
    stand.position.y += 4 * floor;
    stand.rotation.y = -Math.PI / 2;
    for (const part of partnerCases[0].getChildMeshes()) {
      if (!(part instanceof Mesh)) continue;
      const instance = part.createInstance("opposite luggage");
      instance.parent = stand;
      instance.isPickable = false;
    }
    stand.setEnabled(false);
    return stand;
  }));

  const roomFabricsAndFinishes = [fabric, stripe, rug, wood, inset, velvet];
  const peerMaterials = new Map(roomFabricsAndFinishes.map(material =>
    [material, material.clone("other wing " + material.name)!] as const));
  const peerLockPlateFinish = lockPlateFinish.clone("other wing engraved bridge lock plate")!;
  peerMaterials.set(lockPlateFinish, peerLockPlateFinish);
  const peerFixtures = new Map<TransformNode, TransformNode>();
  const detailRoots = scene.rootNodes.filter((node): node is TransformNode =>
    node instanceof TransformNode && !beforeDetails.has(node));
  for (const fixture of [...cabin.getChildren().filter((node): node is TransformNode => node instanceof TransformNode),
    ...suiteFixtures, ...detailRoots]) {
    const copy = fixture.clone("peer " + fixture.name, partner);
    if (!copy) continue;
    peerFixtures.set(fixture, copy);
    for (const mesh of [...(copy instanceof Mesh ? [copy] : []), ...copy.getChildMeshes()]) {
      mesh.isPickable = false;
      mesh.metadata = null;
      if (mesh.material instanceof StandardMaterial)
        mesh.material = peerMaterials.get(mesh.material) ?? roomDetails.peerFinishes.get(mesh.material) ?? mesh.material;
    }
  }
  batchRigidParts(partner);
  const peerLever = peerFixtures.get(lever)!;
  const peerGate = peerFixtures.get(leaf)!;
  const peerKnife = peerFixtures.get(master)!.getChildTransformNodes(true)
    .find(node => node.name.endsWith(knife.name))!;
  const peerServiceLeaves = peerFixtures.get(serviceDoorway)!.getChildTransformNodes(true)
    .filter(panel => panel.name.includes("sliding service door panel"));
  const updateSuspension = createSuspension(scene, shaft, cabin, partner, shaftMetal, brass);
  /** Makes a two-sided gate header with independent circuit, dock and inspection readings. */
  function gateInstrument(car: TransformNode, target?: string) {
    const name = car.name + " bridge gate status";
    const face = new DynamicTexture(name, { width: 512, height: 240 }, scene, true);
    const ink = face.getContext() as CanvasRenderingContext2D;
    const glass = finish(name, "#ffffff", 0);
    glass.emissiveTexture = face;
    glass.disableLighting = true;
    for (const outside of [false, true]) {
      const z = outside ? 3.81 : 3.08;
      box(2.56, 3.42, outside ? 3.73 : 3.23, 0.35, 0.30, outside ? 0.14 : 0.27, iron, target, car);
      box(2.56, 3.42, z, 1.66, 0.62, 0.095, brass, target, car);
      const panel = place(
        MeshBuilder.CreatePlane(name, { width: 1.60, height: 0.58 }, scene),
        2.56, 3.42, z + (outside ? 0.051 : -0.051), glass, target, car,
      );
      if (outside) panel.rotation.y = Math.PI;
    }
    let previous = "";
    /** Repopulates the shared face only when one of the physical readings changes. */
    return function update(energized: boolean, docked: boolean, open: boolean, tripped: boolean) {
      const gate = open ? "UNLOCKED" : "LOCKED";
      const circuit = energized ? "POWER ON" : "NO POWER";
      const dock = docked ? "GANGWAY DOCKED" : "GANGWAY RETRACTED";
      const seal = open ? "SEAL ACCEPTED" : tripped ? "SEAL TRIPPED" : "SEAL PENDING";
      const readings = [gate, circuit, dock, seal].join("/");
      if (previous === readings) return;
      previous = readings;
      ink.fillStyle = "#222521";
      ink.fillRect(0, 0, 512, 240);
      ink.fillStyle = energized ? "#254c3e" : "#643927";
      ink.fillRect(8, 79, 496, 80);
      ink.textAlign = "center";
      ink.fillStyle = "#f6dda5";
      ink.font = "bold 28px sans-serif";
      ink.fillText("WEST ROOM SOCKET CIRCUIT", 256, 45);
      ink.font = "bold 38px sans-serif";
      ink.fillText(gate, 137, 133);
      ink.fillText(circuit, 384, 133);
      ink.fillRect(255, 94, 2, 48);
      ink.font = "bold 20px sans-serif";
      ink.fillText(dock, 137, 205);
      ink.fillText(seal, 384, 205);
      face.update();
    };
  }
  const localGateInstrument = gateInstrument(cabin, "door");
  const peerGateInstrument = gateInstrument(partner);
  /** Mounts a legible wing name on both outward-facing fascias. */
  function wingLabels(car: TransformNode, wing: string) {
    const enamel = wing === "EAST ROOM" ? "#763629" : "#1c4a58";
    const front = plaque(wing, -1.25, 3.53, 3.73, 3.8, 0.5, car, enamel);
    front.rotation.y = Math.PI;
    const rear = plaque(wing, 0, 3.50, rearWallZ - 0.19, 3.8, 0.52, car, enamel);
    const sides = [-1, 1].map(side => {
      const sign = plaque(wing, side * 4.54, 0.54, -2.1, 4.65, 0.75, car, enamel);
      sign.rotation.y = -side * Math.PI / 2;
      return sign;
    });
    return [front, rear, ...sides];
  }
  const eastLabels = [wingLabels(cabin, "EAST ROOM"), wingLabels(partner, "EAST ROOM")];
  const westLabels = [wingLabels(cabin, "WEST ROOM"), wingLabels(partner, "WEST ROOM")];

  const interactions = new Map<string, string>([
    ["pulley", "brake"],
    [keyTarget, "key"],
    ["brake key switch", "switch"],
  ]);
  const outlines = new SelectionOutlineLayer("hovered furnishing", scene, { mainTextureRatio: 1 });
  outlines.outlineColor = Color3.FromHexString("#8ae5dc");
  outlines.outlineThickness = 1.75;
  outlines.occlusionStrength = 1;
  const furnishings = new Map<string, Mesh[]>();
  for (const mesh of scene.meshes) {
    if (!(mesh instanceof Mesh) || typeof mesh.metadata !== "string" || mesh.visibility < 1) continue;
    const parts = furnishings.get(mesh.metadata);
    if (parts) parts.push(mesh);
    else furnishings.set(mesh.metadata, [mesh]);
  }

  /** Resolves a case through its current rack and the authoritative action catalogue. */
  function caseInteraction(id: string) {
    if (id === medicalCase.name) return state.canCollectFreight ? id : null;
    const target = baggage.caseTarget(id);
    if (target === "unload") return state.canUnloadCase ? target : null;
    if (target && !state.loadableRackFloors.includes(Number(target.slice("load:".length)))) return null;
    return target;
  }

  let revealObjects = false;
  let hoveredObject: string | null = null;
  let lastReveal = -Infinity;
  /** Both pointer picking and the optional overview follow the current location and instrument state. */
  function usableObject(id: string) {
    if (id === "reach_refuge") return doorReachable;
    return !state.gateOpen && !state.onLanding && !travelling && caseInteraction(id) !== null;
  }
  /** Highlights physical parts together and omits unavailable suitcase slots. */
  function refreshOutlines() {
    outlines.clearSelection();
    for (const [id, parts] of furnishings) {
      if ((id === hoveredObject || revealObjects) && usableObject(id)) {
        outlines.addSelection(parts.filter(part => part.isEnabled() && !part.isDescendantOf(partner)));
      }
    }
  }
  /** Selects the visible parts of the hovered object and shares its action with the hint. */
  function hoverFurnishing(id: string | null) {
    hoveredObject = id;
    refreshOutlines();
    const keyedSwitch = id === keyTarget || id === "brake key switch";
    onTargetChange?.(id === null || keyedSwitch ? id : caseInteraction(id) ?? interactions.get(id) ?? id);
  }

  positionFurniture(state.room);
  const cabinBounds = { minX: -4.22, maxX: 4.22, minZ: rearWallZ + 0.04, maxZ: 3.13 };
  const landingBounds = { minX: -3.37, maxX: 3.37, minZ: -12.35, maxZ: rearWallZ - 0.20 };
  const walkingBounds = { ...cabinBounds };
  const cabinFurniture = [
    chairFootprint,
    tableFootprint,
    { minX: -3.78, maxX: -1.52, minZ: 1.6, maxZ: 2.48 },
    { minX: 0.32, maxX: 0.96, minZ: 1.81, maxZ: 2.45 },
    roomDetails.deskFootprint,
  ];
  const landingFacade = [
    { minX: -3.50, maxX: -doorwayHalfWidth, minZ: -7.13, maxZ: rearWallZ },
    { minX: doorwayHalfWidth, maxX: 3.50, minZ: -7.13, maxZ: rearWallZ },
  ];
  const walkingFurniture = [...cabinFurniture];
  let serviceDoorOpen = false;
  let landingFloor: number | null = null;
  let lastCrossingStep = -Infinity;
  let fromWalkingUntil = -Infinity;

  /** Converts motion across the docked threshold into a server-validated location change. */
  function requestCrossing(x: number, z: number) {
    if (!serviceDoorOpen || Math.abs(x) > doorwayHalfWidth - guestRadius) return;
    if (landingFloor !== null && landingFloor !== state.floor) return;
    const towardDoor = landingFloor !== null ? z > rearWallZ : z < rearWallZ;
    if (!towardDoor) return;
    const now = performance.now();
    const continuing = now - lastCrossingStep < 220;
    lastCrossingStep = now;
    if (continuing) return;
    fromWalkingUntil = now + 2500;
    onPick(landingFloor !== null ? "board_lift" : "step_onto_landing");
  }

  const welcomeCamera = new FreeCamera("refuge arrival", Vector3.Zero(), scene);
  welcomeCamera.parent = bridge;
  welcomeCamera.minZ = 0.05;
  welcomeCamera.fov = camera.fov;
  /** Frames the illuminated lodge from the bridge after the walking guest reaches the porch. */
  function showWelcome() {
    if (scene.activeCamera !== welcomeCamera) {
      const { far, near } = avalancheLanding;
      const setback = near.subtract(far).normalize().scale(5.2);
      welcomeCamera.position.copyFrom(far).addInPlace(setback);
      welcomeCamera.position.y += 1.64;
      welcomeCamera.setTarget(far.subtract(setback.scale(0.25)).add(new Vector3(0, 1.72, 0)));
      scene.activeCamera = welcomeCamera;
    }
    avalancheLanding.welcome(true);
  }
  const drawGuest = createGuestAvatar(scene, bridge);
  const porchForward = avalancheLanding.far.subtract(avalancheLanding.threshold).normalize();
  /** Places a finished guest beside the door, facing back down the span. */
  function waitingPose(east: boolean): BridgePose {
    const side = east ? 0.62 : -0.62;
    const porch = avalancheLanding.threshold;
    return { x: porch.x + porchForward.z * side, z: porch.z - porchForward.x * side,
      yaw: Math.atan2(porchForward.x, porchForward.z) };
  }
  /** Maps an East-oriented crossing point back into the current cabin's frame. */
  function cabinPoint(point: Vector3) {
    const { x, z } = wingPoint(point.x, point.z, state.room === "cool");
    return new Vector3(x, point.y + bridge.position.y - cabin.position.y, z);
  }

  /** Publishes the shared exterior in hotel coordinates and draws the peer on their own deck. */
  function stepPresence(frameMs: number) {
    const { x, z } = camera.position;
    const west = state.room === "cool";
    const outside = z > cabinBounds.maxZ || x > cabinBounds.maxX;
    const current = wingPoint(x, z, west);
    const localWalk = west ? westWalk : eastWalk;
    const visible = state.gateOpen && !state.onLanding && !state.escaped && outside
      && localWalk.floorAt(current.x, current.z) !== undefined;
    onBridgePose?.(visible ? { ...current, yaw: camera.rotation.y + (west ? Math.PI : 0) } : null);
    const pose = state.partnerEscaped ? waitingPose(west) : state.partnerBridgePose;
    const peerWalk = west ? eastWalk : westWalk;
    // Wire coordinates round to 5 cm, including a legal sample taken directly against a rail.
    const floor = state.partnerEscaped ? avalancheLanding.threshold.y : pose && peerWalk.floorAt(pose.x, pose.z, 0.04);
    drawGuest(pose && floor != null ? pose : null, floor ?? 0, west, frameMs);
  }
  let guidedPath: Vector3[] = [];
  let guideRequested = false;
  let celebrationMs = 0;
  let retryArrivalAt = 0;
  let doorReachable = false;
  /** Keeps the accessible door control and the physical push bar in the same reach zone. */
  function updateDoorRange() {
    const porch = cabinPoint(avalancheLanding.threshold);
    const reachable = state.gateOpen && !state.escaped && !state.onLanding &&
      Math.hypot(camera.position.x - porch.x, camera.position.z - porch.z) <= 2.1;
    if (reachable === doorReachable) return;
    doorReachable = reachable;
    onDoorRange?.(reachable);
  }
  let travelling = false;
  let animateLift = true;
  let skipGuidedCrossing = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  /** Joins the cabin and terrace at the sill. Only the acknowledged side of the threshold is walkable. */
  function exteriorFloor(x: number, z: number) {
    if (serviceDoorOpen && Math.abs(x) <= doorwayHalfWidth - guestRadius) {
      const atSill = landingFloor !== null
        ? z <= rearWallZ && z >= landingBounds.maxZ - guestRadius
        : z >= rearWallZ && z <= cabinBounds.minZ + guestRadius;
      if (atSill) return 0;
    }
    if (state.onLanding || !state.gateOpen || leaf.position.x < 2.55) return undefined;
    const west = state.room === "cool";
    const point = wingPoint(x, z, west);
    const floor = (west ? westWalk : eastWalk).floorAt(point.x, point.z);
    return floor === undefined ? undefined : floor + bridge.position.y - cabin.position.y;
  }
  const walking = createWalkingControls(
    canvas, scene, camera, walkingBounds, walkingFurniture,
    (id) => {
      const target = caseInteraction(id);
      if (target === null) return;
      if (target) baggage.selectCase(id);
      onPick(target ?? interactions.get(id) ?? id);
    },
    hoverFurnishing,
    requestCrossing,
    usableObject,
    exteriorFloor,
  );

  /** Guides the guest along the decks and celebrates on the porch before reporting arrival. */
  function stepRefuge(frameMs: number) {
    if (!state.gateOpen || state.onLanding) return;
    if (leaf.position.x >= 2.55 && (guideRequested || (skipGuidedCrossing && guidedPath.length))) {
      if (skipGuidedCrossing) {
        camera.position.copyFrom(cabinPoint(avalancheLanding.threshold));
        showWelcome();
        guidedPath = [];
      } else {
        const west = state.room === "cool";
        const { x, z } = wingPoint(camera.position.x, camera.position.z, west);
        guidedPath = (west ? westWalk : eastWalk).guide(new Vector3(x, 0, z), (x, z) => {
          const local = wingPoint(x, z, west);
          return local.z <= 3 && walking.canStand(local.x, local.z);
        }).map(cabinPoint);
      }
      guideRequested = false;
    }
    const point = guidedPath[0];
    if (point) {
      const dx = point.x - camera.position.x, dz = point.z - camera.position.z;
      const distance = Math.hypot(dx, dz);
      if (distance < 0.001) guidedPath.shift();
      else {
        const heading = Math.atan2(dx, dz) - camera.rotation.y;
        const yaw = Math.atan2(Math.sin(heading), Math.cos(heading));
        const turnLimit = frameMs * 0.00072;
        camera.rotation.y += Math.max(-turnLimit, Math.min(turnLimit, yaw));
        camera.rotation.x -= Math.max(-turnLimit, Math.min(turnLimit, camera.rotation.x));
        // Let the guest see around each bend before the guided camera moves into it.
        const alignment = Math.max(0, Math.cos(yaw));
        const step = Math.min(distance, frameMs / 1000 * walkSpeed * alignment * alignment);
        camera.position.x += dx / distance * step;
        camera.position.z += dz / distance * step;
        if (distance <= step) guidedPath.shift();
      }
    }
    const deck = exteriorFloor(camera.position.x, camera.position.z);
    camera.position.y = 1.73 + (deck ?? 0);
    updateDoorRange();
    const west = state.room === "cool";
    const pointOnBridge = wingPoint(camera.position.x, camera.position.z, west);
    if (!celebrationMs && !(west ? westWalk : eastWalk).onMat(pointOnBridge.x, pointOnBridge.z)) return;
    guidedPath = [];
    if (!celebrationMs) avalancheLanding.arrivals(state.room === "warm", true, state.partnerEscaped);
    celebrationMs += frameMs;
    avalancheLanding.welcome(true);
    if (celebrationMs >= retryArrivalAt && !state.escaped) {
      retryArrivalAt = celebrationMs + 1500;
      onPick("reach_refuge");
    }
  }

  // Suite furnishings are assembled in cabin-local coordinates. The two cars and
  // the fixed building are independent roots once all the fixtures are installed.
  for (const fixture of [...scene.rootNodes]) {
    if (fixture !== world && fixture !== cabin && fixture !== shaft && fixture !== partner && fixture !== fill)
      fixture.parent = cabin;
  }
  createAlpineLandscape(scene);
  const liftSound = createLiftSound();
  const playback = createLiftPlayback(keyTravelTime);
  let initialized = false;

  /** Places the walking camera on the acknowledged side of a stationary service doorway. */
  function positionGuest(next: HotelSceneState, newService: boolean) {
    walking.clearHover();
    landingFloor = next.onLanding ? next.floor : null;
    Object.assign(walkingBounds, next.onLanding ? landingBounds : cabinBounds);
    walkingFurniture.splice(0, walkingFurniture.length, ...(next.onLanding ? landingFacade : cabinFurniture));
    const fromWalking = !newService && performance.now() < fromWalkingUntil;
    camera.parent = next.onLanding ? shaft : cabin;
    if (next.onLanding) {
      camera.position.set(fromWalking ? camera.position.x : 0, 1.73 + next.floor * 4, fromWalking ? rearWallZ : rearWallZ - 0.48);
      if (!fromWalking) camera.setTarget(new Vector3(0, 1.48 + next.floor * 4, -10.4));
    } else if (newService) {
      camera.position.copyFrom(entryPosition);
      camera.setTarget(entryLookAt);
    } else {
      camera.position.set(fromWalking ? camera.position.x : 0, 1.73, fromWalking ? rearWallZ : rearWallZ + 0.32);
      if (!fromWalking) camera.setTarget(new Vector3(0, 1.60, -2.0));
    }
    fromWalkingUntil = -Infinity;
    lastCrossingStep = performance.now();
  }

  const resize = new ResizeObserver(() => engine.resize());
  resize.observe(canvas);
  engine.runRenderLoop(() => {
    const frameMs = Math.min(engine.getDeltaTime(), 80);
    const ease = 1 - Math.exp(-frameMs * 0.0034);
    const nextFloor = playback.destination;
    const landed = playback.step(frameMs, animateLift, baggage.step);
    const floor = playback.floor;
    if (landed !== undefined) onFloorChange?.(landed);
    if (!playback.pending && (landingFloor !== null) !== state.onLanding) positionGuest(state, false);
    // Interpolate a single cable displacement. Holding at a service stop holds both cars.
    const other = state.floor + state.partnerFloor - floor;
    cabin.position.y = floor * 4;
    partner.position.y = other * 4;
    updateSuspension();

    sheave.rotation.z = floor * 4 / 0.17;
    const atStop = nextFloor === undefined || landed !== undefined;
    liftSound.update(initialized && landingFloor === null && !atStop ? nextFloor - floor : 0);
    serviceDoorOpen = floor === state.floor && !playback.travelling &&
      !playback.transferringBaggage && (state.canStepOntoLanding || state.onLanding);
    for (const [index, panel] of serviceLeaves.entries()) {
      const center = serviceDoorOpen ? doorwayHalfWidth * 1.52 : doorwayHalfWidth / 2;
      panel.position.x = index === 0 ? -center : center;
    }
    for (const [level, door] of shaftDoors.entries()) door.setEnabled(!serviceDoorOpen || level !== state.floor);

    const peerServiceOpen = other === state.partnerFloor && !playback.travelling && !state.partnerGateOpen;
    for (const [level, door] of oppositeDoors.entries())
      door.setEnabled(level !== state.partnerFloor || !peerServiceOpen);
    for (const [index, panel] of peerServiceLeaves.entries()) {
      const center = peerServiceOpen ? doorwayHalfWidth * 1.52 : doorwayHalfWidth / 2;
      panel.position.x = index === 0 ? -center : center;
    }

    const moving = playback.travelling;
    if (moving !== travelling) {
      travelling = moving;
      onTravelChange?.(moving);
    }
    const keyTime = playback.keyTime;
    lever.rotation.z +=
      ((atStop || keyTime !== 0 ? -0.65 : 0.68) - lever.rotation.z) * ease * 1.7;
    peerLever.rotation.z +=
      ((atStop || keyTime !== keyTravelTime ? -0.44 : 0.42) - peerLever.rotation.z) * ease * 1.7;
    positionBrakeKey(keyTime, partner.position.y - cabin.position.y);
    // Wait for the key to seat before closing the contact on either side of the hatch.
    const seated = state.room === "cool" ? keyTime === 0 : keyTime === keyTravelTime;
    const energized = state.bridgePowered && seated;
    if (energized && soundRelayOnSeat) {
      soundRelayOnSeat = false;
      liftSound.relayClack();
    }
    beaconGlass.diffuseColor.copyFrom(energized ? hotGlass : coldGlass);
    beaconGlass.emissiveColor.set(energized ? 1 : 0.018, energized ? 0.79 : 0.028, energized ? 0.36 : 0.028);
    knife.rotation.z += ((energized ? 0 : -0.66) - knife.rotation.z) * ease;
    peerKnife.rotation.z = knife.rotation.z;
    pilot.emissiveColor.set(energized ? 0.28 : 0.025, energized ? 0.90 : 0.035, energized ? 0.63 : 0.03);
    const arrived = state.atRescueBridge && !playback.travelling && floor === state.floor;
    dockGallery(arrived);
    localGateInstrument(energized, arrived, state.gateOpen, state.lockTripped);
    peerGateInstrument(energized, arrived, state.partnerGateOpen, state.partnerLockTripped);
    const gateX = arrived && state.gateOpen ? 2.8 : 0;
    leaf.position.x += (gateX - leaf.position.x) * ease;
    peerGate.position.x += ((arrived && state.partnerGateOpen ? 2.8 : 0) - peerGate.position.x) * ease;
    if (!state.escaped && !guidedPath.length && !celebrationMs) walking.step(engine.getDeltaTime());
    stepRefuge(frameMs);
    stepPresence(frameMs);
    if (revealObjects && performance.now() - lastReveal > 250) {
      refreshOutlines();
      lastReveal = performance.now();
    }
    scene.render();
  });
  return {
    /** Clears the scene's physical selection before a transfer requested from the controls. */
    clearBaggageSelection: baggage.clearSelection,
    /** Updates the guest's comfort preference for automatic lift travel and the guided crossing. */
    setMotionPreferences(skipCrossing: boolean, animateRide: boolean) {
      skipGuidedCrossing = skipCrossing;
      animateLift = animateRide;
    },
    /** Gives the waiting guest a first-person view from their own side of the refuge entrance. */
    watchBridge() {
      if (!state.escaped) return;
      const pose = waitingPose(state.room === "warm");
      camera.position.copyFrom(cabinPoint(new Vector3(pose.x, avalancheLanding.threshold.y + 1.73, pose.z)));
      const occupiedHeight = (state.floor + state.partnerFloor) * 2 + 1.73;
      camera.setTarget(cabinPoint(new Vector3(6.7, occupiedHeight - bridge.position.y, 0)));
      scene.activeCamera = camera;
    },
    /** Walks from the current guest position to the refuge porch along the physical decks. */
    walkToRefuge() {
      if (!state.gateOpen || state.onLanding || state.escaped) return;
      if (doorReachable) onPick("reach_refuge");
      else if (!celebrationMs) guideRequested = true;
    },
    /** Lets the guest inspect all currently available objects at once. */
    setRevealObjects(enabled: boolean) {
      revealObjects = enabled;
      refreshOutlines();
    },
    /** Shows interaction cues only while the guest can operate the room. */
    setWalkingEnabled(enabled: boolean) {
      outlines.isEnabled = enabled;
      walking.setWalkingEnabled(enabled);
    },
    /** Maintains one vertical field of view across manual play and the arrival camera. */
    setVerticalFov(degrees: number) {
      welcomeCamera.fov = camera.fov = degrees * Math.PI / 180;
    },
    setWalkVector: walking.setWalkVector,
    /** Applies the authoritative room snapshot, animating travel between discrete stops. */
    update(next: HotelSceneState) {
      const newService = !initialized || state.route !== next.route || state.room !== next.room;
      if (newService) {
        const west = next.room === "cool";
        applyWingFrame(cabin, west);
        applyWingFrame(partner, !west);
        applyWingFrame(shaft, west);
        applyWingFrame(oppositeStation, !west);
        playback.restore(next.floor, next.hasBrakeKey);
        positionGuest(next, true);
        onFloorChange?.(next.floor);
      } else if (state.hasBrakeKey !== next.hasBrakeKey) {
        // A coalesced update can report ownership and travel together. The handoff is at level 2.
        playback.enqueueTravel(state.floor, 2);
        playback.enqueueKey(next.hasBrakeKey);
        playback.enqueueTravel(2, next.floor);
      } else {
        playback.enqueueTravel(state.floor, next.floor);
      }
      const eastDestination = next.room === "warm" ? 4 - next.partnerDestination : next.partnerDestination;
      placeGallery(eastDestination);
      avalancheLanding.update(eastDestination);
      if (newService || !next.atRescueBridge) ringOnDocking = false;
      else if (!state.atRescueBridge) ringOnDocking = true;
      lockPlateFinish.emissiveColor = next.lockTripped ? new Color3(0.78, 0.12, 0.06) : new Color3(0.14, 0.12, 0.09);
      peerLockPlateFinish.emissiveColor = next.partnerLockTripped ? new Color3(0.78, 0.12, 0.06) : new Color3(0.14, 0.12, 0.09);
      master.setEnabled(next.room === "cool");
      peerFixtures.get(master)!.setEnabled(next.room === "warm");
      freight.setEnabled(next.freightClear !== null);
      freightGlass.emissiveColor = Color3.FromHexString(next.freightClear ? "#78d1a1" : "#ed9142");
      avalancheLanding.arrivals(next.room === "warm", next.escaped || (!newService && celebrationMs > 0), next.partnerEscaped);
      if (newService) soundRelayOnSeat = false;
      else if (!state.bridgePowered && next.bridgePowered) soundRelayOnSeat = true;
      state = next;
      if (newService) {
        lever.rotation.z = -0.65;
        peerLever.rotation.z = -0.44;
        leaf.position.x = next.atRescueBridge && next.gateOpen ? 2.8 : 0;
        scene.activeCamera = camera;
        guidedPath = [];
        guideRequested = false;
        retryArrivalAt = 0;
        celebrationMs = 0;
        avalancheLanding.welcome(next.escaped);
        if (next.escaped) {
          const porch = cabinPoint(avalancheLanding.threshold);
          camera.position.set(porch.x, 1.73 + porch.y, porch.z);
          camera.rotation.set(-0.19, Math.PI + 0.20, 0);
          showWelcome();
        }
        initialized = true;
      }
      fabric.diffuseColor = Color3.FromHexString(
        next.room === "warm" ? "#903f30" : "#386468",
      );
      stripe.diffuseColor = Color3.FromHexString(
        next.room === "warm" ? "#602f29" : "#28474e",
      );
      rug.diffuseColor = Color3.FromHexString(
        next.room === "warm" ? "#f3e6ce" : "#b1c1c6",
      );
      positionFurniture(next.room);
      const warmPeer = next.room === "cool";
      const peerColors = warmPeer
        ? ["#903f30", "#602f29", "#f3e6ce", "#69442e", "#846043", "#803e35"]
        : ["#386468", "#28474e", "#b1c1c6", "#4a453c", "#686052", "#386468"];
      roomFabricsAndFinishes.forEach((material, i) => peerMaterials.get(material)!.diffuseColor = Color3.FromHexString(peerColors[i]));
      velvet.diffuseColor = Color3.FromHexString(warmPeer ? "#386468" : "#803e35");
      peerFixtures.get(chair)!.position.x = warmPeer ? -3.15 : 3.04;
      peerFixtures.get(chair)!.rotation.y = warmPeer ? -0.65 : 0.71;
      peerFixtures.get(table)!.position.x = warmPeer ? 3.08 : -3.29;
      roomDetails.update(next.room);
      roomDetails.dressPeer(warmPeer ? "warm" : "cool");
      updateStory(next);
      eastLabels.forEach((labels, i) => labels.forEach(label => label.setEnabled(warmPeer === Boolean(i))));
      westLabels.forEach((labels, i) => labels.forEach(label => label.setEnabled(warmPeer !== Boolean(i))));
      const markedAcross = next.room === "cool" && (next.freightStatus === "aboard" || next.freightStatus === "ready");
      oppositeMedical.setEnabled(markedAcross);
      peerLandingSlots.forEach((slots, floor) => slots.forEach((slot, i) =>
        slot.setEnabled(i < (next.partnerLandingLoads?.[floor] ?? 0))));
      partnerCases.forEach((item, slot) => item.setEnabled(slot < next.partnerLoads && (slot !== 0 || !markedAcross)));
      wood.diffuseColor = Color3.FromHexString(
        next.room === "warm" ? "#69442e" : "#4a453c",
      );
      inset.diffuseColor = Color3.FromHexString(
        next.room === "warm" ? "#846043" : "#686052",
      );
      dispensary.setEnabled(next.room === "warm" && next.freightFloor !== null);
      dispensary.position.y = (next.freightFloor ?? 0) * 4;
      const baggageUpdate = baggage.update(next, newService);
      if (baggageUpdate === "restore") playback.clearBaggage();
      else if (baggageUpdate === "transfer") playback.enqueueBaggage();
      if (!playback.pending && (landingFloor !== null) !== next.onLanding) positionGuest(next, false);
    },
    /** Releases pointer bindings, observers, textures and the rendering context. */
    dispose() {
      resize.disconnect();
      landingBell.pause();
      liftSound.dispose();
      walking.dispose();
      scene.dispose();
      engine.dispose();
    },
  };
}
