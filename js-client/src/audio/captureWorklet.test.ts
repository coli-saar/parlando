import { describe, expect, it } from "vitest";
import { PcmCaptureResampler } from "./captureWorklet";

/** Appends a signal in browser-sized quanta and returns all emitted PCM samples. */
function resample(signal: Float32Array, rate: number, quantum = 128): Int16Array {
  const resampler = new PcmCaptureResampler();
  const samples: number[] = [];
  for (let offset = 0; offset < signal.length; offset += quantum) {
    for (const frame of resampler.append(signal.slice(offset, offset + quantum), rate)) {
      samples.push(...new Int16Array(frame));
    }
  }
  return Int16Array.from(samples);
}

describe("PcmCaptureResampler", () => {
  it("emits exact 480-sample frames at the canonical rate", () => {
    const signal = new Float32Array(960).fill(0.5);
    const samples = resample(signal, 24_000);
    expect(samples.length).toBe(960);
    expect(Array.from(samples).every((sample) => sample === 16_384)).toBe(true);
  });

  it("maintains duration across common browser sample rates", () => {
    for (const rate of [44_100, 48_000, 96_000]) {
      const signal = new Float32Array(rate).fill(0.25);
      const samples = resample(signal, rate);
      expect(samples.length).toBe(24_000);
      expect(samples[0]).toBe(8_192);
      expect(samples.at(-1)).toBe(8_192);
    }
  });

  it("preserves interpolation continuity across render-quantum boundaries", () => {
    const contiguous = new PcmCaptureResampler();
    const split = new PcmCaptureResampler();
    const signal = Float32Array.from({ length: 960 }, (_, index) => Math.sin(index / 17));
    const contiguousFrames = contiguous.append(signal, 48_000);
    const splitFrames = [
      ...split.append(signal.slice(0, 127), 48_000),
      ...split.append(signal.slice(127), 48_000)
    ];
    expect(splitFrames.map((frame) => Array.from(new Int16Array(frame)))).toEqual(
      contiguousFrames.map((frame) => Array.from(new Int16Array(frame)))
    );
  });

  it("clamps out-of-range input and uses asymmetric PCM16 endpoints", () => {
    const resampler = new PcmCaptureResampler();
    const signal = new Float32Array(480);
    signal.fill(2, 0, 240);
    signal.fill(-2, 240);
    const [frame] = resampler.append(signal, 24_000);
    const pcm = new Int16Array(frame);
    expect(pcm[0]).toBe(32_767);
    expect(pcm[239]).toBe(32_767);
    expect(pcm[240]).toBe(-32_768);
    expect(pcm[479]).toBe(-32_768);
  });

  it("accepts empty quanta and rejects invalid device sample rates", () => {
    const resampler = new PcmCaptureResampler();
    expect(resampler.append(new Float32Array(), 48_000)).toEqual([]);
    for (const rate of [0, -1, Number.NaN, Number.POSITIVE_INFINITY]) {
      expect(() => resampler.append(new Float32Array([0]), rate)).toThrow("finite and positive");
    }
  });
});
