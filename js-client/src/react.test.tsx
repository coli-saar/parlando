// @vitest-environment happy-dom

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import "@testing-library/jest-dom/vitest";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { VoiceStatus } from "./audio/types";
import { initialVoiceStatus } from "./audio/types";
import { MicrophoneMuteButton } from "./react";
import { MicrophoneLevelMeter, TranscriptionProgress } from "./voiceComponents";

afterEach(cleanup);

/** Creates one voice status without repeating the complete public shape. */
function status(overrides: Partial<VoiceStatus> = {}): VoiceStatus {
  return { ...initialVoiceStatus, ...overrides };
}

describe("voice React components", () => {
  it("labels, disables, and reports the microphone mute button state", () => {
    const changeMuted = vi.fn();
    const { rerender } = render(<MicrophoneMuteButton enabled={false} onMutedChange={changeMuted} />);
    expect(screen.getByRole("button", { name: "Microphone unavailable" })).toBeDisabled();

    rerender(<MicrophoneMuteButton enabled onMutedChange={changeMuted} status={status({ connecting: true })} />);
    expect(screen.getByRole("button", { name: "Connecting" })).toBeDisabled();

    rerender(<MicrophoneMuteButton enabled onMutedChange={changeMuted} status={status({ connected: true, microphoneEnabled: true })} />);
    fireEvent.click(screen.getByRole("button", { name: "Mute mic" }));
    expect(changeMuted).toHaveBeenCalledWith(true);

    rerender(<MicrophoneMuteButton enabled onMutedChange={changeMuted} status={status({ connected: true, microphoneEnabled: false })} />);
    expect(screen.getByRole("button", { name: "Unmute mic" })).toHaveAttribute("aria-pressed", "true");

    rerender(<MicrophoneMuteButton enabled onMutedChange={changeMuted} status={status({ connected: true, microphoneEnabled: false, microphoneChanging: true })} />);
    expect(screen.getByRole("button", { name: "Unmuting…" })).toBeDisabled();
  });

  it("clamps invalid microphone meter values to a normalized visual range", () => {
    const { container, rerender } = render(<MicrophoneLevelMeter active label="Input" level={2} />);
    expect(container.querySelector(".mic-meter-track span")).toHaveStyle({ transform: "scaleX(1)" });
    rerender(<MicrophoneLevelMeter active label="Input" level={Number.NaN} />);
    expect(container.querySelector(".mic-meter-track span")).toHaveStyle({ transform: "scaleX(0)" });
    rerender(<MicrophoneLevelMeter active={false} label="Input" level={0.7} />);
    expect(container.querySelector(".mic-meter-track span")).toHaveStyle({ transform: "scaleX(0)" });
    rerender(<MicrophoneLevelMeter active label="Input" level={0.7} muted />);
    expect(container.querySelector(".mic-meter")).toHaveClass("muted");
    expect(container.querySelector(".mic-meter-track span")).toHaveStyle({ transform: "scaleX(0.7)" });
  });

  it("renders transcription headings for game, transport, and ready states", () => {
    const { rerender } = render(<TranscriptionProgress connected={false} />);
    expect(screen.getByText("Waiting for game connection")).toBeInTheDocument();
    rerender(<TranscriptionProgress connected status={status()} />);
    expect(screen.getByText("Preparing voice connection")).toBeInTheDocument();
    rerender(<TranscriptionProgress connected status={status({ connected: true, transcriptionReady: true, transcriptionMessage: "ASR ready" })} />);
    expect(screen.getByText("Transcription ready")).toBeInTheDocument();
    expect(screen.getAllByRole("listitem").every((item) => item.className === "done")).toBe(true);
  });

});
