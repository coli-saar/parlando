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
    this.prepared = false;
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
  lastContext: AudioSessionContext | null = null;
  currentEnabled = true;

  async connect(input: MicrophoneInput, context: AudioSessionContext) {
    this.connectCalls += 1;
    this.lastInput = input;
    this.lastContext = context;
    context.onVoiceStatus({
      connected: true,
      connecting: false,
      microphoneEnabled: this.currentEnabled,
      microphoneChanging: false,
      message: this.currentEnabled ? "Microphone live" : "Microphone muted"
    });
  }

  async setInputEnabled(enabled: boolean) {
    this.currentEnabled = enabled;
    this.enabled.push(enabled);
  }

  async disconnect() {
    return undefined;
  }
}

function context(): AudioSessionContext {
  return {
    roomId: "room",
    role: "A",
    selectedAudioInputId: "default",
    selectedAudioInputLabel: "Default microphone",
    getAudioSession: vi.fn(async () => ({
      enabled: true,
      websocketUrl: "ws://example.test/ws/audio/room",
      token: "token",
      protocolVersion: 1,
      sampleRateHz: 24_000,
      channels: 1,
      frameDurationMs: 20,
      jitterBufferMs: 100
    })),
    logVoice: vi.fn(),
    onVoiceStatus: vi.fn()
  };
}

// Creates an externally controlled operation for deterministic controller races.
function deferred(): { promise: Promise<void>; resolve: () => void } {
  let resolve!: () => void;
  const promise = new Promise<void>((resolvePromise) => {
    resolve = resolvePromise;
  });
  return { promise, resolve };
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
    await controller.connect(context());

    expect(microphone.calls).toBe(1);
    expect(sink.connectCalls).toBe(1);
    expect(sink.lastInput?.track).toBe(microphone["inputValue"].track);
  });

  it("sets explicit mute state without reacquiring the microphone", async () => {
    const microphone = new FakeMicrophoneSource();
    const sink = new FakeSink();
    const controller = new AudioSessionController({
      microphone: microphone as any,
      sink
    });

    await controller.prepare("default", "Default microphone");
    await controller.connect(context());
    await controller.setMicrophoneMuted(true, context());
    await controller.setMicrophoneMuted(false, context());

    expect(microphone.calls).toBe(1);
    expect(sink.connectCalls).toBe(1);
    expect(sink.enabled).toEqual([true, false, true]);
  });

  it("preserves the prepared microphone when transport connection fails", async () => {
    const microphone = new FakeMicrophoneSource();
    const sink = new FakeSink();
    sink.connect = vi.fn(async () => {
      throw new Error("worklet failed");
    });
    const controller = new AudioSessionController({ microphone: microphone as any, sink });
    await controller.prepare("default", "Default microphone");

    await expect(controller.connect(context())).rejects.toThrow("worklet failed");

    expect(microphone.calls).toBe(1);
    expect(microphone.prepared).toBe(true);
    expect(microphone.stops).toBe(0);
    expect(controller.snapshot().voicePreflight.ready).toBe(true);
  });

  it("coalesces simultaneous connection requests into one transport operation", async () => {
    const microphone = new FakeMicrophoneSource();
    const sink = new FakeSink();
    const gate = deferred();
    sink.connect = vi.fn(async (input, sessionContext) => {
      sink.connectCalls += 1;
      sink.lastInput = input;
      sink.lastContext = sessionContext;
      await gate.promise;
      sessionContext.onVoiceStatus({ connected: true, connecting: false, microphoneEnabled: true });
    });
    const controller = new AudioSessionController({ microphone: microphone as any, sink });
    await controller.prepare("default");

    const first = controller.connect(context());
    const second = controller.connect(context());
    expect(first).toBe(second);
    await vi.waitFor(() => expect(sink.connectCalls).toBe(1));
    gate.resolve();
    await Promise.all([first, second]);

    expect(controller.snapshot().voiceStatus).toMatchObject({ connected: true, connecting: false });
  });

  it("ignores status callbacks owned by a disconnected transport generation", async () => {
    const microphone = new FakeMicrophoneSource();
    const sink = new FakeSink();
    const controller = new AudioSessionController({ microphone: microphone as any, sink });
    await controller.prepare("default");
    await controller.connect(context());
    const staleStatus = sink.lastContext!.onVoiceStatus;

    await controller.disconnect();
    staleStatus({ connected: true, microphoneEnabled: true, message: "stale resurrection" });

    expect(controller.snapshot().voiceStatus).not.toMatchObject({ connected: true });
    expect(controller.snapshot().voiceStatus.message).not.toBe("stale resurrection");
  });

  it("leaves mute state unchanged and exposes an error when the sink rejects muting", async () => {
    const microphone = new FakeMicrophoneSource();
    const sink = new FakeSink();
    const controller = new AudioSessionController({ microphone: microphone as any, sink });
    await controller.prepare("default");
    await controller.connect(context());
    sink.setInputEnabled = vi.fn(async () => { throw new Error("transport gone"); });

    await expect(controller.setMicrophoneMuted(true, context())).rejects.toThrow("transport gone");
    expect(controller.snapshot().voiceStatus.microphoneEnabled).toBe(true);
    expect(controller.snapshot().voiceStatus).toMatchObject({
      microphoneChanging: false,
      error: "Could not mute the microphone."
    });
  });

  it("preserves mute preference across an automatic transport reconnect", async () => {
    const microphone = new FakeMicrophoneSource();
    const sink = new FakeSink();
    const controller = new AudioSessionController({ microphone: microphone as any, sink });
    await controller.prepare("default");
    await controller.connect(context());
    await controller.setMicrophoneMuted(true, context());

    sink.lastContext!.onVoiceStatus({ connected: false, microphoneEnabled: false, message: "Voice disconnected" });
    await controller.connect(context());

    expect(sink.connectCalls).toBe(2);
    expect(sink.enabled).toEqual([true, false, false]);
    expect(controller.snapshot().voiceStatus).toMatchObject({ connected: true, microphoneEnabled: false });
  });

  it("resets a muted preference for the next deliberately started session", async () => {
    const microphone = new FakeMicrophoneSource();
    const sink = new FakeSink();
    const controller = new AudioSessionController({ microphone: microphone as any, sink });
    await controller.prepare("default");
    await controller.connect(context());
    await controller.setMicrophoneMuted(true, context());

    await controller.disconnect(true);
    await controller.prepare("default");
    await controller.connect(context());

    expect(sink.enabled).toEqual([true, false, true, true]);
    expect(controller.snapshot().voiceStatus).toMatchObject({ connected: true, microphoneEnabled: true });
  });

  it("reconciles a preference changed while muting is in flight", async () => {
    const microphone = new FakeMicrophoneSource();
    const sink = new FakeSink();
    const gate = deferred();
    const originalSetter = sink.setInputEnabled.bind(sink);
    sink.setInputEnabled = vi.fn(async (enabled: boolean) => {
      if (!enabled) await gate.promise;
      await originalSetter(enabled);
    });
    const controller = new AudioSessionController({ microphone: microphone as any, sink });
    await controller.prepare("default");
    await controller.connect(context());

    const muting = controller.setMicrophoneMuted(true, context());
    const unmuting = controller.setMicrophoneMuted(false, context());
    gate.resolve();
    await Promise.all([muting, unmuting]);

    expect(controller.snapshot().voiceStatus).toMatchObject({ microphoneEnabled: true, microphoneChanging: false });
    expect(sink.enabled).toEqual([true, false, true]);
  });
});
