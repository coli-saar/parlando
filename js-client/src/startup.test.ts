import { describe, expect, it } from "vitest";
import { isVoiceEnabled, normalizePresence, resolveStartupTitle, voiceStatusUpdate } from "./startup";
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
});
