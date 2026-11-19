// @vitest-environment happy-dom

import { act, renderHook } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import { ParticipantClient } from "../protocol.js";
import { createDefaultAudioController } from "./microphone.js";
import { useParticipantVoice } from "./useParticipantVoice.js";

it("ignores a late microphone permission failure after the participant leaves", async () => {
  const audio = createDefaultAudioController();
  const client = new ParticipantClient();
  let denyPermission!: (error: Error) => void;
  vi.spyOn(audio, "prepare").mockImplementation(() => new Promise<void>((_, reject) => {
    denyPermission = reject;
  }));
  const setError = vi.fn();
  const { result, unmount } = renderHook(() => useParticipantVoice(audio, client, null, true, setError));
  let preparation!: Promise<boolean>;
  act(() => {
    preparation = result.current.prepareVoice("headset");
    result.current.cancelPreparation();
  });
  await act(async () => {
    denyPermission(new Error("late permission failure"));
    expect(await preparation).toBe(false);
  });
  expect(setError.mock.calls).toEqual([[""]]);
  unmount();
});
