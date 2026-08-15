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
