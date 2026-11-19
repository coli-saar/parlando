// @vitest-environment happy-dom

import { fireEvent, render, screen } from "@testing-library/react";
import "@testing-library/jest-dom/vitest";
import { expect, it, vi } from "vitest";
import { initialVoicePreflight, initialVoiceStatus } from "../audio/types.js";
import { SessionTools } from "./sessionControls.js";

it("keeps a draft while interaction is paused and sends it once when play resumes", () => {
  const send = vi.fn();
  const props = {
    connected: true,
    conversation: [],
    onLeave: vi.fn(),
    onMessage: send,
    onMutedChange: vi.fn(),
    role: "A" as const,
    sessionId: "room",
    voiceEnabled: false,
    voicePreflight: initialVoicePreflight,
    voiceStatus: initialVoiceStatus,
  };
  const { rerender, unmount } = render(<SessionTools {...props} interactionEnabled />);
  const input = screen.getByRole("textbox", { name: "Message" });
  fireEvent.change(input, { target: { value: "  Hello  " } });
  rerender(<SessionTools {...props} interactionEnabled={false} />);
  const submit = screen.getByRole("button", { name: "Send message" });
  expect(submit).toBeDisabled();
  expect(input).toHaveValue("  Hello  ");
  fireEvent.click(submit);
  expect(send).not.toHaveBeenCalled();
  rerender(<SessionTools {...props} interactionEnabled />);
  fireEvent.click(submit);
  expect(send.mock.calls).toEqual([["Hello"]]);
  expect(input).toHaveValue("");
  expect(submit).toBeDisabled();
  unmount();
});
