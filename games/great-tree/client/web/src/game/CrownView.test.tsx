import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { CrownView } from "./CrownView";
import { ALL_LIMB_IDS } from "./types";

describe("CrownView", () => {
  it("renders one gate per limb", () => {
    const limbs = ALL_LIMB_IDS.map((id) => ({ id, sun: false, water: false }));
    render(<CrownView limbs={limbs} onSetSun={() => {}} />);
    expect(screen.getAllByRole("button")).toHaveLength(5);
  });

  it("clicking a dark limb's gate sends SetSun with lit: true", async () => {
    const limbs = ALL_LIMB_IDS.map((id) => ({ id, sun: false, water: false }));
    const onSetSun = vi.fn();
    render(<CrownView limbs={limbs} onSetSun={onSetSun} />);
    await userEvent.click(screen.getAllByRole("button")[0]);
    expect(onSetSun).toHaveBeenCalledWith("spire", true);
  });

  it("clicking an already-lit limb's gate sends SetSun with lit: false", async () => {
    const limbs = ALL_LIMB_IDS.map((id) => ({ id, sun: id === "hook", water: false }));
    const onSetSun = vi.fn();
    render(<CrownView limbs={limbs} onSetSun={onSetSun} />);
    const hookIndex = ALL_LIMB_IDS.indexOf("hook");
    await userEvent.click(screen.getAllByRole("button")[hookIndex]);
    expect(onSetSun).toHaveBeenCalledWith("hook", false);
  });
});
