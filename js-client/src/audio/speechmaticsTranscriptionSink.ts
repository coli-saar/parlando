import type { AudioSessionContext, LocalAudioSink, MicrophoneInput } from "./types";

type SpeechmaticsCredentials = {
  enabled: boolean;
  realtimeUrl: string;
  temporaryKey: string;
  language: string;
  model: string | null;
  maxDelay: number;
  enablePartials: boolean;
  endOfUtteranceSilenceTrigger: number;
};

type PendingUtterance = {
  texts: string[];
  startMs: number;
  endMs: number;
  resultIds: string[];
};

export class SpeechmaticsTranscriptionSink implements LocalAudioSink {
  readonly id = "speechmatics-transcription";
  readonly provider = "speechmatics";
  readonly purposes = ["transcription"] as const;
  private socket: WebSocket | null = null;
  private audioContext: AudioContext | null = null;
  private source: MediaStreamAudioSourceNode | null = null;
  private processor: ScriptProcessorNode | null = null;
  private stream: MediaStream | null = null;
  private startedAt = 0;
  private inputEnabled = true;
  private sentFirstFrame = false;
  private pendingUtterance: PendingUtterance | null = null;
  private useUtteranceEndMessages = false;

  async connect(input: MicrophoneInput, context: AudioSessionContext): Promise<void> {
    const credentials = await this.resolveCredentials(context);
    this.useUtteranceEndMessages = credentials.endOfUtteranceSilenceTrigger > 0;
    if (!credentials.enabled || !credentials.realtimeUrl || !credentials.temporaryKey) {
      context.logVoice("transcription_token_disabled");
      context.onVoiceStatus({
        transcriptionMessage: "ASR idle",
        transcriptionReady: true
      });
      return;
    }
    if (!context.postTranscriptSegment) {
      throw new Error("Speechmatics transcription requires postTranscriptSegment in the audio context.");
    }

    this.startedAt = performance.now();
    const browserGlobal = globalThis as typeof globalThis & { webkitAudioContext?: typeof AudioContext };
    const AudioContextClass = browserGlobal.AudioContext ?? browserGlobal.webkitAudioContext;
    if (!AudioContextClass) {
      throw new Error("This browser does not support Web Audio transcription streaming.");
    }
    this.audioContext = new AudioContextClass();
    await this.audioContext.resume().catch(() => undefined);
    this.stream = input.createMediaStream("speechmatics-transcription");
    this.source = this.audioContext.createMediaStreamSource(this.stream);
    this.processor = this.audioContext.createScriptProcessor(4096, 1, 1);

    const url = speechmaticsWebSocketUrl(credentials.realtimeUrl, credentials.temporaryKey);
    const socket = new WebSocket(url);
    socket.binaryType = "arraybuffer";
    this.socket = socket;
    context.onVoiceStatus({
      transcriptionMessage: "Waiting for transcription service",
      transcriptionReady: false
    });
    context.logVoice("transcription_stream_connecting", {
      provider: "speechmatics",
      language: credentials.language,
      model: credentials.model,
      sample_rate: this.audioContext.sampleRate
    });

    await new Promise<void>((resolve, reject) => {
      socket.addEventListener("open", () => {
        socket.send(
          JSON.stringify({
            message: "StartRecognition",
            audio_format: {
              type: "raw",
              encoding: "pcm_s16le",
              sample_rate: this.audioContext?.sampleRate ?? 48000
            },
            transcription_config: transcriptionConfig(credentials)
          })
        );
        context.logVoice("transcription_stream_started", {
          provider: "speechmatics",
          participant_session_id: context.participantSessionId,
          role: context.role
        });
        context.onVoiceStatus({
          transcriptionMessage: "ASR listening",
          transcriptionReady: true
        });
        resolve();
      });
      socket.addEventListener("error", () => {
        context.logVoice("transcription_stream_error", { provider: "speechmatics" });
        context.onVoiceStatus({ transcriptionMessage: "ASR error", transcriptionReady: false });
        reject(new Error("Speechmatics WebSocket failed to connect."));
      });
    });

    socket.addEventListener("message", (event) => {
      this.handleMessage(event.data, context);
    });
    socket.addEventListener("close", (event) => {
      context.logVoice("transcription_stream_closed", {
        provider: "speechmatics",
        code: event.code,
        reason: event.reason
      });
    });

    this.processor.onaudioprocess = (event) => {
      if (!this.inputEnabled || socket.readyState !== WebSocket.OPEN) return;
      const pcm = float32ToPcm16(event.inputBuffer.getChannelData(0));
      socket.send(pcm);
      if (!this.sentFirstFrame) {
        this.sentFirstFrame = true;
        context.logVoice("transcription_audio_frame_sent", {
          provider: "speechmatics",
          sample_rate: this.audioContext?.sampleRate ?? null
        });
      }
    };
    this.source.connect(this.processor);
    this.processor.connect(this.audioContext.destination);
  }

