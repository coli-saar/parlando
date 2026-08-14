import { MicrophoneSource } from "./microphoneSource.js";
import type {
  AudioSessionContext,
  AudioSessionSnapshot,
  LocalAudioSink,
  VoicePreflight,
  VoiceStatus
} from "./types.js";
import { initialVoicePreflight, initialVoiceStatus } from "./types.js";

type Listener = (snapshot: AudioSessionSnapshot) => void;

export class AudioSessionController {
  private microphone: MicrophoneSource;
  private sink: LocalAudioSink;
  private voiceStatus: VoiceStatus = initialVoiceStatus;
  private voicePreflight: VoicePreflight = initialVoicePreflight;
  private listeners = new Set<Listener>();

  /** Creates a controller for Parlando's single provider-neutral audio transport. */
  constructor({ microphone, sink }: { microphone: MicrophoneSource; sink: LocalAudioSink }) {
    this.microphone = microphone;
    this.sink = sink;
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

  /** Toggles mute for a connected session or connects the already prepared microphone. */
  async toggle(context: AudioSessionContext): Promise<void> {
    if (this.voiceStatus.connected) {
      const nextEnabled = !this.voiceStatus.microphoneEnabled;
      context.logVoice("microphone_toggle_requested", { enabled: nextEnabled });
      await this.sink.setInputEnabled(nextEnabled);
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
      const input = this.microphone.input();
      await this.sink.connect(input, {
        ...context,
        onVoiceStatus: (status) => this.updateVoiceStatus(status)
      });
    } catch (error) {
      context.logVoice("voice_connect_failed", { error: error instanceof Error ? error.message : String(error) });
      await this.disconnectTransport();
      throw error;
    }
  }

  async disconnect(resetPreflight = false): Promise<void> {
    await this.disconnectTransport();
    if (resetPreflight) {
      this.microphone.reset();
    } else {
      this.microphone.stop();
    }
    this.emit();
  }

  /** Tears down only the room transport while preserving the prepared microphone input. */
  private async disconnectTransport(): Promise<void> {
    await this.sink.disconnect();
    this.voiceStatus = initialVoiceStatus;
    this.emit();
  }

  private emit(): void {
    const snapshot = this.snapshot();
    for (const listener of this.listeners) listener(snapshot);
  }
}
