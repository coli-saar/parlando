import readline from "node:readline";
import WebSocket from "ws";
import { ParlandoAudioSink } from "../dist/audio/parlandoAudioSink.js";

class ContractPort {
  onmessage = null;
  messages = [];

  /** Records worklet-bound messages so the Rust driver can inspect playback. */
  postMessage(message) {
    this.messages.push(message);
  }
}

class ContractWorkletNode {
  static instances = [];
  port = new ContractPort();

  /** Retains capture and playback nodes in their production construction order. */
  constructor() {
    ContractWorkletNode.instances.push(this);
  }

  /** Implements the Web Audio connection surface used by the sink. */
  connect() {}

  /** Implements deterministic graph teardown. */
  disconnect() {}
}

class ContractAudioContext {
  audioWorklet = { addModule: async () => undefined };
  destination = {};

  /** Returns the minimal source-node contract used by the production sink. */
  createMediaStreamSource() {
    return { connect() {}, disconnect() {} };
  }

  /** Models successful browser audio activation. */
  async resume() {}

  /** Models successful browser audio teardown. */
  async close() {}
}

/** Writes one line-delimited protocol response to the Rust test driver. */
function respond(value) {
  process.stdout.write(`${JSON.stringify(value)}\n`);
}

/** Waits until the fake playback worklet has received the requested message count. */
async function waitForPlayback(count) {
  const deadline = Date.now() + 2_000;
  while ((ContractWorkletNode.instances[1]?.port.messages.length ?? 0) < count) {
    if (Date.now() >= deadline) throw new Error(`playback did not reach ${count} messages`);
    await new Promise((resolve) => setTimeout(resolve, 5));
  }
}

/** Emits one canonical capture quantum through the real sink callback. */
function emitCapture(marker) {
  const pcm = new ArrayBuffer(960);
  new Uint8Array(pcm).fill(marker);
  ContractWorkletNode.instances[0].port.onmessage({ data: pcm });
}

const fixture = JSON.parse(process.env.PARLANDO_MUTE_CONTRACT ?? "null");
if (!fixture) throw new Error("PARLANDO_MUTE_CONTRACT is required");

globalThis.WebSocket = WebSocket;
globalThis.AudioContext = ContractAudioContext;
globalThis.AudioWorkletNode = ContractWorkletNode;
globalThis.window = { location: { origin: fixture.origin } };

const transportTrack = { enabled: true, stop() {} };
const stream = {
  getAudioTracks: () => [transportTrack],
  getTracks: () => [transportTrack]
};
const input = {
  deviceId: "contract-microphone",
  deviceLabel: "Contract microphone",
  stream,
  track: transportTrack,
  createTrackClone: () => transportTrack,
  createMediaStream: () => stream
};
const sink = new ParlandoAudioSink();
const plan = {
  enabled: fixture.plan.enabled,
  websocketUrl: fixture.plan.websocket_url ?? null,
  token: fixture.plan.token ?? null,
  protocolVersion: fixture.plan.protocol_version,
  sampleRateHz: fixture.plan.sample_rate_hz,
  channels: fixture.plan.channels,
  frameDurationMs: fixture.plan.frame_duration_ms,
  jitterBufferMs: fixture.plan.jitter_buffer_ms
};
await sink.connect(input, {
  sessionId: fixture.public_session_id,
  role: "A",
  selectedAudioInputId: input.deviceId,
  selectedAudioInputLabel: input.deviceLabel,
  getAudioSession: async () => plan,
  logVoice() {},
  onVoiceStatus() {}
});
respond({ type: "ready" });

const lines = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
for await (const line of lines) {
  const command = JSON.parse(line);
  if (command.type === "capture") {
    emitCapture(command.marker);
    respond({ type: "captured", marker: command.marker, track_enabled: transportTrack.enabled });
    continue;
  }
  if (command.type === "mute") {
    await sink.setInputEnabled(!command.muted);
    respond({ type: "muteChanged", muted: command.muted, track_enabled: transportTrack.enabled });
    continue;
  }
  if (command.type === "waitForPlayback") {
    await waitForPlayback(command.count);
    respond({ type: "playback", count: ContractWorkletNode.instances[1].port.messages.length });
    continue;
  }
  if (command.type === "disconnect") {
    await sink.disconnect();
    respond({ type: "disconnected" });
    break;
  }
  throw new Error(`unknown contract command: ${command.type}`);
}
process.exit(0);
