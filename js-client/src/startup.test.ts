import { describe, expect, it, vi } from "vitest";
import {
  CLIENT_HEARTBEAT_INTERVAL_MS,
  canSendGameMessage,
  completedSessionPatch,
  isVoiceEnabled,
  normalizePresence,
  participantMicrophoneLabel,
  platformLabel,
  selectableAudioInputs,
  sendActionIfGameActive,
  sendMessageIfGameActive,
  voiceStatusUpdate
} from "./startup";
import type { ExperimentInfo } from "./protocol";
import { experimentAllowsIntake, requiredConsentsAccepted, transcriptionProgressForStatus } from "./helpers";

function publicConfig(overrides: Partial<ExperimentInfo> = {}): ExperimentInfo {
  return {
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
  it("uses the reviewed one-second transport heartbeat", () => {
    expect(CLIENT_HEARTBEAT_INTERVAL_MS).toBe(1_000);
  });

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

  it("stores game-specific completion payloads from completed server messages", () => {
    const patch = completedSessionPatch({
      outcome: "success",
      dyadScore: 12,
      playerScores: { A: 7, B: 5 }
    });

    expect(patch).toEqual({
      completed: true,
      completion: {
        outcome: "success",
        dyadScore: 12,
        playerScores: { A: 7, B: 5 }
      }
    });
  });

  it("guards participant game messages after completion", () => {
    expect(canSendGameMessage({ completed: false })).toBe(true);
    expect(canSendGameMessage({ completed: true })).toBe(false);
    expect(canSendGameMessage(null)).toBe(false);
  });

  it("does not send actions or chat after completion", () => {
    const socket = {} as WebSocket;
    const apiClient = {
      sendAction: vi.fn(),
      sendMessage: vi.fn()
    };

    sendActionIfGameActive(apiClient, { socket, completed: true }, { type: "finish" });
    sendMessageIfGameActive(apiClient, { socket, completed: true }, "late hello");
    expect(apiClient.sendAction).not.toHaveBeenCalled();
    expect(apiClient.sendMessage).not.toHaveBeenCalled();

    sendActionIfGameActive(apiClient, { socket, completed: false }, { type: "finish" });
    sendMessageIfGameActive(apiClient, { socket, completed: false }, "hello");
    expect(apiClient.sendAction).toHaveBeenCalledWith(socket, { type: "finish" });
    expect(apiClient.sendMessage).toHaveBeenCalledWith(socket, "hello");
  });
});
