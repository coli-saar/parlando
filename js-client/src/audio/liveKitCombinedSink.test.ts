import { beforeEach, describe, expect, it, vi } from "vitest";
import { LiveKitCombinedSink } from "./liveKitCombinedSink";
import type { AudioSessionContext, MicrophoneInput } from "./types";

const livekit = vi.hoisted(() => {
  const instances: any[] = [];
  class MockRoom {
    state = "connected";
    localParticipant = {
      identity: "participant",
      publishTrack: vi.fn(async () => ({
        trackSid: "track-1",
        source: "microphone",
        isMuted: false,
        mute: vi.fn(),
        unmute: vi.fn()
      })),
      unpublishTrack: vi.fn()
    };
    connect = vi.fn(async () => undefined);
    startAudio = vi.fn(async () => undefined);
    disconnect = vi.fn();
    on = vi.fn();

    constructor() {
      instances.push(this);
    }
  }
  return {
    instances,
    MockRoom
  };
});

vi.mock("livekit-client", () => ({
  Room: livekit.MockRoom,
  RoomEvent: {
    Connected: "Connected",
    ConnectionStateChanged: "ConnectionStateChanged",
    Reconnecting: "Reconnecting",
    Reconnected: "Reconnected",
    ParticipantConnected: "ParticipantConnected",
    ParticipantDisconnected: "ParticipantDisconnected",
    TrackPublished: "TrackPublished",
    TrackSubscribed: "TrackSubscribed",
    TrackSubscriptionFailed: "TrackSubscriptionFailed",
    TrackUnsubscribed: "TrackUnsubscribed",
    Disconnected: "Disconnected",
    AudioPlaybackStatusChanged: "AudioPlaybackStatusChanged",
    LocalTrackPublished: "LocalTrackPublished",
    LocalTrackSubscribed: "LocalTrackSubscribed",
    LocalAudioSilenceDetected: "LocalAudioSilenceDetected"
  },
  Track: {
    Kind: { Audio: "audio" },
    Source: { Microphone: "microphone" }
  }
}));

function microphoneInput(): MicrophoneInput {
  const track = { stop: vi.fn() } as unknown as MediaStreamTrack;
  return {
    deviceId: "default",
    deviceLabel: "Default microphone",
    stream: { getTracks: () => [track] } as unknown as MediaStream,
    track,
    createTrackClone: () => track,
    createMediaStream: () => ({ getTracks: () => [track] }) as unknown as MediaStream
  };
}

function context(overrides: Partial<AudioSessionContext> = {}): AudioSessionContext {
  return {
    roomId: "room",
    participantSessionId: "participant",
    role: "A",
    selectedAudioInputId: "default",
    selectedAudioInputLabel: "Default microphone",
    getLiveKitToken: vi.fn(async () => ({
      enabled: true,
      url: "wss://fallback.example",
      token: "fallback-token",
      identity: "fallback-identity"
    })),
    logVoice: vi.fn(),
    onVoiceStatus: vi.fn(),
    ...overrides
  };
}

describe("LiveKitCombinedSink", () => {
  beforeEach(() => {
    livekit.instances.length = 0;
    vi.clearAllMocks();
  });

  it("uses the audio session plan credentials when a livekit sink is available", async () => {
    const sink = new LiveKitCombinedSink();
    const audioContext = context({
      getAudioSession: vi.fn(async () => ({
        enabled: true,
        capture: { audio: true },
        sinks: [
          {
            id: "livekit-combined",
            provider: "livekit",
            purposes: ["partner-audio", "transcription"],
            transport: "webrtc-room",
            credentials: {
              enabled: true,
              url: "wss://planned.example",
              token: "planned-token",
              identity: "planned-identity"
            }
          }
        ]
      }))
    });

    await sink.connect(microphoneInput(), audioContext);

    expect(audioContext.getAudioSession).toHaveBeenCalledTimes(1);
    expect(audioContext.getLiveKitToken).not.toHaveBeenCalled();
    expect(livekit.instances[0].connect).toHaveBeenCalledWith("wss://planned.example", "planned-token");
  });

  it("disables voice when the audio session plan has no livekit sink", async () => {
    const sink = new LiveKitCombinedSink();
    const audioContext = context({
      getAudioSession: vi.fn(async () => ({
        enabled: true,
        capture: { audio: true },
        sinks: []
      }))
    });

    await sink.connect(microphoneInput(), audioContext);

    expect(audioContext.getLiveKitToken).not.toHaveBeenCalled();
    expect(livekit.instances).toHaveLength(0);
    expect(audioContext.onVoiceStatus).toHaveBeenCalledWith(expect.objectContaining({ connected: false }));
  });

  it("falls back to the legacy livekit token endpoint if the audio session plan cannot be fetched", async () => {
    const sink = new LiveKitCombinedSink();
    const audioContext = context({
      getAudioSession: vi.fn(async () => {
        throw new Error("404");
      })
    });

    await sink.connect(microphoneInput(), audioContext);

    expect(audioContext.getAudioSession).toHaveBeenCalledTimes(1);
    expect(audioContext.getLiveKitToken).toHaveBeenCalledTimes(1);
    expect(livekit.instances[0].connect).toHaveBeenCalledWith("wss://fallback.example", "fallback-token");
  });
});
