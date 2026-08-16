// @vitest-environment happy-dom

import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import "@testing-library/jest-dom/vitest";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { AudioSessionSnapshot, VoiceStatus } from "./audio/types";
import { initialVoicePreflight, initialVoiceStatus } from "./audio/types";
import {
  DeviceSelect,
  MicrophoneMuteButton,
  TranscriptionStatusChip,
  VoiceStatusChip,
  useVoiceController
} from "./react";
import { MicLevelMeter, TranscriptionProgress } from "./voiceComponents";

afterEach(cleanup);

/** Creates one voice status without repeating the complete public shape. */
function status(overrides: Partial<VoiceStatus> = {}): VoiceStatus {
  return { ...initialVoiceStatus, ...overrides };
}

/** Creates the browser device shape required by the selector component. */
function device(deviceId: string, label: string): MediaDeviceInfo {
  return { deviceId, label, groupId: "", kind: "audioinput", toJSON: () => ({}) };
}

class FakeController {
  snapshotValue: AudioSessionSnapshot = {
    voiceStatus: initialVoiceStatus,
    voicePreflight: initialVoicePreflight
  };
  listeners = new Set<(snapshot: AudioSessionSnapshot) => void>();
  subscriptions = 0;
  unsubscriptions = 0;

  /** Returns the current immutable-by-replacement controller snapshot. */
  snapshot(): AudioSessionSnapshot {
    return this.snapshotValue;
  }

  /** Registers a listener and reports cleanup calls for lifecycle assertions. */
  subscribe(listener: (snapshot: AudioSessionSnapshot) => void): () => void {
    this.subscriptions += 1;
    this.listeners.add(listener);
    listener(this.snapshotValue);
    return () => {
      this.unsubscriptions += 1;
      this.listeners.delete(listener);
    };
  }

  /** Publishes a new snapshot to every current hook subscriber. */
  publish(message: string): void {
    this.snapshotValue = {
      ...this.snapshotValue,
      voiceStatus: status({ message })
    };
    for (const listener of this.listeners) listener(this.snapshotValue);
  }
}

/** Renders the current hook message for subscription lifecycle tests. */
function HookProbe({ controller }: { controller: FakeController }) {
  const snapshot = useVoiceController(controller as never);
  return <span>{snapshot.voiceStatus.message}</span>;
}

