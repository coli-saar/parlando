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

it("keeps the latest microphone list when passive scans finish out of order", async () => {
  const devices = new EventTarget();
  const enumerateDevices = vi.fn<() => Promise<MediaDeviceInfo[]>>();
  const original = Object.getOwnPropertyDescriptor(navigator, "mediaDevices");
  Object.defineProperty(navigator, "mediaDevices", {
    configurable: true, value: Object.assign(devices, { enumerateDevices }),
  });
  let finishFirst!: (devices: MediaDeviceInfo[]) => void;
  enumerateDevices.mockReturnValueOnce(new Promise((resolve) => { finishFirst = resolve; }));
  const client = new ParticipantClient();
  const audio = createDefaultAudioController();
  const setError = vi.fn();
  const { result, unmount } = renderHook(() => useParticipantVoice(audio, client, null, true, setError));
  try {
    const headset = { deviceId: "headset", kind: "audioinput" } as MediaDeviceInfo;
    enumerateDevices.mockResolvedValueOnce([headset, { kind: "audiooutput" } as MediaDeviceInfo]);
    await act(async () => { devices.dispatchEvent(new Event("devicechange")); });
    await act(async () => { finishFirst([]); });
    expect(result.current.audioInputs).toEqual([headset]);
    enumerateDevices.mockRejectedValueOnce(new Error("Device list unavailable"));
    await act(async () => { devices.dispatchEvent(new Event("devicechange")); });
    expect(result.current.audioInputs).toEqual([headset]);
    expect(setError).not.toHaveBeenCalled();
  } finally {
    unmount();
    if (original) Object.defineProperty(navigator, "mediaDevices", original);
    else Reflect.deleteProperty(navigator, "mediaDevices");
  }
});
