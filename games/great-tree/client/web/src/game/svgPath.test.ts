import { describe, expect, it } from "vitest";
import { offsetPathX, reversePath } from "./svgPath";

describe("reversePath", () => {
  it("reverses a channel path's point order end to end", () => {
    expect(reversePath("M96 132 C 80 190 60 250 50 300")).toBe("M50 300 C 60 250 80 190 96 132");
  });
});

describe("offsetPathX", () => {
  it("shifts every x-coordinate by dx and leaves y unchanged", () => {
    expect(offsetPathX("M96 132 C 80 190 60 250 50 300", -4)).toBe(
      "M92 132 C 76 190 56 250 46 300"
    );
  });

  it("shifts in the positive direction too", () => {
    expect(offsetPathX("M10 20 C 30 40 50 60 70 80", 3)).toBe("M13 20 C 33 40 53 60 73 80");
  });
});
