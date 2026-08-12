import { describe, expect, it, vi } from "vitest";
import {
  canSendGameMessage,
  completedSessionPatch,
  isVoiceEnabled,
  normalizePresence,
  resolveStartupTitle,
  sendActionIfGameActive,
  sendChatMessageIfGameActive,
  voiceStatusUpdate
} from "./startup";
import type { PublicConfigResponse } from "./protocol";

function publicConfig(overrides: Partial<PublicConfigResponse> = {}): PublicConfigResponse {
  return {
    study_name: "Startup Test",
    require_consent: false,
    consents: [],
    livekit: { enabled: false, url: null },
    transcription: { enabled: false, provider: "livekit" },
    conversation: { enabled: true },
    agents: { mode: "human_vs_human", human_vs_agent: false },
    ...overrides
  };
}

describe("Parlando startup helpers", () => {
  it("resolves startup titles from app labels, then public study config", () => {
    expect(resolveStartupTitle({ title: "Custom Title" }, publicConfig({ study_name: "Configured Study" }))).toBe("Custom Title");
    expect(resolveStartupTitle({}, publicConfig({ study_name: "Configured Study" }))).toBe("Configured Study");
    expect(resolveStartupTitle({}, null)).toBe("Parlando Experiment");
  });

  it("treats disabled LiveKit as a no-voice startup", () => {
    expect(isVoiceEnabled(publicConfig())).toBe(false);
    expect(isVoiceEnabled(publicConfig({ livekit: { enabled: true, url: "wss://livekit.test" } }))).toBe(true);
  });

  it("normalizes human-human and human-agent presence snapshots", () => {
    const presence = normalizePresence({
      A: { participantSessionId: "human-a", connected: true, audioReady: true },
      B: { participantSessionId: "agent-b", connected: true, audioReady: true }
    });

    expect(presence.A).toEqual({ participantSessionId: "human-a", connected: true, audioReady: true });
    expect(presence.B).toEqual({ participantSessionId: "agent-b", connected: true, audioReady: true });
  });

  it("keeps missing seats visible as waiting-room gaps", () => {
    const presence = normalizePresence({
      A: { participantSessionId: "human-a", connected: true }
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

  it("stores completion summaries from completed server messages", () => {
    const patch = completedSessionPatch({
      outcome: "success",
      dyadScore: 12,
      playerScores: { A: 7, B: 5 }
    });

    expect(patch).toEqual({
      completed: true,
      completionSummary: {
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
      sendChatMessage: vi.fn()
    };

    sendActionIfGameActive(apiClient, { socket, completed: true }, { type: "finish" });
    sendChatMessageIfGameActive(apiClient, { socket, completed: true }, "late hello");
    expect(apiClient.sendAction).not.toHaveBeenCalled();
    expect(apiClient.sendChatMessage).not.toHaveBeenCalled();

    sendActionIfGameActive(apiClient, { socket, completed: false }, { type: "finish" });
    sendChatMessageIfGameActive(apiClient, { socket, completed: false }, "hello");
    expect(apiClient.sendAction).toHaveBeenCalledWith(socket, { type: "finish" });
    expect(apiClient.sendChatMessage).toHaveBeenCalledWith(socket, "hello");
  });
});
