import { useMemo } from "react";
import type { VoiceStatus } from "./audio/types.js";
import { initialVoiceStatus } from "./audio/types.js";
import { transcriptionProgressForStatus } from "./helpers.js";

/** Renders a normalized microphone level without exposing device details. */
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

/** Renders the shared hosted-transcription readiness progression. */
export function TranscriptionProgress({
  gameConnected = true,
  voiceStatus = initialVoiceStatus
}: {
  gameConnected?: boolean;
  voiceStatus?: VoiceStatus;
}) {
  const progress = useMemo(
    () => transcriptionProgressForStatus(voiceStatus.transcriptionMessage, voiceStatus.transcriptionReady),
    [voiceStatus.transcriptionMessage, voiceStatus.transcriptionReady]
  );
  const transportStarted = gameConnected && (voiceStatus.connecting || voiceStatus.connected);
  const heading = !gameConnected
    ? "Waiting for game connection"
    : !transportStarted
      ? "Preparing voice connection"
      : voiceStatus.transcriptionReady
        ? "Transcription ready"
        : "Waiting for transcription service";
  const detail = !gameConnected
    ? "Transcription has not started."
    : !transportStarted
      ? "Transcription starts after the voice transport connects."
      : voiceStatus.transcriptionMessage;
  return (
    <div className="transcription-progress" aria-label="Transcription service progress">
      <div>
        <strong>{heading}</strong>
        <span>{detail}</span>
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
