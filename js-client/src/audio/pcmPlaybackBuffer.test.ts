import { describe, expect, it } from "vitest";
import { PcmPlaybackBuffer } from "./playbackWorklet";

describe("PcmPlaybackBuffer", () => {
  it("linearly interpolates 24 kHz samples for a 48 kHz output", () => {
    const buffer = new PcmPlaybackBuffer();
    buffer.append(new Int16Array([0, 16_384, 32_767]));
    const output = new Float32Array(5);

    const result = buffer.render(output, 0.5);

    expect(result).toEqual({ underrun: false, renderedSamples: 5 });
    expect(Array.from(output)).toEqual([0, 0.25, 0.5, 0.7499847412109375, 0.999969482421875]);
  });

  it("reports an underrun and fills the unavailable tail with silence", () => {
    const buffer = new PcmPlaybackBuffer();
    buffer.append(new Int16Array([16_384]));
    const output = new Float32Array(4);

    const result = buffer.render(output, 1);

    expect(result).toEqual({ underrun: true, renderedSamples: 1 });
    expect(Array.from(output)).toEqual([0.5, 0, 0, 0]);
  });

  it("trims stale audio while retaining the newest requested samples", () => {
    const buffer = new PcmPlaybackBuffer();
    buffer.append(new Int16Array([1, 2, 3, 4, 5]));

    buffer.trimTo(2);
    const output = new Float32Array(2);
    buffer.render(output, 1);

    expect(Array.from(output)).toEqual([4 / 32768, 5 / 32768]);
  });

  it("normalizes both signed PCM16 extrema asymmetrically", () => {
    const buffer = new PcmPlaybackBuffer();
    buffer.append(new Int16Array([-32_768, 32_767]));
    const output = new Float32Array(2);
    buffer.render(output, 1);
    expect(Array.from(output)).toEqual([-1, 32_767 / 32_768]);
  });

  it("handles empty input and output without inventing samples", () => {
    const buffer = new PcmPlaybackBuffer();
    expect(buffer.render(new Float32Array(0), 1)).toEqual({ underrun: false, renderedSamples: 0 });
    expect(buffer.render(new Float32Array(2), 1)).toEqual({ underrun: true, renderedSamples: 0 });
  });

  it("rejects non-progressing and non-finite playback ratios", () => {
    const buffer = new PcmPlaybackBuffer();
    for (const ratio of [0, -1, Number.NaN, Number.POSITIVE_INFINITY]) {
      expect(() => buffer.render(new Float32Array(1), ratio)).toThrow("finite and positive");
    }
  });

  it("rounds fractional trim limits down and clamps negative limits to zero", () => {
    const fractional = new PcmPlaybackBuffer();
    fractional.append(new Int16Array([1, 2, 3, 4]));
    fractional.trimTo(2.9);
    expect(fractional.availableSamples()).toBe(2);

    fractional.trimTo(-1);
    expect(fractional.availableSamples()).toBe(0);
  });

  it("preserves a fractional cursor across successive output quanta", () => {
    const buffer = new PcmPlaybackBuffer();
    buffer.append(new Int16Array([0, 8_192, 16_384, 24_576, 32_767]));
    const first = new Float32Array(2);
    const second = new Float32Array(2);
    buffer.render(first, 0.75);
    buffer.render(second, 0.75);
    expect(Array.from([...first, ...second])).toEqual([
      0,
      0.1875,
      0.375,
      0.5625
    ]);
  });
});
