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
});
