import type { AudioSessionContext, LocalAudioSink, MicrophoneInput } from "./types.js";

const HEADER_BYTES = 13;
const PCM_BYTES = 960;

/// Single browser transport for partner audio and server-side transcription.
export class ParlandoAudioSink implements LocalAudioSink {
  readonly id = "parlando-audio";
  readonly provider = "parlando";
  readonly purposes = ["partner-audio", "transcription"] as const;
  private socket: WebSocket | null = null;
  private audioContext: AudioContext | null = null;
  private source: MediaStreamAudioSourceNode | null = null;
  private capture: AudioWorkletNode | null = null;
  private playback: AudioWorkletNode | null = null;
  private stream: MediaStream | null = null;
  private enabled = true;
  private sequence = 0;
  private startedAt = 0;

  async connect(input: MicrophoneInput, context: AudioSessionContext): Promise<void> {
    const plan = await context.getAudioSession();
    if (!plan.enabled || !plan.websocket_url || !plan.token) { context.onVoiceStatus({ connecting: false, message: "Voice is disabled for this study" }); return; }
    const audio = new AudioContext();
    this.audioContext = audio;
    await Promise.all([
      audio.audioWorklet.addModule(new URL("./captureWorklet.js", import.meta.url)),
      audio.audioWorklet.addModule(new URL("./playbackWorklet.js", import.meta.url))
    ]);
    await audio.resume();
    this.stream = input.createMediaStream("parlando-audio");
    this.source = audio.createMediaStreamSource(this.stream);
    this.capture = new AudioWorkletNode(audio, "parlando-capture", { numberOfOutputs: 0 });
    this.playback = new AudioWorkletNode(audio, "parlando-playback", { numberOfInputs: 0, outputChannelCount: [1] });
    this.playback.port.postMessage({ jitterSamples: Math.round(plan.sample_rate_hz * plan.jitter_buffer_ms / 1000) });
    this.playback.port.onmessage = (event: MessageEvent<{ type?: string; count?: number; bufferedSamples?: number }>) => {
      if (event.data.type === "playbackUnderrun") context.logVoice("audio_playback_underrun", { count: event.data.count, buffered_samples: event.data.bufferedSamples });
    };
    this.source.connect(this.capture);
    this.playback.connect(audio.destination);
    const url = new URL(plan.websocket_url);
    url.searchParams.set("token", plan.token);
    const socket = new WebSocket(url);
    socket.binaryType = "arraybuffer";
    this.socket = socket;
    this.startedAt = performance.now();
    await waitForSocketOpen(socket);
    context.logVoice("parlando_audio_connected", { protocol_version: plan.protocol_version });
    context.onVoiceStatus({ connected: true, connecting: false, microphoneEnabled: true, message: "Microphone live", transcriptionMessage: "Waiting for transcription service", transcriptionReady: false });
    this.capture.port.onmessage = (event: MessageEvent<ArrayBuffer>) => {
      if (this.enabled && socket.readyState === WebSocket.OPEN) socket.send(encodeFrame(this.sequence++, Math.round(performance.now() - this.startedAt), event.data));
    };
    socket.addEventListener("message", (event) => {
      if (typeof event.data === "string") {
        const status = JSON.parse(event.data) as { type?: string; ready?: boolean; message?: string };
        if (status.type === "transcriptionStatus") context.onVoiceStatus({ transcriptionReady: Boolean(status.ready), transcriptionMessage: status.message ?? "ASR idle" });
        return;
      }
      if (!(event.data instanceof ArrayBuffer) || event.data.byteLength !== HEADER_BYTES + PCM_BYTES) return;
      const pcm = event.data.slice(HEADER_BYTES);
      this.playback?.port.postMessage(pcm, [pcm]);
      context.onVoiceStatus({ remoteAudio: true, message: "Voice connected" });
    });
    socket.addEventListener("close", () => context.onVoiceStatus({ connected: false, microphoneEnabled: false, remoteAudio: false, message: "Voice disconnected", transcriptionReady: false, transcriptionMessage: "ASR idle" }));
  }

  async setInputEnabled(enabled: boolean): Promise<void> { this.enabled = enabled; }
  async disconnect(): Promise<void> {
    this.socket?.close(); this.socket = null;
    this.capture?.disconnect(); this.playback?.disconnect(); this.source?.disconnect();
    for (const track of this.stream?.getTracks() ?? []) track.stop();
    this.capture = null; this.playback = null; this.source = null; this.stream = null;
    await this.audioContext?.close().catch(() => undefined); this.audioContext = null;
  }
}

/// Resolves on WebSocket open and rejects if the transport errors or closes first.
export function waitForSocketOpen(socket: WebSocket): Promise<void> {
  return new Promise<void>((resolve, reject) => {
    const cleanup = () => {
      socket.removeEventListener("open", onOpen);
      socket.removeEventListener("error", onError);
      socket.removeEventListener("close", onClose);
    };
    const onOpen = () => { cleanup(); resolve(); };
    const onError = () => { cleanup(); reject(new Error("Parlando audio WebSocket failed to connect.")); };
    const onClose = () => { cleanup(); reject(new Error("Parlando audio WebSocket closed before connecting.")); };
    socket.addEventListener("open", onOpen, { once: true });
    socket.addEventListener("error", onError, { once: true });
    socket.addEventListener("close", onClose, { once: true });
  });
}

/// Encodes one versioned PCM WebSocket frame.
export function encodeFrame(sequence: number, timestampMs: number, pcm: ArrayBuffer): ArrayBuffer {
  if (pcm.byteLength !== PCM_BYTES) throw new Error(`PCM frame must contain ${PCM_BYTES} bytes.`);
  const frame = new ArrayBuffer(HEADER_BYTES + PCM_BYTES);
  const view = new DataView(frame);
  view.setUint8(0, 1); view.setUint32(1, sequence, false); view.setBigUint64(5, BigInt(timestampMs), false);
  new Uint8Array(frame, HEADER_BYTES).set(new Uint8Array(pcm));
  return frame;
}
