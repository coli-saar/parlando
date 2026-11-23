import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { ActiveGreatTree, GreatTreeSuccess } from "./App";
import { ALL_LIMB_IDS, ALL_ROOT_IDS } from "./game/types";
import type { GreatTreeObservation } from "./game/types";

/** Creates the subset of participant state consumed by the game view. */
function makeSnapshot(overrides: {
  observation: GreatTreeObservation;
  interactionEnabled?: boolean;
}) {
  return {
    observation: overrides.observation,
    interactionEnabled: overrides.interactionEnabled ?? true,
    act: vi.fn()
  };
}

describe("ActiveGreatTree", () => {
  it("renders CrownView when the observation role is crown", () => {
    const snapshot = makeSnapshot({
      observation: { role: "crown", limbs: ALL_LIMB_IDS.map((id) => ({ id, sun: false, water: false })) }
    });
    render(<ActiveGreatTree snapshot={snapshot} />);
    expect(screen.getByRole("img", { name: /crown/i })).toBeInTheDocument();
  });

  it("renders RootView when the observation role is root", () => {
    const snapshot = makeSnapshot({
      observation: { role: "root", roots: ALL_ROOT_IDS.map((id) => ({ id, thawed: false, running: false })) }
    });
    render(<ActiveGreatTree snapshot={snapshot} />);
    expect(screen.getByRole("img", { name: /roots/i })).toBeInTheDocument();
  });

  it("keeps replay gates visible and disables pointer and keyboard actions", () => {
    const snapshot = makeSnapshot({
      observation: { role: "crown", limbs: ALL_LIMB_IDS.map((id) => ({ id, sun: false, water: false })) },
      interactionEnabled: false
    });
    render(<ActiveGreatTree snapshot={snapshot} />);
    for (const gate of screen.getAllByRole("button", { name: "gate" })) {
      expect(gate).toHaveAttribute("aria-disabled", "true");
      fireEvent.click(gate);
      fireEvent.keyDown(gate, { key: "Enter" });
    }
    expect(snapshot.act).not.toHaveBeenCalled();
  });

  it("describes normal completion as a cooperative success", () => {
    render(<GreatTreeSuccess completion={{ floweredLimbs: ["spire", "hook", "fork"] }} />);
    expect(screen.getByText("Success")).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "The Great Tree is in bloom!" })).toBeInTheDocument();
    expect(screen.getByText("3 of five limbs flowered together.")).toBeInTheDocument();
  });
});
