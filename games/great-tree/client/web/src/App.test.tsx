import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { ActiveGreatTree } from "./App";
import { ALL_LIMB_IDS, ALL_ROOT_IDS } from "./game/types";
import type { GreatTreeObservation, GreatTreeCompletion } from "./game/types";

function makeSession(overrides: {
  observation: GreatTreeObservation;
  completion?: GreatTreeCompletion | null;
}) {
  return {
    observation: overrides.observation,
    completed: overrides.completion != null,
    completion: overrides.completion ?? null,
    sendAction: vi.fn(),
    sendMessage: vi.fn(),
    leave: vi.fn(),
    voiceEnabled: false,
    voiceStatus: {
      connected: false,
      connecting: false,
      microphoneEnabled: false,
      microphoneChanging: false,
      remoteAudio: false,
      message: "",
      error: null,
      transcriptionMessage: "",
      transcriptionReady: false
    },
    voicePreflight: {
      requested: false,
      ready: false,
      preparing: false,
      message: "",
      micLevel: 0,
      micProbeActive: false,
      deviceLabel: ""
    },
    setMicrophoneMuted: vi.fn().mockResolvedValue(undefined)
  } as never;
}

describe("ActiveGreatTree", () => {
  it("renders CrownView when the observation role is crown", () => {
    const session = makeSession({
      observation: { role: "crown", limbs: ALL_LIMB_IDS.map((id) => ({ id, sun: false, water: false })) }
    });
    render(<ActiveGreatTree session={session} />);
    expect(screen.getByRole("img", { name: /crown/i })).toBeInTheDocument();
  });

  it("shows a leave button during active play", () => {
    const session = makeSession({
      observation: { role: "crown", limbs: ALL_LIMB_IDS.map((id) => ({ id, sun: false, water: false })) }
    });
    render(<ActiveGreatTree session={session} />);
    expect(screen.getByRole("button", { name: /leave game/i })).toBeInTheDocument();
  });

  it("renders RootView when the observation role is root", () => {
    const session = makeSession({
      observation: { role: "root", roots: ALL_ROOT_IDS.map((id) => ({ id, thawed: false, running: false })) }
    });
    render(<ActiveGreatTree session={session} />);
    expect(screen.getByRole("img", { name: /roots/i })).toBeInTheDocument();
  });

  it("shows a terminal screen and hides the gates once completed", () => {
    const session = makeSession({
      observation: { role: "crown", limbs: ALL_LIMB_IDS.map((id) => ({ id, sun: true, water: true })) },
      completion: { floweredLimbs: ["spire", "hook", "fork"] }
    });
    render(<ActiveGreatTree session={session} />);
    expect(screen.getByText(/the tree/i)).toBeInTheDocument();
    expect(screen.queryAllByRole("button", { name: "gate" })).toHaveLength(0);
    expect(screen.getByRole("button", { name: /leave game/i })).toBeInTheDocument();
  });
});
