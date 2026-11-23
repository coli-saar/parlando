import { westOrigin } from "./hotel-layout";
import { Vector3 } from "@babylonjs/core/Maths/math.vector";
import { MeshBuilder } from "@babylonjs/core/Meshes/meshBuilder";
import { TransformNode } from "@babylonjs/core/Meshes/transformNode";
import type { StandardMaterial } from "@babylonjs/core/Materials/standardMaterial";
import type { Scene } from "@babylonjs/core/scene";

/** Installs the common suspension rope. Both vertical falls terminate on the
 * live car roofs. Two quarter-circle wraps join them to a single horizontal
 * run, so complementary lift travel preserves the installed rope length. */
export function createSuspension(scene: Scene, shaft: TransformNode, cabin: TransformNode,
  partner: TransformNode, steel: StandardMaterial, bronze: StandardMaterial) {
  const z = 0, height = 22.32, radius = 0.80;
  const socketX = westOrigin.x / 2;
  const left = -1, right = westOrigin.z - left;
  // The traction frame spans the room centers along the longitudinal shaft axis.
  const frame = new TransformNode("paired lift headframe", scene);
  frame.parent = shaft;
  frame.position.x = socketX;
  frame.rotation.y = -Math.PI / 2;
  /** Places a fixed steelwork member in its owner's coordinate frame. */
  function beam(parent: TransformNode, x: number, y: number, depth: number,
    width: number, h: number, d: number, material = steel) {
    const mesh = MeshBuilder.CreateBox("headframe steelwork", {width, height: h, depth: d}, scene);
    mesh.position.set(x,y,depth);
    mesh.parent = parent;
    mesh.isPickable = false;
    mesh.material = material;
    return mesh;
  }
  const wheels: TransformNode[] = [];
  for (const [x, side] of [[left, -1], [right, 1]]) {
    const axle = x - side * radius;
    const wheel = new TransformNode("grooved suspension sheave", scene);
    wheel.position.set(axle, height, z);
    wheel.parent = frame;
    wheels.push(wheel);
    for (const dz of [-0.085,0.085]) {
      const rim = MeshBuilder.CreateTorus("bronze sheave flange", {diameter: radius * 2, thickness: 0.086, tessellation: 48}, scene);
      rim.rotation.x = Math.PI / 2;
      rim.position.z = dz;
      rim.parent = wheel;
      rim.material = bronze;
      rim.isPickable = false;
    }
    for (let spoke = 0; spoke < 4; spoke++)
      beam(wheel, 0, 0, 0, 0.085, 2 * radius, 0.12, bronze).rotation.z = spoke * Math.PI / 4;
    // The transverse girder carries each axle into a pair of columns outside
    // its own lift envelope, leaving the moving roof and gallery clear.
    const columnZ = -side * (socketX + 4.72);
    for (const offset of [-0.48, 0.48]) {
      beam(frame, axle + offset, 9.98, columnZ, 0.17, 21.30, 0.21);
      beam(frame, axle + offset, 20.57, columnZ / 2, 0.20, 0.29, Math.abs(columnZ) + 0.30);
    }
    beam(frame, axle, 20.57, columnZ, 1.18, 0.35, 0.31);
    for (const dz of [-0.34,0.34]) {
      beam(frame, axle, 21.36, z+dz, 0.18, 1.92, 0.16);
      beam(frame, axle, 20.57, z+dz, 0.31, 0.23, 1.74);
    }
    beam(frame, axle, height, z, 0.21, 0.21, 0.87, bronze);
  }
  beam(frame, (left+right)/2, 21.91, z+0.34, right-left, 0.19, 0.16);
  const path: Vector3[] = [];
  for (const [center, start] of [[left + radius, Math.PI], [right - radius, Math.PI/2]]) {
    for (let n = 0; n <= 16; n++) {
      const angle = start - n * Math.PI / 32;
      path.push(new Vector3(center + radius*Math.cos(angle), height + radius*Math.sin(angle), z));
    }
  }
  const rope = MeshBuilder.CreateTube("continuous overhead hoist rope", {path, radius: 0.075, tessellation: 8}, scene);
  rope.parent = frame;
  rope.material = steel;
  rope.isPickable = false;
  const falls = [cabin, partner].map((car, i) => {
    const x = i ? right : left;
    beam(car, 0, 3.87, -1, 8.24, 0.20, 0.25);
    beam(car, socketX, 3.97, -1, 0.39, 0.24, 0.40, bronze);
    const fall = MeshBuilder.CreateCylinder("hoist rope to roof socket", {diameter: 0.15, height: 1, tessellation: 8}, scene);
    fall.parent = frame;
    fall.material = steel;
    fall.isPickable = false;
    fall.position.set(x,0,z);
    return {car, fall};
  });
  /** Moves the roof sockets with the cars and pays rope across both sheaves. */
  return () => {
    for (const {car, fall} of falls) {
      const bottom = car.position.y + 4.03;
      fall.position.y = (height + bottom)/2;
      fall.scaling.y = height-bottom;
    }
    for (const wheel of wheels) wheel.rotation.z = -cabin.position.y / radius;
  };
}
