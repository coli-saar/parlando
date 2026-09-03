import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { ParticipantClient } from "../../js-client/dist/index.js";
import { ParlandoAudioSink, encodeFrame } from "../../js-client/dist/audio/parlandoAudioSink.js";

const requireFromClient = createRequire(new URL("../../js-client/package.json", import.meta.url));
const WebSocket = requireFromClient("ws");

/** Minimal message port that records worklet-bound playback messages. */
class ContractPort {
  onmessage = null;
  messages = [];

  /** Records one message delivered to the simulated worklet. */
  postMessage(message) {
    this.messages.push(message);
  }
}

/** Deterministic AudioWorkletNode substitute for capture and playback. */
class ContractWorkletNode {
  static instances = [];
  port = new ContractPort();

  /** Retains nodes in their production construction order. */
  constructor() {
    ContractWorkletNode.instances.push(this);
  }

  /** Implements the Web Audio graph connection surface. */
  connect() {}

  /** Implements deterministic graph teardown. */
  disconnect() {}
}

/** Minimal AudioContext used to exercise transport without a browser engine. */
class ContractAudioContext {
  audioWorklet = { addModule: async () => undefined };
  destination = {};

  /** Returns the source-node operations used by the production sink. */
  createMediaStreamSource() {
    return { connect() {}, disconnect() {} };
  }

  /** Models successful audio activation. */
  async resume() {}

  /** Models successful audio teardown. */
  async close() {}
}

/** Reads the single JSON fixture supplied by the Rust server owner. */
async function readFixture() {
  const chunks = [];
  for await (const chunk of process.stdin) chunks.push(chunk);
  return JSON.parse(Buffer.concat(chunks).toString("utf8"));
}

/** Waits for a WebSocket to open or reports a stable timeout. */
function waitForOpen(socket) {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error("WebSocket did not open")), 5_000);
    socket.addEventListener("open", () => {
      clearTimeout(timer);
      resolve();
    }, { once: true });
    socket.addEventListener("error", () => {
      clearTimeout(timer);
      reject(new Error("WebSocket failed before opening"));
    }, { once: true });
  });
}

/** Creates a participant through the production client and records required consent. */
async function admit(origin) {
  const client = new ParticipantClient({ baseUrl: origin });
  await client.register();
  await client.acceptConsents({ study: true });
  return client;
}

/** Opens and readies one production game channel using a ParticipantClient ticket. */
async function openGame(client, sessionId) {
  const plan = await client.getGameSession(sessionId);
  const socket = new WebSocket(client.socketUrl(plan));
  await waitForOpen(socket);
  socket.send(JSON.stringify({ type: "ready" }));
  return socket;
}

/** Creates one authenticated WebSocket URL from a production audio plan. */
function audioSocketUrl(plan, origin) {
  const url = new URL(plan.websocketUrl, origin);
  url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
  url.searchParams.set("token", plan.token);
  return url;
}

/** Captures binary frames received by the partner audio socket. */
class AudioInbox {
  frames = [];

  /** Records complete binary messages while ignoring transcription status text. */
  constructor(socket) {
    socket.addEventListener("message", (event) => {
      if (typeof event.data === "string") return;
      this.frames.push(new Uint8Array(event.data));
    });
  }

  /** Waits for a relayed frame carrying the requested first PCM marker byte. */
  async marker(expected) {
    const deadline = Date.now() + 3_000;
    while (Date.now() < deadline) {
      const index = this.frames.findIndex((frame) => frame[13] === expected);
      if (index >= 0) return this.frames.splice(index, 1)[0];
      await new Promise((resolve) => setTimeout(resolve, 5));
    }
    throw new Error(`partner audio did not receive marker ${expected}`);
  }

  /** Verifies that no binary frame arrives during the mute isolation window. */
  async remainsEmpty() {
    await new Promise((resolve) => setTimeout(resolve, 200));
    assert.equal(this.frames.length, 0, "muted capture leaked across the audio WebSocket");
  }
}

/** Waits until the playback worklet has received the requested number of messages. */
async function waitForPlayback(count) {
  const deadline = Date.now() + 2_000;
  while ((ContractWorkletNode.instances[1]?.port.messages.length ?? 0) < count) {
    if (Date.now() >= deadline) throw new Error(`playback did not reach ${count} messages`);
    await new Promise((resolve) => setTimeout(resolve, 5));
  }
}

/** Emits one canonical capture quantum through the production sink callback. */
function emitCapture(marker) {
  const pcm = new ArrayBuffer(960);
  new Uint8Array(pcm).fill(marker);
  ContractWorkletNode.instances[0].port.onmessage({ data: pcm });
}

const { origin } = await readFixture();
globalThis.WebSocket = WebSocket;
globalThis.AudioContext = ContractAudioContext;
globalThis.AudioWorkletNode = ContractWorkletNode;
globalThis.window = { location: { origin, pathname: "/", search: "", hash: "" } };

const a = await admit(origin);
const b = await admit(origin);
const waiting = await a.join();
const joined = await b.join();
assert.equal(waiting.state, "waiting");
assert.equal(joined.public_session_id, waiting.public_session_id);
const sessionId = waiting.public_session_id;
const gameA = await openGame(a, sessionId);
const gameB = await openGame(b, sessionId);

const planB = await b.getAudioSession(sessionId);
assert.equal(planB.enabled, true);
const audioB = new WebSocket(audioSocketUrl(planB, origin));
audioB.binaryType = "arraybuffer";
const partnerAudio = new AudioInbox(audioB);
await waitForOpen(audioB);

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
let resolveTranscriptionReady;
const transcriptionReady = new Promise((resolve) => {
  resolveTranscriptionReady = resolve;
});
await sink.connect(input, {
  sessionId,
  role: "A",
  selectedAudioInputId: input.deviceId,
  selectedAudioInputLabel: input.deviceLabel,
  getAudioSession: () => a.getAudioSession(sessionId),
  logVoice() {},
  onVoiceStatus(status) {
    if (status.transcriptionReady) resolveTranscriptionReady();
  }
});
await Promise.race([
  transcriptionReady,
  new Promise((_, reject) => setTimeout(
    () => reject(new Error("transcription did not become ready")),
    3_000
  ))
]);

emitCapture(1);
await partnerAudio.marker(1);

await sink.setInputEnabled(false);
assert.equal(transportTrack.enabled, false);
emitCapture(2);
await partnerAudio.remainsEmpty();

const partnerPcm = new ArrayBuffer(960);
new Uint8Array(partnerPcm).fill(9);
audioB.send(encodeFrame(0, 0, partnerPcm));
await waitForPlayback(2);

await sink.setInputEnabled(true);
assert.equal(transportTrack.enabled, true);
emitCapture(3);
await partnerAudio.marker(3);

await sink.disconnect();
audioB.close();
gameA.close();
gameB.close();
process.stdout.write(`${JSON.stringify({ status: "passed", relayedMarkers: [1, 3] })}\n`);
process.exit(0);
