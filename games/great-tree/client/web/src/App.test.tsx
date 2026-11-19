import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { ActiveGreatTree, GreatTreeSuccess } from "./App";
import { ALL_LIMB_IDS, ALL_ROOT_IDS } from "./game/types";
import type { GreatTreeObservation, GreatTreeCompletion } from "./game/types";

function makeSnapshot(overrides: {
  observation: GreatTreeObservation;
  completion?: GreatTreeCompletion | null;
}) {
  return {
    observation: overrides.observation,
    act: vi.fn()
  } as never;
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

  it("describes normal completion as a cooperative success", () => {
    render(<GreatTreeSuccess completion={{ floweredLimbs: ["spire", "hook", "fork"] }} />);
    expect(screen.getByText("Success")).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "The Great Tree is in bloom!" })).toBeInTheDocument();
    expect(screen.getByText("3 of five limbs flowered together.")).toBeInTheDocument();
  });
});
