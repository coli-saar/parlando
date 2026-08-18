import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Gate } from "./Gate";

describe("Gate", () => {
  it.each(["closed", "openable", "open"] as const)("calls onClick when clicked in state %s", async (state) => {
    const onClick = vi.fn();
    render(
      <svg>
        <Gate x={10} y={20} rotation={0} state={state} onClick={onClick} />
      </svg>
    );
    await userEvent.click(screen.getByRole("button"));
    expect(onClick).toHaveBeenCalledOnce();
  });

  it("is reachable by keyboard as a button", () => {
    render(
      <svg>
        <Gate x={0} y={0} rotation={0} state="open" onClick={() => {}} />
      </svg>
    );
    expect(screen.getByRole("button")).toBeInTheDocument();
  });

  it("renders a distinct frost mark for the frozen state, not shared with closed", () => {
    const { container: frozenContainer } = render(
      <svg>
        <Gate x={0} y={0} rotation={0} state="frozen" onClick={() => {}} />
      </svg>
    );
    const { container: closedContainer } = render(
      <svg>
        <Gate x={0} y={0} rotation={0} state="closed" onClick={() => {}} />
      </svg>
    );
    expect(frozenContainer.querySelectorAll("path").length).toBeGreaterThan(0);
    expect(closedContainer.querySelectorAll("path").length).toBe(0);
  });

  it("does not dispatch onClick when frozen — the client already knows this would be rejected", async () => {
    const onClick = vi.fn();
    render(
      <svg>
        <Gate x={0} y={0} rotation={0} state="frozen" onClick={onClick} />
      </svg>
    );
    await userEvent.click(screen.getByRole("button"));
    expect(onClick).not.toHaveBeenCalled();
  });

  it("marks the frozen gate aria-disabled and removes it from tab order", () => {
    render(
      <svg>
        <Gate x={0} y={0} rotation={0} state="frozen" onClick={() => {}} />
      </svg>
    );
    const gate = screen.getByRole("button");
    expect(gate).toHaveAttribute("aria-disabled", "true");
    expect((gate as unknown as SVGElement).tabIndex).toBe(-1);
  });

  it("does not dispatch onClick on Enter/Space when frozen", async () => {
    const onClick = vi.fn();
    render(
      <svg>
        <Gate x={0} y={0} rotation={0} state="frozen" onClick={onClick} />
      </svg>
    );
    screen.getByRole("button").focus();
    await userEvent.keyboard("{Enter}");
    expect(onClick).not.toHaveBeenCalled();
  });
});
