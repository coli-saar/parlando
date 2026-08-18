import { describe, expect, it } from "vitest";
import { ALL_LIMB_IDS, ALL_ROOT_IDS, type GreatTreeAction, type GreatTreeObservation } from "./types";

describe("great tree types", () => {
  it("lists exactly the five limb ids the server serializes", () => {
    expect(ALL_LIMB_IDS).toEqual(["spire", "hook", "fork", "cradle", "nub"]);
  });

  it("lists exactly the five root ids the server serializes", () => {
    expect(ALL_ROOT_IDS).toEqual(["hand", "knot", "tip", "swollen", "deep"]);
  });

  it("accepts a crown observation shaped like the Rust adapter's JSON", () => {
    const sample: GreatTreeObservation = {
      role: "crown",
      limbs: ALL_LIMB_IDS.map((id) => ({ id, sun: false, water: false }))
    };
    expect(sample.role).toBe("crown");
  });

  it("accepts a root observation shaped like the Rust adapter's JSON", () => {
    const sample: GreatTreeObservation = {
      role: "root",
      roots: ALL_ROOT_IDS.map((id) => ({ id, thawed: false, running: false }))
    };
    expect(sample.role).toBe("root");
  });

  it('accepts both action shapes the server\'s #[serde(tag = "type")] expects', () => {
    const setSun: GreatTreeAction = { type: "setSun", limb: "hook", lit: true };
    const setFlow: GreatTreeAction = { type: "setFlow", root: "knot", open: false };
    expect(setSun.type).toBe("setSun");
    expect(setFlow.type).toBe("setFlow");
  });
});
