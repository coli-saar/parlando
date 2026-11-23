import type { Scene } from "@babylonjs/core/scene";
import { VertexBuffer } from "@babylonjs/core/Buffers/buffer";
import { Color3 } from "@babylonjs/core/Maths/math.color";
import { StandardMaterial } from "@babylonjs/core/Materials/standardMaterial";
import { Texture } from "@babylonjs/core/Materials/Textures/texture";
import type { Mesh } from "@babylonjs/core/Meshes/mesh";

type Surface = "ashlar" | "dressing" | "flagstone" | "joinery" | "deck" | "steel" | "snow" | "ice" | "scree";

interface FinishScale {
  width: number;
  height: number;
  alongGrain?: boolean;
}

/** Tiles have dimensions in metres. The same diffuse and relief maps share these coordinates. */
const scales: Record<Surface, FinishScale> = {
  ashlar: { width: 4.8, height: 3.2 },
  dressing: { width: 2.4, height: 2.4 },
  flagstone: { width: 2.6, height: 2.6 },
  joinery: { width: 2.8, height: 0.7, alongGrain: true },
  deck: { width: 2.1, height: 0.85, alongGrain: true },
  steel: { width: 0.84, height: 0.84 },
  snow: { width: 3.2, height: 3.2 },
  ice: { width: 3.2, height: 3.2 },
  scree: { width: 4.6, height: 4.6 },
};

const finishScales = new WeakMap<StandardMaterial, FinishScale>();

/** Loads authored tiles with trilinear filtering and keeps colour and relief in register. */
function loadTextures(scene: Scene, surface: Surface) {
  const maps = {
    colour: new Texture(new URL(`../art/exterior/${surface}-colour.webp`, import.meta.url).href, scene),
    normal: new Texture(new URL(`../art/exterior/${surface}-normal.webp`, import.meta.url).href, scene),
  };
  for (const texture of Object.values(maps)) {
    texture.wrapU = texture.wrapV = Texture.WRAP_ADDRESSMODE;
    texture.anisotropicFilteringLevel = 8;
  }
  maps.normal.gammaSpace = false;
  return maps;
}

/** Shares a palette between the hotel and the refuge so matching finishes use one texture pair. */
export function createExteriorFinishes(scene: Scene) {
  const materials = new Map<Surface, StandardMaterial>();
  return (surface: Surface) => {
    const existing = materials.get(surface);
    if (existing) return existing;
    const material = new StandardMaterial("exterior " + surface, scene);
    const { colour, normal } = loadTextures(scene, surface);
    material.diffuseTexture = colour;
    material.bumpTexture = normal;
    const gloss = surface === "steel" ? 0.18 : surface === "ice" ? 0.13 : 0.065;
    material.specularColor = new Color3(gloss, gloss, gloss);
    material.specularPower = surface === "steel" ? 50 : 19;
    material.emissiveColor = surface === "snow" ? new Color3(0.12, 0.15, 0.17)
      : surface === "ice" ? new Color3(0.06, 0.09, 0.12) : new Color3(0.025, 0.024, 0.022);
    materials.set(surface, material);
    finishScales.set(material, scales[surface]);
    return material;
  };
}

/** Projects a box's faces in construction coordinates and keeps timber grain along each member.
 * Adjacent planks sample different lengths of the same grain tile.
 */
export function mapExteriorBox(
  mesh: Mesh, material: StandardMaterial, x: number, y: number, z: number,
  width: number, height: number, depth: number,
) {
  const scale = finishScales.get(material);
  if (!scale) return;
  const position = mesh.getVerticesData(VertexBuffer.PositionKind)!;
  const normal = mesh.getVerticesData(VertexBuffer.NormalKind)!;
  const offset = [x, y, z], dimensions = [width, height, depth];
  const positionHash = Math.sin(x * 36.81 + y * 19.97 + z * 57.42) * 7321.43;
  const phase = positionHash - Math.floor(positionHash);
  const uv: number[] = [];
  for (let i = 0; i < position.length; i += 3) {
    const axis = Math.abs(normal[i + 1]) > 0.5 ? 1 : Math.abs(normal[i + 2]) > 0.5 ? 2 : 0;
    let u = axis === 0 ? 2 : 0;
    let v = axis === 1 ? 2 : 1;
    if (scale.alongGrain && dimensions[v] > dimensions[u]) [u, v] = [v, u];
    uv.push(
      position[i + u] / scale.width + (scale.alongGrain ? phase : offset[u] / scale.width),
      position[i + v] / scale.height + (scale.alongGrain ? phase * 3.53 : offset[v] / scale.height),
    );
  }
  mesh.setVerticesData(VertexBuffer.UVKind, uv);
}
