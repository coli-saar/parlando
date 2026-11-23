import { Vector3 } from "@babylonjs/core/Maths/math.vector";
import { guestRadius } from "./walking";

/** Creates navigation along the actual timber decks, using their construction joints and porch. */
export function createRefugeWalk(approach: readonly Vector3[], far: Vector3, threshold: Vector3) {
  const joints = [...approach, far, threshold];

  /** Projects a position onto the nearest stretch of gallery or bridge in plan view. */
  function locate(x: number, z: number) {
    let distance = Infinity, floor = 0, next = 1;
    for (let i = 1; i < joints.length; i++) {
      const a = joints[i - 1], b = joints[i];
      const dx = b.x - a.x, dz = b.z - a.z;
      const t = Math.max(0, Math.min(1, ((x - a.x) * dx + (z - a.z) * dz) / (dx * dx + dz * dz)));
      const offset = Math.hypot(x - a.x - t * dx, z - a.z - t * dz);
      if (offset < distance) {
        distance = offset;
        floor = a.y + t * (b.y - a.y);
        next = i;
      }
    }
    return { distance, floor, next };
  }

  /** Checks clearance along potential shortcuts across the cabin approach. */
  function clearLine(a: Vector3, b: Vector3, canStand: (x: number, z: number) => boolean) {
    const samples = Math.ceil(Math.hypot(b.x - a.x, b.z - a.z) / 0.08);
    for (let i = 1; i <= samples; i++) {
      if (!canStand(a.x + (b.x - a.x) * i / samples, a.z + (b.z - a.z) * i / samples)) return false;
    }
    return true;
  }

  /** Finds a furniture-clear cabin approach using short steps, then follows the gallery centreline. */
  function guide(start: Vector3, canStand: (x: number, z: number) => boolean) {
    const location = locate(start.x, start.z);
    if (location.distance < 0.9 - guestRadius) return joints.slice(location.next);
    const queue = [{ x: start.x, z: start.z, parent: -1 }];
    const visited = new Set<string>();
    for (let index = 0; index < queue.length; index++) {
      const point = queue[index];
      if (Math.hypot(point.x - joints[0].x, point.z - joints[0].z) < 0.28) {
        const path: Vector3[] = [];
        for (let i = index; i > 0; i = queue[i].parent) path.push(new Vector3(queue[i].x, 0, queue[i].z));
        // Follow straight sight lines between obstacles instead of turning at every grid cell.
        path.reverse().push(joints[0]);
        const approach = [start];
        for (let i = 0; i < path.length; i++) {
          while (i + 1 < path.length && clearLine(approach[approach.length - 1], path[i + 1], canStand)) i++;
          approach.push(path[i]);
        }
        return [...approach.slice(1), ...joints.slice(1)];
      }
      for (const [dx, dz] of [[1, 0], [0, 1], [-1, 0], [0, -1]]) {
        const x = point.x + dx * 0.25, z = point.z + dz * 0.25;
        const key = x.toFixed(3) + "," + z.toFixed(3);
        if (visited.has(key) || !canStand(x, z) ||
            !canStand((x + point.x) / 2, (z + point.z) / 2)) continue;
        visited.add(key);
        queue.push({ x, z, parent: index });
      }
    }
    return [];
  }

  const outward = far.subtract(threshold).normalize();
  /** Tests the rectangular porch in door-local coordinates, including its two outer edges. */
  function onMat(x: number, z: number) {
    const dx = x - threshold.x, dz = z - threshold.z;
    const forward = dx * outward.x + dz * outward.z;
    return forward >= -0.09 && forward <= 0.49 && Math.abs(dx * outward.z - dz * outward.x) <= 0.96;
  }

  return {
    guide, onMat,
    /** Reserves guest clearance at both rails and stops the guest short of the lodge facade. */
    floorAt(x: number, z: number, tolerance = 0) {
      if (onMat(x, z)) return threshold.y;
      const location = locate(x, z);
      if (location.next === joints.length - 1 &&
          (x - threshold.x) * outward.x + (z - threshold.z) * outward.z < -0.09 - tolerance) return undefined;
      return location.distance <= 0.9 - guestRadius + tolerance ? location.floor : undefined;
    },
  };
}
