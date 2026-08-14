import { describe, expect, it } from "vitest";
import type { ReactElement } from "react";
import { ParlandoStartupGate } from "@coli-saar/parlando-client/react";
import { App } from "./App";

describe("Space Game startup", () => {
  it("delegates Parlando setup and waiting-room startup to the SDK gate", () => {
    const element = App() as ReactElement<{ children: ReactElement }>;
    const gate = element.props.children as ReactElement<{
      renderGame: unknown;
    }>;

    expect(element.type).toBe("main");
    expect(gate.type).toBe(ParlandoStartupGate);
    expect(typeof gate.props.renderGame).toBe("function");
  });
});