  async setInputEnabled(enabled: boolean): Promise<void> {
    this.inputEnabled = enabled;
  }

  async disconnect(): Promise<void> {
    if (this.socket?.readyState === WebSocket.OPEN) {
      this.socket.send(JSON.stringify({ message: "EndOfStream" }));
      this.socket.close();
    }
    this.socket = null;
    this.processor?.disconnect();
    this.source?.disconnect();
    this.processor = null;
    this.source = null;
    for (const track of this.stream?.getTracks() ?? []) {
      track.stop();
    }
    this.stream = null;
    await this.audioContext?.close().catch(() => undefined);
    this.audioContext = null;
    this.startedAt = 0;
    this.sentFirstFrame = false;
    this.pendingUtterance = null;
    this.useUtteranceEndMessages = false;
  }

  private async resolveCredentials(context: AudioSessionContext): Promise<SpeechmaticsCredentials> {
    const plan = await context.getAudioSession?.();
    const sink = plan?.sinks.find(
      (candidate) => candidate.provider === "speechmatics" && candidate.purposes.includes("transcription")
    );
    const credentials = sink?.credentials ?? {};
    return {
      enabled: valueAsBoolean(credentials.enabled, false),
      realtimeUrl: valueAsString(credentials.realtime_url) ?? valueAsString(credentials.realtimeUrl) ?? "",
      temporaryKey: valueAsString(credentials.temporary_key) ?? valueAsString(credentials.temporaryKey) ?? "",
      language: normalizeLanguage(valueAsString(credentials.language) ?? "en"),
      model: valueAsString(credentials.model),
      maxDelay: valueAsNumber(credentials.max_delay, 2.0),
      enablePartials: valueAsBoolean(credentials.enable_partials, true),
      endOfUtteranceSilenceTrigger: valueAsNumber(credentials.end_of_utterance_silence_trigger, 0)
    };
  }

  private handleMessage(raw: unknown, context: AudioSessionContext): void {
    if (typeof raw !== "string") return;
    let message: Record<string, unknown>;
    try {
      message = JSON.parse(raw) as Record<string, unknown>;
    } catch {
      return;
    }
    const messageType = String(message.message ?? "");
    if (messageType === "Error") {
      context.logVoice("transcription_stream_error", { provider: "speechmatics", ...message });
      context.onVoiceStatus({ transcriptionMessage: "ASR error", transcriptionReady: false });
      return;
    }
    if (messageType === "AddPartialTranscript") {
      const transcript = transcriptText(message);
      if (transcript) {
        context.logVoice("transcription_turn_started", { provider: "speechmatics", text_length: transcript.length });
        context.onVoiceStatus({ transcriptionMessage: "ASR transcribing" });
      }
      return;
    }
    if (messageType === "AddTranscript") {
      this.addFinalTranscript(message);
      if (!this.useUtteranceEndMessages) {
        this.flushPendingUtterance(context, messageType);
      }
      return;
    }
    if (messageType === "EndOfUtterance") {
      this.flushPendingUtterance(context, messageType);
    }
  }

  private addFinalTranscript(message: Record<string, unknown>): void {
    const text = transcriptText(message);
    if (!text) return;
    const metadata = objectValue(message.metadata);
    const startMs = secondsToMs(valueAsNumber(metadata?.start_time, 0));
    const endMs = secondsToMs(valueAsNumber(metadata?.end_time, Math.max(0, (performance.now() - this.startedAt) / 1000)));
    const resultId = valueAsString(message.id) ?? valueAsString(message.result_id);
    if (!this.pendingUtterance) {
      this.pendingUtterance = { texts: [], startMs, endMs, resultIds: [] };
    }
    this.pendingUtterance.texts.push(text);
    this.pendingUtterance.startMs = Math.min(this.pendingUtterance.startMs, startMs);
    this.pendingUtterance.endMs = Math.max(this.pendingUtterance.endMs, endMs);
    if (resultId) this.pendingUtterance.resultIds.push(resultId);
  }

