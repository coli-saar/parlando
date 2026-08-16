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
  private connectInFlight: Promise<void> | null = null;
  private muteInFlight: Promise<void> | null = null;
  private desiredMicrophoneEnabled = true;
  private transportGeneration = 0;

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

  /** Connects the prepared microphone while preserving the desired mute state across reconnects. */
  connect(context: AudioSessionContext): Promise<void> {
    if (this.connectInFlight) return this.connectInFlight;
    const operation = this.performConnect(context);
    this.connectInFlight = operation;
    void operation.finally(() => {
      if (this.connectInFlight === operation) this.connectInFlight = null;
    }).catch(() => undefined);
    return operation;
  }

  /** Performs one fresh transport connection using the controller-owned microphone preference. */
  private async performConnect(context: AudioSessionContext): Promise<void> {
    if (this.voiceStatus.connected) return;
    context.logVoice("voice_connect_requested", {
      user_agent: typeof navigator === "undefined" ? "" : navigator.userAgent,
      secure_context: typeof window !== "undefined" && window.isSecureContext
    });
    this.updateVoiceStatus({ connecting: true, error: null, message: "Connecting voice..." });
    const generation = ++this.transportGeneration;
    try {
      const input = this.microphone.input();
      await this.sink.setInputEnabled(this.desiredMicrophoneEnabled);
      await this.sink.connect(input, {
        ...context,
        onVoiceStatus: (status) => {
          if (generation === this.transportGeneration) this.updateVoiceStatus(status);
        }
      });
    } catch (error) {
      context.logVoice("voice_connect_failed", { error: error instanceof Error ? error.message : String(error) });
      await this.disconnectTransport();
      throw error;
    }
  }

  /** Sets the participant's desired mute state and reconciles any request made during a transition. */
  setMicrophoneMuted(muted: boolean, context: AudioSessionContext): Promise<void> {
    this.desiredMicrophoneEnabled = !muted;
    if (this.muteInFlight) return this.muteInFlight;
    const operation = this.reconcileMicrophoneState(context);
    this.muteInFlight = operation;
    void operation.finally(() => {
      if (this.muteInFlight === operation) this.muteInFlight = null;
    }).catch(() => undefined);
    return operation;
  }

  /** Applies the latest desired microphone state, including a preference changed mid-transition. */
  private async reconcileMicrophoneState(context: AudioSessionContext): Promise<void> {
    if (!this.voiceStatus.connected) return;
    while (this.voiceStatus.microphoneEnabled !== this.desiredMicrophoneEnabled) {
      const enabled = this.desiredMicrophoneEnabled;
      context.logVoice("microphone_toggle_requested", { enabled });
      this.updateVoiceStatus({ microphoneChanging: true, error: null });
      try {
        await this.sink.setInputEnabled(enabled);
      } catch (error) {
        const action = enabled ? "unmute" : "mute";
        context.logVoice("microphone_toggle_failed", { enabled });
        this.updateVoiceStatus({
          microphoneChanging: false,
          error: `Could not ${action} the microphone.`,
          message: this.voiceStatus.microphoneEnabled ? "Microphone live" : "Microphone muted"
        });
        throw error;
      }
      context.logVoice("microphone_toggle_succeeded", { enabled });
      this.updateVoiceStatus({
        microphoneEnabled: enabled,
        microphoneChanging: false,
        message: enabled ? "Microphone live" : "Microphone muted"
      });
    }
  }

  async disconnect(resetPreflight = false): Promise<void> {
    await this.disconnectTransport();
    if (resetPreflight) {
      this.desiredMicrophoneEnabled = true;
      await this.sink.setInputEnabled(true);
      this.microphone.reset();
    } else {
      this.microphone.stop();
    }
    this.emit();
  }

  /** Tears down only the room transport while preserving the prepared microphone input. */
  private async disconnectTransport(): Promise<void> {
    this.transportGeneration += 1;
    await this.sink.disconnect();
    this.voiceStatus = { ...initialVoiceStatus };
    this.emit();
  }

  private emit(): void {
    const snapshot = this.snapshot();
    for (const listener of this.listeners) listener(snapshot);
  }
}
