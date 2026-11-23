import { Scene } from "@babylonjs/core/scene";
import { Color3 } from "@babylonjs/core/Maths/math.color";
import { DynamicTexture } from "@babylonjs/core/Materials/Textures/dynamicTexture";
import { StandardMaterial } from "@babylonjs/core/Materials/standardMaterial";
import { MeshBuilder } from "@babylonjs/core/Meshes/meshBuilder";
import type { HotelSceneState } from "./hotel";
import { bookingRevision, documentHeight, documentWidth, drawHotelDocument } from "../document-art";

/** A shared wall layout keeps each frame wholly inside its solid, finished bay. */
export const storyWallBays = {
  [-1]: { center: -2.4, signWidth: 1.95 },
  [1]: { center: -1.975, width: 1.92, signWidth: 1.28 },
};

/** Mounts the commissioning plate and route sign on opposite walls and refreshes the sign when the rescue state changes. */
export function createHotelStory(scene: Scene) {
  const brass = new StandardMaterial("worn slip of the hotel archives", scene);
  brass.diffuseColor = Color3.FromHexString("#ac8554");
  /** Frames a sheet at reading height with a raised edge and an inward-facing printed surface. */
  function sheet(name: string, side: -1 | 1, target: "plate" | "route-card") {
    const { center: z, signWidth: width } = storyWallBays[side];
    const height = width * 2 / 3;
    const frame = MeshBuilder.CreateBox(name + " brass frame", {
      width: 0.084, height: height + 0.09, depth: width + 0.09,
    }, scene);
    frame.position.set(side * 4.27, 1.98, z);
    frame.material = brass;
    frame.isPickable = true;
    frame.metadata = target;
    const texture = new DynamicTexture(name, { width: documentWidth, height: documentHeight }, scene, true);
    const material = new StandardMaterial(name, scene);
    material.diffuseTexture = texture;
    material.emissiveColor = new Color3(0.18, 0.18, 0.18);
    material.specularColor = Color3.Black();
    const plane = MeshBuilder.CreatePlane(name, { width, height }, scene);
    plane.position.set(side * 4.221, 1.98, z);
    plane.rotation.y = side * Math.PI / 2;
    plane.material = material;
    plane.isPickable = true;
    plane.metadata = target;
    return texture;
  }
  const plate = sheet("commissioning plate of 1896", -1, "plate");
  drawHotelDocument(plate.getContext() as CanvasRenderingContext2D, "plate", { room: "warm" });
  plate.update();
  const sign = sheet("rescue bridge route sign", 1, "route-card");
  let previous = "";
  /** Keeps a legible printed copy as the rescue route changes. */
  return (state: HotelSceneState) => {
    const signature = bookingRevision(state);
    if (signature === previous) return;
    previous = signature;
    drawHotelDocument(sign.getContext() as CanvasRenderingContext2D, "route-card", state);
    sign.update();
  };
}
