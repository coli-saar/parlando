import { describe, expect, it, vi } from "vitest";
import { AudioSessionController } from "./audioSessionController";
import type { AudioSessionContext, LocalAudioSink, MicrophoneInput } from "./types";

class FakeMicrophoneSource {
  calls = 0;
  prepared = false;
  stops = 0;
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

  input() {
    if (!this.prepared) throw new Error("Microphone has not been prepared.");
    return this.inputValue;
  }

  stop() {
    this.stops += 1;
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
    getAudioSession: vi.fn(async () => ({
      enabled: true,
      websocket_url: "ws://example.test/ws/audio/room",
      token: "token",
      protocol_version: 1,
      sample_rate_hz: 24_000,
      channels: 1,
      frame_duration_ms: 20,
      jitter_buffer_ms: 100
    })),
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
    await controller.toggle(context());

    expect(microphone.calls).toBe(1);
    expect(sink.connectCalls).toBe(1);
    expect(sink.lastInput?.track).toBe(microphone["inputValue"].track);
  });

  it("toggles one transport without reacquiring the microphone", async () => {
    const microphone = new FakeMicrophoneSource();
    const sink = new FakeSink();
    const controller = new AudioSessionController({
      microphone: microphone as any,
      sink
    });

    await controller.prepare("default", "Default microphone");
    await controller.toggle(context());
    await controller.toggle(context());
    await controller.toggle(context());

    expect(microphone.calls).toBe(1);
    expect(sink.connectCalls).toBe(1);
    expect(sink.enabled).toEqual([false, true]);
  });

  it("preserves the prepared microphone when transport connection fails", async () => {
    const microphone = new FakeMicrophoneSource();
    const sink = new FakeSink();
    sink.connect = vi.fn(async () => {
      throw new Error("worklet failed");
    });
    const controller = new AudioSessionController({ microphone: microphone as any, sink });
    await controller.prepare("default", "Default microphone");

    await expect(controller.toggle(context())).rejects.toThrow("worklet failed");

    expect(microphone.calls).toBe(1);
    expect(microphone.prepared).toBe(true);
    expect(microphone.stops).toBe(0);
    expect(controller.snapshot().voicePreflight.ready).toBe(true);
  });
});
