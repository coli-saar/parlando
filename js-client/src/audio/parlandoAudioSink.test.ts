import { describe, expect, it } from "vitest";
import { encodeFrame } from "./parlandoAudioSink";

describe("encodeFrame", () => {
  it("encodes the versioned PCM header in network byte order", () => {
    const pcm = new Uint8Array(960).fill(7).buffer;
    const encoded = encodeFrame(0x01020304, 0x0102030405, pcm);
    const view = new DataView(encoded);

    expect(encoded.byteLength).toBe(973);
    expect(view.getUint8(0)).toBe(1);
    expect(view.getUint32(1, false)).toBe(0x01020304);
    expect(view.getBigUint64(5, false)).toBe(0x0102030405n);
    expect(new Uint8Array(encoded, 13).every((byte) => byte === 7)).toBe(true);
  });

  it("rejects incomplete PCM frames", () => {
    expect(() => encodeFrame(0, 0, new ArrayBuffer(958))).toThrow("960 bytes");
  });
});
