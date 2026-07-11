import { MicrophoneSource } from "./microphoneSource";
import type {
  AudioSessionContext,
  AudioSessionSnapshot,
  LocalAudioSink,
  VoicePreflight,
  VoiceStatus
} from "./types";
import { initialVoicePreflight, initialVoiceStatus } from "./types";

type Listener = (snapshot: AudioSessionSnapshot) => void;

export class AudioSessionController {
  private microphone: MicrophoneSource;
  private sinks: LocalAudioSink[];
  private voiceStatus: VoiceStatus = initialVoiceStatus;
  private voicePreflight: VoicePreflight = initialVoicePreflight;
  private listeners = new Set<Listener>();

  constructor({ microphone, sink, sinks }: { microphone: MicrophoneSource; sink?: LocalAudioSink; sinks?: LocalAudioSink[] }) {
    this.microphone = microphone;
    this.sinks = sinks ?? (sink ? [sink] : []);
    this.microphone.subscribe((preflight) => {
      this.voicePreflight = preflight;
      this.emit();
    });
  }

  subscribe(listener: Listener): () => void {
    this.listeners.add(listener);
    listener(this.snapshot());
    return () => this.listeners.delete(listener);
  }

  snapshot(): AudioSessionSnapshot {
    return {
      voiceStatus: this.voiceStatus,
      voicePreflight: this.voicePreflight
    };
  }

  updateVoiceStatus(update: Partial<VoiceStatus>): void {
    this.voiceStatus = { ...this.voiceStatus, ...update };
    this.emit();
  }

  async prepare(deviceId: string, fallbackLabel = "Default microphone"): Promise<void> {
    await this.microphone.prepare(deviceId, fallbackLabel);
    if (!this.voiceStatus.connected) {
      this.updateVoiceStatus({ message: "Microphone permission ready" });
    }
  }

  async toggle(context: AudioSessionContext, deviceId: string, fallbackLabel = "Default microphone"): Promise<void> {
    if (this.voiceStatus.connected) {
      const nextEnabled = !this.voiceStatus.microphoneEnabled;
      context.logVoice("microphone_toggle_requested", { enabled: nextEnabled });
      await Promise.all(this.sinks.map((sink) => sink.setInputEnabled(nextEnabled)));
      context.logVoice("microphone_toggle_succeeded", { enabled: nextEnabled });
      this.updateVoiceStatus({
        microphoneEnabled: nextEnabled,
        message: nextEnabled ? "Microphone live" : "Microphone muted"
      });
      return;
    }

    context.logVoice("voice_connect_requested", {
      user_agent: typeof navigator === "undefined" ? "" : navigator.userAgent,
      secure_context: typeof window !== "undefined" && window.isSecureContext
    });
    this.updateVoiceStatus({ connecting: true, message: "Connecting voice..." });
    try {
      const input = await this.microphone.prepare(deviceId, fallbackLabel);
      let plannedContext = context;
      if (context.getAudioSession && this.sinks.length > 1) {
        const plan = await context.getAudioSession();
        plannedContext = {
          ...context,
          getAudioSession: async () => plan
        };
      }
      await Promise.all(
        this.sinks.map((sink) =>
          sink.connect(input, {
            ...plannedContext,
            onVoiceStatus: (status) => this.updateVoiceStatus(status)
          })
        )
      );
    } catch (error) {
      context.logVoice("voice_connect_failed", { error: error instanceof Error ? error.message : String(error) });
      await this.disconnect();
      throw error;
    }
  }

  async disconnect(resetPreflight = false): Promise<void> {
    await Promise.all(this.sinks.map((sink) => sink.disconnect()));
    this.voiceStatus = initialVoiceStatus;
    if (resetPreflight) {
      this.microphone.reset();
    } else {
      this.microphone.stop();
    }
    this.emit();
  }

  private emit(): void {
    const snapshot = this.snapshot();
    for (const listener of this.listeners) listener(snapshot);
  }
}
