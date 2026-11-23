import { ShaderMaterial } from "@babylonjs/core/Materials/shaderMaterial";
import { Texture } from "@babylonjs/core/Materials/Textures/texture";
import { Mesh } from "@babylonjs/core/Meshes/mesh";
import { MeshBuilder } from "@babylonjs/core/Meshes/meshBuilder";
import type { Scene } from "@babylonjs/core/scene";

/** Surrounds the hotel with the original alpenglow vista. Alternating image direction
 * joins the mountain silhouettes around the horizon. At steep viewing angles the
 * ridges recede into sky or glacial haze, including directly below the bridge.
 */
export function createAlpineLandscape(scene: Scene) {
  const sky = MeshBuilder.CreateSphere("alpenglow above the glacial valley", {
    diameter: 1100, segments: 64, sideOrientation: Mesh.BACKSIDE,
  }, scene);
  const material = new ShaderMaterial("distant Alpine skyline", scene, {
    vertexSource: `
      attribute vec3 position;
      uniform mat4 worldViewProjection;
      varying vec3 direction;
      void main() {
        direction = position;
        gl_Position = worldViewProjection * vec4(position, 1.0);
      }`,
    fragmentSource: `
      precision highp float;
      varying vec3 direction;
      uniform sampler2D mountains;
      void main() {
        vec3 ray = normalize(direction);
        float angle = atan(ray.z, ray.x);
        float elevation = asin(clamp(ray.y, -1.0, 1.0));
        float u = 1.0 - abs(mod(angle * 0.63661977 + 1.45, 2.0) - 1.0);
        float v = 0.46 - elevation / 1.10;
        vec3 vista = texture2D(mountains, vec2(u, clamp(v, 0.0, 1.0))).rgb;
        vista = mix(vista, vec3(0.40, 0.54, 0.72), smoothstep(0.33, 0.505, elevation));
        vista = mix(vista, vec3(0.57, 0.63, 0.72), smoothstep(0.24, 0.52, -elevation));
        gl_FragColor = vec4(vista, 1.0);
      }`,
  }, {attributes: ["position"], uniforms: ["worldViewProjection"], samplers: ["mountains"]});
  const image = new Texture(new URL("../art/alps.jpg", import.meta.url).href, scene, false, false);
  image.wrapU = image.wrapV = Texture.CLAMP_ADDRESSMODE;
  material.setTexture("mountains", image);
  sky.material = material;
  sky.infiniteDistance = true;
  sky.isPickable = false;
}
