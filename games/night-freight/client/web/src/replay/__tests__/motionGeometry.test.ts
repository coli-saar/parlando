/**
 * Yard geometry: the laid track that the motion rules stand on.
 *
 * Checks that every yard track of every station is a plausible railway line (no curve
 * tighter than a turnout of 150 m radius, merging only through the turnouts, all tracks
 * one at the throats), that the edges continue the necks smoothly, and that cars parked
 * within a siding's reach keep clear of every neighbouring track.
 */
import { describe, expect, it } from "vitest";
import {
  FOUL_CLEARANCE, HEADSHUNT_LENGTH, MAX_LEVEL, SIDING_LEVELS, STATION_END, STATION_EXTENT, STATION_HALF, TRACK_SPACING, TURNOUT_LENGTH, divergencePoint, sidingReach, trackLateral
} from "../../scene/geography";
import { motionFixtures } from "./motionFixtures";

const { geo } = motionFixtures()[0];

/** Smallest radius (metres) of a polyline, from circles through three points a few metres apart. */
function minRadius(points: { x: number; z: number }[], step = 3): number {
  let best = Infinity;
  for (let i = step; i < points.length - step; i++) {
    const a = points[i - step], b = points[i], c = points[i + step];
    const ab = Math.hypot(b.x - a.x, b.z - a.z), bc = Math.hypot(c.x - b.x, c.z - b.z), ac = Math.hypot(c.x - a.x, c.z - a.z);
    const area2 = Math.abs((b.x - a.x) * (c.z - a.z) - (b.z - a.z) * (c.x - a.x));
    if (area2 < 1e-9) continue;
    best = Math.min(best, (ab * bc * ac) / (2 * area2));
  }
  return best;
}

describe("yard geometry", () => {
  it("has no curve tighter than 150 m on any yard track", () => {
    for (const [id] of geo.stations) {
      for (let level = -MAX_LEVEL; level <= MAX_LEVEL; level++) {
        const lane = geo.lane(id, level, 1);
        expect(minRadius(lane.path.points, 4), `${id} level ${level}`).toBeGreaterThan(145);
      }
    }
  });

  it("turnouts are 1:6 or flatter", () => {
    for (let level = 1; level <= MAX_LEVEL; level++) {
      for (let u = 0; u < STATION_HALF; u += 0.5) {
        const slope = Math.abs(trackLateral(level, u + 0.25) - trackLateral(level, u - 0.25)) / 0.5;
        // Slopes of the cascade add where two turnouts overlap; the steepest point of one turnout is L / 2R, about 10 degrees.
        expect(slope).toBeLessThan(0.2);
      }
    }
    expect(TURNOUT_LENGTH).toBeGreaterThan(40);
  });

  it("merges every track into its neighbour only at the throats", () => {
    for (let level = -MAX_LEVEL; level <= MAX_LEVEL; level++) {
      expect(trackLateral(level, 0)).toBeCloseTo(level * TRACK_SPACING, 9);
      expect(trackLateral(level, STATION_HALF)).toBeCloseTo(0, 9);
      expect(trackLateral(level, STATION_EXTENT)).toBeCloseTo(0, 9);
      expect(trackLateral(level, -STATION_HALF - 1)).toBeCloseTo(0, 9);
    }
    expect(HEADSHUNT_LENGTH).toBeGreaterThanOrEqual(100);
    for (const level of SIDING_LEVELS) expect(divergencePoint(0, level)).toBeLessThanOrEqual(STATION_HALF);
    expect(divergencePoint(1, 2)).toBeLessThan(divergencePoint(0, 2));
  });

  it("lets cars park within a siding's reach without fouling any other track", () => {
    for (const level of SIDING_LEVELS) {
      const reach = sidingReach(level);
      expect(reach).toBeGreaterThan(30);
      for (let u = 0; u <= reach; u += 0.5) {
        for (const other of [-3, -2, -1, 0, 1, 2, 3]) {
          if (other === level) continue;
          expect(Math.abs(trackLateral(level, u) - trackLateral(other, u)), `level ${level} against ${other} at ${u}`).toBeGreaterThan(FOUL_CLEARANCE - 1e-6 - (Math.abs(other - level) > 1 ? TRACK_SPACING : 0));
        }
      }
    }
  });

  it("leaves every neck for the edges at the junction without a kink", () => {
    for (const e of geo.edges) {
      for (const [id, end, at] of [[e.a, e.endA, 0], [e.b, e.endB, e.path.length]] as const) {
        const site = geo.stations.get(id)!;
        const neck = { x: site.x + end * STATION_END * Math.cos(site.yaw), z: site.z + end * STATION_END * Math.sin(site.yaw) };
        const p = e.path.at(at).p;
        expect(Math.hypot(p.x - neck.x, p.z - neck.z)).toBeLessThan(0.01);
      }
      // The edges are plain main lines: nowhere tighter than 95 m.
      expect(minRadius(e.path.points, 2), `${e.a}-${e.b}`).toBeGreaterThan(95);
    }
  });
});
