import type { TransformNode } from "@babylonjs/core/Meshes/transformNode";

/** The two root frames face across the exchange well. X aligns the two key sockets. */
export const westOrigin = { x: 5.12, z: 9.42 } as const;

/** Edges of the fixed exchange landing in the East frame, shared by both booked galleries. */
export const exchangeLanding = { left: 1.42, right: 3.80, near: 3.81, far: 5.61 } as const;

/** Converts horizontal points between a wing's local frame and the East-oriented hotel frame.
 * The West half-turn about the midpoint between the car origins is its own inverse. */
export function wingPoint(x: number, z: number, west: boolean) {
  return west ? { x: westOrigin.x - x, z: westOrigin.z - z } : { x, z };
}

/** Installs the permanent horizontal address of a wing while preserving the car's cable height. */
export function applyWingFrame(node: TransformNode, west: boolean) {
  node.position.x = west ? westOrigin.x : 0;
  node.position.z = west ? westOrigin.z : 0;
  node.rotation.y = west ? Math.PI : 0;
}
