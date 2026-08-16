import { useMemo } from "react";
import type { VoiceStatus } from "./audio/types.js";
import { initialVoiceStatus } from "./audio/types.js";
import { transcriptionProgressForStatus } from "./helpers.js";

/** Renders a normalized local microphone level, with a colorless fill while transport-muted. */
export function MicrophoneLevelMeter({
  active,
  label,
  level,
  muted = false
}: {
  active: boolean;
  label: string;
  level: number;
  muted?: boolean;
}) {
  const normalizedLevel = Number.isFinite(level) ? Math.max(0, Math.min(1, level)) : 0;
  return (
    <div className={`mic-meter ${muted ? "muted" : "live"}`}>
      <span>{label}</span>
      <div className="mic-meter-track" aria-hidden="true">
        <span style={{ transform: `scaleX(${active ? normalizedLevel : 0})` }} />
      </div>
    </div>
  );
}

/** Renders the shared hosted-transcription readiness progression. */
export function TranscriptionProgress({
  connected = true,
  status = initialVoiceStatus
}: {
  connected?: boolean;
  status?: VoiceStatus;
}) {
  const progress = useMemo(
    () => transcriptionProgressForStatus(status.transcriptionMessage, status.transcriptionReady),
    [status.transcriptionMessage, status.transcriptionReady]
  );
  const transportStarted = connected && (status.connecting || status.connected);
  const heading = !connected
    ? "Waiting for game connection"
    : !transportStarted
      ? "Preparing voice connection"
      : status.transcriptionReady
        ? "Transcription ready"
        : "Waiting for transcription service";
  const detail = !connected
    ? "Transcription has not started."
    : !transportStarted
      ? "Transcription starts after the voice transport connects."
      : status.transcriptionMessage;
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
