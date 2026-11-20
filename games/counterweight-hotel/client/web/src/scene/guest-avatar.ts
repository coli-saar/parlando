import { Color3 } from "@babylonjs/core/Maths/math.color";
import { Vector3 } from "@babylonjs/core/Maths/math.vector";
import { StandardMaterial } from "@babylonjs/core/Materials/standardMaterial";
import { MeshBuilder } from "@babylonjs/core/Meshes/meshBuilder";
import { TransformNode } from "@babylonjs/core/Meshes/transformNode";
import type { Scene } from "@babylonjs/core/scene";
import type { BridgePose } from "../game";

/** Builds the other hotel guest in deck coordinates. The pivot sits between the boots, facing +Z. */
export function createGuestAvatar(scene: Scene, deck: TransformNode) {
  const guest = new TransformNode("other hotel guest", scene);
  guest.parent = deck;
  guest.setEnabled(false);
  /** Slight ambient fill keeps the guest readable on the unlit side of the gallery. */
  function cloth(name: string, hex: string) {
    const material = new StandardMaterial(name, scene);
    material.diffuseColor = Color3.FromHexString(hex);
    material.emissiveColor = material.diffuseColor.scale(0.18);
    material.specularColor = new Color3(0.08, 0.07, 0.06);
    return material;
  }
  const coat = cloth("guest wool overcoat", "#36626b");
  const scarf = cloth("guest travelling scarf", "#e4bd75");
  const wool = cloth("guest flannel trousers", "#373c48");
  const leather = cloth("guest polished leather", "#49352e");
  const skin = cloth("guest complexion", "#b97d5d");
  const felt = cloth("guest felt hat", "#393f47");
  const body = new TransformNode("guest shoulders and hips", scene);
  body.parent = guest;
  /** Shapes softly faceted tailored volumes in the local coordinate frame of each joint. */
  function oval(name: string, parent: TransformNode, material: StandardMaterial, x: number, y: number, z: number,
    width: number, height: number, depth: number) {
    const mesh = MeshBuilder.CreateSphere(name, {diameter: 1, segments: 12}, scene);
    mesh.parent = parent;
    mesh.position.set(x, y, z);
    mesh.scaling.set(width, height, depth);
    mesh.material = material;
    mesh.isPickable = false;
    return mesh;
  }
  /** A tapered elliptical cylinder gives the coat a structured hem and the hat a flat crown. */
  function tailored(name: string, material: StandardMaterial, y: number, height: number, top: number, bottom: number, depth: number) {
    const mesh = MeshBuilder.CreateCylinder(name, {height, diameterTop: top, diameterBottom: bottom, tessellation: 18}, scene);
    mesh.parent = body;
    mesh.position.y = y;
    mesh.scaling.z = depth;
    mesh.material = material;
    mesh.isPickable = false;
  }
  tailored("double breasted walking coat", coat, 1.01, 0.66, 0.45, 0.51, 0.72);
  oval("coat shoulders", body, coat, 0, 1.31, 0, 0.52, 0.25, 0.35);
  oval("scarf collar", body, scarf, 0, 1.405, 0, 0.285, 0.14, 0.265);
  oval("tucked scarf end", body, scarf, -0.073, 1.28, 0.163, 0.088, 0.24, 0.047).rotation.z = -0.27;
  for (const x of [-0.075, 0.075]) for (const y of [0.90, 1.03, 1.16])
    oval("horn coat button", body, leather, x, y, 0.159, 0.026, 0.026, 0.024);
  oval("guest head", body, skin, 0, 1.565, 0.012, 0.228, 0.292, 0.216);
  oval("guest nose", body, skin, 0, 1.558, 0.115, 0.047, 0.063, 0.055);
  oval("guest mouth", body, leather, 0, 1.517, 0.111, 0.049, 0.009, 0.008);
  for (const side of [-1, 1]) {
    oval("ear", body, skin, side * 0.108, 1.56, 0.001, 0.042, 0.065, 0.035);
    oval("eye", body, leather, side * 0.044, 1.593, 0.107, 0.023, 0.012, 0.013);
  }
  tailored("travelling hat brim", felt, 1.662, 0.025, 0.34, 0.35, 0.93);
  tailored("travelling hat crown", felt, 1.716, 0.11, 0.22, 0.254, 0.93);
  tailored("hat ribbon", scarf, 1.685, 0.026, 0.255, 0.262, 0.94);
  /** Hinge both limbs about the shoulder or hip to keep the extremities attached during a stride. */
  function limb(name: string, x: number, y: number) {
    const joint = new TransformNode(name, scene);
    joint.parent = body;
    joint.position.set(x, y, 0);
    return joint;
  }
  const limbs = [-1, 1].map(side => {
    const leg = limb("hip", side * 0.117, 0.78);
    oval("trouser leg", leg, wool, 0, -0.319, 0, 0.174, 0.705, 0.18);
    oval("leather boot", leg, leather, 0, -0.685, 0.042, 0.171, 0.185, 0.29);
    const arm = limb("shoulder", side * 0.222, 1.325);
    oval("coat sleeve", arm, coat, side * 0.029, -0.222, 0, 0.15, 0.508, 0.165);
    oval("leather glove", arm, leather, side * 0.03, -0.49, 0.017, 0.117, 0.16, 0.119);
    return { side, arm, leg };
  });
  let phase = 0;
  let warmOutfit: boolean | undefined;
  const target = Vector3.Zero();
  /** Smooths network samples and derives the stride from distance actually travelled. */
  return function drawGuest(pose: BridgePose | null, floor: number, warm: boolean, frameMs: number) {
    if (!pose) {
      guest.setEnabled(false);
      return;
    }
    if (warm !== warmOutfit) {
      warmOutfit = warm;
      coat.diffuseColor = Color3.FromHexString(warm ? "#a14532" : "#2f626d");
      scarf.diffuseColor = Color3.FromHexString(warm ? "#efe0bb" : "#e3ac59");
      coat.emissiveColor = coat.diffuseColor.scale(0.18);
      scarf.emissiveColor = scarf.diffuseColor.scale(0.18);
    }
    target.set(pose.x, floor, pose.z);
    const easing = 1 - Math.exp(-frameMs / 80);
    const distance = Vector3.Distance(guest.position, target);
    let stride = 0;
    if (!guest.isEnabled() || distance > 2) {
      guest.position.copyFrom(target);
      guest.rotation.y = pose.yaw;
      guest.setEnabled(true);
    } else {
      guest.position.addInPlaceFromFloats((target.x - guest.position.x) * easing,
        (target.y - guest.position.y) * easing, (target.z - guest.position.z) * easing);
      const turn = pose.yaw - guest.rotation.y;
      guest.rotation.y += Math.atan2(Math.sin(turn), Math.cos(turn)) * easing;
      phase += distance * easing * 5.6;
      stride = Math.min(1, distance / 0.1);
    }
    const swing = Math.sin(phase) * stride;
    body.position.y = Math.abs(swing) * 0.025;
    for (const { side, arm, leg } of limbs) {
      arm.rotation.x = side * swing * 0.37;
      leg.rotation.x = -side * swing * 0.47;
    }
  };
}
