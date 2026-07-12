import { useEffect, useMemo, useState } from "react";
import { transcriptionProgressForStatus, voiceButtonLabel } from "./helpers";
import type { AudioSessionController, AudioSessionSnapshot, VoicePreflight, VoiceStatus } from "./index";
import { initialVoicePreflight, initialVoiceStatus } from "./index";

export function useVoiceController(controller: AudioSessionController): AudioSessionSnapshot {
  const [snapshot, setSnapshot] = useState<AudioSessionSnapshot>(() => controller.snapshot());
  useEffect(() => controller.subscribe(setSnapshot), [controller]);
  return snapshot;
}

export function MicLevelMeter({ active, label, level }: { active: boolean; label: string; level: number }) {
  return (
    <div className="mic-meter">
      <span>{label}</span>
      <div className="mic-meter-track" aria-hidden="true">
        <span style={{ transform: `scaleX(${active ? level : 0})` }} />
      </div>
    </div>
  );
}

export function VoiceJoinButton({
  liveKitEnabled,
  onToggleVoice,
  voiceStatus = initialVoiceStatus
}: {
  liveKitEnabled: boolean;
  onToggleVoice: () => void;
  voiceStatus?: VoiceStatus;
}) {
  return (
    <button disabled={!liveKitEnabled || voiceStatus.connecting} onClick={onToggleVoice}>
      {voiceButtonLabel(voiceStatus)}
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

export function VoiceStatusChip({ liveKitEnabled, voiceStatus = initialVoiceStatus }: {
  liveKitEnabled: boolean;
  voiceStatus?: VoiceStatus;
}) {
  return <span>{liveKitEnabled ? voiceStatus.message : "Voice is disabled for this study"}</span>;
}

export function TranscriptionStatusChip({ voiceStatus = initialVoiceStatus }: { voiceStatus?: VoiceStatus }) {
  return (
    <div className={`transcription-chip ${voiceStatus.transcriptionMessage === "ASR error" ? "error" : ""}`}>
      <span aria-hidden="true" />
      {voiceStatus.transcriptionMessage}
    </div>
  );
}

export function TranscriptionProgress({ voiceStatus = initialVoiceStatus }: { voiceStatus?: VoiceStatus }) {
  const progress = useMemo(
    () => transcriptionProgressForStatus(voiceStatus.transcriptionMessage, voiceStatus.transcriptionReady),
    [voiceStatus.transcriptionMessage, voiceStatus.transcriptionReady]
  );
  return (
    <div className="transcription-progress" aria-label="Transcription service progress">
      <div>
        <strong>{voiceStatus.transcriptionReady ? "Transcription ready" : "Waiting for transcription service"}</strong>
        <span>{voiceStatus.transcriptionMessage}</span>
      </div>
      <div className="transcription-progress-track" aria-hidden="true">
        <span style={{ transform: `scaleX(${progress.value})` }} />
      </div>
      <ol>
        {progress.steps.map((step) => (
          <li className={step.done ? "done" : ""} key={step.label}>
            {step.label}
          </li>
        ))}
      </ol>
    </div>
  );
}

export function VoicePreparationControls({
  audioInputs,
  liveKitEnabled,
  onPrepareVoice,
  onSelectedAudioInputChange,
  selectedAudioInputId,
  voicePreflight = initialVoicePreflight
}: {
  audioInputs: MediaDeviceInfo[];
  liveKitEnabled: boolean;
  onPrepareVoice: () => void;
  onSelectedAudioInputChange: (value: string) => void;
  selectedAudioInputId: string;
  voicePreflight?: VoicePreflight;
}) {
  const label = voicePreflight.preparing ? "Preparing" : voicePreflight.ready ? "Voice ready" : "Prepare voice";
  return (
    <>
      <DeviceSelect
        audioInputs={audioInputs}
        disabled={!liveKitEnabled || voicePreflight.preparing || voicePreflight.ready}
        onSelectedAudioInputChange={onSelectedAudioInputChange}
        selectedAudioInputId={selectedAudioInputId}
      />
      {voicePreflight.micProbeActive && (
        <MicLevelMeter active={voicePreflight.micProbeActive} label="Device" level={voicePreflight.micLevel} />
      )}
      <button disabled={!liveKitEnabled || voicePreflight.preparing || voicePreflight.ready} onClick={onPrepareVoice}>
        {label}
      </button>
    </>
  );
}

export {
  ParlandoStartupGate,
  createDefaultAudioController,
  isVoiceEnabled,
  normalizePresence,
  voiceStatusUpdate,
  type ActiveParlandoSession,
  type ParlandoStartupGateProps,
  type ParlandoStartupLabels
} from "./startup";
