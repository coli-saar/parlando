import { describe, expect, it } from "vitest";
import { ALL_LIMB_IDS, ALL_ROOT_IDS } from "./types";
import { LIMB_GEOMETRY, ROOT_GEOMETRY } from "./geometry";

describe("geometry", () => {
  it("has an entry for every limb id, each with at least one bark path", () => {
    for (const id of ALL_LIMB_IDS) {
      expect(LIMB_GEOMETRY[id].barkPaths.length).toBeGreaterThan(0);
      expect(LIMB_GEOMETRY[id].gate).toBeDefined();
    }
  });

  it("has an entry for every root id, each with at least one body path", () => {
    for (const id of ALL_ROOT_IDS) {
      expect(ROOT_GEOMETRY[id].bodyPaths.length).toBeGreaterThan(0);
      expect(ROOT_GEOMETRY[id].gate).toBeDefined();
    }
  });

  it("gives every limb a distinct gate position (so hit targets never overlap)", () => {
    const positions = ALL_LIMB_IDS.map((id) => `${LIMB_GEOMETRY[id].gate.x},${LIMB_GEOMETRY[id].gate.y}`);
    expect(new Set(positions).size).toBe(positions.length);
  });
});
