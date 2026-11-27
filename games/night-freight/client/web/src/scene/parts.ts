/**
 * Part builder: small helpers to assemble vertex-coloured primitives and merge
 * them into one mesh per object.
 *
 * Owns the shared recipe used by the rolling stock and the station set pieces:
 * create a primitive at a local position, paint all its vertices one colour, and
 * merge a list of parts into a single hidden-or-visible mesh with one material.
 * Callers rely on merged meshes keeping the parts' local transforms (no parent
 * transform is applied), so a merged mesh can be instanced or parented freely.
 */
import { Color3, Mesh, MeshBuilder, Scene, StandardMaterial, VertexBuffer, VertexData } from "@babylonjs/core";

/** Creates coloured primitives and merges them. */
export class PartBuilder {
  private counter = 0;
  constructor(private readonly scene: Scene) {}

  /** Paints all vertices of a part with one hex colour. */
  tint(m: Mesh, hex: string): Mesh {
    const c = Color3.FromHexString(hex);
    const n = (m.getVerticesData(VertexBuffer.PositionKind)?.length ?? 0) / 3;
    const colors: number[] = [];
    for (let i = 0; i < n; i++) colors.push(c.r, c.g, c.b, 1);
    m.setVerticesData(VertexBuffer.ColorKind, colors);
    return m;
  }

  /** A coloured box centred at (x,y,z). */
  box(w: number, h: number, d: number, x: number, y: number, z: number, color: string): Mesh {
    const m = MeshBuilder.CreateBox(`p${this.counter++}`, { width: w, height: h, depth: d }, this.scene);
    m.position.set(x, y, z);
    return this.tint(m, color);
  }

  /** A coloured cylinder (or cone when `top` differs) whose length runs along `axis`. */
  cyl(dia: number, len: number, x: number, y: number, z: number, axis: "x" | "y" | "z", color: string, tess = 14, topDia?: number): Mesh {
    const m = MeshBuilder.CreateCylinder(`p${this.counter++}`, {
      diameterBottom: dia, diameterTop: topDia ?? dia, height: len, tessellation: tess
    }, this.scene);
    m.position.set(x, y, z);
    if (axis === "x") m.rotation.z = Math.PI / 2;
    if (axis === "z") m.rotation.x = Math.PI / 2;
    return this.tint(m, color);
  }

  /** A coloured sphere (optionally squashed) at (x,y,z). */
  ball(dia: number, x: number, y: number, z: number, color: string, sy = 1): Mesh {
    const m = MeshBuilder.CreateSphere(`p${this.counter++}`, { diameter: dia, segments: 8 }, this.scene);
    m.position.set(x, y, z);
    m.scaling.y = sy;
    return this.tint(m, color);
  }

  /** A gabled roof prism: width across x, length along z, ridge height h above the base plane y. */
  gable(w: number, len: number, h: number, x: number, y: number, z: number, color: string): Mesh {
    const hw = w / 2, hl = len / 2;
    const v = [
      [-hw, 0, -hl], [hw, 0, -hl], [0, h, -hl],   // front gable
      [-hw, 0, hl], [0, h, hl], [hw, 0, hl],      // back gable
      [-hw, 0, -hl], [0, h, -hl], [0, h, hl], [-hw, 0, hl], // left slope
      [hw, 0, -hl], [hw, 0, hl], [0, h, hl], [0, h, -hl],   // right slope
      [-hw, 0, -hl], [-hw, 0, hl], [hw, 0, hl], [hw, 0, -hl] // underside
    ];
    const idx = [0, 2, 1, 3, 5, 4, 6, 7, 8, 6, 8, 9, 10, 11, 12, 10, 12, 13, 14, 16, 15, 14, 17, 16];
    const m = new Mesh(`p${this.counter++}`, this.scene);
    const vd = new VertexData();
    vd.positions = v.flat();
    vd.indices = idx;
    const normals: number[] = [];
    VertexData.ComputeNormals(vd.positions, idx, normals);
    vd.normals = normals;
    vd.uvs = new Array((v.length) * 2).fill(0);
    vd.applyToMesh(m);
    m.position.set(x, y, z);
    return this.tint(m, color);
  }

  /** Merges parts (their transforms are baked) and applies a material; the result is visible by default. */
  merge(name: string, parts: Mesh[], material: StandardMaterial, visible = true): Mesh {
    const merged = Mesh.MergeMeshes(parts, true, true)!;
    merged.name = name;
    merged.material = material;
    merged.isVisible = visible;
    return merged;
  }
}
