// @vitest-environment happy-dom

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ParlandoAudioSink } from "./parlandoAudioSink";
import type { AudioSessionContext, MicrophoneInput } from "./types";

class FakePort {
  onmessage: ((event: MessageEvent<any>) => void) | null = null;
  postMessage = vi.fn();
}

class FakeNode {
  static instances: FakeNode[] = [];
  port = new FakePort();
  connect = vi.fn();
  disconnect = vi.fn();

  /** Records the capture and playback worklet nodes in construction order. */
  constructor(..._arguments: unknown[]) {
    FakeNode.instances.push(this);
  }
}

class FakeSource {
  connect = vi.fn();
  disconnect = vi.fn();
}

class FakeAudioContext {
  static instances: FakeAudioContext[] = [];
  static addModule = vi.fn(async () => undefined);
  static resumeFailure: Error | null = null;
  destination = {} as AudioDestinationNode;
  source = new FakeSource();
  audioWorklet = { addModule: FakeAudioContext.addModule };
  resume = vi.fn(async () => {
    if (FakeAudioContext.resumeFailure) throw FakeAudioContext.resumeFailure;
  });
  close = vi.fn(async () => undefined);
  createMediaStreamSource = vi.fn(() => this.source as unknown as MediaStreamAudioSourceNode);

  /** Records every context so partial-failure cleanup remains observable. */
  constructor() {
    FakeAudioContext.instances.push(this);
  }
}

class FakeSocket extends EventTarget {
  static CONNECTING = 0;
  static OPEN = 1;
  static CLOSING = 2;
  static CLOSED = 3;
  static instances: FakeSocket[] = [];
  readyState = FakeSocket.CONNECTING;
  binaryType = "blob";
  send = vi.fn();
  close = vi.fn(() => {
    this.readyState = FakeSocket.CLOSED;
  });

  /** Records the authenticated URL selected by the audio sink. */
  constructor(readonly url: URL) {
    super();
    FakeSocket.instances.push(this);
  }

  /** Opens the pending transport. */
  open(): void {
    this.readyState = FakeSocket.OPEN;
    this.dispatchEvent(new Event("open"));
  }
}

/** Creates one cloned microphone stream owned by the sink. */
function input() {
  const track = { enabled: true, stop: vi.fn() } as unknown as MediaStreamTrack;
  const mediaStream = { getAudioTracks: () => [track], getTracks: () => [track] } as unknown as MediaStream;
  return {
    value: {
      deviceId: "mic",
      deviceLabel: "Mic",
      stream: mediaStream,
      track,
      createTrackClone: vi.fn(),
      createMediaStream: vi.fn(() => mediaStream)
    } as MicrophoneInput,
    track
  };
}

/** Creates one enabled audio-session context and its observable callbacks. */
function context(overrides: Record<string, unknown> = {}) {
  const value: AudioSessionContext = {
    sessionId: "room",
    role: "A",
    selectedAudioInputId: "mic",
    selectedAudioInputLabel: "Mic",
    getAudioSession: vi.fn(async () => ({
      enabled: true,
      websocketUrl: "/ws/audio/room",
      token: "secret ticket",
      protocolVersion: 1,
      sampleRateHz: 24_000,
      channels: 1,
      frameDurationMs: 20,
      jitterBufferMs: 100,
      ...overrides
    })),
    logVoice: vi.fn(),
    onVoiceStatus: vi.fn()
  };
  return value;
}

