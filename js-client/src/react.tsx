import { useEffect, useState } from "react";
import { microphoneMuteButtonLabel } from "./helpers.js";
import type { AudioSessionController, AudioSessionSnapshot, VoiceStatus } from "./index.js";
import { initialVoiceStatus } from "./index.js";

export function useVoiceController(controller: AudioSessionController): AudioSessionSnapshot {
  const [snapshot, setSnapshot] = useState<AudioSessionSnapshot>(() => controller.snapshot());
  useEffect(() => controller.subscribe(setSnapshot), [controller]);
  return snapshot;
}

export { MicLevelMeter, TranscriptionProgress } from "./voiceComponents.js";

/** Renders the participant control for muting an already connected microphone. */
export function MicrophoneMuteButton({
  voiceEnabled,
  onMicrophoneMutedChange,
  voiceStatus = initialVoiceStatus
}: {
  voiceEnabled: boolean;
  onMicrophoneMutedChange: (muted: boolean) => void;
  voiceStatus?: VoiceStatus;
}) {
  const muted = voiceStatus.connected && !voiceStatus.microphoneEnabled;
  return (
    <button
      aria-pressed={muted}
      className={`microphone-mute-button ${muted ? "muted" : "live"}`}
      disabled={!voiceEnabled || !voiceStatus.connected || voiceStatus.microphoneChanging}
      onClick={() => onMicrophoneMutedChange(!muted)}
      type="button"
    >
      {microphoneMuteButtonLabel(voiceStatus)}
    </button>
  );
}

export function DeviceSelect({
  audioInputs,
  disabled,
  onSelectedAudioInputChange,
  selectedAudioInputId
}: {
  audioInputs: MediaDeviceInfo[];
  disabled?: boolean;
  onSelectedAudioInputChange: (value: string) => void;
  selectedAudioInputId: string;
}) {
  return (
    <select
      aria-label="Microphone input"
      disabled={disabled}
      onChange={(event) => onSelectedAudioInputChange(event.target.value)}
      value={selectedAudioInputId}
    >
      {audioInputs.length === 0 ? (
        <option value="">Default microphone</option>
      ) : (
        audioInputs.map((device, index) => (
          <option key={device.deviceId || `audio-${index}`} value={device.deviceId}>
            {device.label || `Microphone ${index + 1}`}
          </option>
        ))
      )}
    </select>
  );
}

export function VoiceStatusChip({ voiceEnabled, voiceStatus = initialVoiceStatus }: {
  voiceEnabled: boolean;
  voiceStatus?: VoiceStatus;
}) {
  return <span>{voiceEnabled ? voiceStatus.message : "Voice is disabled for this study"}</span>;
}

export function TranscriptionStatusChip({ voiceStatus = initialVoiceStatus }: { voiceStatus?: VoiceStatus }) {
  return (
    <div className={`transcription-chip ${voiceStatus.transcriptionMessage === "ASR error" ? "error" : ""}`}>
      <span aria-hidden="true" />
      {voiceStatus.transcriptionMessage}
    </div>
  );
}

export {
  ParlandoStartupGate,
  VoicePreparationControls,
  createDefaultAudioController,
  isVoiceEnabled,
  normalizePresence,
  participantMicrophoneLabel,
  platformLabel,
  selectableAudioInputs,
  voiceStatusUpdate,
  type ActiveParlandoSession,
  type ParlandoStartupGateProps
} from "./startup.js";
