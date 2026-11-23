import type { StandardMaterial } from "@babylonjs/core/Materials/standardMaterial";
import { MeshBuilder } from "@babylonjs/core/Meshes/meshBuilder";
import type { TransformNode } from "@babylonjs/core/Meshes/transformNode";
import type { Scene } from "@babylonjs/core/scene";

/** Fits the two cars with always-open ventilators facing the same vertical masonry channel. */
export function installAirWell(
  scene: Scene,
  shaft: TransformNode,
  brass: StandardMaterial,
  dark: StandardMaterial,
  masonry: StandardMaterial,
  plaque: (label: string, x: number, y: number, z: number, w: number, h: number) => void,
) {
  /** Keeps masonry in the building frame and metalwork in its respective car frame. */
  function block(name: string, x: number, y: number, z: number, width: number,
    height: number, depth: number, material: StandardMaterial, parent?: TransformNode) {
    const mesh = MeshBuilder.CreateBox(name, { width, height, depth }, scene);
    mesh.parent = parent ?? null;
    mesh.position.set(x, y, z);
    mesh.isPickable = false;
    mesh.material = material;
    return mesh;
  }

  // The continuous recess lies along the shaft partition; no floor slab caps its throat.
  for (const z of [3.84, 4.55])
    block("air well stone jamb", 0.99, 10, z, 0.13, 22, 0.1, masonry, shaft);
  block("air well shaded throat", 0.94, 10, 4.19, 0.024, 22, 0.61, dark, shaft);

  /** Raised sash edges and pitched louvers reveal dark air gaps between the blades. */
  function grille(x: number, y: number, z: number, parent?: TransformNode) {
    block("ventilator recess", x, y, z, 0.44, 0.78, 0.035, dark, parent);
    for (const offset of [-0.217, 0.217])
      block("ventilator stile", x + offset, y, z - 0.038, 0.032, 0.8, 0.07, brass, parent);
    for (const offset of [-0.386, 0.386])
      block("ventilator rail", x, y + offset, z - 0.038, 0.45, 0.032, 0.07, brass, parent);
    for (let row = -3; row <= 3; row++)
      block("open ventilator louver", x, y + row * 0.099, z - 0.052,
        0.41, 0.02, 0.084, brass, parent).rotation.x = -0.44;
  }

  grille(0.68, 2.32, 3.29);
  plaque("AIR WELL", 0.68, 1.82, 3.26, 0.48, 0.105);

}
