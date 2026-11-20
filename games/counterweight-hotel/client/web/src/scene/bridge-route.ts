import { Vector3 } from "@babylonjs/core/Maths/math.vector";
import route from "./bridge-route.json";

/** Places both approaches in the East shaft frame, vertically relative to the East rescue stop. */
export function bridgeApproach(west: boolean, westOffset: number): Vector3[] {
  const points = (west ? route.west : route.east).map(([x, z]) => new Vector3(x, west ? westOffset : 0, z));
  if (west) {
    // Keep the gangway level between cars. Distribute the rise around the exterior switchback.
    const distance = points.slice(3).map((point, i) => Vector3.Distance(points[i + 2], point));
    const total = distance.reduce((sum, segment) => sum + segment, 0);
    let traveled = 0;
    for (let i = 3; i < points.length; i++) {
      traveled += distance[i - 3];
      points[i].y = westOffset * (1 - traveled / total);
    }
  }
  const [x, z] = route.shared[0];
  points.push(new Vector3(x, 0, z));
  return points;
}