beforeEach(() => {
  FakeNode.instances = [];
  FakeSocket.instances = [];
  FakeAudioContext.instances = [];
  FakeAudioContext.addModule = vi.fn(async () => undefined);
  FakeAudioContext.resumeFailure = null;
  vi.stubGlobal("AudioContext", FakeAudioContext);
  vi.stubGlobal("AudioWorkletNode", FakeNode);
  vi.stubGlobal("WebSocket", FakeSocket);
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("ParlandoAudioSink integration", () => {
  it("short-circuits a disabled server plan before acquiring audio resources", async () => {
    const sink = new ParlandoAudioSink();
    const session = context({ enabled: false });
    await sink.connect(input().value, session);
    expect(FakeAudioContext.instances).toHaveLength(0);
    expect(session.onVoiceStatus).toHaveBeenCalledWith({ connecting: false, message: "Voice is disabled for this experiment" });
  });

  it("connects the graph, sends capture frames, and accepts status and PCM messages", async () => {
    const sink = new ParlandoAudioSink();
    const microphone = input();
    const session = context();
    const connecting = sink.connect(microphone.value, session);
    await vi.waitFor(() => expect(FakeSocket.instances).toHaveLength(1));
    const socket = FakeSocket.instances[0];
    socket.open();
    await connecting;

    expect(socket.url.toString()).toContain("token=secret+ticket");
    expect(socket.binaryType).toBe("arraybuffer");
    expect(FakeNode.instances[1].port.postMessage).toHaveBeenCalledWith({ jitterSamples: 2_400 });
    FakeNode.instances[0].port.onmessage!(new MessageEvent("message", { data: new ArrayBuffer(960) }));
    expect(socket.send).toHaveBeenCalledOnce();
    expect((socket.send.mock.calls[0][0] as ArrayBuffer).byteLength).toBe(973);

    socket.dispatchEvent(new MessageEvent("message", { data: JSON.stringify({ type: "transcriptionStatus", ready: true, message: "ASR ready" }) }));
    socket.dispatchEvent(new MessageEvent("message", { data: new ArrayBuffer(973) }));
    expect(session.onVoiceStatus).toHaveBeenCalledWith({ transcriptionReady: true, transcriptionMessage: "ASR ready" });
    expect(session.onVoiceStatus).toHaveBeenCalledWith({ remoteAudio: true });

    await sink.disconnect();
    expect(microphone.track.stop).toHaveBeenCalledOnce();
    expect(FakeAudioContext.instances[0].close).toHaveBeenCalledOnce();
  });

  it("mutes capture without disconnecting and resumes on unmute", async () => {
    const sink = new ParlandoAudioSink();
    const microphone = input();
    const inputTrack = microphone.track;
    const connecting = sink.connect(microphone.value, context());
    await vi.waitFor(() => expect(FakeSocket.instances).toHaveLength(1));
    const socket = FakeSocket.instances[0];
    socket.open();
    await connecting;
    await sink.setInputEnabled(false);
    expect(inputTrack.enabled).toBe(false);
    FakeNode.instances[0].port.onmessage!(new MessageEvent("message", { data: new ArrayBuffer(960) }));
    expect(socket.send).not.toHaveBeenCalled();
    socket.dispatchEvent(new MessageEvent("message", { data: new ArrayBuffer(973) }));
    expect(FakeNode.instances[1].port.postMessage).toHaveBeenCalledTimes(2);
    await sink.setInputEnabled(true);
    expect(inputTrack.enabled).toBe(true);
    FakeNode.instances[0].port.onmessage!(new MessageEvent("message", { data: new ArrayBuffer(960) }));
    expect(socket.send).toHaveBeenCalledOnce();
  });

  it("releases an AudioContext when worklet loading fails", async () => {
    FakeAudioContext.addModule = vi.fn(async () => { throw new Error("module failed"); });
    const sink = new ParlandoAudioSink();
    await expect(sink.connect(input().value, context())).rejects.toThrow("module failed");
    expect(FakeAudioContext.instances[0].close).toHaveBeenCalledOnce();
  });

  it("cleans every partial resource when the WebSocket closes before open", async () => {
    const sink = new ParlandoAudioSink();
    const microphone = input();
    const connecting = sink.connect(microphone.value, context());
    await vi.waitFor(() => expect(FakeSocket.instances).toHaveLength(1));
    FakeSocket.instances[0].dispatchEvent(new Event("close"));
    await expect(connecting).rejects.toThrow("closed before connecting");
    expect(microphone.track.stop).toHaveBeenCalledOnce();
    expect(FakeNode.instances.every((node) => node.disconnect.mock.calls.length === 1)).toBe(true);
    expect(FakeAudioContext.instances[0].close).toHaveBeenCalledOnce();
  });

  it("ignores capture and inbound audio callbacks owned by a replaced transport", async () => {
    const sink = new ParlandoAudioSink();
    const firstConnect = sink.connect(input().value, context());
    await vi.waitFor(() => expect(FakeSocket.instances).toHaveLength(1));
    const oldSocket = FakeSocket.instances[0];
    oldSocket.open();
    await firstConnect;
    const oldCapture = FakeNode.instances[0];

    const secondSession = context();
    const secondConnect = sink.connect(input().value, secondSession);
    await vi.waitFor(() => expect(FakeSocket.instances).toHaveLength(2));
    const currentSocket = FakeSocket.instances[1];
    currentSocket.open();
    await secondConnect;
    const currentPlayback = FakeNode.instances[3];

    expect(oldCapture.port.onmessage).toBeNull();
    oldSocket.dispatchEvent(new MessageEvent("message", { data: new ArrayBuffer(973) }));
    oldSocket.dispatchEvent(new MessageEvent("message", { data: JSON.stringify({ type: "transcriptionStatus", ready: true }) }));
    expect(currentPlayback.port.postMessage).toHaveBeenCalledTimes(1);
    expect(secondSession.onVoiceStatus).not.toHaveBeenCalledWith(expect.objectContaining({ remoteAudio: true }));
  });

  it("continues teardown when every browser resource throws", async () => {
    const sink = new ParlandoAudioSink();
    const microphone = input();
    const connecting = sink.connect(microphone.value, context());
    await vi.waitFor(() => expect(FakeSocket.instances).toHaveLength(1));
    const socket = FakeSocket.instances[0];
    socket.open();
    await connecting;
    socket.close.mockImplementation(() => { throw new Error("socket close failed"); });
    for (const node of FakeNode.instances) {
      node.disconnect.mockImplementation(() => { throw new Error("node disconnect failed"); });
    }
    FakeAudioContext.instances[0].source.disconnect.mockImplementation(() => { throw new Error("source disconnect failed"); });
    (microphone.track.stop as ReturnType<typeof vi.fn>).mockImplementation(() => { throw new Error("track stop failed"); });
    FakeAudioContext.instances[0].close.mockRejectedValue(new Error("context close failed"));

    await expect(sink.disconnect()).resolves.toBeUndefined();
    expect(socket.close).toHaveBeenCalledOnce();
    expect(FakeNode.instances.every((node) => node.disconnect.mock.calls.length === 1)).toBe(true);
    expect(FakeAudioContext.instances[0].source.disconnect).toHaveBeenCalledOnce();
    expect(microphone.track.stop).toHaveBeenCalledOnce();
    expect(FakeAudioContext.instances[0].close).toHaveBeenCalledOnce();
  });

  it("cleans the context when AudioContext resume fails", async () => {
    FakeAudioContext.resumeFailure = new Error("resume failed");
    const sink = new ParlandoAudioSink();
    const microphone = input();
    const connecting = sink.connect(microphone.value, context());
    await expect(connecting).rejects.toThrow("resume failed");
    expect(FakeAudioContext.instances[0].close).toHaveBeenCalledOnce();
  });
});
