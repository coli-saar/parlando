import { describe, expect, it } from "vitest";
import { limbAppearance, rootAppearance } from "./visualState";
import type { LimbView, RootView } from "./types";

function limb(sun: boolean, water: boolean): LimbView {
  return { id: "spire", sun, water };
}

function root(thawed: boolean, running: boolean): RootView {
  return { id: "hand", thawed, running };
}

describe("limbAppearance", () => {
  it("is dryDark with neither sun nor water", () => {
    expect(limbAppearance(limb(false, false))).toBe("dryDark");
  });
  it("is dryLit with sun only", () => {
    expect(limbAppearance(limb(true, false))).toBe("dryLit");
  });
  it("is wetDark with water only — reachable when sun left after water arrived", () => {
    expect(limbAppearance(limb(false, true))).toBe("wetDark");
  });
  it("is flowered with both", () => {
    expect(limbAppearance(limb(true, true))).toBe("flowered");
  });
});

describe("rootAppearance", () => {
  it("is iced when not thawed", () => {
    expect(rootAppearance(root(false, false))).toBe("iced");
  });
  it("is thawed when thawed but not running", () => {
    expect(rootAppearance(root(true, false))).toBe("thawed");
  });
  it("is running when running (which the server guarantees implies thawed)", () => {
    expect(rootAppearance(root(true, true))).toBe("running");
  });
});
