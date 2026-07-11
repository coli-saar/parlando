import type { MicrophoneInput, VoicePreflight } from "./types";
import { initialVoicePreflight } from "./types";

type Listener = (preflight: VoicePreflight) => void;

interface MicrophoneSourceOptions {
  getUserMedia?: (constraints: MediaStreamConstraints) => Promise<MediaStream>;
  isSecureContext?: () => boolean;
  getAudioContext?: () => typeof AudioContext | undefined;
  requestAnimationFrame?: (callback: FrameRequestCallback) => number;
  cancelAnimationFrame?: (handle: number) => void;
}

export class MicrophoneSource {
  private stream: MediaStream | null = null;
  private track: MediaStreamTrack | null = null;
  private selectedDeviceId = "";
  private selectedDeviceLabel = "Default microphone";
  private preflight: VoicePreflight = initialVoicePreflight;
  private listeners = new Set<Listener>();
  private cleanupProbe: (() => void) | null = null;
  private getUserMediaOverride?: (constraints: MediaStreamConstraints) => Promise<MediaStream>;
  private isSecureContextOverride?: () => boolean;
  private getAudioContextOverride?: () => typeof AudioContext | undefined;
  private requestAnimationFrameOverride?: (callback: FrameRequestCallback) => number;
  private cancelAnimationFrameOverride?: (handle: number) => void;

  constructor(options: MicrophoneSourceOptions = {}) {
    this.getUserMediaOverride = options.getUserMedia;
    this.isSecureContextOverride = options.isSecureContext;
    this.getAudioContextOverride = options.getAudioContext;
    this.requestAnimationFrameOverride = options.requestAnimationFrame;
    this.cancelAnimationFrameOverride = options.cancelAnimationFrame;
  }

  subscribe(listener: Listener): () => void {
    this.listeners.add(listener);
    listener(this.preflight);
    return () => this.listeners.delete(listener);
  }

  snapshot(): VoicePreflight {
    return this.preflight;
  }

  input(): MicrophoneInput {
    if (!this.stream || !this.track) {
      throw new Error("Microphone has not been prepared.");
    }
    return {
      deviceId: this.selectedDeviceId,
      deviceLabel: this.selectedDeviceLabel,
      stream: this.stream,
      track: this.track,
      createTrackClone: () => this.track!.clone(),
      createMediaStream: () => new MediaStream([this.track!.clone()])
    };
  }

  async prepare(deviceId: string, fallbackLabel = "Default microphone"): Promise<MicrophoneInput> {
    if (this.stream && this.track && this.selectedDeviceId === deviceId) {
      return this.input();
    }

    this.stop();

    if (!this.isSecureContext()) {
      const message = "Microphone access requires HTTPS, or localhost on the same computer.";
      this.update({
        ...initialVoicePreflight,
        message
      });
      throw new Error(message);
    }

    const getUserMedia = this.getUserMedia();
    if (!getUserMedia) {
      const message = "This browser does not expose microphone access on this page.";
      this.update({
        ...initialVoicePreflight,
        message
      });
      throw new Error(message);
    }

    this.selectedDeviceId = deviceId;
    this.selectedDeviceLabel = fallbackLabel;
    this.update({
      requested: true,
      ready: false,
      preparing: true,
      message: "Requesting microphone...",
      micLevel: 0,
      micProbeActive: false,
      deviceLabel: fallbackLabel
    });

    try {
      const audio: MediaTrackConstraints | boolean = deviceId ? { deviceId: { exact: deviceId } } : true;
      this.stream = await getUserMedia({ audio });
      this.track = this.stream.getAudioTracks()[0] ?? null;
      if (!this.track) {
        throw new Error("No microphone audio track was returned.");
      }
      this.selectedDeviceLabel = this.track.label || fallbackLabel || "Selected microphone";
      const micProbeActive = this.startProbe();
      this.update({
        requested: true,
        ready: true,
        preparing: false,
        message: `Microphone ready: ${this.selectedDeviceLabel}`,
        micLevel: 0,
        micProbeActive,
        deviceLabel: this.selectedDeviceLabel
      });
      return this.input();
    } catch (error) {
      this.stop();
      this.update({
        requested: false,
        ready: false,
        preparing: false,
        message: "Microphone permission was not granted",
        micLevel: 0,
        micProbeActive: false,
        deviceLabel: "Default microphone"
      });
      throw error;
    }
  }

  stop(): void {
    this.cleanupProbe?.();
    this.cleanupProbe = null;
    for (const track of this.stream?.getTracks() ?? []) {
      track.stop();
    }
    this.stream = null;
    this.track = null;
    this.update({ ...this.preflight, micLevel: 0, micProbeActive: false });
  }

  reset(): void {
    this.stop();
    this.selectedDeviceId = "";
    this.selectedDeviceLabel = "Default microphone";
    this.update(initialVoicePreflight);
  }

  private startProbe(): boolean {
    this.cleanupProbe?.();
    if (!this.stream) return false;
    const AudioContextClass = this.getAudioContext();
    if (!AudioContextClass) return false;

    const context = new AudioContextClass();
    const analyser = context.createAnalyser();
    analyser.fftSize = 512;
    analyser.smoothingTimeConstant = 0.72;
    const source = context.createMediaStreamSource(this.stream);
    source.connect(analyser);
    const samples = new Uint8Array(analyser.fftSize);
    let animationFrame = 0;
    let closed = false;

    const tick = () => {
      if (closed) return;
      analyser.getByteTimeDomainData(samples);
      let sumSquares = 0;
      for (const sample of samples) {
        const centered = (sample - 128) / 128;
        sumSquares += centered * centered;
      }
      const rms = Math.sqrt(sumSquares / samples.length);
      this.update({ ...this.preflight, micLevel: Math.min(1, rms * 7), micProbeActive: true });
      animationFrame = this.requestAnimationFrame(tick);
    };

    void context.resume().catch(() => undefined);
    tick();
    this.cleanupProbe = () => {
      closed = true;
      this.cancelAnimationFrame(animationFrame);
      source.disconnect();
      void context.close().catch(() => undefined);
    };
    return true;
  }

  private update(preflight: VoicePreflight): void {
    this.preflight = preflight;
    for (const listener of this.listeners) listener(this.preflight);
  }

  private getUserMedia(): ((constraints: MediaStreamConstraints) => Promise<MediaStream>) | null {
    return this.getUserMediaOverride ?? navigator.mediaDevices?.getUserMedia?.bind(navigator.mediaDevices) ?? null;
  }

  private isSecureContext(): boolean {
    return this.isSecureContextOverride?.() ?? window.isSecureContext;
  }

  private getAudioContext(): typeof AudioContext | undefined {
    return (
      this.getAudioContextOverride?.() ??
      window.AudioContext ??
      (window as typeof window & { webkitAudioContext?: typeof AudioContext }).webkitAudioContext
    );
  }

  private requestAnimationFrame(callback: FrameRequestCallback): number {
    return (this.requestAnimationFrameOverride ?? window.requestAnimationFrame.bind(window))(callback);
  }

  private cancelAnimationFrame(handle: number): void {
    (this.cancelAnimationFrameOverride ?? window.cancelAnimationFrame.bind(window))(handle);
  }
}
