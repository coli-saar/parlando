import { describe, expect, it, vi } from "vitest";
import { AudioSessionController } from "./audioSessionController";
import type { AudioSessionContext, LocalAudioSink, MicrophoneInput } from "./types";

class FakeMicrophoneSource {
  calls = 0;
  prepared = false;
  private listener: ((preflight: any) => void) | null = null;
  private inputValue: MicrophoneInput;

  constructor() {
    const track = {
      clone: vi.fn(() => ({ clone: vi.fn(), stop: vi.fn() }) as unknown as MediaStreamTrack),
      stop: vi.fn()
    } as unknown as MediaStreamTrack;
    this.inputValue = {
      deviceId: "default",
      deviceLabel: "Default microphone",
      stream: { getTracks: () => [track] } as unknown as MediaStream,
      track,
      createTrackClone: () => track.clone(),
      createMediaStream: () => ({ getTracks: () => [track.clone()] }) as unknown as MediaStream
    };
  }

  subscribe(listener: (preflight: any) => void) {
    this.listener = listener;
    listener({
      requested: false,
      ready: false,
      preparing: false,
      message: "Voice not prepared",
      micLevel: 0,
      micProbeActive: false,
      deviceLabel: "Default microphone"
    });
    return () => undefined;
  }

  async prepare() {
    if (!this.prepared) {
      this.calls += 1;
      this.prepared = true;
      this.listener?.({
        requested: true,
        ready: true,
        preparing: false,
        message: "Microphone ready: Default microphone",
        micLevel: 0,
        micProbeActive: false,
        deviceLabel: "Default microphone"
      });
    }
    return this.inputValue;
  }

  stop() {
    return undefined;
  }

  reset() {
    return undefined;
  }
}

class FakeSink implements LocalAudioSink {
  id = "fake";
  provider = "fake";
  purposes = ["partner-audio", "transcription"] as const;
  connectCalls = 0;
  enabled: boolean[] = [];
  lastInput: MicrophoneInput | null = null;

  async connect(input: MicrophoneInput, context: AudioSessionContext) {
    this.connectCalls += 1;
    this.lastInput = input;
    context.onVoiceStatus({
      connected: true,
      connecting: false,
      microphoneEnabled: true,
      message: "Microphone live"
    });
  }

  async setInputEnabled(enabled: boolean) {
    this.enabled.push(enabled);
  }

  async disconnect() {
    return undefined;
  }
}

function context(): AudioSessionContext {
  return {
    roomId: "room",
    participantSessionId: "participant",
    role: "A",
    selectedAudioInputId: "default",
    selectedAudioInputLabel: "Default microphone",
    getLiveKitToken: vi.fn(),
    logVoice: vi.fn(),
    onVoiceStatus: vi.fn()
  };
}

describe("AudioSessionController", () => {
  it("reuses one prepared microphone input for connect", async () => {
    const microphone = new FakeMicrophoneSource();
    const sink = new FakeSink();
    const controller = new AudioSessionController({
      microphone: microphone as any,
      sink
    });

    await controller.prepare("default", "Default microphone");
    await controller.toggle(context(), "default", "Default microphone");

    expect(microphone.calls).toBe(1);
    expect(sink.connectCalls).toBe(1);
    expect(sink.lastInput?.track).toBe(microphone["inputValue"].track);
  });

  it("connects multiple sinks from one prepared microphone input", async () => {
    const microphone = new FakeMicrophoneSource();
    const firstSink = new FakeSink();
    const secondSink = new FakeSink();
    const controller = new AudioSessionController({
      microphone: microphone as any,
      sinks: [firstSink, secondSink]
    });

    await controller.toggle(context(), "default", "Default microphone");
    await controller.toggle(context(), "default", "Default microphone");
    await controller.toggle(context(), "default", "Default microphone");

    expect(microphone.calls).toBe(1);
    expect(firstSink.connectCalls).toBe(1);
    expect(secondSink.connectCalls).toBe(1);
    expect(firstSink.lastInput?.track).toBe(secondSink.lastInput?.track);
    expect(firstSink.enabled).toEqual([false, true]);
    expect(secondSink.enabled).toEqual([false, true]);
  });

  it("fetches one audio session plan for multiple sinks", async () => {
    const microphone = new FakeMicrophoneSource();
    const firstSink = new FakeSink();
    const secondSink = new FakeSink();
    const audioContext = context();
    audioContext.getAudioSession = vi.fn(async () => ({
      enabled: true,
      capture: { audio: true },
      sinks: []
    }));
    const controller = new AudioSessionController({
      microphone: microphone as any,
      sinks: [firstSink, secondSink]
    });

    await controller.toggle(audioContext, "default", "Default microphone");

    expect(audioContext.getAudioSession).toHaveBeenCalledTimes(1);
  });
});
