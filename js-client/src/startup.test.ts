import { describe, expect, it } from "vitest";
import {
  isVoiceEnabled,
  participantMicrophoneLabel,
  selectableAudioInputs,
  voiceStatusUpdate
} from "./participant/microphone";
import { normalizePresence, platformLabel } from "./participant/lobby";
import type { ExperimentInfo } from "./protocol";
import { experimentAllowsIntake, requiredConsentsAccepted, transcriptionProgressForStatus } from "./helpers";

function publicConfig(overrides: Partial<ExperimentInfo> = {}): ExperimentInfo {
  return {
    gameName: "Tiny Game",
    status: "active",
    consents: [],
    voice: { enabled: false },
    ...overrides
  };
}

// Builds the browser device shape needed by startup helper tests.
function audioInput(deviceId: string, label: string): MediaDeviceInfo {
  return { deviceId, groupId: "", kind: "audioinput", label, toJSON: () => ({}) };
}

describe("Parlando startup helpers", () => {
  it("combines the platform name with an optional institution", () => {
    expect(platformLabel("Saarland University")).toBe("Parlando · Saarland University");
    expect(platformLabel("  ")).toBe("Parlando");
    expect(platformLabel()).toBe("Parlando");
  });

  it("treats disabled voice as a no-voice startup", () => {
    expect(isVoiceEnabled(publicConfig())).toBe(false);
    expect(isVoiceEnabled(publicConfig({ voice: { enabled: true } }))).toBe(true);
  });

  it("derives consent readiness from configured items alone", () => {
    expect(requiredConsentsAccepted({ consents: [] }, {})).toBe(true);
    const configured = {
      consents: [{ id: "study", title: "Study", body: "Agree?", required: true }]
    };
    expect(requiredConsentsAccepted(configured, {})).toBe(false);
    expect(requiredConsentsAccepted(configured, { study: true })).toBe(true);
  });

  it("allows participant intake for both testing and active experiments", () => {
    expect(experimentAllowsIntake("testing")).toBe(true);
    expect(experimentAllowsIntake("active")).toBe(true);
    expect(experimentAllowsIntake("inactive")).toBe(false);
    expect(experimentAllowsIntake("completed")).toBe(false);
    expect(experimentAllowsIntake("archived")).toBe(false);
  });

  it("does not present an idle transcription service as already starting", () => {
    const progress = transcriptionProgressForStatus("ASR idle", false);
    expect(progress.value).toBe(0);
    expect(progress.steps.every((step) => !step.done)).toBe(true);
  });

  it("offers only concrete microphones as post-permission alternatives", () => {
    const inputs = [
      audioInput("default", "Default — Built-in microphone"),
      audioInput("built-in", "Built-in microphone"),
      audioInput("headset", "USB headset")
    ];

    expect(selectableAudioInputs(inputs).map((device) => device.deviceId)).toEqual(["built-in", "headset"]);
  });

  it("removes browser-added USB identifiers from participant-facing microphone names", () => {
    expect(participantMicrophoneLabel("Marantz Umpire Mic (0d8c:1901)")).toBe("Marantz Umpire Mic");
    expect(participantMicrophoneLabel("Conference Mic [ABCD:1234]")).toBe("Conference Mic");
    expect(participantMicrophoneLabel("Built-in Microphone")).toBe("Built-in Microphone");
  });

  it("normalizes two-player presence snapshots", () => {
    const presence = normalizePresence({
      A: { connected: true, audioReady: true },
      B: { connected: true, audioReady: true }
    });

    expect(presence.A).toEqual({ connected: true, audioReady: true });
    expect(presence.B).toEqual({ connected: true, audioReady: true });
  });

  it("keeps missing seats visible as waiting-room gaps", () => {
    const presence = normalizePresence({
      A: { connected: true }
    });

    expect(presence.A?.connected).toBe(true);
    expect(presence.B).toBeUndefined();
  });

  it("maps Speechmatics readiness messages onto voice status updates", () => {
    expect(
      voiceStatusUpdate({
        audioReady: true,
        transcriptionReady: false,
        transcriptionStatus: "ASR worker connected"
      })
    ).toEqual({
      transcriptionMessage: "ASR worker connected",
      transcriptionReady: false
    });

    expect(voiceStatusUpdate({ transcriptionReady: true, transcriptionStatus: "ASR ready" })).toEqual({
      transcriptionMessage: "ASR ready",
      transcriptionReady: true
    });
  });
});
