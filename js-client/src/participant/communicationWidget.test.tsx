// @vitest-environment happy-dom

import { fireEvent, render, screen } from "@testing-library/react";
import "@testing-library/jest-dom/vitest";
import { expect, it, vi } from "vitest";
import { CommunicationContext, CommunicationWidget } from "./communicationWidget.js";

it("keeps a draft while interaction is paused and sends it once when play resumes", () => {
  const send = vi.fn(() => true);
  // Feed the same widget new connection state without replacing the local draft.
  const widget = (enabled: boolean) => (
    <CommunicationContext.Provider value={{ mode: "typed", conversation: [], enabled, send }}>
      <CommunicationWidget />
    </CommunicationContext.Provider>
  );
  const { rerender, unmount } = render(widget(true));
  const input = screen.getByRole("textbox", { name: "Message" });
  fireEvent.change(input, { target: { value: "  Hello  " } });
  rerender(widget(false));
  const submit = screen.getByRole("button", { name: "Send message" });
  expect(submit).toBeDisabled();
  expect(input).toHaveValue("  Hello  ");
  fireEvent.click(submit);
  fireEvent.submit(input.closest("form")!);
  expect(input).toHaveValue("  Hello  ");
  expect(send).not.toHaveBeenCalled();
  rerender(widget(true));
  send.mockReturnValueOnce(false);
  fireEvent.click(submit);
  expect(input).toHaveValue("  Hello  ");
  fireEvent.click(submit);
  expect(send.mock.calls).toEqual([["Hello"], ["Hello"]]);
  expect(input).toHaveValue("");
  expect(submit).toBeDisabled();
  unmount();
});

it("follows new messages only while the reader is at the bottom", () => {
  const send = vi.fn(() => true);

  /** Simulates a growing transcript while the same widget stays mounted. */
  const widget = (count: number) => (
    <CommunicationContext.Provider value={{
      mode: "typed", enabled: true, send,
      conversation: Array.from({ length: count }, (_, index) => ({
        id: String(index), input: "text", sender: "B", text: `Message ${index}`, createdAt: ""
      }))
    }}>
      <CommunicationWidget />
    </CommunicationContext.Provider>
  );
  const { rerender, unmount } = render(widget(1));
  const list = screen.getByRole("list", { name: "Text conversation" });
  let contentHeight = 200;
  Object.defineProperty(list, "scrollHeight", { get: () => contentHeight });
  Object.defineProperty(list, "clientHeight", { value: 100 });

  list.scrollTop = 100;
  fireEvent.scroll(list);
  contentHeight = 250;
  rerender(widget(2));
  expect(list.scrollTop).toBe(250);

  list.scrollTop = 10;
  fireEvent.scroll(list);
  contentHeight = 300;
  rerender(widget(3));
  expect(list.scrollTop).toBe(10);
  unmount();
});
