import type { AudioSessionPlan, LiveKitTokenResponse, TranscriptSegmentInput } from "../protocol";

export interface VoiceStatus {
  connected: boolean;
  connecting: boolean;
  microphoneEnabled: boolean;
  remoteAudio: boolean;
  message: string;
  transcriptionMessage: string;
  transcriptionReady: boolean;
}

export interface VoicePreflight {
  requested: boolean;
  ready: boolean;
  preparing: boolean;
  message: string;
  micLevel: number;
  micProbeActive: boolean;
  deviceLabel: string;
}

export interface MicrophoneInput {
  deviceId: string;
  deviceLabel: string;
  stream: MediaStream;
  track: MediaStreamTrack;
  createTrackClone(label?: string): MediaStreamTrack;
  createMediaStream(label?: string): MediaStream;
}

export interface AudioSessionContext {
  roomId: string;
  participantSessionId: string;
  role: string;
  selectedAudioInputId: string;
  selectedAudioInputLabel: string | null;
  getAudioSession?(): Promise<AudioSessionPlan>;
  getLiveKitToken(): Promise<LiveKitTokenResponse>;
  postTranscriptSegment?(segment: TranscriptSegmentInput): Promise<unknown>;
  logVoice(event: string, metadata?: Record<string, unknown>): void;
  onVoiceStatus(status: Partial<VoiceStatus>): void;
}

export interface LocalAudioSink {
  id: string;
  provider: string;
  purposes: ReadonlyArray<"partner-audio" | "transcription">;
  connect(input: MicrophoneInput, context: AudioSessionContext): Promise<void>;
  setInputEnabled(enabled: boolean): Promise<void>;
  disconnect(): Promise<void>;
}

export interface AudioSessionSnapshot {
  voiceStatus: VoiceStatus;
  voicePreflight: VoicePreflight;
}

export const initialVoiceStatus: VoiceStatus = {
  connected: false,
  connecting: false,
  microphoneEnabled: false,
  remoteAudio: false,
  message: "Voice not connected",
  transcriptionMessage: "ASR idle",
  transcriptionReady: false
};

export const initialVoicePreflight: VoicePreflight = {
  requested: false,
  ready: false,
  preparing: false,
  message: "Voice not prepared",
  micLevel: 0,
  micProbeActive: false,
  deviceLabel: "Default microphone"
};
