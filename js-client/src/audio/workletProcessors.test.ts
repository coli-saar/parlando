import { afterEach, describe, expect, it, vi } from "vitest";

class FakeProcessorBase {
  port = {
    onmessage: null as ((event: MessageEvent<any>) => void) | null,
    postMessage: vi.fn()
  };
}

/** Installs a fake AudioWorklet global and returns registered processor constructors. */
function installWorkletGlobals(rate: number): Map<string, new () => FakeProcessorBase & { process(...arguments_: any[]): boolean }> {
  const processors = new Map<string, new () => FakeProcessorBase & { process(...arguments_: any[]): boolean }>();
  vi.stubGlobal("AudioWorkletProcessor", FakeProcessorBase);
  vi.stubGlobal("sampleRate", rate);
  vi.stubGlobal("registerProcessor", (name: string, processor: new () => FakeProcessorBase & { process(...arguments_: any[]): boolean }) => {
    processors.set(name, processor);
  });
  return processors;
}

afterEach(() => {
  vi.unstubAllGlobals();
  vi.resetModules();
});

describe("audio worklet processors", () => {
  it("registers capture and emits transferable 480-sample frames", async () => {
    const processors = installWorkletGlobals(24_000);
    await import("./captureWorklet");
    const Capture = processors.get("parlando-capture")!;
    const processor = new Capture();
    expect(processor.process([])).toBe(true);
    expect(processor.process([[new Float32Array(960).fill(0.5)]])).toBe(true);
    expect(processor.port.postMessage).toHaveBeenCalledTimes(2);
    for (const [frame, transfers] of processor.port.postMessage.mock.calls) {
      expect((frame as ArrayBuffer).byteLength).toBe(960);
      expect(transfers).toEqual([frame]);
    }
  });

  it("buffers playback to its threshold and reports underrun before resuming", async () => {
    const processors = installWorkletGlobals(48_000);
    await import("./playbackWorklet");
    const Playback = processors.get("parlando-playback")!;
    const processor = new Playback();
    processor.port.onmessage!(new MessageEvent("message", { data: { jitterSamples: 1 } }));
    processor.port.onmessage!(new MessageEvent("message", { data: new Int16Array(480).fill(16_384).buffer }));
    const output = new Float32Array(128);
    expect(processor.process([], [[output]])).toBe(true);
    expect(output[0]).toBe(0.5);
    for (let index = 0; index < 8; index += 1) processor.process([], [[new Float32Array(128)]]);
    expect(processor.port.postMessage).toHaveBeenCalledWith(expect.objectContaining({
      type: "playbackUnderrun",
      count: 1
    }));
    const silence = new Float32Array(128).fill(1);
    processor.process([], [[silence]]);
    expect(Array.from(silence).every((sample) => sample === 0)).toBe(true);
    expect(processor.process([], [])).toBe(true);
  });
});