describe("voice React components", () => {
  it("labels, disables, and reports the microphone mute button state", () => {
    const changeMuted = vi.fn();
    const { rerender } = render(<MicrophoneMuteButton voiceEnabled={false} onMicrophoneMutedChange={changeMuted} />);
    expect(screen.getByRole("button", { name: "Microphone unavailable" })).toBeDisabled();

    rerender(<MicrophoneMuteButton voiceEnabled onMicrophoneMutedChange={changeMuted} voiceStatus={status({ connecting: true })} />);
    expect(screen.getByRole("button", { name: "Connecting" })).toBeDisabled();

    rerender(<MicrophoneMuteButton voiceEnabled onMicrophoneMutedChange={changeMuted} voiceStatus={status({ connected: true, microphoneEnabled: true })} />);
    fireEvent.click(screen.getByRole("button", { name: "Mute mic" }));
    expect(changeMuted).toHaveBeenCalledWith(true);

    rerender(<MicrophoneMuteButton voiceEnabled onMicrophoneMutedChange={changeMuted} voiceStatus={status({ connected: true, microphoneEnabled: false })} />);
    expect(screen.getByRole("button", { name: "Unmute mic" })).toHaveAttribute("aria-pressed", "true");

    rerender(<MicrophoneMuteButton voiceEnabled onMicrophoneMutedChange={changeMuted} voiceStatus={status({ connected: true, microphoneEnabled: false, microphoneChanging: true })} />);
    expect(screen.getByRole("button", { name: "Unmuting…" })).toBeDisabled();
  });

  it("renders fallback device labels and emits the selected id once", () => {
    const selected = vi.fn();
    render(
      <DeviceSelect
        audioInputs={[device("built-in", ""), device("usb", "USB microphone")]}
        onSelectedAudioInputChange={selected}
        selectedAudioInputId="built-in"
      />
    );
    const selector = screen.getByRole("combobox", { name: "Microphone input" });
    expect(screen.getByRole("option", { name: "Microphone 1" })).toBeInTheDocument();
    fireEvent.change(selector, { target: { value: "usb" } });
    expect(selected).toHaveBeenCalledOnce();
    expect(selected).toHaveBeenCalledWith("usb");
  });

  it("offers a default microphone when enumeration is empty", () => {
    render(<DeviceSelect audioInputs={[]} disabled onSelectedAudioInputChange={vi.fn()} selectedAudioInputId="" />);
    expect(screen.getByRole("combobox")).toBeDisabled();
    expect(screen.getByRole("option", { name: "Default microphone" })).toBeInTheDocument();
  });

  it("clamps invalid microphone meter values to a normalized visual range", () => {
    const { container, rerender } = render(<MicLevelMeter active label="Input" level={2} />);
    expect(container.querySelector(".mic-meter-track span")).toHaveStyle({ transform: "scaleX(1)" });
    rerender(<MicLevelMeter active label="Input" level={Number.NaN} />);
    expect(container.querySelector(".mic-meter-track span")).toHaveStyle({ transform: "scaleX(0)" });
    rerender(<MicLevelMeter active={false} label="Input" level={0.7} />);
    expect(container.querySelector(".mic-meter-track span")).toHaveStyle({ transform: "scaleX(0)" });
    rerender(<MicLevelMeter active label="Input" level={0.7} muted />);
    expect(container.querySelector(".mic-meter")).toHaveClass("muted");
    expect(container.querySelector(".mic-meter-track span")).toHaveStyle({ transform: "scaleX(0.7)" });
  });

  it("renders transcription headings for game, transport, and ready states", () => {
    const { rerender } = render(<TranscriptionProgress gameConnected={false} />);
    expect(screen.getByText("Waiting for game connection")).toBeInTheDocument();
    rerender(<TranscriptionProgress gameConnected voiceStatus={status()} />);
    expect(screen.getByText("Preparing voice connection")).toBeInTheDocument();
    rerender(<TranscriptionProgress gameConnected voiceStatus={status({ connected: true, transcriptionReady: true, transcriptionMessage: "ASR ready" })} />);
    expect(screen.getByText("Transcription ready")).toBeInTheDocument();
    expect(screen.getAllByRole("listitem").every((item) => item.className === "done")).toBe(true);
  });

  it("renders disabled voice and transcription error chips", () => {
    const { container } = render(
      <>
        <VoiceStatusChip voiceEnabled={false} />
        <TranscriptionStatusChip voiceStatus={status({ transcriptionMessage: "ASR error" })} />
      </>
    );
    expect(screen.getByText("Voice is disabled for this study")).toBeInTheDocument();
    expect(container.querySelector(".transcription-chip")).toHaveClass("error");
  });
});

describe("useVoiceController", () => {
  it("subscribes once, applies updates, and unsubscribes on unmount", () => {
    const controller = new FakeController();
    const view = render(<HookProbe controller={controller} />);
    expect(controller.subscriptions).toBe(1);
    act(() => controller.publish("Microphone live"));
    expect(screen.getByText("Microphone live")).toBeInTheDocument();
    view.unmount();
    expect(controller.unsubscriptions).toBe(1);
    act(() => controller.publish("late update"));
    expect(controller.listeners.size).toBe(0);
  });

  it("moves subscription ownership when the controller instance changes", () => {
    const oldController = new FakeController();
    const currentController = new FakeController();
    currentController.snapshotValue = {
      ...currentController.snapshotValue,
      voiceStatus: status({ message: "Current controller" })
    };
    const view = render(<HookProbe controller={oldController} />);
    view.rerender(<HookProbe controller={currentController} />);
    expect(oldController.unsubscriptions).toBe(1);
    expect(currentController.subscriptions).toBe(1);
    act(() => currentController.publish("Current update"));
    expect(screen.getByText("Current update")).toBeInTheDocument();
  });
});
