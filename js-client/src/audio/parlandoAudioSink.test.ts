import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import fixtures from "../../../proto/pcm_frame_v1.fixtures.json";
import { encodeFrame, parseTranscriptionStatus, waitForSocketOpen } from "./parlandoAudioSink";

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("encodeFrame", () => {
  it("matches the shared Rust/browser PCM wire fixtures", () => {
    for (const fixture of fixtures.cases) {
      const encoded = encodeFrame(fixture.sequence, fixture.timestamp_ms, new ArrayBuffer(960));
      expect(Array.from(new Uint8Array(encoded, 0, 13)), fixture.name).toEqual(fixture.header_bytes);
    }
  });

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

  it("validates numeric header fields instead of silently wrapping them", () => {
    const pcm = new ArrayBuffer(960);
    for (const sequence of [-1, 0.5, Number.NaN, 0x1_0000_0000]) {
      expect(() => encodeFrame(sequence, 0, pcm)).toThrow("unsigned 32-bit");
    }
    for (const timestamp of [-1, 0.5, Number.NaN, Number.POSITIVE_INFINITY, Number.MAX_SAFE_INTEGER + 1]) {
      expect(() => encodeFrame(0, timestamp, pcm)).toThrow("non-negative safe integer");
    }
  });

  it("copies PCM bytes without mutating or aliasing the caller's buffer", () => {
    const pcm = new Uint8Array(960).fill(3);
    const encoded = encodeFrame(1, 2, pcm.buffer);
    pcm.fill(9);
    expect(new Uint8Array(encoded, 13).every((byte) => byte === 3)).toBe(true);
  });
});

describe("waitForSocketOpen", () => {
  beforeEach(() => {
    vi.stubGlobal("WebSocket", { CONNECTING: 0, OPEN: 1, CLOSING: 2, CLOSED: 3 });
  });

  it("resolves when the socket opens", async () => {
    const socket = Object.assign(new EventTarget(), { readyState: 0 }) as WebSocket;
    const connected = waitForSocketOpen(socket);
    socket.dispatchEvent(new Event("open"));
    await expect(connected).resolves.toBeUndefined();
  });

  it("rejects instead of hanging when the socket closes before opening", async () => {
    const socket = Object.assign(new EventTarget(), { readyState: 0 }) as WebSocket;
    const connected = waitForSocketOpen(socket);
    socket.dispatchEvent(new Event("close"));
    await expect(connected).rejects.toThrow("closed before connecting");
  });

  it("settles immediately for sockets already open or closed", async () => {
    const open = { readyState: 1 } as WebSocket;
    const closed = { readyState: 3 } as WebSocket;
    await expect(waitForSocketOpen(open)).resolves.toBeUndefined();
    await expect(waitForSocketOpen(closed)).rejects.toThrow("already closed");
  });

  it("rejects and cleans listeners when the connection errors", async () => {
    const socket = Object.assign(new EventTarget(), { readyState: 0 }) as WebSocket;
    const remove = vi.spyOn(socket, "removeEventListener");
    const connected = waitForSocketOpen(socket);
    socket.dispatchEvent(new Event("error"));
    await expect(connected).rejects.toThrow("failed to connect");
    expect(remove).toHaveBeenCalledTimes(3);
  });
});

describe("parseTranscriptionStatus", () => {
  it("maps valid provider status messages", () => {
    expect(parseTranscriptionStatus(JSON.stringify({
      type: "transcriptionStatus",
      ready: true,
      message: "ASR ready"
    }))).toEqual({ transcriptionReady: true, transcriptionMessage: "ASR ready" });
  });

  it("ignores malformed, unrelated, and non-object control messages", () => {
    for (const data of ["{", "null", "[]", "42", JSON.stringify({ type: "other" })]) {
      expect(parseTranscriptionStatus(data)).toBeNull();
    }
  });

  it("uses a stable fallback for absent or non-string provider messages", () => {
    expect(parseTranscriptionStatus(JSON.stringify({ type: "transcriptionStatus", ready: 0 }))).toEqual({
      transcriptionReady: false,
      transcriptionMessage: "ASR idle"
    });
  });
});
