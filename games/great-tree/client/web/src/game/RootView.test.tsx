import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { RootView } from "./RootView";
import { ALL_ROOT_IDS } from "./types";

describe("RootView", () => {
  it("renders one gate per root", () => {
    const roots = ALL_ROOT_IDS.map((id) => ({ id, thawed: false, running: false }));
    render(<RootView roots={roots} onSetFlow={() => {}} />);
    expect(screen.getAllByRole("button")).toHaveLength(5);
  });

  it("clicking a thawed, closed root's gate sends SetFlow with open: true", async () => {
    const roots = ALL_ROOT_IDS.map((id) => ({ id, thawed: id === "hand", running: false }));
    const onSetFlow = vi.fn();
    render(<RootView roots={roots} onSetFlow={onSetFlow} />);
    const handIndex = ALL_ROOT_IDS.indexOf("hand");
    await userEvent.click(screen.getAllByRole("button")[handIndex]);
    expect(onSetFlow).toHaveBeenCalledWith("hand", true);
  });

  it("clicking a running root's gate sends SetFlow with open: false", async () => {
    const roots = ALL_ROOT_IDS.map((id) => ({ id, thawed: id === "knot", running: id === "knot" }));
    const onSetFlow = vi.fn();
    render(<RootView roots={roots} onSetFlow={onSetFlow} />);
    const knotIndex = ALL_ROOT_IDS.indexOf("knot");
    await userEvent.click(screen.getAllByRole("button")[knotIndex]);
    expect(onSetFlow).toHaveBeenCalledWith("knot", false);
  });

  it("clicking an iced root's gate does nothing — the client already knows it's frozen", async () => {
    const roots = ALL_ROOT_IDS.map((id) => ({ id, thawed: false, running: false }));
    const onSetFlow = vi.fn();
    render(<RootView roots={roots} onSetFlow={onSetFlow} />);
    await userEvent.click(screen.getAllByRole("button")[0]);
    expect(onSetFlow).not.toHaveBeenCalled();
  });
});