  private flushPendingUtterance(context: AudioSessionContext, reason: string): void {
    const pending = this.pendingUtterance;
    if (!pending) return;
    this.pendingUtterance = null;
    const text = joinTranscriptChunks(pending.texts);
    if (!text) return;
    void context.postTranscriptSegment?.({
      participant_session_id: context.participantSessionId,
      player: context.role,
      start_time_ms: pending.startMs,
      end_time_ms: pending.endMs,
      text,
      metadata: {
        provider: "speechmatics",
        speechmatics_message: reason,
        speechmatics_result_ids: pending.resultIds
      }
    }).then(() => {
      context.logVoice("transcription_transcript_posted", {
        provider: "speechmatics",
        text_length: text.length,
        chunks: pending.texts.length
      });
      context.onVoiceStatus({
        transcriptionMessage: "ASR transcribing",
        transcriptionReady: true
      });
    }).catch((error) => {
      context.logVoice("transcription_transcript_post_failed", {
        provider: "speechmatics",
        error: error instanceof Error ? error.message : String(error)
      });
    });
  }
}

function speechmaticsWebSocketUrl(realtimeUrl: string, temporaryKey: string): string {
  const url = new URL(realtimeUrl);
  url.searchParams.set("jwt", temporaryKey);
  return url.toString();
}

function transcriptionConfig(credentials: SpeechmaticsCredentials): Record<string, unknown> {
  const config: Record<string, unknown> = {
    language: credentials.language,
    max_delay: credentials.maxDelay,
    enable_partials: credentials.enablePartials
  };
  if (credentials.endOfUtteranceSilenceTrigger > 0) {
    config.conversation_config = {
      end_of_utterance_silence_trigger: credentials.endOfUtteranceSilenceTrigger
    };
  }
  if (credentials.model && credentials.model !== "default") {
    config.model = credentials.model;
  }
  return config;
}

function joinTranscriptChunks(chunks: string[]): string {
  return chunks
    .map((chunk) => chunk.trim())
    .filter(Boolean)
    .join(" ")
    .replace(/\s+([,.;:!?])/g, "$1")
    .trim();
}

function secondsToMs(seconds: number): number {
  return Math.max(0, Math.round(seconds * 1000));
}

function float32ToPcm16(samples: Float32Array): ArrayBuffer {
  const output = new ArrayBuffer(samples.length * 2);
  const view = new DataView(output);
  for (let i = 0; i < samples.length; i += 1) {
    const sample = Math.max(-1, Math.min(1, samples[i]));
    view.setInt16(i * 2, sample < 0 ? sample * 0x8000 : sample * 0x7fff, true);
  }
  return output;
}

function transcriptText(message: Record<string, unknown>): string {
  const metadata = objectValue(message.metadata);
  const metadataTranscript = valueAsString(metadata?.transcript);
  if (metadataTranscript) return metadataTranscript.trim();
  const results = Array.isArray(message.results) ? message.results : [];
  return results
    .map((result) => {
      const resultObject = objectValue(result);
      const alternativesValue = resultObject?.alternatives;
      const alternatives = Array.isArray(alternativesValue) ? alternativesValue : [];
      const first = alternatives[0];
      return valueAsString(objectValue(first)?.content) ?? valueAsString(objectValue(first)?.word) ?? "";
    })
    .join(" ")
    .trim();
}

function normalizeLanguage(language: string): string {
  return language.split("-")[0] || language;
}

function objectValue(value: unknown): Record<string, unknown> | null {
  return value && typeof value === "object" ? (value as Record<string, unknown>) : null;
}

function valueAsString(value: unknown): string | null {
  return typeof value === "string" ? value : null;
}

function valueAsNumber(value: unknown, fallback: number): number {
  return typeof value === "number" ? value : fallback;
}

function valueAsBoolean(value: unknown, fallback: boolean): boolean {
  return typeof value === "boolean" ? value : fallback;
}
